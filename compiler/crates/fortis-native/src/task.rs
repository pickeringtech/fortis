//! Structured concurrency for natively lowered programs (Language Reference 12; Appendix C.1 `async func`, `scope`,
//! `launchParallel`; DEL-39).
//!
//! A Fortis `async func` lowers to a Rust `async fn` whose result is `Async<T>`: `Err(Cancelled)` is the
//! cancellation exit, an ordinary path on which the lowered code runs its deferred blocks and `abandon`. A `scope`
//! owns the futures launched on it, so a cooperative child may borrow the parent's locals; awaiting a task drives
//! the scope's children in launch order until that task completes, which is the definitional interpreter's
//! parent-first schedule (interpreter/executor.py). One thread polls the root (the main task); when every task is
//! pending, virtual time advances to the earliest sleeper (the controlled scheduler). The pools are modelled on the
//! same thread, labelled by executor, as the interpreter models them.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// The cancellation exit of an async function (12.4): never a Failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

pub type Async<T> = Result<T, Cancelled>;

/// Tasks a scope may hold, and detached tasks a program may queue (PROG-NFR-13).
pub const SCOPE_BOUND: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ready,
    Running,
    Suspended,
    Done,
}

struct TaskInfo {
    name: String,
    state: State,
    cancelled: bool,
    detached: bool,
    completion: &'static str,
    executor: &'static str,
}

#[derive(Default)]
struct Exec {
    tasks: Vec<TaskInfo>,
    /// the tasks being polled, innermost last (the current task is the last; empty: main)
    stack: Vec<usize>,
    counter: usize,
    /// virtual nanoseconds
    now: i64,
    /// the earliest deadline of the sleepers registered since the last time advance (a single value, so that a
    /// sleep allocates nothing, C.3)
    next_wake: Option<i64>,
    /// bumps on every state change, so a driver knows whether a pass made progress
    progress: u64,
    /// set for the root's idle pass: nothing progressed, so each open scope runs its runnable children
    idle: bool,
    /// the `join` future being polled inline (its trace name; it runs in the current task)
    alias: Option<(&'static str, usize)>,
    detached: Vec<Pin<Box<dyn Future<Output = ()>>>>,
    threads: usize,
    /// the task that most recently ran user code (`MAIN` for the main task): switching to a suspended task from
    /// another is a `resume`, a sleeper that simply continues is not
    last: usize,
}

const MAIN: usize = usize::MAX;

thread_local! {
    static EXEC: RefCell<Exec> = RefCell::new(Exec { last: MAIN, ..Default::default() });
}

fn me() -> usize {
    with(|e| e.alias).map_or_else(|| current().unwrap_or(MAIN), |(_, n)| alias_id(n))
}

/// The identity of a `join` future polled inline, for the switch detection of `resume` events.
fn alias_id(n: usize) -> usize {
    (MAIN >> 1) + n
}

/// The current task resumes after a suspension: a `resume` event when another task ran meanwhile.
fn resumed() {
    let id = me();
    let switched = with(|e| {
        let s = e.last != id;
        e.last = id;
        s
    });
    if switched {
        trace(|| format!("resume {}", current_name()));
    }
}

fn with<R>(f: impl FnOnce(&mut Exec) -> R) -> R {
    EXEC.with(|e| f(&mut e.borrow_mut()))
}

/// Record a scheduler event when tracing; the event text is formatted only then, so an untraced run allocates
/// nothing for it.
fn trace(event: impl FnOnce() -> String) {
    crate::trace::record(event);
}

/// The name of the task running now (`main` outside any task), for a defect record.
pub fn running_task() -> String {
    current_name()
}

/// A task's name (`fname#n`), for traces and records.
fn task_name(id: usize) -> String {
    with(|e| e.tasks[id].name.clone())
}

fn bump() {
    with(|e| e.progress += 1);
}

fn current() -> Option<usize> {
    with(|e| e.stack.last().copied())
}

fn current_name() -> String {
    if let Some((f, n)) = with(|e| e.alias) {
        return format!("{f}#{n}");
    }
    with(|e| e.stack.last().map_or_else(|| "main".to_string(), |&t| e.tasks[t].name.clone()))
}

/// Whether the current task has been cancelled (main never is).
pub fn cancelled() -> bool {
    with(|e| e.stack.last().map_or(false, |&t| e.tasks[t].cancelled))
}

/// The trace of the run (scheduler events), when `FORTIS_TRACE` is set.
pub fn take_trace() -> Vec<String> {
    crate::trace::take()
}

/// The run's trace as one JSON record on standard error (`executor-trace`), when `FORTIS_TRACE` is set; the
/// conformance harness compares it with a fixture's `execute-trace`.
pub fn flush_trace() {
    crate::trace::flush();
}

/// Tasks launched over the run (the runtime inventory and the thread probe).
pub fn launched() -> usize {
    with(|e| e.tasks.len())
}

/// OS threads started for pool work (0 in the controlled scheduler).
pub fn threads_started() -> usize {
    with(|e| e.threads)
}

fn noop_cx<R>(f: impl FnOnce(&mut Context<'_>) -> R) -> R {
    let mut cx = Context::from_waker(Waker::noop());
    f(&mut cx)
}

// ---- the root ---------------------------------------------------------------------------------------------------

/// Run the main task's future to completion under the controlled scheduler; then the detached tasks.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let out = loop {
        let before = with(|e| e.progress);
        match noop_cx(|cx| fut.as_mut().poll(cx)) {
            Poll::Ready(v) => break v,
            Poll::Pending => {
                if with(|e| e.progress) == before {
                    // an idle pass: every open scope runs its runnable children (`Scope::waiting`)
                    with(|e| e.idle = true);
                    let r = noop_cx(|cx| fut.as_mut().poll(cx));
                    with(|e| e.idle = false);
                    if let Poll::Ready(v) = r {
                        break v;
                    }
                    if with(|e| e.progress) == before && !advance_time() {
                        crate::dynamic("executor: the main task waits for something that cannot progress");
                    }
                }
            }
        }
    };
    drain_detached();
    out
}

