//! The cooperative executor of the VM (Language Reference 12.3–12.6; DEL-38), a port of the definitional
//! interpreter's (interpreter/executor.py) with the same schedule, so traces agree event for event.
//!
//! Every task of one scope tree runs on one logical thread: a task runs until it completes or awaits something
//! pending, and the scheduler then picks the next runnable task in launch order. Each task is an OS thread and
//! the tasks pass a single baton, so the interleaving is deterministic and the evaluator's recursion stays
//! ordinary. The run's global state (console lines, trace, records, failure JSON, constants) travels with the baton.
//! Cancellation is a flag observed only at an `await` (and at `checkpoint()` by pool tasks): a cancelled task
//! resumes there, unwinds (defers, abandon, drops) and completes as cancelled. Time is virtual: it advances only
//! when no task can run, to the earliest sleeper.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use fortis_mir::model::Program;

use crate::machine::{dynamic, Exit, Frame, Halt, Record, Vm, R};
use crate::sync::Rc;
use crate::value::{display, FutureVal, ScopeVal, TaskRef, Value};

/// Tasks a scope may hold (launched, not yet joined), and detached tasks a program may queue (PROG-NFR-13).
pub const SCOPE_BOUND: usize = 256;

/// Task threads running now (a development session waits for them at `finish`, SP-07 design 8).
pub static THREADS_LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// In a development session under the fault switch `launch-current`, the session's current generation.
pub static SESSION_CURRENT: Mutex<Option<std::sync::Weak<crate::generation::Generation>>> = Mutex::new(None);

