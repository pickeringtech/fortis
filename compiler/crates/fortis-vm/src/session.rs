//! The development session (spikes/SP-07/design.md 2 and 5–9): a host that keeps a program's long-lived state in
//! scopes, runs each root invocation on the generation current when it starts, reloads new source, and restarts the
//! scopes a reload affects. `fortisc dev` drives it from a script; the session writes a semantic event log (JSON
//! lines) and, apart, the physical measurements (resident memory, live generation objects, task threads).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use fortis_diag::Json;
use fortis_mir::model::{Mode, Program};
use fortis_types::types::Type;

use crate::executor::{Sched, TState};
use crate::generation::{self, Change, Generation, Item};
use crate::machine::{Exit, Flow, Halt, Record, Vm};
use crate::value::{display, Value};

/// The message prefix of a version conflict (design 6, R7): a code value that does not resolve at use.
pub const CONFLICT: &str = "version conflict: ";

/// Whether VMs of a session log every frame they run (root invocation, generation, function): the N1 check.
pub static FRAME_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A compiled source: what a reload installs.
pub struct Compiled {
    pub program: Program,
    pub sha256: String,
    /// each DI context with the classes the elaborator generated for it
    pub di: Vec<(String, Vec<String>)>,
    /// the identities of functions declared generic (type parameters on the function or on its type)
    pub generic: BTreeSet<String>,
}

/// The compiler a session uses: fortisc's pipeline (check, elaborate, IR, link, validate), or the diagnostics that
/// reject the source.
pub type Compile<'c> = &'c (dyn Fn(&Path) -> Result<Compiled, Vec<String>> + Sync);

/// A fault switch of the apparatus check (design 13): only a build with the `reload-faults` feature has them.
pub fn fault(name: &str) -> bool {
    #[cfg(feature = "reload-faults")]
    {
        // a comma-separated list: a fault-build scenario's own switch, and the apparatus check's fault under test
        std::env::var("FORTIS_RELOAD_FAULT").map(|v| v.split(',').any(|f| f == name)).unwrap_or(false)
    }
    #[cfg(not(feature = "reload-faults"))]
    {
        let _ = name;
        false
    }
}

struct Scope {
    factory: String,
    factory_ident: String,
    args: Vec<(String, Json)>,
    state: Option<Value>,
    state_ty: Type,
    stopped: Option<String>,
    /// a stalled scope's state is kept until its tasks finish, then released on this generation (design 6)
    held: Option<Arc<Generation>>,
}

struct Inv {
    id: String,
    scope: String,
    gen: u32,
    /// the task running it (None: a synchronous invocation, already complete)
    task: Option<usize>,
    done: bool,
}

/// What a reload does to a scope (design 6).
enum Plan {
    Restart { rule: &'static str, ident: String, change: String, path: Vec<String> },
    Stop { why: String },
    OldReaders { gens: BTreeSet<u32>, ident: String, change: String, path: Vec<String> },
}

pub struct Session<'c> {
    compile: Compile<'c>,
    dir: PathBuf,
    file: String,
    sched: Arc<Sched>,
    current: Option<Arc<Generation>>,
    gens: BTreeMap<u32, (Weak<Generation>, usize)>,
    reclaimed: BTreeSet<u32>,
    scopes: BTreeMap<String, Scope>,
    invs: BTreeMap<usize, Inv>,
    next_root: usize,
    next_gen: u32,
    caps: (usize, usize),
    seq: usize,
    /// restarts for version conflicts met since the last operation (R7)
    pending: BTreeMap<String, Plan>,
    /// the event log, written as the session goes so that it holds nothing of it (the retention measure, design 12)
    sink: &'c mut (dyn std::io::Write + Send),
    /// the frame log, when asked for (N1)
    frames: Option<&'c mut (dyn std::io::Write + Send)>,
    /// a write to the event log or the frame log failed: the run is reported as a host failure
    write_failed: bool,
    pub metrics: Vec<String>,
    out: Vec<String>,
    trace: Vec<String>,
    reports: Vec<Record>,
    consts: HashMap<String, Value>,
}

fn ms(ns: i64) -> i64 {
    ns / 1_000_000
}

fn show_failure(v: &Value) -> String {
    match v {
        Value::Failure(f) => {
            let f = f.borrow();
            let payload: Vec<String> = f.payload.iter().map(|(k, v)| format!("{k}={}", display(v))).collect();
            let cause = f.cause.as_ref().map(|c| format!(" cause {}", show_failure(&Value::Failure(c.clone())))).unwrap_or_default();
            format!("{}{{{}}} \"{}\"{cause}", f.code, payload.join(", "), f.message)
        }
        other => display(other),
    }
}

fn arg_value(j: &Json) -> Value {
    match j {
        Json::Int(i) => Value::Int(*i as i128),
        Json::Str(s) => Value::str(s),
        Json::Bool(b) => Value::Bool(*b),
        Json::Arr(xs) => Value::list(xs.iter().map(arg_value).collect()),
        _ => Value::Void,
    }
}

/// Whether a script argument fits a parameter's type, refinements and list elements included (the check of a
/// factory's arguments, at its start and in a restart recipe, R4).
fn arg_fits(j: &Json, t: &Type) -> bool {
    match t {
        Type::Loan { inner, .. } => arg_fits(j, inner),
        Type::Refined { base, lo, hi, .. } => {
            if fault("recipe-unrefined") {
                return arg_fits(j, &Type::Named(base.clone()));
            }
            match (j, base.name.as_str()) {
                (Json::Int(v), "Int") => lo.map_or(true, |l| (*v as i128) >= l) && hi.map_or(true, |h| (*v as i128) <= h),
                (Json::Str(v), "String") => {
                    let n = v.chars().count() as i128;
                    lo.map_or(true, |l| n >= l) && hi.map_or(true, |h| n <= h)
                }
                _ => false,
            }
        }
        Type::Named(n) => match (j, n.name.as_str()) {
            (Json::Int(_), "Int") | (Json::Str(_), "String") | (Json::Bool(_), "Bool") => true,
            (Json::Arr(xs), "List") => n.args.first().map_or(true, |et| xs.iter().all(|x| arg_fits(x, et))),
            _ => false,
        },
        _ => false,
    }
}

fn type_name(t: &Type) -> Option<&str> {
    match t {
        Type::Named(n) => Some(n.name.as_str()),
        Type::Loan { inner, .. } => type_name(inner),
        _ => None,
    }
}

fn rss_kb() -> i64 {
    std::fs::read_to_string("/proc/self/statm").ok().and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<i64>().ok())).map(|p| p * 4).unwrap_or(0)
}