/// Nothing can run: virtual time advances to the earliest sleeper. False when there is none.
fn advance_time() -> bool {
    let next = with(|e| {
        let n = e.next_wake.take();
        n
    });
    match next {
        Some(t) => {
            with(|e| {
                e.now = t.max(e.now);
                e.progress += 1;
            });
            trace(|| format!("time {}ms", with(|e| e.now) / 1_000_000));
            true
        }
        None => false,
    }
}

/// The Supervisor's wait at the program's end (15, 12.5): the main task waits for each detached task in detach
/// order; meanwhile the detached tasks run in detach order, and the main task resumes first when the one it waits
/// for completes.
fn drain_detached() {
    let mut list: Vec<Option<Pin<Box<dyn Future<Output = ()>>>>> = with(|e| std::mem::take(&mut e.detached)).into_iter().map(Some).collect();
    for target in 0..list.len() {
        if list[target].is_none() {
            continue;
        }
        trace(|| format!("suspend {}", current_name()));
        'scan: loop {
            let before = with(|e| e.progress);
            for i in 0..list.len() {
                let Some(f) = list[i].as_mut() else { continue };
                if noop_cx(|cx| f.as_mut().poll(cx)).is_ready() {
                    list[i] = None;
                    bump();
                    if i == target {
                        break 'scan;
                    }
                    continue 'scan;
                }
            }
            if with(|e| e.progress) == before && !advance_time() {
                crate::dynamic("executor: a detached task waits for something that cannot progress");
            }
        }
        resumed();
    }
}

// ---- tasks and scopes ---------------------------------------------------------------------------------------------

/// A launched future (12.5): a typed handle on the scope that owns the future.
pub struct Task<'s, 'a, T> {
    id: usize,
    scope: &'s Scope<'a>,
    _t: PhantomData<fn() -> T>,
}

/// `Task(name#n)`, as the interpreter displays a task.
impl<T> crate::show::Show for Task<'_, '_, T> {
    fn show(&self) -> String {
        format!("Task({})", with(|e| e.tasks[self.id].name.clone()))
    }
}

impl<T> Clone for Task<'_, '_, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Task<'_, '_, T> {}

impl<'s, 'a, T: 'static> Task<'s, 'a, T> {
    /// `await task`.
    pub fn join(self) -> impl Future<Output = Async<T>> + use<'s, 'a, T> {
        self.scope.join(self)
    }

    /// `task.abandon()` (12.4): cancel the task and wait for it to finish unwinding.
    pub async fn abandon(self) -> Async<()> {
        let cancels = with(|e| {
            let live = !e.tasks[self.id].cancelled && e.tasks[self.id].state != State::Done;
            if live {
                e.tasks[self.id].cancelled = true;
                e.progress += 1;
            }
            live
        });
        if cancels {
            trace(|| format!("cancel {}", task_name(self.id)));
        }
        let scope = self.scope;
        let id = self.id;
        std::future::poll_fn(move |cx| scope.poll_until(id, cx)).await;
        trace(|| format!("join {}", with(|e| e.tasks[id].name.clone())));
        scope.children.borrow_mut().iter_mut().filter(|c| c.id == id).for_each(|c| c.consumed = true);
        Ok(())
    }
}