struct ThreadCount;
impl Drop for ThreadCount {
    fn drop(&mut self) {
        THREADS_LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Stack reserved for a task thread: the evaluator recurses once per nested call and expression.
const TASK_STACK: usize = 256 << 20;

/// What a future runs when it is polled.
#[derive(Debug)]
pub enum Work {
    /// an async function call with its arguments bound; in a development session, with the identity a task of
    /// another generation rebinds it by (SP-07 design 5)
    Call { func: usize, frame: Frame, code: Option<Rc<crate::generation::CodeId>> },
    /// a `launchBlocking` closure
    Closure(Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TState {
    Ready,
    Running,
    Suspended,
    Done,
}

#[derive(Debug)]
pub struct TaskSt {
    pub name: String,
    pub fname: String,
    pub state: TState,
    pub cancelled: bool,
    /// waiting for a cancelled `join`'s futures: its cancellation does not make it runnable (12.5)
    pub ignores_cancel: bool,
    pub waiting_on: Option<usize>,
    pub sleep_until: Option<i64>,
    pub executor: &'static str,
    pub detached: bool,
    pub consumed: bool,
    pub result: Option<Value>,
    pub error: Option<Exit>,
    pub completion: &'static str,
    pub work: Option<Work>,
    baton: Arc<Baton>,
    /// in a development session, the generation the task runs until it completes (Platform 11.2), then None
    pub gen: Option<Arc<crate::generation::Generation>>,
    /// in a development session, the root invocation the task belongs to (0: none)
    pub root: usize,
    /// a development session's root invocation task: its result is shown and destroyed by its own VM, in its own
    /// generation, and the session reads `shown` (SP-07 design 2)
    pub session_root: bool,
    pub shown: Option<(&'static str, String)>,
}

/// One task's turn: woken (`Go`), or the run is over (`Shutdown`) and a parked thread must leave without running.
#[derive(Debug, Default)]
struct Baton {
    flag: Mutex<u8>,
    cv: Condvar,
}

const IDLE: u8 = 0;
const GO: u8 = 1;
const SHUTDOWN: u8 = 2;

impl Baton {
    fn wake(&self, how: u8) {
        let mut f = self.flag.lock().unwrap_or_else(|p| p.into_inner());
        if *f != SHUTDOWN {
            *f = how;
        }
        self.cv.notify_all();
    }
    /// Wait for the baton; false when the run is shutting down.
    fn wait(&self) -> bool {
        let mut f = self.flag.lock().unwrap_or_else(|p| p.into_inner());
        while *f == IDLE {
            f = self.cv.wait(f).unwrap_or_else(|p| p.into_inner());
        }
        let go = *f == GO;
        if go {
            *f = IDLE;
        }
        go
    }
}

/// The run's global state, held by the task that has the baton. Failure JSON travels with it: a task's `report`
/// is written on the task's own VM, and the line would otherwise end with that thread.
#[derive(Debug, Default)]
pub struct Globals {
    pub out: Vec<String>,
    pub trace: Vec<String>,
    pub reports: Vec<Record>,
    pub consts: HashMap<String, Value>,
    pub failure_lines: Vec<String>,
    pub held_failure: Option<String>,
}

#[derive(Debug)]
pub struct State {
    /// task 0 is the main task; the others in launch order
    pub tasks: Vec<TaskSt>,
    pub current: usize,
    pub counter: usize,
    /// virtual time in nanoseconds
    pub now: i64,
    pub detached: Vec<usize>,
    globals: Option<Globals>,
}

pub struct Sched {
    pub st: Mutex<State>,
    pub prog: Arc<Program>,
    pub file: String,
    /// source name -> declaring module file, copied onto every task VM (the root VM's map)
    pub decl_files: HashMap<String, String>,
    /// OS threads started for tasks over the run (the thread probe, PROG-NFR-14)
    pub threads_started: std::sync::atomic::AtomicUsize,
    /// whether the run has created the capabilities: the main VM's flag, which every task's VM shares (11.6; D-110)
    pub caps_made: Arc<std::sync::atomic::AtomicBool>,
}

impl Sched {
    pub fn new(prog: &Program, file: &str) -> Sched {
        let main = TaskSt {
            name: "main".into(),
            fname: "main".into(),
            state: TState::Running,
            cancelled: false,
            ignores_cancel: false,
            waiting_on: None,
            sleep_until: None,
            executor: "cooperative",
            detached: false,
            consumed: false,
            result: None,
            error: None,
            completion: "Completed",
            work: None,
            baton: Arc::new(Baton::default()),
            gen: None,
            root: 0,
            session_root: false,
            shown: None,
        };
        Sched {
            st: Mutex::new(State { tasks: vec![main], current: 0, counter: 0, now: 0, detached: vec![], globals: None }),
            prog: Arc::new(prog.clone()),
            file: file.to_string(),
            decl_files: HashMap::new(),
            threads_started: std::sync::atomic::AtomicUsize::new(0),
            caps_made: Default::default(),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The run is over: every thread still parked on its baton leaves without running Fortis code.
    pub fn shutdown(&self) {
        let batons: Vec<Arc<Baton>> = self.lock().tasks.iter().skip(1).filter(|t| t.state != TState::Done).map(|t| t.baton.clone()).collect();
        for b in batons {
            b.wake(SHUTDOWN);
        }
    }

    /// Tasks launched over the run, by executor (the runtime inventory).
    pub fn inventory(&self) -> (usize, usize, usize, usize, usize, i64) {
        let st = self.lock();
        let all = &st.tasks[1..];
        let by = |e: &str| all.iter().filter(|t| t.executor == e).count();
        (all.len(), by("cooperative"), by("parallel"), by("blocking"), st.detached.len(), st.now)
    }
}

fn can_run(st: &State, t: usize) -> bool {
    let task = &st.tasks[t];
    match task.state {
        TState::Ready => true,
        TState::Suspended => {
            if task.cancelled && !task.ignores_cancel {
                return true;
            }
            if let Some(u) = task.sleep_until {
                return u <= st.now;
            }
            task.waiting_on.map_or(true, |w| st.tasks[w].state == TState::Done)
        }
        _ => false,
    }
}

/// Parent first: the task awaiting `finished` resumes before any other task starts; then ready tasks in launch
/// order; then any other task that can run; when nothing can, virtual time advances to the earliest sleeper.
fn runnable(st: &mut State, finished: Option<usize>, trace: &mut Vec<String>) -> Option<usize> {
    let order: Vec<usize> = (1..st.tasks.len()).chain(std::iter::once(0)).collect();
    if let Some(f) = finished {
        for &t in &order {
            if st.tasks[t].state == TState::Suspended && st.tasks[t].waiting_on == Some(f) && t != st.current {
                return Some(t);
            }
        }
    }
    for t in 1..st.tasks.len() {
        if can_run(st, t) {
            return Some(t);
        }
    }
    if st.tasks[0].state == TState::Suspended && can_run(st, 0) && st.current != 0 {
        return Some(0);
    }
    // only the waiting task itself can go on (its deadline has passed)
    if st.tasks[st.current].state == TState::Suspended && can_run(st, st.current) {
        return Some(st.current);
    }
    let earliest = order.iter().filter(|&&t| st.tasks[t].state == TState::Suspended && !st.tasks[t].cancelled).filter_map(|&t| st.tasks[t].sleep_until).min();
    if let Some(u) = earliest {
        st.now = u;
        trace.push(format!("time {}ms", st.now / 1_000_000));
        return runnable(st, None, trace);
    }
    None
}

fn cancel_exit() -> Exit {
    Exit::Halt(Halt::Cancel)
}

impl<'p> Vm<'p> {
    /// The run's scheduler, created on first use (a synchronous run never creates one).
    pub fn sched(&mut self) -> Arc<Sched> {
        if self.sched.is_none() {
            let mut s = Sched::new(self.prog, &self.file);
            s.caps_made = self.caps_made.clone();
            s.decl_files = self.decl_files.clone();
            self.sched = Some(Arc::new(s));
        }
        self.sched.clone().unwrap()
    }

    fn park(&mut self, st: &mut State) {
        st.globals = Some(Globals {
            out: std::mem::take(&mut self.out),
            trace: std::mem::take(&mut self.trace),
            reports: std::mem::take(&mut self.reports),
            consts: std::mem::take(&mut self.consts),
            failure_lines: std::mem::take(&mut self.failure_lines),
            held_failure: std::mem::take(&mut self.held_failure),
        });
    }

    fn unpark(&mut self, st: &mut State) {
        if let Some(g) = st.globals.take() {
            self.out = g.out;
            self.trace = g.trace;
            self.reports = g.reports;
            self.consts = g.consts;
            self.failure_lines = g.failure_lines;
            self.held_failure = g.held_failure;
        }
    }

    /// Whether the current task has been cancelled (always false without a scheduler: main is never cancelled).
    fn me_cancelled(&self) -> bool {
        match &self.sched {
            Some(s) => s.lock().tasks[self.task].cancelled,
            None => false,
        }
    }

    // ---- creation ---------------------------------------------------------------------------------------------

    /// `scope.launch(work)` (and the pool variants): a task in launch order, ready to run.
    pub fn launch(&mut self, work: Work, fname: &str, scope: Option<&ScopeVal>, executor: &'static str) -> R<Value> {
        if let Some(sc) = scope {
            if sc.tasks.borrow().len() >= SCOPE_BOUND {
                return Err(Exit::Halt(Halt::Defect("out_of_memory".into())));
            }
        }
        let s = self.sched();
        let mut st = s.lock();
        st.counter += 1;
        let name = format!("{fname}#{}", st.counter);
        let root = if self.root != 0 { self.root } else { st.tasks[self.task].root };
        st.tasks.push(TaskSt {
            name: name.clone(),
            fname: fname.to_string(),
            state: TState::Ready,
            cancelled: false,
            ignores_cancel: false,
            waiting_on: None,
            sleep_until: None,
            executor,
            detached: false,
            consumed: false,
            result: None,
            error: None,
            completion: "Completed",
            work: Some(work),
            baton: Arc::new(Baton::default()),
            gen: if crate::session::fault("launch-current") || (crate::session::fault("launch-current-nap") && fname == "nap") { SESSION_CURRENT.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|w| w.upgrade()) } else { self.gen.clone() },
            root,
            session_root: false,
            shown: None,
        });
        let id = st.tasks.len() - 1;
        drop(st);
        if let Some(sc) = scope {
            sc.tasks.borrow_mut().push(id);
        }
        self.trace.push(if executor == "cooperative" { format!("launch {name}") } else { format!("launch {name} on {executor}") });
        Ok(Value::Task(Rc::new(TaskRef { id, name })))
    }

    /// A launched future becomes a task: its work moves into the scheduler.
    pub fn launch_future(&mut self, fut: &FutureVal, scope: Option<&ScopeVal>, executor: &'static str) -> R<Value> {
        let work = match fut.work.borrow_mut().take() {
            Some(w) => w,
            None => return dynamic(format!("Future({}) was already polled", fut.fname)),
        };
        let t = self.launch(work, &fut.fname, scope, executor)?;
        if let Value::Task(r) = &t {
            fut.task.set(Some(r.id));
        }
        Ok(t)
    }


    // ---- switching --------------------------------------------------------------------------------------------

    /// Hand the baton to `t` and wait until it comes back.
    fn switch_to(&mut self, t: usize) -> R<()> {
        let s = self.sched();
        let me = self.task;
        let mut st = s.lock();
        st.current = t;
        let fresh = st.tasks[t].state == TState::Ready;
        st.tasks[t].state = TState::Running;
        if !fresh {
            let name = st.tasks[t].name.clone();
            self.trace.push(format!("resume {name}"));
        }
        self.park(&mut st);
        let baton = st.tasks[t].baton.clone();
        let mine = st.tasks[me].baton.clone();
        let me_done = st.tasks[me].state == TState::Done;
        drop(st);
        if fresh {
            start_thread(&s, t);
        }
        baton.wake(GO);
        if me_done {
            return Ok(());
        }
        if !mine.wait() {
            return Err(Exit::Halt(Halt::Shutdown));
        }
        let mut st = s.lock();
        st.current = me;
        self.unpark(&mut st);
        Ok(())
    }

    /// The run's global state, taken out of this VM so that the baton can be handed on without it.
    fn take_globals(&mut self) -> Globals {
        Globals {
            out: std::mem::take(&mut self.out),
            trace: std::mem::take(&mut self.trace),
            reports: std::mem::take(&mut self.reports),
            consts: std::mem::take(&mut self.consts),
            failure_lines: std::mem::take(&mut self.failure_lines),
            held_failure: std::mem::take(&mut self.held_failure),
        }
    }

    /// The current task waits for `target` to complete, or for virtual time `until` (an await): suspend, run the
    /// other tasks, resume when the condition holds or the task is cancelled.
    pub fn block_until(&mut self, target: Option<usize>, until: Option<i64>) -> R<()> {
        self.wait_for(target, until, true)
    }

    /// The wait of `block_until`; with `observe_cancel` false the task's cancellation neither ends the wait nor
    /// makes it runnable (a cancelled `join` waits for both futures, 12.5).
    fn wait_for(&mut self, target: Option<usize>, until: Option<i64>, observe_cancel: bool) -> R<()> {
        let s = self.sched();
        let me = self.task;
        s.lock().tasks[me].ignores_cancel = !observe_cancel;
        let r = self.wait_loop(target, until, observe_cancel);
        s.lock().tasks[me].ignores_cancel = false;
        r
    }

    fn wait_loop(&mut self, target: Option<usize>, until: Option<i64>, observe_cancel: bool) -> R<()> {
        let s = self.sched();
        let me = self.task;
        loop {
            {
                let st = s.lock();
                let satisfied = match target {
                    Some(t) => st.tasks[t].state == TState::Done,
                    None => st.now >= until.unwrap_or(0),
                };
                if satisfied {
                    return Ok(());
                }
                if st.tasks[me].cancelled && observe_cancel {
                    return Err(cancel_exit());
                }
            }
            let mut st = s.lock();
            st.tasks[me].state = TState::Suspended;
            st.tasks[me].waiting_on = target;
            st.tasks[me].sleep_until = until;
            let name = st.tasks[me].name.clone();
            self.trace.push(format!("suspend {name}"));
            let mut trace = std::mem::take(&mut self.trace);
            let mut nxt = runnable(&mut st, None, &mut trace);
            self.trace = trace;
            if nxt.is_none() || nxt == Some(me) {
                match target {
                    Some(t) if st.tasks[t].state == TState::Ready => nxt = Some(t),
                    _ if nxt == Some(me) => {
                        st.tasks[me].state = TState::Running;
                        st.tasks[me].waiting_on = None;
                        st.tasks[me].sleep_until = None;
                        continue;
                    }
                    _ => {
                        drop(st);
                        return dynamic(format!("executor: {name} waits for something that cannot progress"));
                    }
                }
            }
            drop(st);
            self.switch_to(nxt.unwrap())?;
            let mut st = s.lock();
            st.tasks[me].waiting_on = None;
            st.tasks[me].sleep_until = None;
            st.tasks[me].state = TState::Running;
            if st.tasks[me].cancelled && observe_cancel {
                return Err(cancel_exit());
            }
        }
    }

    /// Run every task that can run at the current virtual time, then return: a development session's safe point
    /// (SP-07 design 2). The caller sleeps until now, which the scheduler reaches only when no other task can run
    /// without time moving.
    pub fn settle(&mut self) -> R<()> {
        let s = self.sched();
        let me = self.task;
        loop {
            let mut st = s.lock();
            if !(1..st.tasks.len()).any(|t| t != me && can_run(&st, t)) {
                return Ok(());
            }
            st.tasks[me].state = TState::Suspended;
            st.tasks[me].waiting_on = None;
            st.tasks[me].sleep_until = Some(st.now);
            let mut trace = std::mem::take(&mut self.trace);
            let nxt = runnable(&mut st, None, &mut trace);
            self.trace = trace;
            match nxt {
                Some(n) if n != me => {
                    drop(st);
                    self.switch_to(n)?;
                }
                _ => {
                    st.tasks[me].state = TState::Running;
                    st.tasks[me].sleep_until = None;
                    return Ok(());
                }
            }
            let mut st = s.lock();
            st.tasks[me].state = TState::Running;
            st.tasks[me].sleep_until = None;
        }
    }

    /// `checkpoint()` (12.4): a cancellation point for parallel and blocking tasks, modelled as a yield so the
    /// parent may act; cooperative tasks observe cancellation only at `await`.
    pub fn checkpoint(&mut self) -> R<()> {
        let s = self.sched();
        let me = self.task;
        let mut st = s.lock();
        if st.tasks[me].executor == "cooperative" {
            return Ok(());
        }
        let name = st.tasks[me].name.clone();
        self.trace.push(format!("checkpoint {name}"));
        if st.tasks[me].cancelled {
            self.trace.push(format!("cancelled {name}"));
            return Err(cancel_exit());
        }
        st.tasks[me].state = TState::Suspended;
        st.tasks[me].waiting_on = None;
        st.tasks[me].sleep_until = Some(st.now);
        let order: Vec<usize> = (1..st.tasks.len()).chain(std::iter::once(0)).collect();
        let nxt = order.iter().copied().find(|&t| t != me && st.tasks[t].state == TState::Suspended && can_run(&st, t)).or_else(|| (1..st.tasks.len()).find(|&t| t != me && st.tasks[t].state == TState::Ready));
        let nxt = match nxt {
            Some(n) => n,
            None => {
                st.tasks[me].state = TState::Running;
                st.tasks[me].sleep_until = None;
                return Ok(());
            }
        };
        drop(st);
        self.switch_to(nxt)?;
        let mut st = s.lock();
        st.tasks[me].state = TState::Running;
        st.tasks[me].waiting_on = None;
        st.tasks[me].sleep_until = None;
        if st.tasks[me].cancelled {
            self.trace.push(format!("cancelled {name}"));
            return Err(cancel_exit());
        }
        Ok(())
    }

    // ---- cancellation and joins -------------------------------------------------------------------------------

    pub fn cancel(&mut self, t: usize) {
        let s = self.sched();
        let mut st = s.lock();
        // the fault switch `stall-nap` makes a task of `nap` ignore cancellation: a drain then stalls (G3)
        if crate::session::fault("stall-nap") && st.tasks[t].fname == "nap" {
            return;
        }
        if st.tasks[t].state != TState::Done && !st.tasks[t].cancelled {
            st.tasks[t].cancelled = true;
            let name = st.tasks[t].name.clone();
            drop(st);
            self.trace.push(format!("cancel {name}"));
        }
    }

    pub fn task_done(&mut self, t: usize) -> bool {
        self.sched().lock().tasks[t].state == TState::Done
    }

    pub fn task_name(&mut self, t: usize) -> String {
        self.sched().lock().tasks[t].name.clone()
    }

    /// Run `t` to completion from the current task (`Task.abandon`).
    pub fn join_task(&mut self, t: usize) -> R<()> {
        if !self.task_done(t) {
            self.block_until(Some(t), None)?;
        }
        let name = self.task_name(t);
        self.trace.push(format!("join {name}"));
        let err = self.sched().lock().tasks[t].error.take();
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// `scope.detach(task, supervisor)`: the supervisor takes over the join obligation (12.5).
    pub fn detach(&mut self, scope: &ScopeVal, t: usize) -> R<()> {
        let s = self.sched();
        let mut st = s.lock();
        if st.detached.len() >= SCOPE_BOUND {
            return Err(Exit::Halt(Halt::Defect("out_of_memory".into())));
        }
        st.tasks[t].detached = true;
        st.detached.push(t);
        let name = st.tasks[t].name.clone();
        // in a session, a task that had already completed when detached has its result consumed now, in this
        // (its) generation: its supervisor never reads it
        let done = if self.gen.is_some() && st.tasks[t].state == TState::Done && !crate::session::fault("detached-keeps-result") {
            Some((st.tasks[t].completion, st.tasks[t].result.take(), st.tasks[t].error.take()))
        } else {
            None
        };
        drop(st);
        if let Some((completion, result, error)) = done {
            let _ = crate::session::show_result(self, completion, result, error);
        }
        scope.tasks.borrow_mut().retain(|x| *x != t);
        self.trace.push(format!("detach {name}"));
        Ok(())
    }

    /// At the end of `main`, detached tasks run to completion (their supervisor observes the completion).
    pub fn drain_detached(&mut self) -> R<()> {
        let s = match &self.sched {
            Some(s) => s.clone(),
            None => return Ok(()),
        };
        let list: Vec<usize> = s.lock().detached.clone();
        for t in list {
            if !self.task_done(t) {
                self.block_until(Some(t), None)?;
            }
        }
        Ok(())
    }

    /// A scope's exit (12.5): its unjoined tasks are cancelled, then each is joined in launch order; a task's
    /// defect reaches the scope's owner unless the block is already unwinding; a never-awaited failure is
    /// reported as abandoned.
    pub fn exit_scope(&mut self, scope: &ScopeVal, unwinding: bool) -> R<()> {
        let tasks: Vec<usize> = scope.tasks.borrow().clone();
        for &t in &tasks {
            if !self.task_done(t) {
                self.cancel(t);
            }
        }
        for &t in &tasks {
            let consumed = self.sched().lock().tasks[t].consumed;
            if consumed {
                continue;
            }
            if !self.task_done(t) {
                self.block_until(Some(t), None)?;
            }
            let name = self.task_name(t);
            self.trace.push(format!("join {name}"));
            let (err, result) = {
                let s = self.sched();
                let mut st = s.lock();
                (if unwinding { None } else { st.tasks[t].error.take() }, st.tasks[t].result.take())
            };
            if let Some(e) = err {
                return Err(e);
            }
            if let Some(r) = result {
                if r.is_failure() {
                    self.emit_report(&r, "abandoned");
                }
            }
        }
        scope.tasks.borrow_mut().clear();
        Ok(())
    }

    // ---- await --------------------------------------------------------------------------------------------------

    /// `await e` (12.3): a never-polled future runs inline; a task is awaited through the scheduler; cancellation
    /// of the current task is observed here and only here.
    pub fn await_value(&mut self, v: Value) -> R<Value> {
        if self.me_cancelled() {
            let name = self.task_name(self.task);
            self.trace.push(format!("cancelled {name}"));
            return Err(cancel_exit());
        }
        match v.deref() {
            Value::Ready(x) => Ok((*x).clone()),
            Value::Join(futs) => {
                // both futures run to completion inside the parent, even when one fails or the parent is cancelled
                let mut tasks = vec![];
                for f in futs.iter() {
                    match f {
                        Value::Future(fv) => {
                            if let Value::Task(t) = self.launch_future(fv, None, "cooperative")? {
                                tasks.push(t.id);
                            }
                        }
                        other => return dynamic(format!("join of a value that is not a future: {}", display(other))),
                    }
                }
                let mut results = vec![];
                for &t in &tasks {
                    if let Err(e) = self.block_until(Some(t), None) {
                        if matches!(e, Exit::Halt(Halt::Cancel)) {
                            for &u in &tasks {
                                self.cancel(u);
                            }
                            for &u in &tasks {
                                if !self.task_done(u) {
                                    self.wait_for(Some(u), None, false)?;
                                }
                            }
                            let me = self.task_name(self.task);
                            self.trace.push(format!("cancelled {me}"));
                        }
                        return Err(e);
                    }
                    let name = self.task_name(t);
                    self.trace.push(format!("join {name}"));
                    let (err, result) = {
                        let s = self.sched();
                        let mut st = s.lock();
                        st.tasks[t].consumed = true;
                        (st.tasks[t].error.take(), st.tasks[t].result.take())
                    };
                    if let Some(e) = err {
                        return Err(e);
                    }
                    results.push(result.unwrap_or(Value::Void));
                }
                Ok(Value::Tuple(Rc::new(results)))
            }
            Value::Sleep(ns) => {
                let until = self.sched().lock().now + ns;
                if let Err(e) = self.block_until(None, Some(until)) {
                    if matches!(e, Exit::Halt(Halt::Cancel)) {
                        let me = self.task_name(self.task);
                        self.trace.push(format!("cancelled {me}"));
                    }
                    return Err(e);
                }
                Ok(Value::Void)
            }
            Value::Future(f) => match f.task.get() {
                None => {
                    let work = match f.work.borrow_mut().take() {
                        Some(w) => w,
                        None => return dynamic(format!("Future({}) awaited twice", f.fname)),
                    };
                    self.run_work(work)
                }
                Some(t) => self.await_task(t),
            },
            Value::Task(t) => self.await_task(t.id),
            other => dynamic(format!("await of a value that is not a future or task: {}", display(&other))),
        }
    }

    fn await_task(&mut self, t: usize) -> R<Value> {
        if !self.task_done(t) {
            if let Err(e) = self.block_until(Some(t), None) {
                if matches!(e, Exit::Halt(Halt::Cancel)) {
                    let me = self.task_name(self.task);
                    self.trace.push(format!("cancelled {me}"));
                }
                return Err(e);
            }
        }
        let name = self.task_name(t);
        self.trace.push(format!("join {name}"));
        let (err, result, completion) = {
            let s = self.sched();
            let mut st = s.lock();
            st.tasks[t].consumed = true;
            (st.tasks[t].error.take(), st.tasks[t].result.take(), st.tasks[t].completion)
        };
        if let Some(e) = err {
            return Err(e);
        }
        if completion == "Cancelled" {
            return dynamic(format!("awaited task {name} was cancelled: cancellation of a child reaches the parent (12.5)"));
        }
        Ok(result.unwrap_or(Value::Void))
    }

    /// Run a future's work in the current task (inline at its first await, or as a launched task).
    pub fn run_work(&mut self, w: Work) -> R<Value> {
        match w {
            Work::Call { func, frame, code } => {
                let (func, frame) = self.rebind(func, frame, code.as_deref())?;
                self.run_frame(func, frame)
            }
            // launched work is owned by its task: the call consumes it (7.9, X-250; D-30)
            Work::Closure(c) => self.call_consuming(c, vec![]),
        }
    }

    fn task_completion(&mut self, name: &str, outcome: &str) {
        self.emit(vec![("kind".into(), "task-completion".into()), ("task".into(), name.into()), ("outcome".into(), outcome.into()), ("rule".into(), "LANG-12-04".into())]);
    }

    pub(crate) fn not_started(&mut self, fname: &str) {
        self.emit(vec![("kind".into(), "task-lifecycle".into()), ("function".into(), fname.into()), ("state".into(), "not_started".into()), ("rule".into(), "LANG-12-03".into())]);
        let schema = self.prog.functions.iter().find(|f| Self::source_name_of(f) == fname).and_then(|f| f.observe.clone());
        if let Some(schema) = schema {
            self.observation(&schema, Some(fname), &[], Some("not_started"));
        }
    }
}

fn start_thread(s: &Arc<Sched>, t: usize) {
    s.threads_started.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    THREADS_LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let s2 = s.clone();
    std::thread::Builder::new().name(format!("fortis-task-{t}")).stack_size(TASK_STACK).spawn(move || task_body(s2, t)).expect("spawn a task thread");
}

/// A task's thread: wait for the baton, run the future, record the completion, hand the baton on.
fn task_body(s: Arc<Sched>, t: usize) {
    let _count = ThreadCount;
    let shared = s.prog.clone();
    // a session task runs its own generation's program (SP-07 design 5)
    let gen = s.lock().tasks[t].gen.clone();
    let globals = {
        let prog: &Program = match &gen {
            Some(g) => &g.program,
            None => &shared,
        };
        let mut vm = Vm::new(prog, &s.file);
        vm.decl_files = s.decl_files.clone();
        vm.gen = gen.clone();
        vm.caps_made = s.caps_made.clone();
        vm.sched = Some(s.clone());
        vm.task = t;
        let baton = s.lock().tasks[t].baton.clone();
        if !baton.wait() {
            return;
        }
        {
            let mut st = s.lock();
            vm.unpark(&mut st);
        }
        let (cancelled, work, name, fname) = {
            let mut st = s.lock();
            let task = &mut st.tasks[t];
            (task.cancelled, task.work.take(), task.name.clone(), task.fname.clone())
        };
        let (result, error, completion) = if cancelled {
            vm.trace.push(format!("cancelled {name}"));
            vm.not_started(&fname);
            // what the task's work owned goes with it: its owned arguments, a consumed closure (7.9, 12.3; D-30)
            match work.map(|w| vm.destroy_work(w)) {
                Some(Err(e)) if !matches!(e, Exit::Halt(Halt::Cancel)) => (None, Some(e), "Defect"),
                _ => (None, None, "Cancelled"),
            }
        } else {
            vm.trace.push(format!("start {name}"));
            match work.map(|w| vm.run_work(w)) {
                Some(Ok(v)) => (Some(v), None, "Completed"),
                Some(Err(Exit::Halt(Halt::Cancel))) => (None, None, "Cancelled"),
                Some(Err(Exit::Halt(Halt::Shutdown))) => return,
                Some(Err(e)) => (None, Some(e), "Defect"),
                None => (None, None, "Completed"),
            }
        };
        // a session's root invocation: its result is shown and destroyed here, in the generation that made it
        let (session_root, detached_now) = {
            let st = s.lock();
            (st.tasks[t].session_root, st.tasks[t].detached)
        };
        // in a session, a detached task's result is its supervisor's to consume: destroyed here, in its generation
        let dispose = detached_now && vm.gen.is_some() && !crate::session::fault("detached-keeps-result");
        let (result, error, shown) = if session_root {
            (None, None, Some(crate::session::show_result(&mut vm, completion, result, error)))
        } else if dispose {
            let _ = crate::session::show_result(&mut vm, completion, result, error);
            (None, None, None)
        } else {
            (result, error, None)
        };
        let detached = {
            let mut st = s.lock();
            let task = &mut st.tasks[t];
            task.result = result;
            task.error = error;
            task.shown = shown;
            task.completion = completion;
            task.state = TState::Done;
            if !crate::session::fault("task-keeps-gen") {
                task.gen = None;
            }
            task.detached
        };
        vm.trace.push(format!("complete {name}"));
        if detached {
            vm.task_completion(&name, completion);
        }
        vm.take_globals()
    };
    // the task's last pin of its generation goes before the baton does, so a session sees the generation free at
    // its next safe point (SP-07 design 8)
    drop(gen);
    hand_off(&s, t, globals);
}

/// A task finished: wake whoever runs next. The finishing thread then ends.
fn hand_off(s: &Arc<Sched>, finished: usize, mut g: Globals) {
    let mut st = s.lock();
    let nxt = runnable(&mut st, Some(finished), &mut g.trace);
    let nxt = match nxt {
        Some(n) => n,
        None => {
            // nothing can run: the main task is waiting for something that cannot progress
            g.trace.push("executor: no runnable task".into());
            0
        }
    };
    st.current = nxt;
    let fresh = st.tasks[nxt].state == TState::Ready;
    st.tasks[nxt].state = TState::Running;
    if !fresh {
        let name = st.tasks[nxt].name.clone();
        g.trace.push(format!("resume {name}"));
    }
    st.globals = Some(g);
    let baton = st.tasks[nxt].baton.clone();
    drop(st);
    if fresh {
        start_thread(s, nxt);
    }
    baton.wake(GO);
}