impl<'c> Session<'c> {
    pub fn new(compile: Compile<'c>, dir: &Path, sink: &'c mut (dyn std::io::Write + Send), frames: Option<&'c mut (dyn std::io::Write + Send)>) -> Session<'c> {
        FRAME_LOG.store(frames.is_some(), Ordering::SeqCst);
        Session {
            compile,
            dir: dir.to_path_buf(),
            file: String::new(),
            // the scheduler of a session holds no program: every task runs its own generation's (design 8)
            sched: Arc::new(Sched::new(&Program::default(), "session")),
            current: None,
            gens: BTreeMap::new(),
            reclaimed: BTreeSet::new(),
            scopes: BTreeMap::new(),
            invs: BTreeMap::new(),
            next_root: 1,
            next_gen: 1,
            caps: (usize::MAX, usize::MAX),
            seq: 0,
            pending: BTreeMap::new(),
            sink,
            frames,
            write_failed: false,
            metrics: vec![],
            out: vec![],
            trace: vec![],
            reports: vec![],
            consts: HashMap::new(),
        }
    }

    fn event(&mut self, kind: &str, mut fields: Vec<(&str, Json)>) {
        fields.push(("event", Json::str(kind)));
        let mut line = Json::obj(fields).to_line();
        line.push('\n');
        if self.sink.write_all(line.as_bytes()).is_err() {
            self.write_failed = true;
        }
    }

    fn now(&self) -> i64 {
        ms(self.sched.lock().now)
    }

    /// The driver's VM on a generation: task 0, holding the run's global state while the driver has the baton.
    fn vm<'g>(&mut self, g: &'g Arc<Generation>) -> Vm<'g> {
        let mut vm = Vm::new(&g.program, &self.file);
        vm.sched = Some(self.sched.clone());
        vm.task = 0;
        vm.gen = Some(g.clone());
        vm.out = std::mem::take(&mut self.out);
        vm.trace = std::mem::take(&mut self.trace);
        vm.reports = std::mem::take(&mut self.reports);
        vm.consts = std::mem::take(&mut self.consts);
        vm
    }

    /// Take the global state back from the driver's VM, logging the lines tasks printed (attributed when emitted).
    fn park(&mut self, mut vm: Vm) {
        let reports = std::mem::take(&mut vm.reports);
        self.consts = std::mem::take(&mut vm.consts);
        // console lines are logged from their records; the trace is not kept, so its memory stays bounded
        drop(vm);
        for r in reports {
            let get = |k: &str| r.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
            if get("kind") == "frame" {
                // the fault switch `frames-off` drops the frame log's records
                if fault("frames-off") {
                    continue;
                }
                if let Some(f) = self.frames.as_mut() {
                    let root: usize = get("root").parse().unwrap_or(0);
                    let id = self.invs.get(&root).map(|i| i.id.clone()).unwrap_or_else(|| "session".into());
                    let mut line = Json::obj([("id", Json::str(id)), ("gen", Json::Int(get("gen").parse().unwrap_or(0))), ("ident", Json::str(get("ident")))]).to_line();
                    line.push('\n');
                    if f.write_all(line.as_bytes()).is_err() {
                        self.write_failed = true;
                    }
                }
                continue;
            }
            if get("kind") == "print" {
                let root: usize = get("root").parse().unwrap_or(0);
                let id = self.invs.get(&root).map(|i| i.id.clone()).unwrap_or_else(|| "session".into());
                let mut gen: i64 = get("gen").parse().unwrap_or(0);
                if fault("print-at-collect") {
                    gen = self.current.as_ref().map(|g| g.id as i64).unwrap_or(gen);
                }
                self.event("print", vec![("id", Json::str(id)), ("gen", Json::Int(gen)), ("text", Json::str(get("text")))]);
            }
        }
    }

    fn current(&self) -> Arc<Generation> {
        self.current.clone().expect("an open session")
    }

    // ---- the operations ----------------------------------------------------------------------------------------

    pub fn run(&mut self, ops: &[Json]) {
        for (i, op) in ops.iter().enumerate() {
            let name = op.get("op").and_then(|o| o.as_str()).unwrap_or("").to_string();
            // every operation's events follow its marker, so a reader can align the log with the script
            self.event("op", vec![("index", Json::Int(i as i64)), ("op", Json::str(&name))]);
            match name.as_str() {
                "open" => self.open(op),
                "start" => self.start(op),
                "call" => self.call(op),
                "run" => self.run_until(op),
                "reload" => self.reload(op),
                "restart" => {
                    let s = op.get("scope").and_then(|s| s.as_str()).unwrap_or("").to_string();
                    let mut plans = BTreeMap::new();
                    plans.insert(s, Plan::Restart { rule: "requested", ident: String::new(), change: String::new(), path: vec![] });
                    self.apply(plans, None);
                }
                "caps" => {
                    let g = op.get("generations").and_then(|v| v.as_i64()).unwrap_or(i64::MAX).max(1) as usize;
                    let b = op.get("bytes").and_then(|v| v.as_i64()).unwrap_or(i64::MAX) as usize;
                    self.caps = (g, b);
                    self.event("caps", vec![("generations", Json::Int(g as i64)), ("bytes", Json::Int(b.min(i64::MAX as usize) as i64))]);
                    let mut plans = self.evictions(None, &BTreeSet::new());
                    if let Some(Plan::Stop { why }) = plans.remove("") {
                        self.event("caps-unmet", vec![("message", Json::str(why))]);
                    }
                    if !plans.is_empty() {
                        self.apply(plans, None);
                    }
                }
                "snapshot" => self.snapshot(i),
                "finish" => self.finish(i),
                other => self.event("error", vec![("op", Json::str(other)), ("message", Json::str("unknown operation"))]),
            }
            self.collect();
            if !self.pending.is_empty() {
                let p = std::mem::take(&mut self.pending);
                self.apply(p, None);
                self.collect();
            }
            self.reclaim();
            // a session whose source never compiled has nothing to run the rest of the script on
            if self.current.is_none() {
                break;
            }
        }
        self.sched.shutdown();
    }

    fn compile_gen(&mut self, path: &str) -> Result<Arc<Generation>, Vec<String>> {
        let full = self.dir.join(path);
        let c = (self.compile)(&full)?;
        let id = self.next_gen;
        let prev = self.current.clone();
        Ok(Arc::new(Generation::new(id, c.program, path, &c.sha256, &c.di, &c.generic, prev.as_deref())))
    }

    fn install(&mut self, g: Arc<Generation>) {
        self.next_gen = g.id + 1;
        // the fault switch `zero-bytes` makes the memory cap's accounting count nothing
        let bytes = if fault("zero-bytes") { 0 } else { g.bytes };
        self.gens.insert(g.id, (Arc::downgrade(&g), bytes));
        self.event("generation", vec![("gen", Json::Int(g.id as i64)), ("source", Json::str(&g.source)), ("sha256", Json::str(&g.sha256)), ("bytes", Json::Int(bytes as i64))]);
        if fault("session-holds-strong") {
            std::mem::forget(g.clone());
        }
        *crate::executor::SESSION_CURRENT.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::downgrade(&g));
        self.current = Some(g);
    }

    fn open(&mut self, op: &Json) {
        let src = op.get("source").and_then(|s| s.as_str()).unwrap_or("").to_string();
        self.file = src.clone();
        match self.compile_gen(&src) {
            Ok(g) => self.install(g),
            Err(d) => self.event("rejected", vec![("source", Json::str(src)), ("diagnostics", Json::strs(d))]),
        }
    }

    /// Call a scope's factory on a generation: the scope's state, or why it could not be built.
    fn build(&mut self, g: &Arc<Generation>, name: &str, factory: &str, args: &[(String, Json)]) -> Result<(Value, Type, String), String> {
        let (ident, f) = match g.function_named(factory) {
            Some((i, f)) => (i.clone(), f.clone()),
            None => return Err(format!("no factory `{factory}` in generation {}", g.id)),
        };
        if f.is_async || type_name(&f.ret) != Some("Shared") {
            return Err(format!("factory `{factory}` must be synchronous and return Shared[…]"));
        }
        let func = &g.program.functions[f.index];
        let mut vals = Vec::new();
        for (p, t) in func.params.iter().zip(f.params.iter()) {
            if let Some(kind) = type_name(t).and_then(crate::intrinsics::cap_kind) {
                vals.push((p.name.clone(), crate::intrinsics::cap(kind)));
                continue;
            }
            match args.iter().find(|(n, _)| *n == p.name) {
                Some((_, j)) if arg_fits(j, t) => vals.push((p.name.clone(), arg_value(j))),
                Some(_) => return Err(format!("argument `{}` does not fit {t}", p.name)),
                None if p.default.is_some() => {}
                None => return Err(format!("missing argument `{}`", p.name)),
            }
        }
        let root = self.next_root;
        self.next_root += 1;
        self.invs.insert(root, Inv { id: format!("{name}#factory@{}", g.id), scope: name.to_string(), gen: g.id, task: None, done: true });
        let mut vm = self.vm(g);
        vm.root = root;
        let r = vm.call(f.index, vals, &[]);
        vm.root = 0;
        self.park(vm);
        match r {
            Ok(v) if v.is_failure() => Err(format!("factory failed: {}", show_failure(&v))),
            Ok(v) => Ok((v, f.ret.clone(), ident)),
            Err(Exit::Flow(Flow::Fail(v))) => Err(format!("factory failed: {}", show_failure(&v))),
            Err(e) => Err(format!("factory ended: {e:?}")),
        }
    }

    fn start(&mut self, op: &Json) {
        let name = op.get("scope").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let factory = op.get("factory").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let args: Vec<(String, Json)> = match op.get("args") {
            Some(Json::Obj(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => vec![],
        };
        let g = self.current();
        match self.build(&g, &name, &factory, &args) {
            Ok((state, ty, ident)) => {
                self.event("started", vec![("scope", Json::str(&name)), ("gen", Json::Int(g.id as i64)), ("factory", Json::str(&ident))]);
                self.scopes.insert(name, Scope { factory, factory_ident: ident, args, state: Some(state), state_ty: ty, stopped: None, held: None });
            }
            Err(why) => self.event("error", vec![("op", Json::str("start")), ("scope", Json::str(&name)), ("message", Json::str(why))]),
        }
    }

    fn call(&mut self, op: &Json) {
        let id = op.get("id").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let sname = op.get("scope").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let handler = op.get("handler").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let args: Vec<(String, Json)> = match op.get("args") {
            Some(Json::Obj(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => vec![],
        };
        let err = |s: &mut Self, m: String| s.event("error", vec![("op", Json::str("call")), ("id", Json::str(&id)), ("message", Json::str(m))]);
        let (state, state_ty) = match self.scopes.get(&sname) {
            Some(Scope { stopped: Some(why), .. }) => return err(self, format!("scope `{sname}` is stopped: {why}")),
            Some(Scope { state: Some(s), state_ty, .. }) => (s.clone(), state_ty.clone()),
            _ => return err(self, format!("no scope `{sname}`")),
        };
        let mut g = self.current();
        if fault("call-oldest") {
            if let Some(old) = self.gens.values().filter_map(|(w, _)| w.upgrade()).min_by_key(|x| x.id) {
                g = old;
            }
        }
        let f = match g.function_named(&handler) {
            Some((_, f)) => f.clone(),
            None => return err(self, format!("no handler `{handler}` in generation {}", g.id)),
        };
        let func = &g.program.functions[f.index];
        if !f.params.first().map(|p| generation::unify(p, &state_ty)).unwrap_or(false) {
            return err(self, format!("handler `{handler}` does not take the scope's state first"));
        }
        let root = self.next_root;
        self.next_root += 1;
        let mut vm = self.vm(&g);
        let mut vals = Vec::new();
        let mut problem = None;
        for (k, (p, t)) in func.params.iter().zip(f.params.iter()).enumerate() {
            if k == 0 {
                // an owned parameter gets its own handle, which its frame releases; a loan borrows the session's
                let v = if p.mode == Mode::Owned { crate::intrinsics::call(&mut vm, "Shared.retain", vec![state.clone()]) } else { Ok(state.clone()) };
                match v {
                    Ok(v) => vals.push((p.name.clone(), v)),
                    Err(e) => problem = Some(format!("{e:?}")),
                }
                continue;
            }
            if let Some(kind) = type_name(t).and_then(crate::intrinsics::cap_kind) {
                vals.push((p.name.clone(), crate::intrinsics::cap(kind)));
                continue;
            }
            match args.iter().find(|(n, _)| *n == p.name) {
                Some((_, j)) => vals.push((p.name.clone(), arg_value(j))),
                None if p.default.is_some() => {}
                None => problem = Some(format!("missing argument `{}`", p.name)),
            }
        }
        if let Some(m) = problem {
            self.park(vm);
            return err(self, m);
        }
        let at = ms(self.sched.lock().now);
        self.invs.insert(root, Inv { id: id.clone(), scope: sname.clone(), gen: g.id, task: None, done: false });
        vm.root = root;
        let r = vm.call(f.index, vals, &[]);
        let r = match r {
            Ok(Value::Future(fut)) => match vm.launch_future(&fut, None, "cooperative") {
                Ok(Value::Task(t)) => {
                    self.sched.lock().tasks[t.id].session_root = true;
                    self.invs.get_mut(&root).unwrap().task = Some(t.id);
                    None
                }
                Ok(other) => Some(Ok(other)),
                Err(e) => Some(Err(e)),
            },
            other => Some(other),
        };
        vm.root = 0;
        self.event("invocation", vec![("id", Json::str(&id)), ("scope", Json::str(&sname)), ("handler", Json::str(&handler)), ("gen", Json::Int(g.id as i64)), ("at", Json::Int(at))]);
        match r {
            None => {
                let _ = vm.settle();
                self.park(vm);
            }
            Some(r) => {
                // a synchronous invocation ran on the driver's VM, in its own generation: shown and destroyed there
                vm.root = root;
                let (outcome, value) = match r {
                    Ok(v) => show_result(&mut vm, "Completed", Some(v), None),
                    Err(e) => show_result(&mut vm, "Defect", None, Some(e)),
                };
                vm.root = 0;
                let _ = vm.settle();
                self.park(vm);
                self.invs.get_mut(&root).unwrap().done = true;
                self.finished(root, outcome, value);
            }
        }
    }

    /// Log an invocation's completion; a version conflict also schedules its scope's restart (R7).
    fn finished(&mut self, root: usize, outcome: &str, value: String) {
        let (id, gen, scope) = {
            let i = &self.invs[&root];
            (i.id.clone(), i.gen, i.scope.clone())
        };
        if outcome == "conflict" {
            self.event("conflict", vec![("id", Json::str(&id)), ("gen", Json::Int(gen as i64)), ("message", Json::str(&value))]);
            self.pending.insert(scope, Plan::Restart { rule: "version-conflict", ident: String::new(), change: value.clone(), path: vec![format!("invocation {id}")] });
        }
        self.completed(&id, gen, outcome, value);
    }

    fn completed(&mut self, id: &str, gen: u32, outcome: &str, value: String) {
        let at = self.now();
        self.event("completed", vec![("id", Json::str(id)), ("gen", Json::Int(gen as i64)), ("outcome", Json::str(outcome)), ("value", Json::str(value)), ("at", Json::Int(at))]);
    }

    fn run_until(&mut self, op: &Json) {
        let t = op.get("until").and_then(|v| v.as_i64()).unwrap_or(0) * 1_000_000;
        let g = self.current();
        let mut vm = self.vm(&g);
        if let Err(e) = vm.block_until(None, Some(t)) {
            self.park(vm);
            return self.event("error", vec![("op", Json::str("run")), ("message", Json::str(format!("{e:?}")))]);
        }
        let _ = vm.settle();
        self.park(vm);
    }

    /// Log every invocation whose root task completed since the last safe point (its result was shown and destroyed
    /// by its own VM), and restart the scope of any that met a version conflict (R7).
    fn collect(&mut self) {
        let mut done = Vec::new();
        {
            let mut st = self.sched.lock();
            for (root, inv) in self.invs.iter_mut() {
                if inv.done {
                    continue;
                }
                if let Some(t) = inv.task {
                    if st.tasks[t].state == TState::Done {
                        inv.done = true;
                        let task = &mut st.tasks[t];
                        let shown = task.shown.take().unwrap_or(("ok", String::new()));
                        done.push((*root, shown));
                    }
                }
            }
        }
        for (root, (outcome, value)) in done {
            self.finished(root, outcome, value);
        }
        self.release_held();
    }

    /// The roots with an unfinished task, detached descendants included: what a scope's activity is made of.
    fn active_roots(&self) -> BTreeSet<usize> {
        let st = self.sched.lock();
        (1..st.tasks.len()).filter(|&t| st.tasks[t].state != TState::Done && st.tasks[t].root != 0).map(|t| st.tasks[t].root).collect()
    }

    /// Release a stalled scope's kept state once none of its tasks is left, on the generation it was valid in.
    fn release_held(&mut self) {
        let waiting: Vec<String> = self.scopes.iter().filter(|(_, s)| s.held.is_some()).map(|(n, _)| n.clone()).collect();
        if waiting.is_empty() {
            return;
        }
        let active = self.active_roots();
        for name in waiting {
            if self.invs.iter().any(|(r, i)| i.scope == name && active.contains(r)) {
                continue;
            }
            let sc = self.scopes.get_mut(&name).unwrap();
            let (gen, state) = (sc.held.take().unwrap(), sc.state.take());
            if let Some(st) = state {
                self.event("release", vec![("scope", Json::str(&name)), ("gen", Json::Int(gen.id as i64))]);
                let mut vm = self.vm(&gen);
                let _ = vm.destroy(st);
                self.park(vm);
            }
        }
    }

    /// Log each old generation whose last reference is gone (design 8).
    fn reclaim(&mut self) {
        let cur = self.current.as_ref().map(|g| g.id).unwrap_or(0);
        let gone: Vec<u32> = self.gens.iter().filter(|(g, (w, _))| **g != cur && !self.reclaimed.contains(g) && w.upgrade().is_none()).map(|(g, _)| *g).collect();
        for g in gone {
            self.reclaimed.insert(g);
            // its constants' cache entries go with it (design 8)
            if !fault("consts-kept") {
                let prefix = format!("{g}#");
                self.consts.retain(|k, _| !k.starts_with(&prefix));
            }
            let at = self.now();
            self.event("reclaimed", vec![("gen", Json::Int(g as i64)), ("at", Json::Int(at))]);
        }
    }

    /// The generations of each scope's active invocations (its accessors, A(S)), detached descendants included.
    fn accessors(&self, scope: &str, except: &BTreeSet<usize>) -> BTreeSet<u32> {
        let active = self.active_roots();
        self.invs.iter().filter(|(r, i)| active.contains(r) && i.scope == scope && !except.contains(r)).map(|(_, i)| i.gen).collect()
    }

    fn live(&self, id: u32) -> Option<Arc<Generation>> {
        self.gens.get(&id).and_then(|(w, _)| w.upgrade())
    }

    // ---- reload --------------------------------------------------------------------------------------------------

    fn reload(&mut self, op: &Json) {
        let src = op.get("source").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let new = match self.compile_gen(&src) {
            Ok(g) => g,
            Err(d) => {
                self.event("rejected", vec![("source", Json::str(&src)), ("reason", Json::str("diagnostics")), ("diagnostics", Json::strs(d))]);
                if fault("rejected-resets") {
                    let all: Vec<String> = self.scopes.keys().cloned().collect();
                    let plans = all.into_iter().map(|s| (s, Plan::Restart { rule: "requested", ident: String::new(), change: String::new(), path: vec![] })).collect();
                    self.apply(plans, None);
                }
                return;
            }
        };
        let cur = self.current();
        let changes = generation::classify(&cur, &new);
        let compatible = changes.iter().all(|c| c.compatible);
        let mut plans: BTreeMap<String, Plan> = BTreeMap::new();
        let names: Vec<String> = self.scopes.keys().cloned().collect();
        for name in names {
            if let Some(p) = self.examine(&name, &cur, &new, &changes) {
                plans.insert(name, p);
            }
        }
        if fault("rebuild-all") {
            for name in self.scopes.keys() {
                plans.entry(name.clone()).or_insert(Plan::Restart { rule: "requested", ident: String::new(), change: String::new(), path: vec![] });
            }
        }
        if fault("rebuild-shop-silently") && compatible && self.scopes.contains_key("shop") {
            // the shop's state is replaced by a fresh one, with no event: only the migration checks can see it
            let (factory, args) = (self.scopes["shop"].factory.clone(), self.scopes["shop"].args.clone());
            if let Ok((state, _, _)) = self.build(&cur, "shop", &factory, &args) {
                self.scopes.get_mut("shop").unwrap().state = Some(state);
            }
        }
        let cancelled: BTreeSet<usize> = self.planned_cancellations(&plans);
        let mut ev = self.evictions(Some(&new), &cancelled);
        if let Some(Plan::Stop { why }) = ev.remove("") {
            // bounded admission: kept state pins generations no eviction can free
            self.event("rejected", vec![("source", Json::str(&src)), ("reason", Json::str("retention")), ("message", Json::str(why))]);
            return;
        }
        for (s, p) in ev {
            plans.entry(s).or_insert(p);
        }
        let changes_json = Json::Arr(
            changes
                .iter()
                .map(|c| {
                    let kind = if fault("body-as-summary") && c.kind == "body" { "summary" } else { c.kind };
                    Json::obj([("ident", Json::str(&c.ident)), ("kind", Json::str(kind)), ("base", Json::str(c.base)), ("decl", Json::str(c.decl)), ("compatible", Json::Bool(c.compatible && kind != "summary")), ("detail", Json::str(&c.detail))])
                })
                .collect(),
        );
        self.event("reload", vec![("gen", Json::Int(new.id as i64)), ("source", Json::str(&src)), ("compatible", Json::Bool(compatible)), ("changes", changes_json)]);
        self.apply(plans, Some(new));
    }

    /// Design 6's rules for one scope, in order: R4, R1, R3, R2.
    fn examine(&self, name: &str, cur: &Arc<Generation>, new: &Arc<Generation>, changes: &[Change]) -> Option<Plan> {
        let s = &self.scopes[name];
        if s.stopped.is_some() {
            return None;
        }
        // R4: the restart recipe must bind in the new generation
        if let Some(why) = recipe_problem(new, &s.factory, &s.args) {
            return Some(Plan::Stop { why });
        }
        let acc = self.accessors(name, &BTreeSet::new());
        // R1: what the state can hold under the current generation and every accessor must be usable in the new one
        let mut views: Vec<Arc<Generation>> = vec![cur.clone()];
        if !fault("r1-adjacent-only") {
            views.extend(acc.iter().filter(|g| **g != cur.id).filter_map(|g| self.live(*g)));
        }
        // the fault switch `skip-shop` makes R1 and R3 ignore the scope `shop` (the R7 scenario, K2)
        let skip = fault("skip-shop") && name == "shop";
        if !skip {
            for g in &views {
                for (item, path) in generation::rep(g, &s.state_ty) {
                    if let Some(why) = generation::differs(g, new, &item) {
                        let mut p = path.clone();
                        p.push(why.clone());
                        // a declaration the DI elaborator generated is attributed to its context (design 6)
                        let rule = match generation::di_context(g, &item).or_else(|| generation::di_context(new, &item)) {
                            Some(ctx) => {
                                p.push(format!("generated by the DI context {ctx}"));
                                "di"
                            }
                            None => "state",
                        };
                        return Some(Plan::Restart { rule, ident: item.ident().to_string(), change: why, path: cut(p) });
                    }
                }
            }
        }
        // R3: an incompatible change to something the scope depends on
        let incompatible: Vec<&Change> = changes.iter().filter(|c| !c.compatible).collect();
        if !incompatible.is_empty() && !skip {
            let deps = [generation::dep(cur, &s.state_ty, &s.factory_ident), generation::dep(new, &s.state_ty, &s.factory_ident)];
            for c in incompatible {
                let item = match c.decl {
                    "function" => Item::Code(c.ident.clone()),
                    "constant" => Item::Const(c.ident.clone()),
                    _ => Item::Type(c.ident.clone()),
                };
                for d in &deps {
                    if let Some(path) = d.get(&item) {
                        if fault("r3-no-values") && path.iter().any(|p| p.contains("through a value")) {
                            continue;
                        }
                        let mut p = path.clone();
                        p.push(format!("{} ({}{})", c.ident, c.kind, if c.detail.is_empty() { String::new() } else { format!(": {}", c.detail) }));
                        let rule = if c.kind == "di" { "di" } else { "dependency" };
                        // the fault switch `path-rewire` routes the explanation through real but unrelated calls
                        if fault("path-rewire") && p.len() > 1 {
                            p.insert(1, "calls main::function refund".into());
                        }
                        return Some(Plan::Restart { rule, ident: c.ident.clone(), change: format!("{}: {}", c.kind, c.detail), path: cut(p) });
                    }
                }
            }
        }
        // R2: an old accessor must be able to use whatever the new generation can store
        if !fault("skip-r2") {
            let old: Vec<Arc<Generation>> = acc.iter().filter_map(|g| self.live(*g)).collect();
            if !old.is_empty() {
                let mut gens = BTreeSet::new();
                let mut first: Option<(String, String, Vec<String>)> = None;
                for (item, path) in generation::rep_mutable(new, &s.state_ty) {
                    for g in &old {
                        if let Some(why) = generation::differs(g, new, &item) {
                            gens.insert(g.id);
                            if first.is_none() {
                                let mut p = path.clone();
                                p.push(why.clone());
                                first = Some((item.ident().to_string(), why, cut(p)));
                            }
                        }
                    }
                }
                if let Some((ident, change, path)) = first {
                    return Some(Plan::OldReaders { gens, ident, change, path });
                }
            }
        }
        None
    }

    fn planned_cancellations(&self, plans: &BTreeMap<String, Plan>) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        let active = self.active_roots();
        for (s, p) in plans {
            for (r, i) in &self.invs {
                if !active.contains(r) || &i.scope != s {
                    continue;
                }
                match p {
                    Plan::OldReaders { gens, .. } if !gens.contains(&i.gen) => {}
                    _ => {
                        out.insert(*r);
                    }
                }
            }
        }
        out
    }

    /// The scopes to restart so that the caps hold once `new` is installed (or now, with no new generation).
    fn evictions(&self, new: Option<&Arc<Generation>>, cancelled: &BTreeSet<usize>) -> BTreeMap<String, Plan> {
        let mut plans = BTreeMap::new();
        let mut gone: BTreeSet<usize> = cancelled.clone();
        let active = self.active_roots();
        let cur = self.current.as_ref().map(|g| g.id).unwrap_or(0);
        let head = new.map(|g| g.id).unwrap_or(cur);
        loop {
            // a stalled scope's kept state pins its generation too (design 8)
            let held: BTreeSet<u32> = if fault("held-uncounted") { BTreeSet::new() } else { self.scopes.values().filter_map(|s| s.held.as_ref().map(|g| g.id)).filter(|g| *g != head).collect() };
            let pinned: BTreeSet<u32> = self.invs.iter().filter(|(r, i)| active.contains(r) && !gone.contains(r) && i.gen != head).map(|(_, i)| i.gen).chain(held.iter().copied()).collect();
            let count = pinned.len() + if fault("cap-after-install") { 0 } else { 1 };
            let bytes: usize = pinned.iter().map(|g| self.gens.get(g).map(|x| x.1).unwrap_or(0)).sum();
            let rule = if count > self.caps.0 {
                "generation-cap"
            } else if bytes > self.caps.1 {
                "memory-cap"
            } else {
                break;
            };
            // the oldest generation an eviction can free: one pinned only by kept state cannot be (its task stalled)
            // an invocation whose unfinished tasks are all cancelled already has stalled: restarting it frees nothing
            let stuck: BTreeSet<usize> = {
                let st = self.sched.lock();
                active.iter().copied().filter(|r| (1..st.tasks.len()).filter(|&t| st.tasks[t].root == *r && st.tasks[t].state != TState::Done).all(|t| st.tasks[t].cancelled)).collect()
            };
            let oldest = match pinned.iter().find(|g| self.invs.iter().any(|(r, i)| active.contains(r) && !gone.contains(r) && !stuck.contains(r) && i.gen == **g)) {
                Some(g) => *g,
                None => {
                    plans.insert(String::new(), Plan::Stop { why: format!("the caps cannot hold: generations {pinned:?} are pinned by kept state") });
                    break;
                }
            };
            let scopes: BTreeSet<String> = self.invs.iter().filter(|(r, i)| active.contains(r) && !gone.contains(r) && i.gen == oldest).map(|(_, i)| i.scope.clone()).collect();
            for s in scopes {
                for (r, i) in &self.invs {
                    if active.contains(r) && i.scope == s {
                        gone.insert(*r);
                    }
                }
                plans.insert(s, Plan::Restart { rule, ident: format!("generation {oldest}"), change: format!("generation {oldest} is evicted"), path: vec![format!("generation {oldest} pins an invocation of the scope")] });
            }
        }
        plans
    }

    /// Carry out the plans in design 6's phases, installing `new` between the release and the rebuild.
    fn apply(&mut self, plans: BTreeMap<String, Plan>, new: Option<Arc<Generation>>) {
        let cur = self.current();
        let mut order: Vec<(String, Plan)> = plans.into_iter().collect();
        if fault("unseeded-order") {
            use std::hash::{BuildHasher, Hash, Hasher};
            let rs = std::collections::hash_map::RandomState::new();
            order.sort_by_key(|(s, _)| {
                let mut h = rs.build_hasher();
                s.hash(&mut h);
                h.finish()
            });
        }
        // phase 2: signal every task of the scope's active invocations, detached descendants included
        let active = self.active_roots();
        let mut signalled: Vec<(String, usize)> = Vec::new();
        let mut signal_scopes: Vec<String> = Vec::new();
        let mut cancelled_ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (s, p) in &order {
            if fault("restart-no-cancel") && matches!(p, Plan::Restart { .. }) {
                continue;
            }
            let roots: Vec<usize> = self
                .invs
                .iter()
                .filter(|(r, i)| active.contains(r) && &i.scope == s)
                .filter(|(_, i)| match p {
                    Plan::OldReaders { gens, .. } => gens.contains(&i.gen),
                    _ => true,
                })
                .map(|(r, _)| *r)
                .collect();
            let tasks: Vec<usize> = {
                let st = self.sched.lock();
                (1..st.tasks.len()).filter(|&t| st.tasks[t].state != TState::Done && roots.contains(&st.tasks[t].root)).collect()
            };
            for r in &roots {
                cancelled_ids.entry(s.clone()).or_default().push(self.invs[r].id.clone());
            }
            self.event("signal", vec![("scope", Json::str(s)), ("tasks", Json::Arr(tasks.iter().map(|t| Json::Int(*t as i64)).collect()))]);
            signal_scopes.push(s.clone());
            let mut vm = self.vm(&cur);
            for t in tasks {
                vm.cancel(t);
                signalled.push((s.clone(), t));
            }
            self.park(vm);
        }
        let release = |me: &mut Self, gen: &Arc<Generation>, order: &[(String, Plan)], stalled: &BTreeSet<String>| {
            for (s, p) in order {
                if matches!(p, Plan::OldReaders { .. }) {
                    continue;
                }
                // a stalled scope keeps its state, pinned to its generation, until its tasks finish (design 6)
                if stalled.contains(s) {
                    if let Some(sc) = me.scopes.get_mut(s) {
                        sc.stopped = Some("a cancelled task did not finish without time".into());
                        sc.held = Some(gen.clone());
                    }
                    continue;
                }
                let state = me.scopes.get_mut(s).and_then(|sc| sc.state.take());
                if let Some(st) = state {
                    me.event("release", vec![("scope", Json::str(s)), ("gen", Json::Int(gen.id as i64))]);
                    let retained = match &st {
                        Value::SharedRef(b) => b.strong.get() > 1,
                        _ => false,
                    };
                    let mut vm = me.vm(gen);
                    let _ = vm.destroy(st);
                    me.park(vm);
                    if retained {
                        me.event("state-retained", vec![("scope", Json::str(s))]);
                    }
                }
            }
        };
        if fault("release-before-drain") {
            release(self, &cur, &order, &BTreeSet::new());
        }
        // phase 3: drain, without advancing time
        let mut vm = self.vm(&cur);
        let _ = vm.settle();
        self.park(vm);
        // the cancelled invocations' completions are logged before the state is released
        self.collect();
        let (stalled, drained): (BTreeSet<String>, Vec<(String, Vec<usize>, Vec<usize>)>) = {
            let st = self.sched.lock();
            let stalled = signalled.iter().filter(|(_, t)| st.tasks[*t].state != TState::Done).map(|(s, _)| s.clone()).collect();
            let mut by: BTreeMap<String, (Vec<usize>, Vec<usize>)> = signal_scopes.iter().map(|s| (s.clone(), (vec![], vec![]))).collect();
            for (s, t) in &signalled {
                let e = by.entry(s.clone()).or_default();
                if st.tasks[*t].state == TState::Done {
                    e.0.push(*t);
                } else {
                    e.1.push(*t);
                }
            }
            (stalled, by.into_iter().map(|(s, (d, p))| (s, d, p)).collect())
        };
        for (s, done, pending) in drained {
            self.event("drained", vec![("scope", Json::str(&s)), ("done", Json::Arr(done.iter().map(|t| Json::Int(*t as i64)).collect())), ("stalled", Json::Arr(pending.iter().map(|t| Json::Int(*t as i64)).collect()))]);
        }
        // after the drain, the caps are judged on what is actually still live: the generations of unfinished
        // invocations and of a stalled scope's kept state, whatever rule cancelled them (design 8)
        let broken = {
            let active = self.active_roots();
            let mut live: BTreeSet<u32> = self.invs.iter().filter(|(r, _)| active.contains(r)).map(|(_, i)| i.gen).collect();
            if !fault("held-uncounted") {
                live.extend(self.scopes.values().filter_map(|s| s.held.as_ref().map(|g| g.id)));
            }
            if !stalled.is_empty() {
                live.insert(cur.id);
            }
            if let Some(n) = &new {
                live.remove(&n.id);
            } else {
                live.remove(&cur.id);
            }
            let bytes: usize = live.iter().map(|g| self.gens.get(g).map(|x| x.1).unwrap_or(0)).sum();
            live.len() + 1 > self.caps.0 || bytes > self.caps.1
        };
        let evicting = broken || order.iter().any(|(_, p)| matches!(p, Plan::Restart { rule: "generation-cap" | "memory-cap", .. }));
        // old readers that did not drain could meet the new generation's values: the reload is not admitted
        let readers_stalled = !fault("reader-stall-admitted") && order.iter().any(|(s, p)| matches!(p, Plan::OldReaders { .. }) && stalled.contains(s));
        if !stalled.is_empty() && (evicting || readers_stalled) && new.is_some() {
            // bounded admission (design 8): the caps would break, so the reload is refused; drained scopes rebuild on n
            if !fault("release-before-drain") {
                release(self, &cur, &order, &stalled);
            }
            let reason = if readers_stalled && !evicting { "old-readers" } else { "retention" };
            self.event("rejected", vec![("source", Json::str(new.as_ref().map(|g| g.source.clone()).unwrap_or_default())), ("reason", Json::str(reason)), ("stalled", Json::strs(stalled.iter().cloned()))]);
            drop(new);
            for (s, p) in &order {
                if matches!(p, Plan::Restart { .. }) && !stalled.contains(s) {
                    self.rebuild(s, &cur);
                }
                if stalled.contains(s) && !matches!(p, Plan::OldReaders { .. }) {
                    self.event("stopped", vec![("scope", Json::str(s)), ("rule", Json::str("stall")), ("message", Json::str("a cancelled task did not finish without time; the state is kept until it does")), ("cancelled", Json::strs(cancelled_ids.get(s).cloned().unwrap_or_default()))]);
                }
            }
            return;
        }
        if !stalled.is_empty() && evicting {
            // a lowered cap whose drain stalls cannot hold yet: the stalled scopes stop, their generations stay live
            self.event("caps-unmet", vec![("stalled", Json::strs(stalled.iter().cloned()))]);
        }
        // phase 4: release, on the generation the state was last valid in
        if !fault("release-before-drain") {
            let rel = match (&new, fault("release-new")) {
                (Some(n), true) => n.clone(),
                _ => cur.clone(),
            };
            release(self, &rel, &order, &stalled);
        }
        // phase 5: install
        let installed = match new {
            Some(n) => {
                let n2 = n.clone();
                self.install(n);
                n2
            }
            None => cur.clone(),
        };
        drop(cur);
        // phase 6: rebuild, and explain
        for (s, p) in order {
            self.seq += 1;
            let cancelled = Json::strs(cancelled_ids.get(&s).cloned().unwrap_or_default());
            match p {
                Plan::Restart { rule, ident, change, path } => {
                    if stalled.contains(&s) {
                        self.event("stopped", vec![("scope", Json::str(&s)), ("rule", Json::str("stall")), ("message", Json::str("a cancelled task did not finish without time")), ("cancelled", cancelled)]);
                        if let Some(sc) = self.scopes.get_mut(&s) {
                            sc.stopped = Some("a cancelled task stalled".into());
                        }
                        continue;
                    }
                    self.event("restart", vec![("scope", Json::str(&s)), ("rule", Json::str(rule)), ("ident", Json::str(&ident)), ("change", Json::str(&change)), ("path", Json::strs(path)), ("cancelled", cancelled), ("restart", Json::Int(self.seq as i64))]);
                    self.rebuild(&s, &installed);
                }
                Plan::Stop { why } => {
                    self.event("stopped", vec![("scope", Json::str(&s)), ("rule", Json::str("entry")), ("message", Json::str(&why)), ("cancelled", cancelled)]);
                    if let Some(sc) = self.scopes.get_mut(&s) {
                        sc.stopped = Some(why);
                    }
                }
                Plan::OldReaders { gens, ident, change, path } => {
                    self.event("old-reader", vec![("scope", Json::str(&s)), ("rule", Json::str("old-reader")), ("gens", Json::Arr(gens.iter().map(|g| Json::Int(*g as i64)).collect())), ("ident", Json::str(&ident)), ("change", Json::str(&change)), ("path", Json::strs(path)), ("cancelled", cancelled)]);
                }
            }
        }
    }

    fn rebuild(&mut self, s: &str, g: &Arc<Generation>) {
        let (factory, args) = match self.scopes.get(s) {
            Some(sc) => (sc.factory.clone(), sc.args.clone()),
            None => return,
        };
        match self.build(g, s, &factory, &args) {
            Ok((state, ty, ident)) => {
                self.event("started", vec![("scope", Json::str(s)), ("gen", Json::Int(g.id as i64)), ("factory", Json::str(&ident))]);
                let sc = self.scopes.get_mut(s).unwrap();
                sc.state = Some(state);
                sc.state_ty = ty;
                sc.factory_ident = ident;
            }
            Err(why) => {
                self.event("stopped", vec![("scope", Json::str(s)), ("rule", Json::str("rebuild")), ("message", Json::str(&why)), ("cancelled", Json::Arr(vec![]))]);
                if let Some(sc) = self.scopes.get_mut(s) {
                    sc.stopped = Some(why);
                }
            }
        }
    }

    // ---- observation -------------------------------------------------------------------------------------------

    fn snapshot(&mut self, op: usize) {
        let cur = self.current.as_ref().map(|g| g.id).unwrap_or(0);
        let live: Vec<u32> = self.gens.iter().filter(|(_, (w, _))| w.upgrade().is_some()).map(|(g, _)| *g).collect();
        let active = self.active_roots();
        let pinned: BTreeSet<u32> = self.invs.iter().filter(|(r, _)| active.contains(r)).map(|(_, i)| i.gen).chain(self.scopes.values().filter_map(|s| s.held.as_ref().map(|g| g.id))).collect();
        let const_gens: BTreeSet<i64> = self.consts.keys().filter_map(|k| k.split_once('#').and_then(|(g, _)| g.parse::<i64>().ok())).collect();
        let old_bytes: usize = live.iter().filter(|g| **g != cur).map(|g| self.gens[g].1).sum();
        let objects = generation::LIVE.load(Ordering::SeqCst);
        self.event(
            "snapshot",
            vec![
                ("current", Json::Int(cur as i64)),
                ("live", Json::Arr(live.iter().map(|g| Json::Int(*g as i64)).collect())),
                ("pinned", Json::Arr(pinned.iter().map(|g| Json::Int(*g as i64)).collect())),
                ("old_bytes", Json::Int(old_bytes as i64)),
                ("objects", Json::Int(objects as i64)),
                ("const_gens", Json::Arr(const_gens.iter().map(|g| Json::Int(*g)).collect())),
                ("at", Json::Int(self.now())),
            ],
        );
        let tasks = self.sched.lock().tasks.len();
        self.metrics.push(
            Json::obj([
                ("op", Json::Int(op as i64)),
                ("rss_kb", Json::Int(rss_kb())),
                ("objects", Json::Int(objects as i64)),
                ("threads", Json::Int(crate::executor::THREADS_LIVE.load(Ordering::SeqCst) as i64)),
                ("task_records", Json::Int(tasks as i64)),
                ("roots", Json::Int(self.next_root as i64 - 1)),
            ])
            .to_line(),
        );
    }

    fn finish(&mut self, op: usize) {
        let g = self.current();
        loop {
            // every task of the session, detached descendants included
            let next = {
                let st = self.sched.lock();
                (1..st.tasks.len()).find(|&t| st.tasks[t].state != TState::Done && st.tasks[t].root != 0)
            };
            let t = match next {
                Some(t) => t,
                None => break,
            };
            let mut vm = self.vm(&g);
            let r = vm.block_until(Some(t), None);
            let _ = vm.settle();
            self.park(vm);
            if let Err(e) = r {
                self.event("error", vec![("op", Json::str("finish")), ("message", Json::str(format!("{e:?}")))]);
                break;
            }
        }
        drop(g);
        self.collect();
        self.reclaim();
        // task threads leave after handing the baton on: wait for them, with a deadline (G4)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::executor::THREADS_LIVE.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.snapshot(op);
    }
}

/// A root invocation's outcome and value, shown by the VM that ran it; a successful value is then destroyed there,
/// in its own generation (its cleanups run that generation's code).
pub(crate) fn show_result(vm: &mut Vm, completion: &str, result: Option<Value>, error: Option<Exit>) -> (&'static str, String) {
    match (completion, result, error) {
        ("Cancelled", _, _) => ("cancelled", String::new()),
        (_, _, Some(e)) => outcome_of(e),
        (_, Some(v), None) if v.is_failure() => ("failure", show_failure(&v)),
        (_, Some(v), None) => {
            let s = display(&v);
            let _ = vm.destroy(v);
            ("ok", s)
        }
        (_, None, None) => ("ok", String::new()),
    }
}

fn outcome_of(e: Exit) -> (&'static str, String) {
    match e {
        Exit::Flow(Flow::Fail(v)) => ("failure", show_failure(&v)),
        Exit::Halt(Halt::Cancel) => ("cancelled", String::new()),
        Exit::Halt(Halt::Defect(k)) => ("defect", k),
        Exit::Halt(Halt::Dynamic(m)) if m.starts_with(CONFLICT) => ("conflict", m[CONFLICT.len()..].to_string()),
        Exit::Halt(Halt::Dynamic(m)) => ("error", m),
        other => ("error", format!("{other:?}")),
    }
}

/// An explanation's path; the fault switch `path-first` cuts it to its first step.
fn cut(p: Vec<String>) -> Vec<String> {
    if fault("path-first") {
        p.into_iter().take(1).collect()
    } else {
        p
    }
}

/// Why a restart recipe does not bind in a generation (R4), if it does not.
fn recipe_problem(g: &Generation, factory: &str, args: &[(String, Json)]) -> Option<String> {
    let (_, f) = match g.function_named(factory) {
        Some(x) => x,
        None => return Some(format!("the factory `{factory}` does not exist in generation {}", g.id)),
    };
    if f.is_async || type_name(&f.ret) != Some("Shared") {
        return Some(format!("the factory `{factory}` is not a synchronous function returning Shared[…] in generation {}", g.id));
    }
    let func = &g.program.functions[f.index];
    for (p, t) in func.params.iter().zip(f.params.iter()) {
        if type_name(t).and_then(crate::intrinsics::cap_kind).is_some() {
            continue;
        }
        match args.iter().find(|(n, _)| *n == p.name) {
            Some((_, j)) if !arg_fits(j, t) => return Some(format!("the factory `{factory}`'s recorded argument `{}` does not fit {t} in generation {}", p.name, g.id)),
            None if p.default.is_none() => return Some(format!("the factory `{factory}` needs `{}`, which the recipe does not give, in generation {}", p.name, g.id)),
            _ => {}
        }
    }
    for (n, _) in args {
        if !func.params.iter().any(|p| &p.name == n) {
            return Some(format!("the factory `{factory}` has no parameter `{n}` in generation {}", g.id));
        }
    }
    None
}

/// Run a session script (a JSON array of operations) with paths relative to its directory, writing the event log to
/// `sink` as JSON lines; the metrics are returned, one JSON line per snapshot.
pub fn run_script<'c>(compile: Compile<'c>, script: &Path, sink: &'c mut (dyn std::io::Write + Send), frames: Option<&'c mut (dyn std::io::Write + Send)>) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(script).map_err(|e| format!("{}: {e}", script.display()))?;
    let ops = match Json::parse(&text)? {
        Json::Arr(xs) => xs,
        _ => return Err("a session script is a JSON array of operations".into()),
    };
    let dir = script.parent().map(Path::to_path_buf).unwrap_or_default();
    crate::on_big_stack(|| {
        let mut s = Session::new(compile, &dir, sink, frames);
        s.run(&ops);
        if s.sink.flush().is_err() {
            s.write_failed = true;
        }
        if let Some(f) = s.frames.as_mut() {
            if f.flush().is_err() {
                s.write_failed = true;
            }
        }
        if s.write_failed {
            return Err("the event log or the frame log could not be written".to_string());
        }
        Ok(std::mem::take(&mut s.metrics))
    })
}