enum Outcome {
    Value(Box<dyn Any>),
    Cancelled,
}

/// A child's future: one that may borrow the parent's locals, or a `'static` one (which may be detached).
enum ChildFut<'a> {
    Local(Pin<Box<dyn Future<Output = Outcome> + 'a>>),
    Static(Pin<Box<dyn Future<Output = Outcome>>>),
}

impl ChildFut<'_> {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Outcome> {
        match self {
            ChildFut::Local(f) => f.as_mut().poll(cx),
            ChildFut::Static(f) => f.as_mut().poll(cx),
        }
    }
}

/// A task's failure value when its result was never awaited (reported as abandoned at the scope's exit, 12.5).
type FailureOf = fn(Box<dyn Any>) -> Option<crate::Failure>;

struct Child<'a> {
    id: usize,
    fut: Option<ChildFut<'a>>,
    result: Option<Outcome>,
    done: bool,
    started: bool,
    consumed: bool,
    fname: String,
    failure_of: Option<FailureOf>,
}

/// A `scope`: the join set of the tasks launched on it, polled by its owner (C.1).
pub struct Scope<'a> {
    children: RefCell<Vec<Child<'a>>>,
    _owner: Cell<()>,
}

impl<'a> Default for Scope<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Scope<'a> {
    pub fn new() -> Scope<'a> {
        Scope { children: RefCell::new(vec![]), _owner: Cell::new(()) }
    }

    fn register(&self, fname: &str, executor: &'static str) -> usize {
        if self.children.borrow().iter().filter(|c| !c.done).count() >= SCOPE_BOUND {
            crate::defect("out_of_memory");
        }
        let id = with(|e| {
            e.counter += 1;
            e.tasks.push(TaskInfo { name: format!("{fname}#{}", e.counter), state: State::Ready, cancelled: false, detached: false, completion: "Completed", executor });
            e.progress += 1;
            e.tasks.len() - 1
        });
        trace(|| if executor == "cooperative" { format!("launch {}", task_name(id)) } else { format!("launch {} on {executor}", task_name(id)) });
        id
    }

    fn push(&self, id: usize, fname: &str, fut: ChildFut<'a>, failure_of: Option<FailureOf>) {
        self.children.borrow_mut().push(Child { id, fut: Some(fut), result: None, done: false, started: false, consumed: false, fname: fname.to_string(), failure_of });
    }

    fn local<F, T>(fut: F) -> ChildFut<'a>
    where
        F: Future<Output = Async<T>> + 'a,
        T: 'static,
    {
        ChildFut::Local(Box::pin(async move {
            match fut.await {
                Ok(v) => Outcome::Value(Box::new(v)),
                Err(Cancelled) => Outcome::Cancelled,
            }
        }))
    }

    fn statik<F, T>(fut: F) -> ChildFut<'a>
    where
        F: Future<Output = Async<T>> + 'static,
        T: 'static,
    {
        ChildFut::Static(Box::pin(async move {
            match fut.await {
                Ok(v) => Outcome::Value(Box::new(v)),
                Err(Cancelled) => Outcome::Cancelled,
            }
        }))
    }

    /// `scope.launch(f())`: a cooperative task in launch order, ready to run.
    pub fn launch<'s, F, T>(&'s self, fname: &str, fut: F) -> Task<'s, 'a, T>
    where
        F: Future<Output = Async<T>> + 'a,
        T: 'static,
    {
        let id = self.register(fname, "cooperative");
        self.push(id, fname, Self::local(fut), None);
        Task { id, scope: self, _t: PhantomData }
    }

    /// `scope.launch(f())` of a `try` function: a failure it returns is reported if the task is never awaited.
    pub fn launch_try<'s, F, U>(&'s self, fname: &str, fut: F) -> Task<'s, 'a, crate::Outcome<U>>
    where
        F: Future<Output = Async<crate::Outcome<U>>> + 'a,
        U: 'static,
    {
        let id = self.register(fname, "cooperative");
        self.push(id, fname, Self::local(fut), Some(|v: Box<dyn Any>| v.downcast::<crate::Outcome<U>>().ok().and_then(|o| (*o).err())));
        Task { id, scope: self, _t: PhantomData }
    }

    /// A cooperative launch whose future owns all it holds (`'static`), so the task may be detached.
    pub fn launch_detachable<'s, F, T>(&'s self, fname: &str, fut: F) -> Task<'s, 'a, T>
    where
        F: Future<Output = Async<T>> + 'static,
        T: 'static,
    {
        let id = self.register(fname, "cooperative");
        self.push(id, fname, Self::statik(fut), None);
        Task { id, scope: self, _t: PhantomData }
    }

    /// `scope.launchParallel(f())`: pool work. The controlled scheduler runs it on the task thread, labelled by its
    /// executor, as the interpreter does; its future is `'static` (C.2: a future holding a loan is not launched on
    /// a parallel pool).
    pub fn launch_parallel<'s, F, T>(&'s self, fname: &str, fut: F) -> Task<'s, 'a, T>
    where
        F: Future<Output = Async<T>> + 'static,
        T: 'static,
    {
        let id = self.register(fname, "parallel");
        self.push(id, fname, Self::statik(fut), None);
        Task { id, scope: self, _t: PhantomData }
    }

    /// `scope.launchBlocking(fn once() T)`: blocking work, run when the scheduler reaches it.
    pub fn launch_blocking<'s, W, T>(&'s self, work: W) -> Task<'s, 'a, T>
    where
        W: FnOnce() -> T + 'a,
        T: 'static,
    {
        let id = self.register("work", "blocking");
        self.push(id, "work", Self::local(async move { Ok(work()) }), None);
        Task { id, scope: self, _t: PhantomData }
    }

    /// `scope.launchBlocking(fn once() T)` lowered as a future (its body may reach `checkpoint()`).
    pub fn launch_blocking_future<'s, F, T>(&'s self, work: F) -> Task<'s, 'a, T>
    where
        F: Future<Output = Async<T>> + 'a,
        T: 'static,
    {
        let id = self.register("work", "blocking");
        self.push(id, "work", Self::local(work), None);
        Task { id, scope: self, _t: PhantomData }
    }

    /// `scope.detach(task, supervisor)`: the supervisor takes over the join obligation; the task runs when the main
    /// task yields or at the program's end, and its completion is recorded (12.5, 12.6). A detached future is
    /// `'static`.
    pub fn detach<T>(&self, t: Task<'_, 'a, T>) {
        if with(|e| e.detached.len()) >= SCOPE_BOUND {
            crate::defect("out_of_memory");
        }
        let child = {
            let mut cs = self.children.borrow_mut();
            let i = cs.iter().position(|c| c.id == t.id).expect("detach of a task this scope does not hold");
            cs.remove(i)
        };
        let mut child = match child.fut {
            Some(ChildFut::Static(f)) => Child { id: child.id, fut: Some(ChildFut::Static(f)), result: None, done: false, started: child.started, consumed: false, fname: child.fname, failure_of: None },
            Some(ChildFut::Local(_)) => crate::dynamic("detach of a task whose future holds a loan (it must own what it holds)"),
            None => return,
        };
        let id = child.id;
        with(|e| e.tasks[id].detached = true);
        trace(|| format!("detach {}", task_name(id)));
        let fut: Pin<Box<dyn Future<Output = ()> + 'static>> = Box::pin(std::future::poll_fn(move |cx| {
            let mut c = Child { id: child.id, fut: child.fut.take(), result: None, done: false, started: child.started, consumed: false, fname: std::mem::take(&mut child.fname), failure_of: None };
            let r = drive_child(&mut c, cx);
            child.fut = c.fut.take();
            child.started = c.started;
            child.fname = c.fname;
            match r {
                Poll::Ready(()) => {
                    let (name, completion) = with(|e| (e.tasks[id].name.clone(), e.tasks[id].completion));
                    crate::records::task_completion(&name, completion);
                    Poll::Ready(())
                }
                Poll::Pending => Poll::Pending,
            }
        }));
        with(|e| e.detached.push(fut));
    }

    fn index_of(&self, id: usize) -> usize {
        self.children.borrow().iter().position(|c| c.id == id).expect("a task of this scope")
    }

    /// Poll one unfinished child (moved out while it runs, so a nested scope may use this one). True when it
    /// completed.
    fn poll_child(&self, i: usize, cx: &mut Context<'_>) -> bool {
        let mut child = {
            let mut cs = self.children.borrow_mut();
            Child { id: cs[i].id, fut: cs[i].fut.take(), result: None, done: false, started: cs[i].started, consumed: cs[i].consumed, fname: std::mem::take(&mut cs[i].fname), failure_of: cs[i].failure_of }
        };
        let r = drive_child(&mut child, cx);
        let mut cs = self.children.borrow_mut();
        cs[i].fut = child.fut;
        cs[i].started = child.started;
        cs[i].fname = child.fname;
        if r.is_ready() {
            cs[i].result = child.result;
            cs[i].done = true;
            return true;
        }
        false
    }

    fn is_done(&self, id: usize) -> bool {
        self.children.borrow()[self.index_of(id)].done
    }

    /// Drive the children in launch order until `id` completes (the parent then resumes first) or none can
    /// progress; after any child completes the scan restarts from the first task, as the interpreter's scheduler
    /// picks the first runnable task in launch order.
    fn poll_until(&self, id: usize, cx: &mut Context<'_>) -> Poll<()> {
        'scan: loop {
            if self.is_done(id) {
                return Poll::Ready(());
            }
            let before = with(|e| e.progress);
            let n = self.children.borrow().len();
            for i in 0..n {
                if self.children.borrow()[i].done {
                    continue;
                }
                let completed = self.poll_child(i, cx);
                if self.is_done(id) {
                    return Poll::Ready(());
                }
                if completed {
                    continue 'scan;
                }
            }
            if with(|e| e.progress) == before {
                return Poll::Pending;
            }
        }
    }

    /// An await of something other than one of this scope's tasks (a sleep, an inline call, `join`), inside the
    /// scope's block: in the root's idle pass the scope's runnable children run first, in launch order, as the
    /// interpreter's scheduler runs the first runnable task whenever the current one suspends (12.3). The pass stops
    /// at a child's completion, so a task waiting on it resumes first; enclosing scopes wrap first, so their
    /// earlier-launched children run first.
    pub async fn waiting<F: Future>(&self, fut: F) -> F::Output {
        let mut fut = std::pin::pin!(fut);
        std::future::poll_fn(move |cx| {
            if with(|e| e.idle) {
                let before = with(|e| e.progress);
                let n = self.children.borrow().len();
                for i in 0..n {
                    let runnable = self.children.borrow().get(i).map_or(false, |c| !c.done && c.fut.is_some());
                    if runnable && self.poll_child(i, cx) {
                        return Poll::Pending;
                    }
                }
                if with(|e| e.progress) != before {
                    return Poll::Pending;
                }
            }
            fut.as_mut().poll(cx)
        })
        .await
    }

    /// `await task` (12.3, 12.5): the parent suspends, the scope's tasks run in launch order, and the parent resumes
    /// first when the task completes; its result is the task's value. Cancellation of the awaiting task is
    /// observed here.
    pub fn join<'s, T: 'static>(&'s self, t: Task<'_, 'a, T>) -> impl Future<Output = Async<T>> + use<'s, 'a, T> {
        let t = Task::<'s, 'a, T> { id: t.id, scope: self, _t: PhantomData };
        let mut suspended = false;
        std::future::poll_fn(move |cx| {
            if cancelled() {
                resumed();
            trace(|| format!("cancelled {}", current_name()));
                return Poll::Ready(Err(Cancelled));
            }
            let done = self.is_done(t.id);
            if !done {
                if !suspended {
                    suspended = true;
                    set_state(current(), State::Suspended);
                    trace(|| format!("suspend {}", current_name()));
                }
                if self.poll_until(t.id, cx).is_pending() {
                    return Poll::Pending;
                }
            }
            if suspended {
                set_state(current(), State::Running);
                resumed();
            }
            trace(|| format!("join {}", task_name(t.id)));
            let mut cs = self.children.borrow_mut();
            let i = cs.iter().position(|c| c.id == t.id).unwrap();
            cs[i].consumed = true;
            match cs[i].result.take() {
                Some(Outcome::Value(v)) => Poll::Ready(Ok(*v.downcast::<T>().expect("a task's result type"))),
                Some(Outcome::Cancelled) => crate::dynamic(&format!("awaited task {} was cancelled: cancellation of a child reaches the parent (12.5)", task_name(t.id))),
                None => crate::dynamic("awaited task has no result"),
            }
        })
    }

    /// The scope's exit (12.5): its unjoined tasks are cancelled, then each is joined in launch order; a never-awaited
    /// failure is reported as abandoned by the caller (the scope does not know result types).
    pub fn finish<'s>(&'s self) -> impl Future<Output = Async<()>> + use<'s, 'a> {
        let mut cancelled_all = false;
        let mut idx = 0usize;
        let mut suspended = false;
        std::future::poll_fn(move |cx| {
            if !cancelled_all {
                cancelled_all = true;
                let ids: Vec<usize> = self.children.borrow().iter().filter(|c| !c.done).map(|c| c.id).collect();
                for id in ids {
                    with(|e| {
                        if !e.tasks[id].cancelled && e.tasks[id].state != State::Done {
                            e.tasks[id].cancelled = true;
                            e.progress += 1;
                        }
                    });
                    trace(|| format!("cancel {}", with(|e| e.tasks[id].name.clone())));
                }
            }
            loop {
                let (id, consumed, done) = match self.children.borrow().get(idx) {
                    Some(c) => (c.id, c.consumed, c.done),
                    None => break,
                };
                if consumed {
                    idx += 1;
                    continue;
                }
                if !done {
                    if !suspended {
                        suspended = true;
                        trace(|| format!("suspend {}", current_name()));
                    }
                    if self.poll_until(id, cx).is_pending() {
                        return Poll::Pending;
                    }
                    if suspended {
                        suspended = false;
                        resumed();
                    }
                }
                trace(|| format!("join {}", with(|e| e.tasks[id].name.clone())));
                let failure = {
                    let mut cs = self.children.borrow_mut();
                    cs[idx].consumed = true;
                    match (cs[idx].failure_of, cs[idx].result.take()) {
                        (Some(f), Some(Outcome::Value(v))) => f(v),
                        _ => None,
                    }
                };
                if let Some(f) = failure {
                    crate::records::report(&f, "abandoned");
                }
                idx += 1;
            }
            Poll::Ready(Ok(()))
        })
    }
}


fn set_state(t: Option<usize>, s: State) {
    if let Some(t) = t {
        with(|e| e.tasks[t].state = s);
    }
}

/// Poll one child once (start, resume, or observe a cancellation before it started).
fn drive_child(child: &mut Child<'_>, cx: &mut Context<'_>) -> Poll<()> {
    let id = child.id;
    let cancelled_before = with(|e| e.tasks[id].cancelled && !child.started);
    if cancelled_before {
        trace(|| format!("cancelled {}", task_name(id)));
        crate::records::not_started(&child.fname);
        child.fut = None;
        child.result = Some(Outcome::Cancelled);
        with(|e| {
            e.tasks[id].state = State::Done;
            e.tasks[id].completion = "Cancelled";
            e.progress += 1;
            e.last = id;
        });
        trace(|| format!("complete {}", task_name(id)));
        return Poll::Ready(());
    }
    if !child.started {
        child.started = true;
        trace(|| format!("start {}", task_name(id)));
        with(|e| {
            e.progress += 1;
            e.last = id;
        });
    }
    with(|e| {
        e.tasks[id].state = State::Running;
        e.stack.push(id);
    });
    let r = match child.fut.as_mut() {
        Some(f) => f.poll(cx),
        None => Poll::Ready(Outcome::Cancelled),
    };
    with(|e| {
        e.stack.pop();
    });
    match r {
        Poll::Ready(out) => {
            let completion = if matches!(out, Outcome::Cancelled) { "Cancelled" } else { "Completed" };
            child.result = Some(out);
            child.fut = None;
            with(|e| {
                e.tasks[id].state = State::Done;
                e.tasks[id].completion = completion;
                e.progress += 1;
            });
            trace(|| format!("complete {}", task_name(id)));
            Poll::Ready(())
        }
        Poll::Pending => {
            let was_running = with(|e| e.tasks[id].state == State::Running);
            if was_running {
                with(|e| e.tasks[id].state = State::Suspended);
            }
            Poll::Pending
        }
    }
}

// ---- awaiting -------------------------------------------------------------------------------------------------

/// `await f()` of an unlaunched future: it runs inline in the current task; the current task's cancellation is
/// observed before it runs and whenever it resumes.
pub async fn awaited<F, T>(fut: F) -> Async<T>
where
    F: Future<Output = Async<T>>,
{
    // pinned in place inside the awaiting future's state: an await allocates nothing (C.2, SP-04-async)
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(move |cx| {
        if cancelled() {
            resumed();
            trace(|| format!("cancelled {}", current_name()));
            return Poll::Ready(Err(Cancelled));
        }
        fut.as_mut().poll(cx)
    })
    .await
}

/// `clock.sleep(d)` awaited: the task suspends until virtual time reaches the deadline.
pub fn sleep(ns: i64) -> impl Future<Output = Async<()>> {
    let mut until: Option<i64> = None;
    let mut suspended = false;
    std::future::poll_fn(move |_cx| {
        if cancelled() {
            resumed();
            trace(|| format!("cancelled {}", current_name()));
            return Poll::Ready(Err(Cancelled));
        }
        let now = with(|e| e.now);
        let deadline = *until.get_or_insert(now + ns);
        if now >= deadline {
            if suspended {
                resumed();
            }
            return Poll::Ready(Ok(()));
        }
        if !suspended {
            suspended = true;
            trace(|| format!("suspend {}", current_name()));
        }
        with(|e| e.next_wake = Some(e.next_wake.map_or(deadline, |w| w.min(deadline))));
        Poll::Pending
    })
}

/// One future of a `join`, polled inline by the parent: named like a task in the trace (`fname#n`), it runs in the
/// parent's task, so it observes the parent's cancellation.
struct Inline<'f, F: Future> {
    fname: &'static str,
    n: usize,
    fut: Pin<&'f mut F>,
    started: bool,
    out: Option<F::Output>,
}

impl<F: Future> Inline<'_, F> {
    fn done(&self) -> bool {
        self.out.is_some()
    }

    /// Poll once under the future's trace name; true when it completed now.
    fn poll(&mut self, cx: &mut Context<'_>) -> bool {
        let alias = alias_id(self.n);
        let prev = with(|e| e.alias.replace((self.fname, self.n)));
        if !self.started {
            self.started = true;
            trace(|| format!("start {}#{}", self.fname, self.n));
            with(|e| {
                e.progress += 1;
                e.last = alias;
            });
        }
        let r = self.fut.as_mut().poll(cx);
        with(|e| e.alias = prev);
        match r {
            Poll::Ready(v) => {
                self.out = Some(v);
                trace(|| format!("complete {}#{}", self.fname, self.n));
                with(|e| {
                    e.progress += 1;
                    e.last = alias;
                });
                true
            }
            Poll::Pending => false,
        }
    }
}

/// `join(a, b)` awaited (12.5): both futures run to completion inside the parent, which waits for the first and
/// then the second; a failure value is part of the returned pair, and when the parent is cancelled both futures
/// observe it and run to completion first. Both futures are pinned in the join's own state: it allocates nothing.
pub async fn join2<A, B, TA, TB>(fa: &'static str, a: A, fb: &'static str, b: B) -> Async<(TA, TB)>
where
    A: Future<Output = Async<TA>>,
    B: Future<Output = Async<TB>>,
{
    if cancelled() {
        resumed();
        trace(|| format!("cancelled {}", current_name()));
        return Err(Cancelled);
    }
    let (na, nb) = with(|e| {
        e.counter += 2;
        (e.counter - 1, e.counter)
    });
    trace(|| format!("launch {fa}#{na}"));
    trace(|| format!("launch {fb}#{nb}"));
    let a = std::pin::pin!(a);
    let b = std::pin::pin!(b);
    let mut left = Inline { fname: fa, n: na, fut: a, started: false, out: None };
    let mut right = Inline { fname: fb, n: nb, fut: b, started: false, out: None };
    let mut target = 0usize;
    let mut suspended = false;
    let mut cancel_seen = false;
    let mut launched = false;
    std::future::poll_fn(|cx| {
        loop {
            if cancelled() && !cancel_seen {
                // the parent resumes to observe its cancellation, cancels both futures and waits for each again
                cancel_seen = true;
                resumed();
                for (f, n, done) in [(left.fname, left.n, left.done()), (right.fname, right.n, right.done())] {
                    if !done {
                        trace(|| format!("cancel {f}#{n}"));
                    }
                }
                suspended = false;
            }
            if target == 2 {
                return Poll::Ready(());
            }
            let target_done = if target == 0 { left.done() } else { right.done() };
            if target_done {
                if suspended {
                    suspended = false;
                    resumed();
                }
                if !cancel_seen {
                    let (f, n) = if target == 0 { (left.fname, left.n) } else { (right.fname, right.n) };
                    trace(|| format!("join {f}#{n}"));
                }
                target += 1;
                continue;
            }
            if !suspended {
                suspended = true;
                trace(|| format!("suspend {}", current_name()));
            }
            if !launched {
                // the two futures are the newest tasks: every task launched before them runs first, in launch
                // order, as the interpreter's scheduler picks the first runnable task (12.3)
                launched = true;
                bump();
                return Poll::Pending;
            }
            // drive both in order until the awaited one completes; a completion restarts the scan
            let before = with(|e| e.progress);
            let mut completed = false;
            if !left.done() {
                completed = left.poll(cx);
            }
            let target_now = if target == 0 { left.done() } else { right.done() };
            if !completed && !target_now && !right.done() {
                right.poll(cx);
            }
            if with(|e| e.progress) == before {
                return Poll::Pending;
            }
        }
    })
    .await;
    match (left.out.take(), right.out.take()) {
        (Some(Ok(x)), Some(Ok(y))) if !cancel_seen => Ok((x, y)),
        _ => {
            trace(|| format!("cancelled {}", current_name()));
            Err(Cancelled)
        }
    }
}

/// `checkpoint()` (12.4): a cancellation point for pool tasks; cooperative tasks observe cancellation only at await.
pub fn checkpoint() -> impl Future<Output = Async<()>> {
    let mut yielded = false;
    std::future::poll_fn(move |_cx| {
        let cooperative = with(|e| e.stack.last().map_or(true, |&t| e.tasks[t].executor == "cooperative"));
        if cooperative {
            return Poll::Ready(Ok(()));
        }
        if !yielded {
            trace(|| format!("checkpoint {}", current_name()));
        }
        if yielded {
            resumed();
        }
        if cancelled() {
            trace(|| format!("cancelled {}", current_name()));
            return Poll::Ready(Err(Cancelled));
        }
        if !yielded {
            yielded = true;
            bump();
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    })
}

/// A block-exit event the lowered code records (`loans-end`, `defer`, `abandon T`, `drop T`), when tracing.
pub fn trace_event(event: &str) {
    crate::trace::event(event);
}

/// Whether a scope has launched any task (its exit then records `loans-end`, as the interpreter's does).
impl Scope<'_> {
    pub fn has_tasks(&self) -> bool {
        !self.children.borrow().is_empty()
    }
}

/// Turn the trace on or off (tests; programs read `FORTIS_TRACE`).
pub fn set_tracing(on: bool) {
    crate::trace::set_tracing(on);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print(out: &RefCell<Vec<String>>, s: &str) {
        out.borrow_mut().push(s.to_string());
    }

    #[test]
    fn two_tasks_awaited_in_turn() {
        set_tracing(true);
        let out = RefCell::new(vec![]);
        async fn fetch(out: &RefCell<Vec<String>>, n: i64) -> Async<i64> {
            print(out, &format!("fetch {n}"));
            Ok(n)
        }
        let r = block_on(async {
            let tasks = Scope::new();
            let a = tasks.launch("fetch", fetch(&out, 1));
            let b = tasks.launch("fetch", fetch(&out, 2));
            let x = tasks.join(a).await?;
            let y = tasks.join(b).await?;
            trace_event("loans-end");
            tasks.finish().await?;
            Ok::<i64, Cancelled>(x + y)
        });
        assert_eq!(r, Ok(3));
        assert_eq!(out.borrow().join("|"), "fetch 1|fetch 2");
        assert_eq!(
            take_trace().join("|"),
            "launch fetch#1|launch fetch#2|suspend main|start fetch#1|complete fetch#1|resume main|join fetch#1|suspend main|start fetch#2|complete fetch#2|resume main|join fetch#2|loans-end"
        );
    }

    #[test]
    fn a_sleeping_sibling_runs_after_virtual_time_advances() {
        set_tracing(true);
        let out = RefCell::new(vec![]);
        async fn quick(out: &RefCell<Vec<String>>) -> Async<i64> {
            print(out, "quick");
            Ok(0)
        }
        async fn slow(out: &RefCell<Vec<String>>) -> Async<i64> {
            print(out, "slow start");
            awaited(sleep(10_000_000)).await?;
            print(out, "slow end");
            Ok(7)
        }
        let r = block_on(async {
            let tasks = Scope::new();
            let a = tasks.launch("quick", quick(&out));
            let b = tasks.launch("slow", slow(&out));
            let x = tasks.join(a).await?;
            let y = tasks.join(b).await?;
            trace_event("loans-end");
            tasks.finish().await?;
            Ok::<i64, Cancelled>(x + y)
        });
        assert_eq!(r, Ok(7));
        assert_eq!(out.borrow().join("|"), "quick|slow start|slow end");
        assert_eq!(
            take_trace().join("|"),
            "launch quick#1|launch slow#2|suspend main|start quick#1|complete quick#1|resume main|join quick#1|suspend main|start slow#2|suspend slow#2|time 10ms|complete slow#2|resume main|join slow#2|loans-end"
        );
    }

    #[test]
    fn a_task_cancelled_before_it_starts_never_runs() {
        set_tracing(true);
        let out = RefCell::new(vec![]);
        async fn note(out: &RefCell<Vec<String>>, s: &'static str) -> Async<i64> {
            print(out, s);
            Ok(1)
        }
        let r = block_on(async {
            let tasks = Scope::new();
            let a = tasks.launch("quick", note(&out, "quick"));
            let _late = tasks.launch("late", note(&out, "late"));
            let x = tasks.join(a).await?;
            trace_event("loans-end");
            tasks.finish().await?;
            Ok::<i64, Cancelled>(x)
        });
        assert_eq!(r, Ok(1));
        assert_eq!(out.borrow().join("|"), "quick");
        assert_eq!(
            take_trace().join("|"),
            "launch quick#1|launch late#2|suspend main|start quick#1|complete quick#1|resume main|join quick#1|loans-end|cancel late#2|suspend main|cancelled late#2|complete late#2|resume main|join late#2"
        );
    }
}
