//! The evaluator: runs typed-IR functions over tagged values. Control flow that leaves an expression or statement
//! (break, continue, return, give, recover, a propagating failure) travels as `Exit::Flow`; a defect or a dynamic
//! check outside the checked language travels as `Exit::Halt`. Block exit follows the interpreter exactly
//! (interpreter/eval.py `exit_block`): deferred blocks LIFO, then the block's locals destroyed in reverse.

use std::collections::{HashMap, HashSet};
use crate::sync::Rc;

use fortis_mir::model::*;
use fortis_runtime::telemetry;

use crate::value::*;

#[derive(Debug)]
pub enum Halt {
    /// a defect terminates the task and is never caught (10.8)
    Defect(String),
    /// a dynamic check the static layers exclude: the program is outside the checked language
    Dynamic(String),
    /// cancellation observed at an await or checkpoint: unwinds the task (defers, abandon, drops); never a
    /// Failure (12.4)
    Cancel,
    /// the run is over while this task's thread was parked: it leaves at once, running nothing
    Shutdown,
}

#[derive(Debug)]
pub enum Flow {
    Break(Option<String>),
    Continue(Option<String>),
    Return(Value),
    Give(Value),
    Recover(Value),
    Fail(Value),
}

#[derive(Debug)]
pub enum Exit {
    Halt(Halt),
    Flow(Flow),
}

pub type R<T> = Result<T, Exit>;

pub fn dynamic<T>(msg: impl Into<String>) -> R<T> {
    Err(Exit::Halt(Halt::Dynamic(msg.into())))
}

pub fn defect<T>(kind: impl Into<String>) -> R<T> {
    Err(Exit::Halt(Halt::Defect(kind.into())))
}

/// A record the run emits (runtime failures, observation frames and events, test results, defects).
pub type Record = Vec<(String, String)>;

/// The records a task's reporter buffers before overflow becomes visible (PROG-NFR-13; interpreter/executor.py).
pub const OBSERVATION_BOUND: usize = 1000;

#[derive(Debug)]
pub struct Frame {
    pub locals: Vec<Value>,
    pub func: usize,
    /// the closure a consuming call (`move f(…)`, launched work) owns: destroyed when the frame ends, after its
    /// owned parameters, or with the future if the frame never runs (7.9, X-250; D-30)
    pub consumed: Option<Value>,
    /// the `let` locals that hold a loan rather than own their value: bound or assigned a value that lends (an
    /// `alias` or `editable` place, a `get`, a result declared `from` a parameter, the present side of a `??`).
    /// Their end destroys nothing; the owner's does (7.3, 7.9; D-338, D-350)
    pub lent: HashSet<LocalId>,
    /// the pattern bindings whose type is a loan but which took a value of its own from an aggregate their pattern
    /// owned: a tuple or record element that holds an owned value (a `??` whose fallback is owned). They own it, and
    /// their end destroys it (7.9; D-366)
    pub owned: HashSet<LocalId>,
    /// whether the value the frame's `return` gave is a loan (true until a `return` says otherwise): a result declared
    /// `from` a parameter is bound by the caller as this says, so a value of the callee's own (a `??` whose fallback
    /// it moved or made, a moved local) is owned where it lands (7.9; D-373)
    pub ret_lent: bool,
}

const MAX_DEPTH: usize = 2000;

/// The stderr line the interpreter writes when the reporter's buffer overflows (PROG-NFR-13).
fn overflow_line(dropped: usize) -> String {
    fortis_diag::Json::obj([
        ("bound", fortis_diag::Json::Int(OBSERVATION_BOUND as i64)),
        ("dropped", fortis_diag::Json::Int(dropped as i64)),
        ("kind", fortis_diag::Json::str("buffer-overflow")),
        ("rule", fortis_diag::Json::str("PROG-NFR-13")),
    ])
    .to_line()
}

pub struct Vm<'p> {
    pub prog: &'p Program,
    pub funcs: HashMap<&'p str, usize>,
    pub types: HashMap<&'p str, &'p TypeDef>,
    pub out: Vec<String>,
    pub trace: Vec<String>,
    pub reports: Vec<Record>,
    /// interpreter-shaped runtime-failure JSON, one line per report (`fortisc vm` writes these to stderr)
    pub failure_lines: Vec<String>,
    /// the line held at the observation bound: the 1000th failure, or the overflow line that replaced it
    pub held_failure: Option<String>,
    /// a source name to the module file that declares it; a name absent here uses `file` (interpreter `decl_file`)
    pub decl_files: HashMap<String, String>,
    /// the source names of the active functions (a failure's origin)
    pub stack: Vec<String>,
    /// the observation frames of the active functions (None without a schema)
    pub obs: Vec<Option<Vec<(String, Value)>>>,
    pub cleanup_depth: usize,
    pub consts: HashMap<String, Value>,
    pub file: String,
    depth: usize,
    /// the run's scheduler, created when the first task is launched or awaited
    pub sched: Option<std::sync::Arc<crate::executor::Sched>>,
    /// this Vm's task (0: main)
    pub task: usize,
    /// for each block being run, the indices of its deferred blocks registered so far (`defer` reached)
    defer_stack: Vec<Vec<usize>>,
    /// the branches taken (node address, branch) when the test runner guides fuzzing by coverage (X-131)
    pub coverage: Option<std::collections::HashSet<(usize, u32)>>,
    /// the execution steps left (calls and loop iterations) when the runner bounds a case; exhausted is a hang
    pub fuel: Option<u64>,
    /// in a development session, the generation this VM runs (SP-07 design 5): code values made here carry its
    /// identities, and code values of other generations resolve through it
    pub gen: Option<std::sync::Arc<crate::generation::Generation>>,
    /// in a development session, the root invocation the tasks this VM launches belong to (the session driver
    /// sets it; 0 otherwise)
    pub root: usize,
    /// whether this run has created the capabilities (11.6): shared with the run's tasks through the scheduler, so a
    /// second creation anywhere in the run is the runtime-invariant defect (10.8; D-110)
    pub caps_made: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// whether the value the last `give` or `recover` gave is a loan (the side of an `if`, a block or a `match` that
    /// gave it lends), so a binding of that expression holds the loan rather than owns it (7.9; D-338)
    gave_lent: bool,
    /// whether the last call that ran gave a loan (a `from` result, an intrinsic's) or a value of its own, which the
    /// binding that receives the result then owns (7.9; D-373)
    ret_lent: bool,
}

impl<'p> Vm<'p> {
    pub fn new(prog: &'p Program, file: &str) -> Vm<'p> {
        let _ = telemetry::mode();
        let funcs = prog.functions.iter().enumerate().map(|(i, f)| (f.name.as_str(), i)).collect();
        let types = prog.types.iter().map(|t| (t.name.as_str(), t)).collect();
        Vm {
            prog,
            funcs,
            types,
            out: vec![],
            trace: vec![],
            reports: vec![],
            failure_lines: vec![],
            held_failure: None,
            decl_files: HashMap::new(),
            stack: vec![],
            obs: vec![],
            cleanup_depth: 0,
            consts: HashMap::new(),
            file: file.to_string(),
            depth: 0,
            sched: None,
            task: 0,
            defer_stack: vec![],
            coverage: None,
            fuel: None,
            gen: None,
            root: 0,
            caps_made: Default::default(),
            gave_lent: false,
            ret_lent: true,
        }
    }

    /// Record a branch for the fuzzer's coverage guidance.
    #[inline]
    fn hit(&mut self, at: usize, branch: u32) {
        if let Some(c) = &mut self.coverage {
            c.insert((at, branch));
        }
    }

    /// Spend one execution step; a case that runs out is a possible hang (Programme 8.3).
    #[inline]
    fn step(&mut self) -> R<()> {
        if let Some(f) = &mut self.fuel {
            if *f == 0 {
                return dynamic("budget: the case exhausted its execution step budget (a possible hang)");
            }
            *f -= 1;
        }
        Ok(())
    }

    pub fn func_index(&self, name: &str) -> R<usize> {
        match self.funcs.get(name) {
            Some(i) => Ok(*i),
            None => dynamic(format!("no function `{name}` in the program")),
        }
    }

    /// The identity a code value made here carries (a session only).
    #[inline]
    pub fn code_of(&self, fi: usize) -> Option<Rc<crate::generation::CodeId>> {
        self.gen.as_ref().map(|g| g.code[fi].clone())
    }

    /// The function a code value runs in this VM's generation: its own if it was made here, otherwise the function
    /// of the same identity, summary and environment (SP-07 design 5); anything else is a version conflict.
    pub fn resolve(&self, fi: usize, code: Option<&crate::generation::CodeId>) -> R<usize> {
        match (&self.gen, code) {
            (Some(_), Some(_)) if crate::session::fault("index-not-identity") => Ok(fi),
            (Some(g), Some(c)) if c.gen != g.id => match g.resolve(c) {
                Ok(i) => Ok(i),
                Err(why) => dynamic(format!("{}{why}", crate::session::CONFLICT)),
            },
            _ => Ok(fi),
        }
    }

    /// An unstarted future's frame, rebound into this VM's generation when it was bound in another: its arguments
    /// and captures move by position into the slots of the resolved function's frame.
    pub fn rebind(&self, fi: usize, frame: Frame, code: Option<&crate::generation::CodeId>) -> R<(usize, Frame)> {
        let (g, c) = match (&self.gen, code) {
            (Some(g), Some(c)) if c.gen != g.id => (g, c),
            _ => return Ok((fi, frame)),
        };
        let ni = self.resolve(fi, Some(c))?;
        let mut old = frame.locals;
        let mut locals = vec![Value::Void; self.prog.functions[ni].locals.len()];
        let decls = &self.prog.functions[ni].locals;
        let captures: Vec<LocalId> = self.prog.functions[ni].closure.as_ref().map(|c| c.captures.iter().map(|x| x.local).collect()).unwrap_or_default();
        for (from, to) in c.slots.iter().zip(g.code[ni].slots.iter()) {
            if crate::session::fault("future-no-move") {
                break;
            }
            let v = std::mem::replace(&mut old[*from as usize], Value::Moved);
            // the destination decides whether the slot lives in a shared cell (a lambda of its body refers to it);
            // a capture's cell is the closure's own storage or its owner's, and stays shared
            let boxed = decls.get(*to as usize).map(|d| d.boxed).unwrap_or(false) || captures.contains(to);
            locals[*to as usize] = match (boxed, v) {
                (true, Value::Cell(c)) => Value::Cell(c),
                (true, v) => Value::Cell(shared(v)),
                (false, Value::Cell(c)) => std::mem::replace(&mut *c.borrow_mut(), Value::Moved),
                (false, v) => v,
            };
        }
        Ok((ni, Frame { locals, func: ni, consumed: frame.consumed, lent: HashSet::new(), owned: HashSet::new(), ret_lent: true }))
    }

    /// The interpreter's name for a function in records: `main`, `parse`, `Type.method`.
    pub fn source_name_of(f: &Function) -> String {
        Self::source_name(f)
    }

    fn source_name(f: &Function) -> String {
        let l = &f.label;
        l.strip_prefix("function ").or_else(|| l.strip_prefix("method ").map(|m| m.rsplit('.').next().unwrap_or(m))).unwrap_or(l).split('#').next().unwrap_or("").to_string()
    }

    // ---- calls ----------------------------------------------------------------------------------------------

    /// Call a function: bind its arguments, then run its body; an `async` function's call binds and returns a
    /// Future without running (12.3).
    pub fn call(&mut self, fi: usize, args: Vec<(String, Value)>, captures: &[Value]) -> R<Value> {
        self.call_with(fi, args, captures, &[])
    }

    /// A closure's call: `owned` says, per capture, whether the closure owns it (a `copy` or an owned `move` capture)
    /// or holds a loan (an `alias` or `editable` capture, a loan moved in), which its body then holds (7.9; D-374).
    pub fn call_with(&mut self, fi: usize, args: Vec<(String, Value)>, captures: &[Value], owned: &[bool]) -> R<Value> {
        self.step()?;
        let frame = self.bind_call(fi, args, captures, owned)?;
        let f = &self.prog.functions[fi];
        if f.has("async") && f.body.is_some() {
            let code = self.code_of(fi);
            return Ok(Value::Future(Rc::new(crate::value::FutureVal::new(&Self::source_name(f), crate::executor::Work::Call { func: fi, frame, code }))));
        }
        self.run_frame(fi, frame)
    }

    /// A function's frame with its captures and parameters bound (defaults evaluated at the call).
    pub fn bind_call(&mut self, fi: usize, args: Vec<(String, Value)>, captures: &[Value], owned: &[bool]) -> R<Frame> {
        let f = &self.prog.functions[fi];
        if f.body.is_none() {
            return dynamic(format!("call to the trusted function `{}`, which has no body", f.name));
        }
        if self.depth >= MAX_DEPTH {
            return defect("stack_exhaustion");
        }
        let mut frame = Frame { locals: vec![Value::Void; f.locals.len()], func: fi, consumed: None, lent: HashSet::new(), owned: HashSet::new(), ret_lent: true };
        if let Some(c) = &f.closure {
            for (i, (cd, v)) in c.captures.iter().zip(captures.iter()).enumerate() {
                frame.locals[cd.local as usize] = v.clone();
                // a capture the closure does not own is a loan its body holds: moving it on (into a tuple, a
                // binding) moves the loan, and nothing destroys what it lends (7.9; D-374)
                if owned.get(i).map_or(false, |o| !o) {
                    frame.lent.insert(cd.local);
                }
            }
        }
        let mut given: Vec<(String, Value)> = args;
        for p in &f.params {
            let pos = given.iter().position(|(n, _)| *n == p.name);
            let v = match pos {
                Some(i) => given.remove(i).1,
                None => match &p.default {
                    Some(d) => {
                        self.depth += 1;
                        let v = self.eval(&mut frame, d);
                        self.depth -= 1;
                        v?
                    }
                    None => return dynamic(format!("missing argument `{}` of {}", p.name, f.label)),
                },
            };
            let decl = &f.locals[p.local as usize];
            frame.locals[p.local as usize] = if decl.boxed { Value::Cell(shared(v)) } else { v };
        }
        Ok(frame)
    }

    /// Run a function's body in a bound frame.
    pub fn run_frame(&mut self, fi: usize, mut frame: Frame) -> R<Value> {
        // a development session's frame log (N1): every frame's root invocation, generation and function
        if let Some(g) = &self.gen {
            if crate::session::FRAME_LOG.load(std::sync::atomic::Ordering::Relaxed) {
                let root = if self.root != 0 { self.root } else { self.sched.as_ref().map(|s| s.lock().tasks[self.task].root).unwrap_or(0) };
                let rec = vec![("kind".into(), "frame".into()), ("root".into(), root.to_string()), ("gen".into(), g.id.to_string()), ("ident".into(), g.code[fi].ident.to_string())];
                self.reports.push(rec);
            }
        }
        let f = &self.prog.functions[fi];
        let body = match &f.body {
            Some(b) => b,
            None => return dynamic(format!("call to the trusted function `{}`, which has no body", f.name)),
        };
        if self.depth >= MAX_DEPTH {
            return defect("stack_exhaustion");
        }
        self.depth += 1;
        self.stack.push(Self::source_name(f));
        self.obs.push(f.observe.as_ref().map(|_| vec![]));
        let mut r = self.block(&mut frame, body);
        // the owned parameters are the function's oldest values: destroyed last, unless moved (7.9); a defect runs
        // no cleanup (10.8)
        if !matches!(r, Err(Exit::Halt(ref h)) if !matches!(h, Halt::Cancel)) {
            let owned: Vec<u32> = self.prog.functions[fi].params.iter().filter(|p| p.mode == Mode::Owned).map(|p| p.local as u32).collect();
            for l in owned.into_iter().rev() {
                let v = std::mem::replace(&mut frame.locals[l as usize], Value::Moved);
                let v = match v {
                    Value::Cell(c) => std::mem::replace(&mut *c.borrow_mut(), Value::Moved),
                    v => v,
                };
                if matches!(v, Value::Moved) {
                    continue;
                }
                if let Some(t) = self.cleanup_runs(&v) {
                    self.trace.push(format!("drop {t}"));
                }
                let ty = &self.prog.functions[fi].locals[l as usize].ty;
                if let Err(e) = self.destroy_typed(v, ty) {
                    r = Err(e);
                    break;
                }
            }
            // a consuming call's closure, after the owned parameters: the captures its body has not moved out
            if let Some(c) = frame.consumed.take() {
                if !matches!(r, Err(Exit::Halt(ref h)) if !matches!(h, Halt::Cancel)) {
                    if let Err(e) = self.destroy(c) {
                        r = Err(e);
                    }
                }
            }
        }
        self.stack.pop();
        let frame_rec = self.obs.pop().flatten();
        self.depth -= 1;
        let (result, outcome) = match r {
            Ok(()) => (Ok(Value::Void), "success"),
            Err(Exit::Flow(Flow::Return(v))) => (Ok(v), "success"),
            Err(Exit::Flow(Flow::Fail(v))) => {
                if f.is_try() {
                    (Ok(v), "failure")
                } else {
                    let code = match &v {
                        Value::Failure(fv) => fv.borrow().code.to_string(),
                        _ => "?".into(),
                    };
                    (dynamic(format!("`fail` in a function without `try` ({code})")), "failure")
                }
            }
            Err(Exit::Flow(other)) => (dynamic(format!("{other:?} escaped {}", f.label)), "success"),
            // the outcome class of an activation that ends by cancellation or a defect (13)
            Err(Exit::Halt(Halt::Cancel)) => (Err(Exit::Halt(Halt::Cancel)), "cancelled"),
            Err(e) => (Err(e), "defect"),
        };
        if let (Some(schema), Some(fields)) = (&f.observe, frame_rec) {
            let mut shown: Vec<(String, String)> = fields.iter().map(|(k, v)| (k.clone(), display(v))).collect();
            let bounds: Vec<(&str, usize)> = self
                .prog
                .types
                .iter()
                .filter(|t| &t.name == schema)
                .flat_map(|t| match &t.kind {
                    TypeKind::Observation { fields, .. } => fields
                        .iter()
                        .filter_map(|(n, ty, _)| match ty {
                            fortis_types::types::Type::Refined { hi: Some(hi), kind, .. } if kind == "scalars" => Some((n.as_str(), *hi as usize)),
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                    _ => vec![],
                })
                .collect();
            let truncated = telemetry::bound_fields(&mut shown, &bounds);
            self.observation_truncated(schema, Some(&Self::source_name(f)), &shown, Some(outcome), &truncated);
        }
        // last, after the frame's own destructors ran their calls: the caller reads it as the call's result (D-373)
        self.ret_lent = frame.ret_lent;
        result
    }

    /// A completed frame (`function` and `outcome` given) or an executed `record`: under a telemetry policy the
    /// canonical permitted record (Platform 6.3), and the diagnostic plane's record, which carries captured values
    /// only under `permit-all` (Language Reference 15).
    pub fn observation(&mut self, schema: &str, function: Option<&str>, fields: &[(String, String)], outcome: Option<&str>) {
        self.observation_truncated(schema, function, fields, outcome, &[])
    }

    fn observation_truncated(&mut self, schema: &str, function: Option<&str>, fields: &[(String, String)], outcome: Option<&str>, truncated: &[String]) {
        let version = self
            .prog
            .types
            .iter()
            .find_map(|t| match &t.kind {
                TypeKind::Observation { version, .. } if t.name == schema => Some(*version),
                _ => None,
            })
            .unwrap_or(1);
        let fs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let cut: Vec<&str> = truncated.iter().map(|s| s.as_str()).collect();
        telemetry::observe(&telemetry::Observation { schema, version, function, outcome, fields: &fs, truncated: &cut });
        let shown = if telemetry::diagnostic_values() { fields.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",") } else { String::new() };
        let mut rec: Record = vec![("kind".into(), if function.is_some() { "observation-frame" } else { "observation-event" }.into()), ("schema".into(), schema.into())];
        if let Some(f) = function {
            rec.push(("function".into(), f.into()));
        }
        rec.push(("fields".into(), shown));
        if let Some(o) = outcome {
            rec.push(("outcome".into(), o.into()));
        }
        self.emit(rec);
    }

    /// Call any callable value with positional arguments.
    pub fn call_value(&mut self, f: &Value, args: Vec<Value>) -> R<Value> {
        match f {
            Value::Closure(c) => {
                let fi = self.resolve(c.func, c.code.as_deref())?;
                let func = &self.prog.functions[fi];
                let named: Vec<(String, Value)> = func.params.iter().map(|p| p.name.clone()).zip(args).collect();
                let caps = c.captures.clone();
                let owned = c.owned.clone();
                self.call_with(fi, named, &caps, &owned)
            }
            Value::Func(i) => {
                let named: Vec<(String, Value)> = self.prog.functions[*i].params.iter().map(|p| p.name.clone()).zip(args).collect();
                self.call(*i, named, &[])
            }
            Value::Intrinsic(name) => {
                let name = name.clone();
                let r = crate::intrinsics::call(self, &name, args);
                self.ret_lent = true; // an intrinsic that lends lends (D-373)
                r
            }
            Value::Cell(c) => {
                let v = c.borrow().clone();
                self.call_value(&v, args)
            }
            other => dynamic(format!("call of a non-callable: {}", display(other))),
        }
    }

    /// A call that consumes its callee (`move f(…)`, launched work): a closure goes into the frame, which destroys
    /// it when it ends, after its owned parameters; an `async` closure's frame is the future's (7.9, X-250; D-30).
    pub fn call_consuming(&mut self, f: Value, args: Vec<Value>) -> R<Value> {
        let c = match f {
            Value::Closure(c) => c,
            Value::Cell(cell) => {
                let v = std::mem::replace(&mut *cell.borrow_mut(), Value::Moved);
                return self.call_consuming(v, args);
            }
            other => return self.call_value(&other, args),
        };
        let fi = self.resolve(c.func, c.code.as_deref())?;
        let named: Vec<(String, Value)> = self.prog.functions[fi].params.iter().map(|p| p.name.clone()).zip(args).collect();
        let caps = c.captures.clone();
        let owned = c.owned.clone();
        self.step()?;
        let mut frame = self.bind_call(fi, named, &caps, &owned)?;
        frame.consumed = Some(Value::Closure(c));
        let f = &self.prog.functions[fi];
        if f.has("async") && f.body.is_some() {
            let code = self.code_of(fi);
            return Ok(Value::Future(Rc::new(crate::value::FutureVal::new(&Self::source_name(f), crate::executor::Work::Call { func: fi, frame, code }))));
        }
        self.run_frame(fi, frame)
    }

    /// A value's display form, as interpolation writes it (LR 3.6): a class or enum whose type declares its own
    /// `display` (Displayable, 11.1) runs it into a fresh Builder; a class or enum without one shows its fields, and
    /// a list its items, each the same way; anything else as `display` shows it (D-83).
    pub fn show(&mut self, v: &Value) -> R<String> {
        let v = v.deref();
        let ty: Option<Rc<str>> = match &v {
            Value::Obj(o) => Some(o.borrow().ty.clone()),
            Value::Enum(e) => Some(e.ty.clone()),
            _ => None,
        };
        let own = ty.as_ref().and_then(|t| self.types.get(&**t)).and_then(|t| t.methods.iter().find(|(m, _)| m == "display")).map(|(_, f)| f.clone());
        // Displayable's `display(self, into: editable Builder) Void` (11.1), not merely a method of that name
        let own = match own {
            Some(f) => {
                let fi = self.func_index(&f)?;
                let fd = &self.prog.functions[fi];
                let is_impl = !fd.has("async")
                    && !fd.is_try()
                    && fd.params.len() == 2
                    && fd.params[0].mode == Mode::Read
                    && fd.params[1].mode == Mode::Editable
                    && matches!(fortis_types::types::strip(&fd.locals[fd.params[1].local as usize].ty), fortis_types::types::Type::Named(n) if n.name == "Builder")
                    && matches!(fortis_types::types::strip(&fd.ret), fortis_types::types::Type::Named(n) if n.name == "Void");
                if is_impl { Some(f) } else { None }
            }
            None => None,
        };
        if let Some(f) = own {
            let fi = self.func_index(&f)?;
            let into = Value::Builder(shared(String::new()));
            let param = self.prog.functions[fi].params.get(1).map(|p| p.name.clone()).unwrap_or_else(|| "into".into());
            match self.call(fi, vec![("self".into(), v.clone()), (param, into.clone())], &[]) {
                Ok(_) | Err(Exit::Flow(Flow::Return(_))) => {}
                Err(e) => return Err(e),
            }
            return Ok(match into {
                Value::Builder(b) => b.borrow().clone(),
                _ => String::new(),
            });
        }
        Ok(match &v {
            Value::Obj(o) => {
                let (ty, fields): (Rc<str>, Vec<(Rc<str>, Value)>) = {
                    let o = o.borrow();
                    (o.ty.clone(), o.fields.iter().map(|(k, x)| (k.clone(), x.clone())).collect())
                };
                let mut parts = Vec::new();
                for (k, x) in fields {
                    parts.push(format!("{k}: {}", self.show(&x)?));
                }
                format!("{ty}({})", parts.join(", "))
            }
            Value::Enum(e) if !e.fields.is_empty() => {
                let mut parts = Vec::new();
                for (k, x) in e.fields.iter() {
                    parts.push(format!("{k}: {}", self.show(x)?));
                }
                format!("{}({})", e.variant, parts.join(", "))
            }
            Value::List(items) | Value::Array(items) => {
                let xs: Vec<Value> = items.borrow().clone();
                let mut parts = Vec::new();
                for x in xs {
                    parts.push(self.show(&x)?);
                }
                format!("[{}]", parts.join(", "))
            }
            Value::SharedRef(b) => {
                let inner = b.value.borrow().clone();
                self.show(&inner)?
            }
            other => display(other),
        })
    }

    /// The function implementing `method` for a receiver's run-time type.
    pub fn dispatch(&mut self, recv: &Value, method: &str) -> R<Value> {
        let tname: String = match recv {
            Value::Obj(o) => o.borrow().ty.to_string(),
            Value::Enum(e) => e.ty.to_string(),
            Value::Cell(c) => {
                let v = c.borrow().clone();
                return self.dispatch(&v, method);
            }
            other => crate::intrinsics::builtin_type(other).to_string(),
        };
        if let Some(t) = self.types.get(tname.as_str()) {
            if let Some((_, f)) = t.methods.iter().find(|(m, _)| m == method) {
                return Ok(Value::Func(self.func_index(f)?));
            }
        }
        for m in fortis_mir::build::LIBRARY_MODULES {
            let n = format!("fortis.{m}::method {tname}.{method}");
            if let Some(i) = self.funcs.get(n.as_str()) {
                return Ok(Value::Func(*i));
            }
        }
        Ok(Value::Intrinsic(Rc::from(format!("{tname}.{method}"))))
    }

    // ---- blocks and statements ------------------------------------------------------------------------------

    pub fn block(&mut self, fr: &mut Frame, b: &Block) -> R<()> {
        self.block_scoped(fr, b, None)
    }

    /// A block, with the scope a `scope` statement declares for it (its tasks are joined at its exit).
    pub fn block_scoped(&mut self, fr: &mut Frame, b: &Block, scope: Option<&crate::value::ScopeVal>) -> R<()> {
        self.defer_stack.push(vec![]);
        let mut r = Ok(());
        for s in &b.stmts {
            r = self.stmt(fr, s);
            if r.is_err() {
                break;
            }
        }
        let registered = self.defer_stack.pop().unwrap_or_default();
        if let Err(Exit::Halt(Halt::Defect(_) | Halt::Shutdown)) = &r {
            return r; // a defect terminates the task at once: no cleanup runs (10.8)
        }
        // a failure or a cancellation unwinds: deferred blocks, abandon, drops (12.4)
        let unwinding = matches!(r, Err(Exit::Flow(Flow::Fail(_))) | Err(Exit::Halt(Halt::Cancel)));
        let ex = self.exit_block(fr, b, unwinding, scope, &registered);
        match (r, ex) {
            // a deferred block's dynamic check failure or defect stops the task however the block itself left: a `return`
            // carrying the block's result must not hide it, so a missed check fails loudly (12.1; D-391)
            (Err(Exit::Flow(_)), Err(e @ Exit::Halt(_))) => Err(e),
            (Err(e), _) => Err(e),
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// The class whose `cleanup` destroying this value runs: a class value that declares one and that its owner has
    /// not destroyed yet. A destruction is announced in the trace exactly when it runs that `cleanup` (D-351). Only
    /// an owner destroys, so the caller has already decided that the value is owned (7.9; D-338, D-348).
    fn cleanup_runs(&self, v: &Value) -> Option<String> {
        if let Value::Obj(o) = v.deref() {
            let o = o.borrow();
            if !o.destroyed && self.types.get(&*o.ty).map_or(false, |t| t.cleanup.is_some()) {
                return Some(o.ty.to_string());
            }
        }
        None
    }

    /// A live local whose class implements `Abandonable`: abandoned when its block unwinds (12.4).
    fn abandonable(&self, v: &Value) -> bool {
        if let Value::Obj(o) = v.deref() {
            let ty = o.borrow().ty.clone();
            return self.types.get(&*ty).map_or(false, |t| t.implements.iter().any(|i| i == "Abandonable"));
        }
        false
    }

    /// An expression that can lend: `alias p` or `editable p`, or a value whose type is a loan or an optional loan
    /// (`xs.get(0)`, `m.get(k)`, a cursor's `next()`, a call whose result is declared `from` a parameter, a `??`
    /// with a loan on one side).
    fn lends(init: &Expr) -> bool {
        matches!(init.kind, ExprKind::Place(_, Use::Alias | Use::Editable)) || Self::loan_ty(&init.ty)
    }

    /// A type whose value is a loan: a loan, or an optional loan (`(alias T from xs)?`).
    fn loan_ty(ty: &fortis_types::types::Type) -> bool {
        use fortis_types::types::Type;
        match ty {
            Type::Loan { .. } => true,
            Type::Optional(i) => Self::loan_ty(i),
            _ => false,
        }
    }

    /// A call whose declared result lends (7.6): a loan or an optional loan, or an outcome whose success type is
    /// one (the try side of an `or recover` or a `catch` is typed as the outcome; D-451).
    fn call_lends(ty: &fortis_types::types::Type) -> bool {
        use fortis_types::types::Type;
        match ty {
            Type::Named(n) if n.name == "Outcome" && !n.args.is_empty() => Self::loan_ty(&n.args[0]),
            _ => Self::loan_ty(ty),
        }
    }

    /// A type whose value holds a loan somewhere: a loan, or a tuple, a record or an optional with one in it. Such an
    /// aggregate owns its other parts, and destroying it destroys only those (7.9; D-349).
    fn holds_loan_ty(ty: &fortis_types::types::Type) -> bool {
        use fortis_types::types::Type;
        match ty {
            Type::Loan { .. } => true,
            Type::Optional(i) => Self::holds_loan_ty(i),
            Type::Tuple(ts) => ts.iter().any(Self::holds_loan_ty),
            Type::Record(fs) => fs.iter().any(|(_, t)| Self::holds_loan_ty(t)),
            _ => false,
        }
    }

    /// Evaluate the value a `let` binds or an assignment stores, and whether it is a loan the local then holds rather
    /// than a value it owns. An expression that cannot lend gives an owned value. One that can lend but can also give
    /// an owned value (a `??` whose fallback moves or makes one, an `if`, a block, a `match`, a `catch` or an
    /// `or recover` whose sides differ) is decided by the side that gave the value (7.3, 7.9; D-338, D-348). A
    /// loan-carrying value (a guard, a cursor) is owned by its binding even when it is declared `from` its origin
    /// (7.7), so dropping a guard unlocks.
    fn eval_held(&mut self, fr: &mut Frame, e: &Expr) -> R<(Value, bool)> {
        if !Self::lends(e) {
            return Ok((self.eval(fr, e)?, false));
        }
        let (v, lent) = match &e.kind {
            // a moved place gives what it owned (a loan, when the local held one, or when the aggregate it was part of
            // borrowed that part); a copy is owned
            ExprKind::Place(p, Use::Move) => {
                let held = if p.proj.is_empty() { self.holds_loan(fr, p.local) } else { self.part_lent(fr, p, &e.ty) };
                (self.eval(fr, e)?, held)
            }
            ExprKind::Place(_, Use::Copy) => (self.eval(fr, e)?, false),
            ExprKind::Coalesce(a, b) => {
                let (v, lent) = self.eval_held(fr, a)?;
                if matches!(v.deref(), Value::Null) {
                    self.eval_held(fr, b)?
                } else {
                    (v, lent)
                }
            }
            ExprKind::Present(a) => self.eval_held(fr, a)?,
            ExprKind::OrRecover(a, b) => {
                let v = self.eval(fr, a)?;
                self.hit(a.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if v.is_failure() {
                    self.eval_held(fr, b)?
                } else {
                    (v, true)
                }
            }
            ExprKind::Catch { value, bind, handler } => {
                let v = self.eval(fr, value)?;
                self.hit(value.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if !v.is_failure() {
                    (v, true)
                } else {
                    fr.locals[*bind as usize] = v;
                    self.gave_lent = true;
                    match self.block(fr, handler) {
                        Err(Exit::Flow(Flow::Recover(r))) => (r, self.gave_lent),
                        Ok(()) => return dynamic("catch block fell through without recover/return/fail"),
                        Err(e) => return Err(e),
                    }
                }
            }
            ExprKind::If { .. } | ExprKind::Block(_) | ExprKind::Match { .. } => {
                // the `give` (or a `match` arm's expression) that ends the branch taken says what it gave
                self.gave_lent = true;
                let v = self.eval(fr, e)?;
                (v, self.gave_lent)
            }
            // a call whose result is declared `from` a parameter gives the loan the callee returned, or a value of
            // the callee's own (a `??` whose fallback it moved or made, a moved local), which the binding that
            // receives it then owns; the callee's `return` says which, an intrinsic always lends (7.9; D-373)
            ExprKind::Call { .. } => {
                let v = self.eval(fr, e)?;
                (v, self.ret_lent)
            }
            _ => (self.eval(fr, e)?, true),
        };
        let carried = matches!(v.deref(), Value::Guard(_) | Value::Cursor(_));
        Ok((v, lent && !carried))
    }

    /// What a statement temporary holds (5.7, 7.9; D-451): the value its expression gave, and whether that value is
    /// a loan the temporary holds rather than owns, so the statement's end destroys nothing and only a value the
    /// expression made or moved (an `or recover`'s or a `catch` handler's own replacement) is destroyed there.
    /// Never decided by the static type, unlike `eval_held` for a binding: a temporary of an outcome whose sides
    /// differ is typed with the value type (`Token`, not `alias Token from xs`), and only the run says which side
    /// gave the value, a loan or one of the statement's own. An expression that cannot forward a loan (a
    /// construction, a copy, a literal, an aggregate of the statement's own, an `await`) gives an owned value.
    fn eval_temp(&mut self, fr: &mut Frame, e: &Expr) -> R<(Value, bool)> {
        let (v, lent) = match &e.kind {
            // a moved place gives what it owned: a loan when the local held one (7.9; D-375)
            ExprKind::Place(p, Use::Move) => {
                let held = if p.proj.is_empty() { self.holds_loan(fr, p.local) } else { self.part_lent(fr, p, &e.ty) };
                (self.eval(fr, e)?, held)
            }
            ExprKind::Coalesce(a, b) => {
                let (v, lent) = self.eval_temp(fr, a)?;
                if matches!(v.deref(), Value::Null) {
                    self.eval_temp(fr, b)?
                } else {
                    (v, lent)
                }
            }
            ExprKind::Present(a) => self.eval_temp(fr, a)?,
            ExprKind::OrFail(a) => {
                let (v, lent) = self.eval_temp(fr, a)?;
                if v.is_failure() {
                    return Err(Exit::Flow(Flow::Fail(v)));
                }
                (v, lent)
            }
            // the side that ran gives the value, and says what it gave: a try side that succeeded with the loan
            // the callee returned leaves the temporary holding it, and the made replacement of a try side that
            // failed is the temporary's own (7.9; D-451)
            ExprKind::OrRecover(a, b) => {
                let (v, lent) = self.eval_temp(fr, a)?;
                self.hit(a.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if v.is_failure() {
                    self.eval_temp(fr, b)?
                } else {
                    (v, lent)
                }
            }
            ExprKind::Catch { value, bind, handler } => {
                let (v, lent) = self.eval_temp(fr, value)?;
                self.hit(value.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if !v.is_failure() {
                    (v, lent)
                } else {
                    fr.locals[*bind as usize] = v;
                    self.gave_lent = true;
                    match self.block(fr, handler) {
                        Err(Exit::Flow(Flow::Recover(r))) => (r, self.gave_lent),
                        Ok(()) => return dynamic("catch block fell through without recover/return/fail"),
                        Err(e) => return Err(e),
                    }
                }
            }
            ExprKind::If { .. } | ExprKind::Block(_) | ExprKind::Match { .. } => {
                // the `give` (or a `match` arm's expression) that ends the branch taken says what it gave
                self.gave_lent = true;
                let v = self.eval(fr, e)?;
                (v, self.gave_lent)
            }
            // a call lends only where its declared result is a loan (7.6): an intrinsic that hands back a value
            // of its own (`pop`, `String.from`) lends nothing whatever `ret_lent` says, and where the result is
            // declared `from` a parameter the callee's `return` says whether it gave the loan or a value of its
            // own (7.9; D-373). The try side of an outcome is typed as the outcome, so its success type decides
            ExprKind::Call { .. } => {
                let v = self.eval(fr, e)?;
                (v, Self::call_lends(&e.ty) && self.ret_lent)
            }
            _ => (self.eval(fr, e)?, false),
        };
        let carried = matches!(v.deref(), Value::Guard(_) | Value::Cursor(_));
        Ok((v, lent && !carried))
    }

    /// What a `return` gives: a loan, or a value of the function's own, decided by the expression and the value and
    /// never by the type alone, since a library body's typed IR need not say `from` (`List.first` is
    /// `=> self.get(0)`). The function's own is a moved place the frame owns, a copy, a value constructed here (a
    /// tuple or a record, which keeps the loan mask of its parts, a class or variant value, a closure, a literal, an
    /// arithmetic or string result), a `??` or `present` side that is one, a branch that gave one, an outcome
    /// expression whose value side or whose handler's `recover` gave one (D-385), or a call the callee reported as
    /// its own; anything else (a place read, a lending intrinsic, a value whose kind does not say) is a loan, as
    /// every `from` result was before (D-373, D-381).
    fn ret_held(&mut self, fr: &mut Frame, e: &Expr) -> R<(Value, bool)> {
        let (v, lent) = match &e.kind {
            ExprKind::Place(p, Use::Move) => {
                let held = if p.proj.is_empty() { self.holds_loan(fr, p.local) } else { self.part_lent(fr, p, &e.ty) };
                (self.eval(fr, e)?, held)
            }
            ExprKind::Place(_, Use::Copy) => (self.eval(fr, e)?, false),
            ExprKind::Tuple(_)
            | ExprKind::Record(_)
            | ExprKind::Construct { .. }
            | ExprKind::Variant { .. }
            | ExprKind::MakeFailure { .. }
            | ExprKind::Closure { .. }
            | ExprKind::Const(_)
            | ExprKind::Interpolate(_)
            | ExprKind::Binary(..)
            | ExprKind::Unary(..)
            | ExprKind::Wrap { .. } => (self.eval(fr, e)?, false),
            ExprKind::Coalesce(a, b) => {
                let (v, lent) = self.ret_held(fr, a)?;
                if matches!(v.deref(), Value::Null) {
                    self.ret_held(fr, b)?
                } else {
                    (v, lent)
                }
            }
            ExprKind::Present(a) => self.ret_held(fr, a)?,
            // an outcome expression gives its value side, or what its handler recovered, each decided by its own
            // expression as a `??` side is (D-385)
            ExprKind::OrFail(a) => {
                let (v, lent) = self.ret_held(fr, a)?;
                if v.is_failure() {
                    return Err(Exit::Flow(Flow::Fail(v)));
                }
                (v, lent)
            }
            ExprKind::OrRecover(a, b) => {
                let (v, lent) = self.ret_held(fr, a)?;
                self.hit(a.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if v.is_failure() {
                    self.ret_held(fr, b)?
                } else {
                    (v, lent)
                }
            }
            ExprKind::Catch { value, bind, handler } => {
                let (v, lent) = self.ret_held(fr, value)?;
                self.hit(value.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if !v.is_failure() {
                    (v, lent)
                } else {
                    fr.locals[*bind as usize] = v;
                    self.gave_lent = true;
                    match self.block(fr, handler) {
                        Err(Exit::Flow(Flow::Recover(r))) => (r, self.gave_lent),
                        Ok(()) => return dynamic("catch block fell through without recover/return/fail"),
                        Err(e) => return Err(e),
                    }
                }
            }
            ExprKind::If { .. } | ExprKind::Block(_) | ExprKind::Match { .. } => {
                self.gave_lent = true;
                let v = self.eval(fr, e)?;
                (v, self.gave_lent)
            }
            ExprKind::Call { .. } => {
                let v = self.eval(fr, e)?;
                (v, self.ret_lent)
            }
            _ => (self.eval(fr, e)?, true),
        };
        let carried = matches!(v.deref(), Value::Guard(_) | Value::Cursor(_));
        Ok((v, lent && !carried))
    }

    /// Whether a part of a local (a tuple element, a record field, at any depth) is a loan: the local holds a loan,
    /// or the aggregate the part is in recorded it as one. Where nothing was recorded, the part's type decides
    /// (7.9; D-349, D-366).
    fn part_lent(&self, fr: &Frame, p: &Place, ty: &fortis_types::types::Type) -> bool {
        if self.holds_loan(fr, p.local) {
            return true;
        }
        let mut v = fr.locals[p.local as usize].deref();
        for pr in &p.proj {
            let (lent, next) = match (pr, &v) {
                (Proj::Tuple(i), Value::Tuple(t)) => (lent_part(&t.lent, *i as usize), t.get(*i as usize).cloned()),
                (Proj::Field(f), Value::Record(r)) => {
                    let r = r.borrow();
                    match r.fields.iter().position(|(k, _)| &**k == f) {
                        Some(i) => (lent_part(&r.lent, i), Some(r.fields[i].1.clone())),
                        None => (None, None),
                    }
                }
                _ => (None, None),
            };
            match (lent, next) {
                (Some(true), _) => return true,
                (Some(false), Some(n)) => v = n.deref(),
                _ => return Self::loan_ty(ty),
            }
        }
        false
    }

    /// The record literal a place's last projection is a field of, and that field's index, when it is one.
    fn record_field(&self, fr: &Frame, p: &Place) -> Option<(Shared<RecordVal>, usize)> {
        let Some(Proj::Field(f)) = p.proj.last() else { return None };
        let mut v = fr.locals[p.local as usize].deref();
        for pr in &p.proj[..p.proj.len() - 1] {
            v = self.project(v, pr).ok()?;
        }
        match v.deref() {
            Value::Record(r) => {
                let i = r.borrow().fields.iter().position(|(k, _)| &**k == f)?;
                Some((r, i))
            }
            _ => None,
        }
    }

    /// The bindings of a pattern matched against a value: where the statement owns that value (a `let` pattern of a
    /// moved or new value, a `match` on one), a binding whose type is a loan owns what it took when the aggregate it
    /// took it from owned that part, and its end destroys it; otherwise it holds a loan (7.9; D-366).
    fn claim_owned_parts(&self, fr: &mut Frame, p: &Pattern, v: &Value, owned: bool) {
        let v = v.deref();
        match p {
            Pattern::Bind(l) | Pattern::Otherwise(Some(l)) => {
                if owned && Self::loan_ty(self.local_ty(fr, *l)) {
                    fr.owned.insert(*l);
                } else {
                    fr.owned.remove(l);
                }
            }
            Pattern::Present(i) => self.claim_owned_parts(fr, i, &v, owned),
            Pattern::Tuple(ps) => {
                let Value::Tuple(t) = &v else { return };
                for (i, (sp, x)) in ps.iter().zip(t.iter()).enumerate() {
                    self.claim_owned_parts(fr, sp, x, owned && lent_part(&t.lent, i) == Some(false));
                }
            }
            Pattern::Record(fs) => {
                let Value::Record(r) = &v else { return };
                let parts: Vec<(Value, bool)> = {
                    let r = r.borrow();
                    fs.iter()
                        .map(|(n, _)| match r.fields.iter().position(|(k, _)| &**k == n) {
                            Some(i) => (r.fields[i].1.clone(), lent_part(&r.lent, i) == Some(false)),
                            None => (Value::Null, false),
                        })
                        .collect()
                };
                for ((_, sp), (x, part)) in fs.iter().zip(parts) {
                    self.claim_owned_parts(fr, sp, &x, owned && part);
                }
            }
            _ => {}
        }
    }

    /// Record whether a `let` local holds a loan of the value just stored in it.
    fn hold(fr: &mut Frame, local: LocalId, lent: bool) {
        if lent {
            fr.lent.insert(local);
        } else {
            fr.lent.remove(&local);
        }
    }

    /// A local that holds a loan at its block's exit: a `let` bound or last assigned a value that lends, or a pattern
    /// binding of a loan (a borrowed element of a destructured tuple or record). Its end destroys nothing (7.9). A
    /// pattern binding that took a value an owned aggregate held in a loan's place owns it (D-366).
    fn holds_loan(&self, fr: &Frame, l: LocalId) -> bool {
        if fr.lent.contains(&l) {
            return true;
        }
        if fr.owned.contains(&l) {
            return false;
        }
        let d = &self.prog.functions[fr.func].locals[l as usize];
        d.kind == LocalKind::Binding && Self::loan_ty(&d.ty)
    }

    /// The declared type of a local of the running function.
    fn local_ty(&self, fr: &Frame, l: LocalId) -> &'p fortis_types::types::Type {
        &self.prog.functions[fr.func].locals[l as usize].ty
    }

    /// Destroy a value its owner holds, with the type its owner declares. A loan in it is not the owner's: a tuple
    /// or a record that holds a loan beside owned parts destroys only those (7.9; D-349). The owner has already
    /// decided that the value itself is owned: a local whose type is a loan can own what a `??` fallback or an `if`
    /// moved into it.
    fn destroy_typed(&mut self, v: Value, ty: &fortis_types::types::Type) -> R<()> {
        if Self::loan_ty(ty) {
            return self.destroy(v);
        }
        self.destroy_part(v, ty)
    }

    /// A part of an owned value: a loan part destroys nothing.
    fn destroy_part(&mut self, v: Value, ty: &fortis_types::types::Type) -> R<()> {
        use fortis_types::types::Type;
        if !Self::holds_loan_ty(ty) {
            return self.destroy(v);
        }
        match (ty, v) {
            (Type::Loan { .. }, _) => Ok(()),
            (_, Value::Moved | Value::Null | Value::Void) => Ok(()),
            (_, Value::Cell(c)) => {
                let inner = std::mem::replace(&mut *c.borrow_mut(), Value::Moved);
                self.destroy_part(inner, ty)
            }
            (Type::Optional(i), v) => self.destroy_part(v, i),
            (Type::Tuple(ts), Value::Tuple(items)) => {
                if let Ok(t) = Rc::try_unwrap(items) {
                    let TupleVal { items, lent } = t;
                    for (i, (x, t)) in items.into_iter().zip(ts.iter()).enumerate() {
                        self.destroy_element(x, t, lent_part(&lent, i))?;
                    }
                }
                Ok(())
            }
            (Type::Record(ts), Value::Record(fs)) => {
                if Rc::strong_count(&fs) == 1 {
                    let RecordVal { fields, lent } = std::mem::take(&mut *fs.borrow_mut());
                    for (i, (n, x)) in fields.into_iter().enumerate() {
                        match ts.iter().find(|(k, _)| k.as_str() == &*n) {
                            Some((_, t)) => self.destroy_element(x, t, lent_part(&lent, i))?,
                            None if lent_part(&lent, i) == Some(true) => {}
                            None => self.destroy(x)?,
                        }
                    }
                }
                Ok(())
            }
            (_, v) => self.destroy(v),
        }
    }

    /// An element or a field of an owned aggregate, as the aggregate recorded it when it was built: a loan destroys
    /// nothing, and a value of its own is destroyed even where its type is a loan (a `??` whose fallback is owned).
    /// Where nothing was recorded, its type decides (7.9; D-349, D-366).
    fn destroy_element(&mut self, x: Value, ty: &fortis_types::types::Type, lent: Option<bool>) -> R<()> {
        match lent {
            Some(true) => Ok(()),
            Some(false) => self.destroy_typed(x, ty),
            None => self.destroy_part(x, ty),
        }
    }

    /// Block exit (5.1, 12.2, 12.4): the block's loans end and the tasks launched on a scope declared in it are
    /// cancelled and joined; then the registered deferred blocks LIFO; then, when unwinding, `abandon` on every
    /// live Abandonable local in reverse initialisation order; then locals drop in reverse order.
    fn exit_block(&mut self, fr: &mut Frame, b: &Block, unwinding: bool, scope: Option<&crate::value::ScopeVal>, registered: &[usize]) -> R<()> {
        // a local that holds a loan is not abandoned, not announced and not destroyed: its owner is (D-338)
        let lent: Vec<LocalId> = b.locals.iter().copied().filter(|l| self.holds_loan(fr, *l)).collect();
        let abandon: Vec<LocalId> = if unwinding { b.locals.iter().copied().filter(|l| !lent.contains(l) && self.abandonable(&fr.locals[*l as usize])).collect() } else { vec![] };
        let scope_tasks = scope.map_or(false, |s| !s.tasks.borrow().is_empty());
        let noisy = !registered.is_empty() || scope_tasks || !abandon.is_empty() || b.locals.iter().any(|l| !lent.contains(l) && self.cleanup_runs(&fr.locals[*l as usize]).is_some());
        if noisy {
            self.trace.push("loans-end".into());
        }
        if let Some(sc) = scope {
            self.exit_scope(sc, unwinding)?;
        }
        for &i in registered.iter().rev() {
            self.trace.push("defer".into());
            self.cleanup_depth += 1;
            let r = self.block(fr, &b.defers[i]);
            self.cleanup_depth -= 1;
            r?;
        }
        for l in abandon.iter().rev() {
            let v = std::mem::replace(&mut fr.locals[*l as usize], Value::Moved).deref();
            let ty = match &v {
                Value::Obj(o) => o.borrow().ty.to_string(),
                _ => continue,
            };
            self.trace.push(format!("abandon {ty}"));
            let target = self.dispatch(&v, "abandon")?;
            if let Value::Func(fi) = target {
                self.call(fi, vec![("self".into(), v)], &[])?;
            }
        }
        // the block's loans end before its owners are destroyed: their handles go and nothing is destroyed, so an owner
        // never waits on a loan's handle (7.9; D-348)
        for l in &lent {
            fr.locals[*l as usize] = Value::Moved;
            fr.lent.remove(l);
        }
        for l in b.locals.iter().rev() {
            let v = std::mem::replace(&mut fr.locals[*l as usize], Value::Moved);
            fr.owned.remove(l);
            if matches!(v, Value::Moved) {
                continue;
            }
            let v = match v {
                Value::Cell(c) => {
                    let inner = std::mem::replace(&mut *c.borrow_mut(), Value::Moved);
                    inner
                }
                v => v,
            };
            if let Some(t) = self.cleanup_runs(&v) {
                self.trace.push(format!("drop {t}"));
            }
            let ty = self.local_ty(fr, *l);
            self.destroy_typed(v, ty)?;
        }
        Ok(())
    }

    /// A class value's own `cleanup`, while its fields are still there (7.9). A report from it is labelled as
    /// cleanup's (12.1; D-154).
    fn run_cleanup(&mut self, o: &Shared<Obj>) -> R<()> {
        let ty = o.borrow().ty.clone();
        let Some(c) = self.types.get(&*ty).and_then(|t| t.cleanup.clone()) else {
            return Ok(());
        };
        let fi = self.func_index(&c)?;
        self.cleanup_depth += 1;
        let r = self.call(fi, vec![("self".into(), Value::Obj(o.clone()))], &[]);
        self.cleanup_depth -= 1;
        match r {
            Ok(_) | Err(Exit::Flow(Flow::Return(_))) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Destroy a value (7.9): a class value with `cleanup` runs it, then its fields are destroyed; the last strong
    /// `Shared` handle destroys its value; a list destroys its items; a guard unlocks; a file closes.
    pub fn destroy(&mut self, v: Value) -> R<()> {
        match v {
            Value::Guard(m) => {
                m.borrow_mut().held = false;
            }
            Value::File(f) => {
                f.borrow_mut().closed = true;
            }
            Value::Obj(o) => {
                // a class value: its own cleanup if declared, then its fields in declaration order (7.9), whether
                // or not the class declares a cleanup. Only an owner destroys: a loan, an alias or a view (a `get`,
                // an `alias` binding, a `from` result, a borrowed element of a tuple) never reaches here, whoever
                // else still holds a handle, so `cleanup` runs once, when the owner goes, and never again (12.2;
                // D-338, D-348).
                if !o.borrow().destroyed {
                    o.borrow_mut().destroyed = true;
                    self.run_cleanup(&o)?;
                }
                // The fields are moved out, not cloned: a nested list, map, tuple or enum is then the unique owner of
                // its contents and destroys them (D-306). A block's loans end before its owners are destroyed, so the
                // owner holds the last handle; only a program the checker should have refused (a second owner,
                // D-352) still shares it here, and the fields then stay for that handle's destruction.
                if Rc::strong_count(&o) == 1 {
                    let fields = std::mem::take(&mut o.borrow_mut().fields);
                    for (_, f) in fields {
                        self.destroy(f)?;
                    }
                }
            }
            Value::SharedRef(b) => {
                b.strong.set(b.strong.get() - 1);
                if b.strong.get() == 0 {
                    let inner = std::mem::replace(&mut *b.value.borrow_mut(), Value::Moved);
                    self.destroy(inner)?;
                }
            }
            Value::Enum(e) => {
                // an enum value drops its payload (7.9). Move the fields out so a nested aggregate is unique.
                if let Ok(em) = Rc::try_unwrap(e) {
                    for (_, f) in em.fields {
                        self.destroy(f)?;
                    }
                }
            }
            Value::Future(f) => {
                // an async call that never ran owns its owned arguments: they go with it (7.9, 12.3; reachable
                // only through a future held in shared state, D-33). One bound in another generation is resolved
                // by identity for its parameters' modes, and read from its own slots.
                // its work is taken once, so a second destruction (a field destroyed through a copy) finds none
                if f.task.get().is_none() && !crate::session::fault("future-args-kept") {
                    let work = f.work.borrow_mut().take();
                    if let Some(w) = work {
                        self.destroy_work(w)?;
                    }
                }
            }
            Value::List(items) => {
                if Rc::strong_count(&items) == 1 {
                    let xs = std::mem::take(&mut *items.borrow_mut());
                    for x in xs {
                        self.destroy(x)?;
                    }
                }
            }
            Value::Array(items) => {
                if Rc::strong_count(&items) == 1 {
                    let xs = std::mem::take(&mut *items.borrow_mut());
                    for x in xs {
                        self.destroy(x)?;
                    }
                }
            }
            Value::Tuple(items) => {
                // move the elements out so a nested tuple or other aggregate is the unique owner (D-306); an element
                // it borrows is its owner's (D-349, D-366)
                if let Ok(t) = Rc::try_unwrap(items) {
                    let TupleVal { items, lent } = t;
                    for (i, x) in items.into_iter().enumerate() {
                        if lent_part(&lent, i) != Some(true) {
                            self.destroy(x)?;
                        }
                    }
                }
            }
            Value::Record(fs) => {
                // a record literal: its fields in written order (7.9), except a field it borrows
                if Rc::strong_count(&fs) == 1 {
                    let RecordVal { fields, lent } = std::mem::take(&mut *fs.borrow_mut());
                    for (i, (_, x)) in fields.into_iter().enumerate() {
                        if lent_part(&lent, i) != Some(true) {
                            self.destroy(x)?;
                        }
                    }
                }
            }
            Value::Map(m) | Value::Set(m) => {
                if Rc::strong_count(&m) == 1 {
                    let taken = std::mem::take(&mut *m.borrow_mut());
                    for (k, x) in taken.entries {
                        self.destroy(k)?;
                        self.destroy(x)?;
                    }
                }
            }
            Value::Closure(c) => {
                // a closure owns its `copy`, `move` and bare captures: destroying it destroys those its body has not
                // moved out, in capture-list order; `alias` and `editable` captures are loans (7.9, X-250; D-30)
                // A capture that holds a tuple with a loan in it destroys only what it owns (D-349).
                let prog = self.prog;
                let tys: Vec<Option<&fortis_types::types::Type>> = match prog.functions.get(c.func).and_then(|f| f.closure.as_ref().map(|cl| (f, cl))) {
                    Some((f, cl)) if c.code.is_none() => cl.captures.iter().map(|x| f.locals.get(x.local as usize).map(|d| &d.ty)).collect(),
                    _ => vec![],
                };
                for (i, (v, owned)) in c.captures.iter().zip(c.owned.iter()).enumerate() {
                    if let (true, Value::Cell(cell)) = (*owned, v) {
                        let inner = std::mem::replace(&mut *cell.borrow_mut(), Value::Moved);
                        if !matches!(inner, Value::Moved) {
                            match tys.get(i).copied().flatten() {
                                Some(ty) => self.destroy_typed(inner, ty)?,
                                None => self.destroy(inner)?,
                            }
                        }
                    }
                }
            }
            Value::Mutex(m) => {
                // a Mutex owns its value (7.9, X-250; D-30)
                let inner = std::mem::replace(&mut m.borrow_mut().value, Value::Moved);
                if !matches!(inner, Value::Moved) {
                    self.destroy(inner)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Work that never ran (a future destroyed unpolled, or a task cancelled before it started): an async call's
    /// owned arguments go with it, and so does a closure it consumed; launched work is a closure (7.9, 12.3; D-30).
    pub fn destroy_work(&mut self, work: crate::executor::Work) -> R<()> {
        if let crate::executor::Work::Closure(c) = work {
            // launched work: the closure goes with it (7.9, X-250; D-30)
            self.destroy(c)?;
        } else if let crate::executor::Work::Call { func, frame, code } = work {
            let consumed = frame.consumed;
            if let Ok(fi) = self.resolve(func, code.as_deref()) {
                let other = matches!((&self.gen, code.as_deref()), (Some(g), Some(c)) if c.gen != g.id);
                let owned: Vec<usize> = self.prog.functions[fi].params.iter().enumerate().filter(|(_, p)| p.mode == Mode::Owned).map(|(k, _)| k).collect();
                let mut locals = frame.locals;
                for k in owned.into_iter().rev() {
                    let slot = if other { code.as_ref().and_then(|c| c.slots.get(k).copied()) } else { self.prog.functions[fi].params.get(k).map(|p| p.local) };
                    if let Some(sl) = slot {
                        let v = std::mem::replace(&mut locals[sl as usize], Value::Moved);
                        let v = match v {
                            Value::Cell(c) => std::mem::replace(&mut *c.borrow_mut(), Value::Moved),
                            v => v,
                        };
                        if !matches!(v, Value::Moved | Value::Void) {
                            self.destroy(v)?;
                        }
                    }
                }
            }
            // the closure a consuming call of an async closure handed to it (7.9, X-250; D-30)
            if let Some(c) = consumed {
                self.destroy(c)?;
            }
        }
        Ok(())
    }

    /// The identity of a value's underlying allocation; 0 for values that have none (their destruction is a
    /// no-op, so nothing has to recognise them). A file, closure, mutex, guard, future or failure has an
    /// allocation too: a binding of one must not be destroyed again with the structure it was taken from.
    fn value_id(v: &Value) -> usize {
        match v.deref() {
            Value::Obj(o) => Rc::as_ptr(&o) as usize,
            Value::Enum(e) => Rc::as_ptr(&e) as usize,
            Value::List(x) | Value::Array(x) => Rc::as_ptr(&x) as usize,
            Value::Tuple(t) => Rc::as_ptr(&t) as usize,
            Value::Record(r) => Rc::as_ptr(&r) as usize,
            Value::Map(m) | Value::Set(m) => Rc::as_ptr(&m) as usize,
            Value::SharedRef(b) => Rc::as_ptr(&b) as usize,
            Value::File(f) => Rc::as_ptr(&f) as usize,
            Value::Closure(c) => Rc::as_ptr(&c) as usize,
            Value::Mutex(m) | Value::Guard(m) => Rc::as_ptr(&m) as usize,
            Value::Future(f) => Rc::as_ptr(&f) as usize,
            Value::Failure(f) => Rc::as_ptr(&f) as usize,
            _ => 0,
        }
    }

    fn remember_bound(fr: &Frame, local: LocalId, ids: &mut HashSet<usize>) {
        let id = Self::value_id(&fr.locals[local as usize]);
        if id != 0 {
            ids.insert(id);
        }
    }

    /// Allocations a destructuring pattern bound. Those stay with the bindings; the rest of the moved source
    /// is destroyed (7.9, D-305).
    fn pattern_bound_ids(&self, fr: &Frame, p: &Pattern) -> HashSet<usize> {
        let mut ids = HashSet::new();
        self.collect_bound(fr, p, &mut ids);
        ids
    }

    fn collect_bound(&self, fr: &Frame, p: &Pattern, ids: &mut HashSet<usize>) {
        match p {
            Pattern::Bind(l) | Pattern::Otherwise(Some(l)) => Self::remember_bound(fr, *l, ids),
            Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => self.collect_bound(fr, i, ids),
            Pattern::Tuple(ps) => {
                for sp in ps {
                    self.collect_bound(fr, sp, ids);
                }
            }
            Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => {
                for (_, sp) in fields {
                    self.collect_bound(fr, sp, ids);
                }
            }
            Pattern::Is { bind, fields, .. } => {
                if let Some(l) = bind {
                    Self::remember_bound(fr, *l, ids);
                }
                for (_, sp) in fields {
                    self.collect_bound(fr, sp, ids);
                }
            }
            _ => {}
        }
    }

    /// `destroy_except` with the type the value's owner declares: a loan in it, bound or not, is not destroyed
    /// (7.9; D-349).
    fn destroy_except_typed(&mut self, v: Value, ty: &fortis_types::types::Type, bound: &HashSet<usize>) -> R<()> {
        use fortis_types::types::Type;
        if !Self::holds_loan_ty(ty) {
            return self.destroy_except(v, bound);
        }
        let id = Self::value_id(&v);
        if id != 0 && bound.contains(&id) {
            return Ok(());
        }
        match (ty, v) {
            (Type::Loan { .. }, _) => Ok(()),
            (_, Value::Moved | Value::Null | Value::Void) => Ok(()),
            (_, Value::Cell(c)) => {
                let inner = std::mem::replace(&mut *c.borrow_mut(), Value::Moved);
                self.destroy_except_typed(inner, ty, bound)
            }
            (Type::Optional(i), v) => self.destroy_except_typed(v, i, bound),
            (Type::Tuple(ts), Value::Tuple(items)) => {
                if let Ok(t) = Rc::try_unwrap(items) {
                    let TupleVal { items, lent } = t;
                    for (i, (x, t)) in items.into_iter().zip(ts.iter()).enumerate() {
                        self.destroy_except_element(x, t, lent_part(&lent, i), bound)?;
                    }
                }
                Ok(())
            }
            (Type::Record(ts), Value::Record(fs)) => {
                if Rc::strong_count(&fs) == 1 {
                    let RecordVal { fields, lent } = std::mem::take(&mut *fs.borrow_mut());
                    for (i, (n, x)) in fields.into_iter().enumerate() {
                        match ts.iter().find(|(k, _)| k.as_str() == &*n) {
                            Some((_, t)) => self.destroy_except_element(x, t, lent_part(&lent, i), bound)?,
                            None if lent_part(&lent, i) == Some(true) => {}
                            None => self.destroy_except(x, bound)?,
                        }
                    }
                }
                Ok(())
            }
            (_, v) => self.destroy_except(v, bound),
        }
    }

    /// `destroy_element` that spares what a pattern bound (D-305, D-366).
    fn destroy_except_element(&mut self, x: Value, ty: &fortis_types::types::Type, lent: Option<bool>, bound: &HashSet<usize>) -> R<()> {
        match lent {
            Some(true) => Ok(()),
            Some(false) if Self::loan_ty(ty) => self.destroy_except(x, bound),
            Some(false) | None => self.destroy_except_typed(x, ty, bound),
        }
    }

    /// Destroy `v`, sparing allocations in `bound` even when they sit inside an aggregate the pattern did not
    /// bind. A tuple is partitioned element by element, including inside a nested tuple: a bound element keeps
    /// its owner and an unbound sibling drops here (7.9, D-301). The aggregate is opened and the parts are
    /// moved out, so a nested value is not skipped for still having an owner (D-305, D-306). This is the one
    /// walk: a second pass over the same tuple would drop a bound element again.
    fn destroy_except(&mut self, v: Value, bound: &HashSet<usize>) -> R<()> {
        if matches!(v, Value::Moved | Value::Void) {
            return Ok(());
        }
        let id = Self::value_id(&v);
        if id != 0 && bound.contains(&id) {
            return Ok(());
        }
        match v {
            Value::Tuple(items) => {
                if let Ok(t) = Rc::try_unwrap(items) {
                    let TupleVal { items, lent } = t;
                    for (i, x) in items.into_iter().enumerate() {
                        if lent_part(&lent, i) != Some(true) {
                            self.destroy_except(x, bound)?;
                        }
                    }
                }
                Ok(())
            }
            Value::Enum(e) => {
                if let Ok(em) = Rc::try_unwrap(e) {
                    for (_, f) in em.fields {
                        self.destroy_except(f, bound)?;
                    }
                }
                Ok(())
            }
            Value::List(items) | Value::Array(items) => {
                if Rc::strong_count(&items) == 1 {
                    let xs = std::mem::take(&mut *items.borrow_mut());
                    for x in xs {
                        self.destroy_except(x, bound)?;
                    }
                }
                Ok(())
            }
            Value::Record(fs) => {
                if Rc::strong_count(&fs) == 1 {
                    let RecordVal { fields, lent } = std::mem::take(&mut *fs.borrow_mut());
                    for (i, (_, x)) in fields.into_iter().enumerate() {
                        if lent_part(&lent, i) != Some(true) {
                            self.destroy_except(x, bound)?;
                        }
                    }
                }
                Ok(())
            }
            Value::Map(m) | Value::Set(m) => {
                if Rc::strong_count(&m) == 1 {
                    let taken = std::mem::take(&mut *m.borrow_mut());
                    for (k, x) in taken.entries {
                        self.destroy_except(k, bound)?;
                        self.destroy_except(x, bound)?;
                    }
                }
                Ok(())
            }
            Value::Obj(o) => {
                if !o.borrow().destroyed {
                    o.borrow_mut().destroyed = true;
                    self.run_cleanup(&o)?;
                }
                if Rc::strong_count(&o) == 1 {
                    let fields = std::mem::take(&mut o.borrow_mut().fields);
                    for (_, f) in fields {
                        self.destroy_except(f, bound)?;
                    }
                }
                Ok(())
            }
            Value::Cell(c) => {
                let inner = std::mem::replace(&mut *c.borrow_mut(), Value::Moved);
                self.destroy_except(inner, bound)
            }
            Value::SharedRef(b) => {
                b.strong.set(b.strong.get() - 1);
                if b.strong.get() == 0 {
                    let inner = std::mem::replace(&mut *b.value.borrow_mut(), Value::Moved);
                    self.destroy_except(inner, bound)?;
                }
                Ok(())
            }
            other => self.destroy(other),
        }
    }

    /// Replace a bound child with `Moved` in the value that still owns it, so a later destroy cannot reach it.
    /// The binding holds the other handle. An aggregate that is not uniquely owned cannot be opened.
    fn excise(&mut self, v: &mut Value, bound: &HashSet<usize>) {
        let id = Self::value_id(v);
        if id != 0 && bound.contains(&id) {
            *v = Value::Moved;
            return;
        }
        if let Value::Cell(c) = v {
            let mut inner = std::mem::replace(&mut *c.borrow_mut(), Value::Moved);
            self.excise(&mut inner, bound);
            *c.borrow_mut() = inner;
            return;
        }
        match v {
            Value::Enum(e) => {
                if let Some(em) = Rc::get_mut(e) {
                    for (_, x) in em.fields.iter_mut() {
                        self.excise(x, bound);
                    }
                }
            }
            Value::Tuple(items) => {
                if let Some(xs) = Rc::get_mut(items) {
                    for x in xs.iter_mut() {
                        self.excise(x, bound);
                    }
                }
            }
            Value::List(xs) | Value::Array(xs) => {
                let mut xs = xs.borrow_mut();
                for x in xs.iter_mut() {
                    self.excise(x, bound);
                }
            }
            Value::Obj(o) => {
                let mut o = o.borrow_mut();
                for (_, x) in o.fields.iter_mut() {
                    self.excise(x, bound);
                }
            }
            Value::Record(fs) => {
                let mut fs = fs.borrow_mut();
                for (_, x) in fs.iter_mut() {
                    self.excise(x, bound);
                }
            }
            Value::Map(m) | Value::Set(m) => {
                let mut m = m.borrow_mut();
                for (k, x) in m.entries.iter_mut() {
                    self.excise(k, bound);
                    self.excise(x, bound);
                }
            }
            _ => {}
        }
    }

    /// A pattern that opens the scrutinee into fields or elements. A whole-value `_` does not.
    fn pattern_opens(p: &Pattern) -> bool {
        match p {
            Pattern::Present(i) | Pattern::Success(i) | Pattern::Failure(i) => Self::pattern_opens(i),
            Pattern::Is { fields, .. } => !fields.is_empty(),
            Pattern::Variant { fields, .. } | Pattern::Class { fields, .. } | Pattern::Record(fields) => !fields.is_empty(),
            Pattern::Tuple(ps) => !ps.is_empty(),
            _ => false,
        }
    }

    /// Bindings of an owning scrutinee own the parts they bound. A pattern that binds nothing still consumes a
    /// moved place: every field matched by `_` dies after the arm, before an enclosing defer (7.9, 9.8). A
    /// wildcard of a temporary stays until the statement ends (D-150). Bound parts are holed and the remainder
    /// is destroyed after the arm. A tuple is not destroyed before the arm body (D-315).
    fn claim_scrutinee(&mut self, fr: &mut Frame, temp_local: Option<LocalId>, owning: bool, v: &mut Value, arm: &Arm) -> R<Option<Value>> {
        if !owning {
            return Ok(None);
        }
        if arm.locals.is_empty() {
            if temp_local.is_some() {
                return Ok(None);
            }
            // A whole-value `_` leaves the local in place. finish_unbound puts it back, or destroys it
            // when another arm binds (D-300). A pattern that opens the value moves it, so a field matched
            // by `_` drops after the arm (D-315).
            if !Self::pattern_opens(&arm.pattern) {
                return Ok(None);
            }
            return Ok(Some(std::mem::replace(v, Value::Moved)));
        }
        let bound: HashSet<usize> = arm.locals.iter().map(|l| Self::value_id(&fr.locals[*l as usize])).filter(|i| *i != 0).collect();
        let vid = Self::value_id(v);
        if vid != 0 && bound.contains(&vid) {
            if let Some(t) = temp_local {
                fr.locals[t as usize] = Value::Moved;
            }
            // the binding owns the whole value and holds its one handle, so the binding's end is the last (D-338)
            *v = Value::Moved;
            return Ok(None);
        }
        // the temporary shares this handle; drop it before opening, or the value is not unique and its nested
        // contents are skipped (D-306)
        if let Some(t) = temp_local {
            fr.locals[t as usize] = Value::Moved;
        }
        self.excise(v, &bound);
        if let Some(t) = temp_local {
            fr.locals[t as usize] = std::mem::replace(v, Value::Moved);
            return Ok(None);
        }
        Ok(Some(std::mem::replace(v, Value::Moved)))
    }

    /// `match move place` whose arm binds nothing leaves the local owning the value. The block drops it once
    /// on the way out, which is when native drops it (7.9). A fresh temporary stays in its slot and drops at
    /// the end of the statement. A defect runs no cleanup (10.8).
    fn finish_unbound_scrutinee(&mut self, fr: &mut Frame, scrutinee: &Expr, v: &mut Value, owning: bool, locals_empty: bool, any_binds: bool, temp_local: Option<LocalId>, halt: bool) -> R<()> {
        if !(owning && locals_empty && temp_local.is_none() && !halt) {
            return Ok(());
        }
        let ExprKind::Place(p, Use::Move) = &scrutinee.kind else { return Ok(()) };
        if !p.proj.is_empty() {
            return Ok(());
        }
        if !any_binds {
            // no arm takes it, so the local still owns it and the block drops it once, with its trace (7.9)
            let moved = std::mem::replace(v, Value::Moved);
            let slot = &mut fr.locals[p.local as usize];
            match slot {
                Value::Cell(c) => *c.borrow_mut() = moved,
                s => *s = moved,
            }
            return Ok(());
        }
        // another arm binds, so this wildcard arm drops the moved scrutinee now. The trace stays with the
        // binding that took the value on the other path (7.9).
        self.destroy_typed(std::mem::replace(v, Value::Moved), &scrutinee.ty)
    }

    /// An owning arm's bindings die at the end of the arm, in reverse binding order (7.9, 9.8). `carried` is
    /// the identity of a value the arm gives back: that binding has moved, and is not destroyed here.
    fn drop_arm_bindings(&mut self, fr: &mut Frame, locals: &[LocalId], carried: usize) -> R<()> {
        if carried != 0 {
            for &l in locals {
                if Self::value_id(&fr.locals[l as usize]) == carried {
                    fr.locals[l as usize] = Value::Moved;
                }
            }
        }
        for &l in locals.iter().rev() {
            // a binding of a borrowed part (an element of the scrutinee that is itself a loan) holds that loan: its
            // end destroys nothing (7.9; D-349)
            let lent = self.holds_loan(fr, l);
            fr.owned.remove(&l);
            let v = std::mem::replace(&mut fr.locals[l as usize], Value::Moved);
            let v = match v {
                Value::Cell(c) => std::mem::replace(&mut *c.borrow_mut(), Value::Moved),
                v => v,
            };
            if lent || matches!(v, Value::Moved | Value::Void) {
                continue;
            }
            if let Some(ty) = self.cleanup_runs(&v) {
                self.trace.push(format!("drop {ty}"));
            }
            let ty = self.local_ty(fr, l);
            self.destroy_typed(v, ty)?;
        }
        Ok(())
    }

    /// Bindings that hold loans (a borrowed scrutinee's arm, a `for` over a borrowed collection) end without
    /// destroying anything: their slots let go of their handles, so the owner's destruction is the last handle's
    /// and runs `cleanup` once (12.2; D-338).
    fn end_loans(fr: &mut Frame, locals: &[LocalId]) {
        for &l in locals {
            fr.locals[l as usize] = Value::Moved;
            fr.owned.remove(&l);
        }
    }

    fn carried_id(r: &Result<(), Exit>) -> usize {
        match r {
            Err(Exit::Flow(Flow::Give(v) | Flow::Return(v) | Flow::Recover(v))) => Self::value_id(v),
            _ => 0,
        }
    }

    /// One statement's temporary scope (5.7, 7.9): the values the statement creates and neither binds nor moves
    /// out are destroyed at its end, in reverse creation order; a defect destroys nothing (10.8). A temporary
    /// that holds a loan is not the value's owner: it lets its handle go and destroys nothing, and the owner's
    /// destruction is the last handle's (7.9; D-451).
    fn stmt(&mut self, fr: &mut Frame, s: &Stmt) -> R<()> {
        let mut r = self.stmt_kind(fr, s);
        if !s.temps.is_empty() && !matches!(r, Err(Exit::Halt(Halt::Defect(_) | Halt::Shutdown))) {
            for &t in s.temps.iter().rev() {
                let v = std::mem::replace(&mut fr.locals[t as usize], Value::Moved);
                let v = match v {
                    Value::Cell(c) => std::mem::replace(&mut *c.borrow_mut(), Value::Moved),
                    v => v,
                };
                let lent = fr.lent.remove(&t);
                if lent || matches!(v, Value::Moved | Value::Void) {
                    continue;
                }
                if let Some(ty) = self.cleanup_runs(&v) {
                    self.trace.push(format!("drop {ty}"));
                }
                let ty = self.local_ty(fr, t);
                if let Err(e) = self.destroy_typed(v, ty) {
                    r = Err(e);
                    break;
                }
            }
        }
        r
    }

    fn stmt_kind(&mut self, fr: &mut Frame, s: &Stmt) -> R<()> {
        match &s.kind {
            StmtKind::Let { local, init } => {
                // a local bound to a loan holds it, and its end destroys nothing (7.3, 7.9; D-338)
                let (v, lent) = self.eval_held(fr, init)?;
                Self::hold(fr, *local, lent);
                let decl = &self.prog.functions[fr.func].locals[*local as usize];
                fr.locals[*local as usize] = if decl.boxed { Value::Cell(shared(v)) } else { v };
                Ok(())
            }
            StmtKind::LetPattern { pattern, init } => {
                let v = self.eval(fr, init)?;
                if !self.pattern(fr, pattern, &v)? {
                    return dynamic("destructuring pattern did not match");
                }
                let owning = !matches!(&init.kind, ExprKind::Place(_, Use::Read | Use::Alias | Use::Editable));
                self.claim_owned_parts(fr, pattern, &v, owning);
                // the bindings own what they took; the rest of the moved source drops at this statement (7.9, D-305),
                // except a part it borrows (D-349)
                let bound = self.pattern_bound_ids(fr, pattern);
                self.destroy_except_typed(v, &init.ty, &bound)?;
                Ok(())
            }
            StmtKind::Assign { place, value, drop_old } => {
                // a whole local: the old value is dropped only when the local owns it, not when it holds a loan; the
                // local then holds what the new value is, a loan or a value of its own (7.9; D-350)
                let whole = place.proj.is_empty().then_some(place.local);
                // a field of a record literal: the record holds what the new value is, a loan or a value of its own,
                // and the old value is destroyed only when the record owned it (7.9; D-366)
                let field = if whole.is_none() { self.record_field(fr, place) } else { None };
                let field_lent = field.as_ref().map_or(false, |(r, i)| lent_part(&r.borrow().lent, *i).unwrap_or_else(|| Self::loan_ty(&value.ty)));
                let (v, lent) = if whole.is_some() || field.is_some() { self.eval_held(fr, value)? } else { (self.eval(fr, value)?, false) };
                let held = whole.map_or(field_lent, |l| fr.lent.contains(&l));
                let old = self.store(fr, place, v)?;
                if let Some(l) = whole {
                    Self::hold(fr, l, lent);
                }
                if let Some((r, i)) = &field {
                    let mut r = r.borrow_mut();
                    if let Some(m) = &r.lent {
                        let mut m = m.to_vec();
                        if let Some(slot) = m.get_mut(*i) {
                            *slot = lent;
                        }
                        r.lent = Some(Rc::from(m));
                    }
                }
                if *drop_old && !held {
                    if let Some(o) = old {
                        if !matches!(o, Value::Moved) {
                            match whole {
                                Some(l) => {
                                    let ty = self.local_ty(fr, l);
                                    self.destroy_typed(o, ty)?;
                                }
                                None => self.destroy(o)?,
                            }
                        }
                    }
                }
                Ok(())
            }
            StmtKind::Compound { place, op, value } => {
                // `x op= e` is `x = x op e` (LR 1.1), evaluated left to right (5.7): `x` is read before `e` runs,
                // and the result is stored into the place `x` names after `e` has run (D-60)
                let old = self.read_place(fr, place)?;
                let v = self.eval(fr, value)?;
                let n = self.binary(*op, old, v)?;
                self.store(fr, place, n)?;
                Ok(())
            }
            StmtKind::Expr(e) => {
                self.eval(fr, e)?;
                Ok(())
            }
            StmtKind::Discard(e) => {
                // `discard move p` consumes the place now (7.9). A read or a statement temporary does not:
                // the temporary dies at the end of the statement.
                if let ExprKind::Place(p, Use::Move) = &e.kind {
                    // a local that holds a loan owns nothing to destroy (7.9; D-338)
                    let held = p.proj.is_empty() && self.holds_loan(fr, p.local);
                    let v = self.eval(fr, e)?;
                    if held {
                        return Ok(());
                    }
                    if let Value::Future(f) = &v {
                        if f.task.get().is_none() && f.work.borrow().is_some() {
                            let fname = f.fname.clone();
                            self.not_started(&fname);
                            self.destroy(v)?;
                            return Ok(());
                        }
                    }
                    if let Some(t) = self.cleanup_runs(&v) {
                        self.trace.push(format!("drop {t}"));
                    }
                    self.destroy_typed(v, &e.ty)?;
                    return Ok(());
                }
                self.eval(fr, e)?;
                Ok(())
            }
            StmtKind::If { cond, then, els } => {
                let c = self.eval(fr, cond)?;
                let t = self.truth(&c)?;
                self.hit(cond as *const Expr as usize, t as u32);
                if t {
                    self.block(fr, then)
                } else if let Some(b) = els {
                    self.block(fr, b)
                } else {
                    Ok(())
                }
            }
            StmtKind::Loop { label, body } => loop {
                self.step()?;
                match self.block(fr, body) {
                    Ok(()) => {}
                    Err(Exit::Flow(Flow::Break(l))) if l.is_none() || l == *label => return Ok(()),
                    Err(Exit::Flow(Flow::Continue(l))) if l.is_none() || l == *label => {}
                    Err(e) => return Err(e),
                }
            },
            StmtKind::For { label, binding, source, body } => self.for_loop(fr, label, *binding, source, body),
            StmtKind::Break(l) => Err(Exit::Flow(Flow::Break(l.clone()))),
            StmtKind::Continue(l) => Err(Exit::Flow(Flow::Continue(l.clone()))),
            StmtKind::Return(e) => {
                let v = match e {
                    Some(e) => {
                        // whether the value returned is a loan or the function's own travels to the caller, which
                        // binds a `from` result as the value says (D-373)
                        let (v, lent) = self.ret_held(fr, e)?;
                        fr.ret_lent = lent;
                        v
                    }
                    None => Value::Void,
                };
                Err(Exit::Flow(Flow::Return(v)))
            }
            StmtKind::Give(e) => {
                // whether the value given is a loan travels with it to the binding that receives it (D-338)
                let (v, lent) = self.eval_held(fr, e)?;
                self.gave_lent = lent;
                Err(Exit::Flow(Flow::Give(v)))
            }
            StmtKind::Recover(e) => {
                let (v, lent) = self.eval_held(fr, e)?;
                self.gave_lent = lent;
                Err(Exit::Flow(Flow::Recover(v)))
            }
            StmtKind::Fail(e) => {
                let v = self.eval(fr, e)?;
                Err(Exit::Flow(Flow::Fail(v)))
            }
            StmtKind::Block(b) => self.block(fr, b),
            StmtKind::Match { scrutinee, arms } => {
                let mut v = self.eval(fr, scrutinee)?;
                let any_binds = arms.iter().any(|a| !a.locals.is_empty());
                // an owning scrutinee (a moved place, or a fresh value the match owns): a matched block arm's
                // bindings own the parts they bound (9.8); the rest of the value still destroys (7.9)
                let temp_local = match &scrutinee.kind {
                    ExprKind::Temp { local, .. } => Some(*local),
                    _ => None,
                };
                let owning = temp_local.is_some() || matches!(&scrutinee.kind, ExprKind::Place(_, Use::Move));
                for a in arms {
                    // an arm that does not run lets go of what its pattern bound: those slots are not owners (D-338)
                    if !self.pattern(fr, &a.pattern, &v)? {
                        Self::end_loans(fr, &a.locals);
                        continue;
                    }
                    if let Some(g) = &a.guard {
                        let gv = self.eval(fr, g)?;
                        if !self.truth(&gv)? {
                            Self::end_loans(fr, &a.locals);
                            continue;
                        }
                    }
                    self.claim_owned_parts(fr, &a.pattern, &v, owning);
                    let rest = self.claim_scrutinee(fr, temp_local, owning, &mut v, a)?;
                    let r = match &a.body {
                        ArmBody::Expr(e) => self.eval(fr, e).map(|_| ()),
                        ArmBody::Block(b) => self.block(fr, b),
                    };
                    let halt = matches!(r, Err(Exit::Halt(Halt::Defect(_) | Halt::Shutdown)));
                    if owning && !halt {
                        let carried = Self::carried_id(&r);
                        self.drop_arm_bindings(fr, &a.locals, carried)?;
                    } else {
                        Self::end_loans(fr, &a.locals);
                    }
                    if let Some(rest) = rest {
                        // a moved scrutinee: what no binding took drops after the arm (7.9); a defect runs no
                        // cleanup (10.8)
                        if !halt {
                            self.destroy_typed(rest, &scrutinee.ty)?;
                        }
                    }
                    self.finish_unbound_scrutinee(fr, scrutinee, &mut v, owning, a.locals.is_empty(), any_binds, temp_local, halt)?;
                    return r;
                }
                Ok(())
            }
            StmtKind::Capture { field, value } => {
                let v = self.eval(fr, value)?;
                let snapshot = deep_copy(&v);
                match self.obs.last_mut() {
                    Some(Some(frame)) => {
                        frame.retain(|(k, _)| k != field);
                        frame.push((field.clone(), snapshot));
                        Ok(())
                    }
                    _ => dynamic("capture outside a function carrying @observe"),
                }
            }
            StmtKind::Record { schema, fields } => {
                let mut shown = Vec::new();
                for (k, e) in fields {
                    let v = self.eval(fr, e)?;
                    shown.push((k.clone(), display(&deep_copy(&v))));
                }
                self.observation(schema, None, &shown, None);
                Ok(())
            }
            StmtKind::Report { value, bind } => {
                let v = self.eval(fr, value)?;
                let code = match &v {
                    Value::Failure(f) => f.borrow().code.clone(),
                    _ => return dynamic("report of a non-failure"),
                };
                self.emit_report(&v, "report");
                if let Some(b) = bind {
                    fr.locals[*b as usize] = Value::Report(code);
                }
                Ok(())
            }
            StmtKind::Defer(i) => {
                if let Some(top) = self.defer_stack.last_mut() {
                    top.push(*i);
                }
                Ok(())
            }
            StmtKind::Scope { local, body } => {
                let name = self.prog.functions[fr.func].locals[*local as usize].name.clone();
                let sv = Rc::new(crate::value::ScopeVal { name, tasks: crate::sync::RefCell::new(vec![]) });
                fr.locals[*local as usize] = Value::Scope(sv.clone());
                self.block_scoped(fr, body, Some(&sv))
            }
            StmtKind::Drop(p) => {
                // an explicit destruction of a value's part (the fields a `...move` spread did not take, 9.2):
                // the place is left moved, and the value drops as a dropped local would
                let old = self.store(fr, p, Value::Moved)?;
                if let Some(v) = old {
                    if !matches!(v, Value::Moved | Value::Void) {
                        if let Some(t) = self.cleanup_runs(&v) {
                            self.trace.push(format!("drop {t}"));
                        }
                        self.destroy(v)?;
                    }
                }
                Ok(())
            }
            StmtKind::Expect { cond, evidence } => {
                // a comparison's operands are evaluated once; the evidence shows those values (14; X-121)
                if let (Some((_, sym, _)), ExprKind::Binary(op, a, b)) = (evidence, &cond.kind) {
                    let av = self.eval(fr, a)?;
                    let bv = self.eval(fr, b)?;
                    let r = self.binary(*op, av.clone(), bv.clone())?;
                    if self.truth(&r)? {
                        return Ok(());
                    }
                    let (a, b) = (self.show(&av)?, self.show(&bv)?);
                    // the failing expect's line rides with the evidence: `expect:LINE:actual=… expected=…` (14)
                    return dynamic(format!("expect:{}:actual={a} expected={sym} {b}", cond.line));
                }
                let c = self.eval(fr, cond)?;
                if self.truth(&c)? {
                    return Ok(());
                }
                dynamic(format!("expect:{}:actual=false expected=true", cond.line))
            }
        }
    }

    /// Move a collection's elements out, so the collection's later destruction cannot reach them (7.9).
    fn take_for_items(&mut self, c: Value) -> R<Vec<Value>> {
        match c.deref() {
            Value::List(xs) | Value::Array(xs) => Ok(std::mem::take(&mut *xs.borrow_mut())),
            Value::Set(m) => {
                let taken = std::mem::take(&mut *m.borrow_mut());
                Ok(taken.entries.into_iter().map(|(k, _)| k).collect())
            }
            Value::Map(m) => {
                let taken = std::mem::take(&mut *m.borrow_mut());
                Ok(taken.entries.into_iter().map(|(k, v)| Value::Tuple(Rc::new(TupleVal::from(vec![k, v])))).collect())
            }
            other => dynamic(format!("for over a non-collection value ({})", other.type_name())),
        }
    }

    /// Destroy a loop element. The binding is not a block local, so this announces nothing: native's loop
    /// variable is not a block cleanup either, and `--core` compares the traces (7.9).
    fn destroy_quietly(&mut self, v: Value) -> R<()> {
        if matches!(v, Value::Moved | Value::Void) {
            return Ok(());
        }
        self.destroy(v)
    }

    fn destroy_loop_binding(&mut self, fr: &mut Frame, b: LocalId) -> R<()> {
        let v = std::mem::replace(&mut fr.locals[b as usize], Value::Moved);
        let v = match v {
            Value::Cell(c) => std::mem::replace(&mut *c.borrow_mut(), Value::Moved),
            v => v,
        };
        self.destroy_quietly(v)
    }

    fn for_loop(&mut self, fr: &mut Frame, label: &Option<String>, binding: Option<LocalId>, source: &ForSource, body: &Block) -> R<()> {
        let mut items = match source {
            ForSource::Range { lo, hi, inclusive } => {
                let lo = self.eval(fr, lo)?.as_int().ok_or(Exit::Halt(Halt::Dynamic("range bound is not an integer".into())))?;
                let hi = self.eval(fr, hi)?.as_int().ok_or(Exit::Halt(Halt::Dynamic("range bound is not an integer".into())))?;
                let top = if *inclusive { hi } else { hi - 1 };
                let mut i = lo;
                while i <= top {
                    if let Some(b) = binding {
                        let boxed = self.prog.functions[fr.func].locals[b as usize].boxed;
                        fr.locals[b as usize] = if boxed { Value::Cell(shared(Value::Int(i))) } else { Value::Int(i) };
                    }
                    self.step()?;
                    match self.block(fr, body) {
                        Ok(()) => {}
                        Err(Exit::Flow(Flow::Break(l))) if l.is_none() || l == *label => return Ok(()),
                        Err(Exit::Flow(Flow::Continue(l))) if l.is_none() || l == *label => {}
                        Err(e) => return Err(e),
                    }
                    i += 1;
                }
                return Ok(());
            }
            ForSource::Items(e) => {
                // a plain read iterates a copy and leaves the collection intact. A move, or any fresh value
                // (including a statement temporary), is owned by the loop (7.9).
                let c = self.eval(fr, e)?;
                // a cursor is what `iterate()` of a class that implements `Iterable` gave the loop (5.3, D-545): it lends
                // its items, so the loop destroys none of them, as over a list it reads
                let lends = matches!(c.deref(), Value::Cursor(_));
                let moving = !lends && (matches!(e.kind, ExprKind::Place(_, Use::Move)) || !matches!(e.kind, ExprKind::Place(..)));
                if !moving {
                    let items = match c.deref() {
                        Value::Cursor(cur) => {
                            let cur = cur.borrow();
                            cur.0[cur.1.min(cur.0.len())..].to_vec()
                        }
                        Value::List(xs) | Value::Array(xs) => xs.borrow().clone(),
                        Value::Set(m) => m.borrow().entries.iter().map(|(k, _)| k.clone()).collect(),
                        Value::Map(m) => m.borrow().entries.iter().map(|(k, v)| Value::Tuple(Rc::new(TupleVal::from(vec![k.clone(), v.clone()])))).collect(),
                        other => return dynamic(format!("for over a non-collection value ({})", other.type_name())),
                    };
                    // the binding holds a loan of each item: it ends with the loop, however the loop ends, and
                    // destroys nothing, so the collection's owner runs each item's cleanup once (12.2; D-338)
                    let mut r = Ok(());
                    for item in items {
                        if let Some(b) = binding {
                            let boxed = self.prog.functions[fr.func].locals[b as usize].boxed;
                            fr.locals[b as usize] = if boxed { Value::Cell(shared(item)) } else { item };
                        }
                        if let Err(e) = self.step() {
                            r = Err(e);
                            break;
                        }
                        match self.block(fr, body) {
                            Ok(()) => {}
                            Err(Exit::Flow(Flow::Break(l))) if l.is_none() || l == *label => break,
                            Err(Exit::Flow(Flow::Continue(l))) if l.is_none() || l == *label => {}
                            Err(e) => {
                                r = Err(e);
                                break;
                            }
                        }
                    }
                    if let Some(b) = binding {
                        Self::end_loans(fr, &[b]);
                    }
                    return r;
                }
                self.take_for_items(c)?
            }
        };
        let mut idx = 0usize;
        let mut outcome = Ok(());
        let mut defect = false;
        while idx < items.len() {
            let item = std::mem::replace(&mut items[idx], Value::Moved);
            idx += 1;
            let unbound = if let Some(b) = binding {
                let boxed = self.prog.functions[fr.func].locals[b as usize].boxed;
                fr.locals[b as usize] = if boxed { Value::Cell(shared(item)) } else { item };
                None
            } else {
                Some(item)
            };
            let r = self.step().and_then(|_| self.block(fr, body));
            if matches!(r, Err(Exit::Halt(Halt::Defect(_) | Halt::Shutdown))) {
                defect = true;
                outcome = r;
                break;
            }
            let dropped = if let Some(b) = binding { self.destroy_loop_binding(fr, b) } else { self.destroy_quietly(unbound.unwrap_or(Value::Void)) };
            if let Err(e) = dropped {
                if matches!(e, Exit::Halt(Halt::Defect(_) | Halt::Shutdown)) {
                    defect = true;
                }
                outcome = Err(e);
                break;
            }
            match r {
                Ok(()) => {}
                Err(Exit::Flow(Flow::Break(l))) if l.is_none() || l == *label => {
                    outcome = Ok(());
                    break;
                }
                Err(Exit::Flow(Flow::Continue(l))) if l.is_none() || l == *label => {}
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            }
        }
        if !defect {
            for item in items.into_iter().skip(idx) {
                self.destroy_quietly(item)?;
            }
        }
        outcome
    }

    /// Emit a record: past the buffer bound the last record becomes one `buffer-overflow` record whose count grows.
    /// The same replacement is the stderr line: a later record of any kind must not leave the held failure JSON.
    pub fn emit(&mut self, rec: Record) {
        if self.reports.len() >= OBSERVATION_BOUND {
            let dropped = {
                let last = self.reports.last_mut().unwrap();
                if last.first().map_or(true, |(_, k)| k != "buffer-overflow") {
                    *last = vec![("kind".into(), "buffer-overflow".into()), ("bound".into(), OBSERVATION_BOUND.to_string()), ("dropped".into(), "1".into())];
                }
                if let Some(slot) = last.iter_mut().find(|(k, _)| k == "dropped") {
                    let n: usize = slot.1.parse().unwrap_or(0);
                    slot.1 = (n + 1).to_string();
                }
                last.iter().find(|(k, _)| k == "dropped").and_then(|(_, v)| v.parse().ok()).unwrap_or(0)
            };
            self.held_failure = Some(overflow_line(dropped));
            return;
        }
        self.reports.push(rec);
    }

    /// The end of a run: the held failure line, or the overflow line that replaced it (PROG-NFR-13).
    pub fn flush_failures(&mut self) {
        if let Some(line) = self.held_failure.take() {
            self.failure_lines.push(line);
        }
    }

    pub fn emit_report(&mut self, f: &Value, where_: &str) {
        if let Value::Failure(fv) = f {
            let fv = fv.borrow();
            let mut chain = Vec::new();
            let mut c = fv.cause.clone();
            while let Some(x) = c {
                chain.push(x.borrow().code.to_string());
                c = x.borrow().cause.clone();
            }
            let payload: Vec<(String, String)> = fv.payload.iter().map(|(k, v)| (k.to_string(), display(v))).collect();
            let origin = if fv.origin.is_empty() { where_.to_string() } else { fv.origin.to_string() };
            // the declaring module, when this run loaded one; the entry module and a standalone file use `file`
            let file = self.decl_files.get(&origin).cloned().unwrap_or_else(|| self.file.clone());
            let annotations: Vec<String> = fv.annotations.iter().map(display).collect();
            let reported = if self.cleanup_depth > 0 { "cleanup".to_string() } else { where_.to_string() };
            // the message template rendered with the payload as the record shows it (10.3; D-340)
            let message = fortis_runtime::failure::render_message(&fv.message, &payload);
            // the same record the interpreter writes (json.dumps sort_keys). The harness keeps the flat
            // `record:` line; this line is what a failing `main` reports on stderr (LR 4.6, D-325).
            let line = fortis_diag::Json::obj([
                ("annotations", fortis_diag::Json::Arr(annotations.iter().map(|a| fortis_diag::Json::str(a.as_str())).collect())),
                ("cause", fortis_diag::Json::Arr(chain.iter().map(|c| fortis_diag::Json::str(c.as_str())).collect())),
                ("code", fortis_diag::Json::str(&*fv.code)),
                ("kind", fortis_diag::Json::str("runtime-failure")),
                ("message", fortis_diag::Json::str(&message)),
                ("origin", fortis_diag::Json::obj([("file", fortis_diag::Json::str(&file)), ("function", fortis_diag::Json::str(&origin))])),
                ("payload", fortis_diag::Json::obj(payload.iter().map(|(k, v)| (k.as_str(), fortis_diag::Json::str(v.as_str()))))),
                ("reported_by", fortis_diag::Json::str(&reported)),
                ("rule", fortis_diag::Json::str("LANG-10-06")),
            ])
            .to_line();
            let before = self.reports.len();
            self.emit(vec![
                ("kind".into(), "runtime-failure".into()),
                ("code".into(), fv.code.to_string()),
                ("message".into(), message.clone()),
                ("payload".into(), payload.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")),
                ("function".into(), origin),
                ("cause".into(), chain.join(",")),
                ("annotations".into(), annotations.join(",")),
                ("reported_by".into(), reported),
            ]);
            // records 1..=999 go out; the 1000th is held; past the bound `emit` already stored the overflow line
            if self.reports.len() > before {
                if self.reports.len() < OBSERVATION_BOUND {
                    self.failure_lines.push(line);
                } else {
                    self.held_failure = Some(line);
                }
            }
        }
    }

    pub fn truth(&self, v: &Value) -> R<bool> {
        match v.deref() {
            Value::Bool(b) => Ok(b),
            other => dynamic(format!("condition is not Bool ({})", other.type_name())),
        }
    }

    // ---- places -----------------------------------------------------------------------------------------------

    pub fn project(&self, v: Value, p: &Proj) -> R<Value> {
        let v = v.deref();
        match p {
            Proj::Field(f) => self.field(&v, f),
            Proj::Tuple(i) | Proj::Index(i) => match &v {
                Value::Tuple(items) => items.get(*i as usize).cloned().ok_or(Exit::Halt(Halt::Dynamic("tuple index".into()))),
                Value::List(items) | Value::Array(items) => {
                    let b = items.borrow();
                    match b.get(*i as usize) {
                        Some(x) => Ok(x.clone()),
                        None => dynamic(format!("index {i} out of range 0..{} (proof-required)", b.len())),
                    }
                }
                other => dynamic(format!("indexing a {}", other.type_name())),
            },
        }
    }

    pub fn field(&self, v: &Value, f: &str) -> R<Value> {
        match v {
            Value::Obj(o) => match o.borrow().get(f) {
                Some(x) => Ok(x.clone()),
                None => dynamic(format!("no field `{f}`")),
            },
            Value::Record(fs) => fs.borrow().iter().find(|(k, _)| &**k == f).map(|(_, v)| v.clone()).ok_or(Exit::Halt(Halt::Dynamic(format!("no field `{f}`")))),
            Value::Enum(e) => e.fields.iter().find(|(k, _)| &**k == f).map(|(_, v)| v.clone()).ok_or(Exit::Halt(Halt::Dynamic(format!("no payload field `{f}`")))),
            Value::Failure(fv) => {
                let fv = fv.borrow();
                if f == "code" {
                    return Ok(Value::Str(fv.code.clone()));
                }
                if f == "message" {
                    return Ok(Value::Str(fv.message.clone()));
                }
                fv.payload.iter().find(|(k, _)| &**k == f).map(|(_, v)| v.clone()).ok_or(Exit::Halt(Halt::Dynamic(format!("no payload field `{f}`"))))
            }
            Value::Report(c) if f == "code" => Ok(Value::Str(c.clone())),
            Value::Tuple(items) => f.parse::<usize>().ok().and_then(|i| items.get(i).cloned()).ok_or(Exit::Halt(Halt::Dynamic("tuple field".into()))),
            Value::SharedRef(b) => {
                let inner = b.value.borrow().clone();
                self.field(&inner, f)
            }
            Value::Cell(c) => {
                let inner = c.borrow().clone();
                self.field(&inner, f)
            }
            other => dynamic(format!("field `{f}` of a {}", other.type_name())),
        }
    }

    pub fn read_place(&self, fr: &Frame, p: &Place) -> R<Value> {
        let mut v = fr.locals[p.local as usize].deref();
        if matches!(v, Value::Moved) {
            return dynamic(format!("`{}` was moved (use after move)", self.prog.functions[fr.func].locals[p.local as usize].name));
        }
        for pr in &p.proj {
            v = self.project(v, pr)?;
        }
        Ok(v)
    }

    /// Store into a place, returning the previous value.
    pub fn store(&mut self, fr: &mut Frame, p: &Place, v: Value) -> R<Option<Value>> {
        if p.proj.is_empty() {
            let slot = &mut fr.locals[p.local as usize];
            return Ok(Some(match slot {
                Value::Cell(c) => std::mem::replace(&mut *c.borrow_mut(), v),
                s => std::mem::replace(s, v),
            }));
        }
        // a tuple is `Rc<Vec<Value>>`, not a cell: moving one element takes the vec out, replaces that index
        // with `Moved`, and puts the vec back. Cloning the element instead would leave the tuple holding it,
        // and destroying the tuple would run its cleanup a second time (7.9).
        if p.proj.iter().all(|pr| matches!(pr, Proj::Tuple(_))) {
            return self.store_tuple_proj(fr, p, v);
        }
        let mut target = fr.locals[p.local as usize].deref();
        for pr in &p.proj[..p.proj.len() - 1] {
            target = self.project(target, pr)?;
        }
        match (p.proj.last().unwrap(), target) {
            (Proj::Field(f), Value::Obj(o)) => Ok(o.borrow_mut().set(f, v)),
            (Proj::Field(f), Value::Record(fs)) => {
                let mut fs = fs.borrow_mut();
                match fs.iter_mut().find(|(k, _)| &**k == f) {
                    Some((_, slot)) => Ok(Some(std::mem::replace(slot, v))),
                    None => {
                        fs.push((Rc::from(f.as_str()), v));
                        Ok(None)
                    }
                }
            }
            (Proj::Field(f), Value::SharedRef(b)) => match &*b.value.borrow() {
                Value::Obj(o) => Ok(o.borrow_mut().set(f, v)),
                _ => dynamic("assignment into a non-object"),
            },
            (Proj::Index(i), Value::List(items)) | (Proj::Index(i), Value::Array(items)) => {
                let mut items = items.borrow_mut();
                match items.get_mut(*i as usize) {
                    Some(slot) => Ok(Some(std::mem::replace(slot, v))),
                    None => dynamic("index out of range"),
                }
            }
            (_, other) => dynamic(format!("assignment into a {}", other.type_name())),
        }
    }

    /// Move one element of a tuple local. The local is taken out first, so `Rc::try_unwrap` sees the only
    /// handle; the element is replaced with `Moved` and the tuple is put back. A shared tuple is restored and
    /// refused: cloning the element would leave both owners to run its cleanup (7.9). A statement temporary
    /// keeps that tuple: the statement drops the other elements after the consumer, in index order (D-312).
    fn store_tuple_proj(&mut self, fr: &mut Frame, p: &Place, v: Value) -> R<Option<Value>> {
        let indices: Vec<u32> = p.proj.iter().map(|pr| match pr {
            Proj::Tuple(i) => *i,
            _ => 0,
        }).collect();
        let slot = std::mem::replace(&mut fr.locals[p.local as usize], Value::Moved);
        let (cell, root) = match slot {
            Value::Cell(c) => {
                let inner = std::mem::replace(&mut *c.borrow_mut(), Value::Moved);
                (Some(c), inner)
            }
            other => (None, other),
        };
        match Self::replace_tuple_path(root, &indices, v) {
            Ok((rebuilt, old)) => {
                // the consumer owns the element. The moved element is `Moved` and is not destroyed again.
                // The other elements, including one nested in a sibling tuple, stay in the slot until the
                // statement ends (7.9, D-301, D-306, D-312).
                Self::put_tuple_slot(&mut fr.locals[p.local as usize], cell, rebuilt);
                Ok(Some(old))
            }
            Err((rebuilt, msg)) => {
                Self::put_tuple_slot(&mut fr.locals[p.local as usize], cell, rebuilt);
                dynamic(msg)
            }
        }
    }

    fn put_tuple_slot(slot: &mut Value, cell: Option<Shared<Value>>, v: Value) {
        match cell {
            Some(c) => {
                *c.borrow_mut() = v;
                *slot = Value::Cell(c);
            }
            None => *slot = v,
        }
    }

    fn replace_tuple_path(v: Value, path: &[u32], new_v: Value) -> Result<(Value, Value), (Value, String)> {
        let (i, rest) = match path.split_first() {
            Some(x) => x,
            None => return Err((v, "tuple move without an index".into())),
        };
        let i = *i as usize;
        let rc = match v {
            Value::Tuple(rc) => rc,
            other => return Err((other, "move of an element of a non-tuple".into())),
        };
        // the tuple is rebuilt with the same record of which elements it borrows (D-366)
        let mut items = match Rc::try_unwrap(rc) {
            Ok(items) => items,
            Err(rc) => return Err((Value::Tuple(rc), "cannot move an element out of a tuple that is still shared".into())),
        };
        if i >= items.len() {
            return Err((Value::Tuple(Rc::new(items)), "tuple index".into()));
        }
        if rest.is_empty() {
            let old = std::mem::replace(&mut items[i], new_v);
            return Ok((Value::Tuple(Rc::new(items)), old));
        }
        let child = std::mem::replace(&mut items[i], Value::Moved);
        match Self::replace_tuple_path(child, rest, new_v) {
            Ok((rebuilt, old)) => {
                items[i] = rebuilt;
                Ok((Value::Tuple(Rc::new(items)), old))
            }
            Err((rebuilt, msg)) => {
                items[i] = rebuilt;
                Err((Value::Tuple(Rc::new(items)), msg))
            }
        }
    }

    fn use_place(&mut self, fr: &mut Frame, p: &Place, u: Use) -> R<Value> {
        let v = self.read_place(fr, p)?;
        match u {
            Use::Copy => Ok(deep_copy(&v)),
            Use::Move => {
                if p.proj.is_empty() {
                    let slot = &mut fr.locals[p.local as usize];
                    match slot {
                        Value::Cell(c) => {
                            *c.borrow_mut() = Value::Moved;
                        }
                        s => *s = Value::Moved,
                    }
                    Ok(v)
                } else {
                    // a field move takes the value out; the place holds `Moved`, so the parent does not
                    // destroy it again (7.9)
                    Ok(self.store(fr, p, Value::Moved)?.unwrap_or(v))
                }
            }
            _ => Ok(v),
        }
    }

    // ---- expressions ------------------------------------------------------------------------------------------

    pub fn eval(&mut self, fr: &mut Frame, e: &Expr) -> R<Value> {
        match &e.kind {
            ExprKind::Const(c) => Ok(konst(c)),
            ExprKind::Place(p, u) => {
                if matches!(u, Use::Alias | Use::Editable) && p.proj.is_empty() {
                    // a loan of a local held in a cell shares the cell itself: writes through it reach the owner
                    if let Value::Cell(c) = &fr.locals[p.local as usize] {
                        return Ok(Value::Cell(c.clone()));
                    }
                }
                self.use_place(fr, p, *u)
            }
            ExprKind::Global(g) => self.global(g),
            ExprKind::FuncRef(name) => match name.strip_prefix("intrinsic:") {
                Some(i) => Ok(Value::Intrinsic(Rc::from(i))),
                None => {
                    let fi = self.func_index(name)?;
                    // in a session a function value may outlive its generation: it carries its identity
                    match self.code_of(fi) {
                        Some(code) => Ok(Value::Closure(Rc::new(ClosureVal { func: fi, captures: vec![], owned: vec![], code: Some(code) }))),
                        None => Ok(Value::Func(fi)),
                    }
                }
            },
            ExprKind::Binary(op, a, b) => {
                let x = self.eval(fr, a)?;
                let y = self.eval(fr, b)?;
                self.binary(*op, x, y)
            }
            ExprKind::Unary(op, a) => {
                let v = self.eval(fr, a)?.deref();
                match (op, v) {
                    (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
                    (UnOp::Neg, Value::Int(n)) => check_int(-n, "neg"),
                    (UnOp::Neg, Value::Float(x)) => Ok(Value::Float(-x)),
                    (UnOp::Neg, Value::F32(x)) => Ok(Value::F32(-x)),
                    (UnOp::Neg, Value::Dec(d)) => Ok(Value::Dec(d.negate())),
                    (UnOp::Neg, Value::Dur(ns)) => ns.checked_neg().map(Value::Dur).ok_or(Exit::Halt(Halt::Dynamic("duration overflow".into()))),
                    (_, v) => dynamic(format!("unary operator on a {}", v.type_name())),
                }
            }
            ExprKind::And(a, b) => {
                let x = self.eval(fr, a)?;
                let t = self.truth(&x)?;
                self.hit(a.as_ref() as *const Expr as usize, t as u32);
                if !t {
                    return Ok(Value::Bool(false));
                }
                let y = self.eval(fr, b)?;
                Ok(Value::Bool(self.truth(&y)?))
            }
            ExprKind::Or(a, b) => {
                let x = self.eval(fr, a)?;
                let t = self.truth(&x)?;
                self.hit(a.as_ref() as *const Expr as usize, t as u32);
                if t {
                    return Ok(Value::Bool(true));
                }
                let y = self.eval(fr, b)?;
                Ok(Value::Bool(self.truth(&y)?))
            }
            ExprKind::Call { callee, args } => self.call_expr(fr, callee, args),
            ExprKind::Construct { ty, fields } => {
                let mut vs = Vec::with_capacity(fields.len());
                for (n, fe) in fields {
                    vs.push((n.as_str(), self.eval(fr, fe)?));
                }
                // fields in declaration order
                let order: Vec<String> = match self.types.get(ty.as_str()).map(|t| &t.kind) {
                    Some(TypeKind::Class { fields }) => fields.iter().map(|f| f.name.clone()).collect(),
                    _ => vs.iter().map(|(n, _)| n.to_string()).collect(),
                };
                let mut out = Vec::with_capacity(order.len());
                for f in &order {
                    if let Some(i) = vs.iter().position(|(n, _)| *n == f) {
                        out.push((Rc::from(f.as_str()), vs.remove(i).1));
                    }
                }
                for (n, v) in vs {
                    out.push((Rc::from(n), v));
                }
                Ok(Value::Obj(shared(Obj::new(Rc::from(ty.as_str()), out))))
            }
            ExprKind::Variant { ty, variant, fields } => {
                let mut vs = Vec::new();
                for (n, fe) in fields {
                    vs.push((Rc::from(n.as_str()), self.eval(fr, fe)?));
                }
                Ok(Value::Enum(Rc::new(EnumVal { ty: Rc::from(ty.as_str()), variant: Rc::from(variant.as_str()), fields: vs })))
            }
            ExprKind::MakeFailure { ty, fields } => {
                let mut vs = Vec::new();
                for (n, fe) in fields {
                    vs.push((Rc::from(n.as_str()), self.eval(fr, fe)?));
                }
                let (code, message) = match self.types.get(ty.as_str()).map(|t| &t.kind) {
                    Some(TypeKind::Error { code, message, .. }) => (code.clone(), message.clone()),
                    _ => (ty.clone(), String::new()),
                };
                let origin = self.stack.last().cloned().unwrap_or_else(|| "main".into());
                Ok(Value::Failure(shared(FailureVal { code: Rc::from(code), payload: vs, origin: Rc::from(origin), message: Rc::from(message), cause: None, annotations: vec![] })))
            }
            // a tuple or record literal records which of its parts are loans, decided as a `let` decides what its
            // local holds: by the value each part's expression gave, so a part whose type is a loan but whose value
            // is the aggregate's own (a `??` whose fallback is owned, an `if` that moved a value in, a moved binding
            // that owns its value) is destroyed with it (7.9; D-349, D-366)
            ExprKind::Tuple(es) => {
                let mut vs = Vec::with_capacity(es.len());
                let mut lent = Vec::with_capacity(es.len());
                for x in es {
                    let (v, l) = self.eval_held(fr, x)?;
                    vs.push(v);
                    lent.push(l);
                }
                Ok(Value::Tuple(Rc::new(TupleVal::new(vs, Some(Rc::from(lent))))))
            }
            ExprKind::Record(fs) => {
                let mut vs = Vec::with_capacity(fs.len());
                let mut lent = Vec::with_capacity(fs.len());
                for (n, x) in fs {
                    let (v, l) = self.eval_held(fr, x)?;
                    vs.push((Rc::from(n.as_str()), v));
                    lent.push(l);
                }
                Ok(Value::Record(shared(RecordVal::new(vs, Some(Rc::from(lent))))))
            }
            ExprKind::Closure { func, captures } => {
                let fi = self.func_index(func)?;
                let mut cs = Vec::new();
                let mut lent = Vec::with_capacity(captures.len());
                for c in captures {
                    // a loan moved into the closure stays a loan: the closure holds it and destroys nothing (D-338)
                    lent.push(matches!(&c.kind, ExprKind::Place(p, Use::Move) if p.proj.is_empty() && self.holds_loan(fr, p.local)));
                    cs.push(self.eval(fr, c)?);
                }
                // a by-reference capture shares its binding's cell; a by-value one gets a cell of its own, owned by
                // the closure (7.9, X-250; D-30)
                let modes: Vec<CaptureMode> = self.prog.functions[fi].closure.as_ref().map(|c| c.captures.iter().map(|x| x.mode).collect()).unwrap_or_default();
                let mut owned = Vec::with_capacity(cs.len());
                let cs = cs
                    .into_iter()
                    .zip(lent)
                    .zip(modes.iter().chain(std::iter::repeat(&CaptureMode::Implicit)))
                    .map(|((v, lent), m)| {
                        owned.push(matches!(m, CaptureMode::Value | CaptureMode::Move) && !lent || matches!(m, CaptureMode::Copy));
                        match m {
                            CaptureMode::Implicit | CaptureMode::Alias | CaptureMode::Editable => match v {
                                Value::Cell(_) => v,
                                other => Value::Cell(shared(other)),
                            },
                            CaptureMode::Copy | CaptureMode::Value => Value::Cell(shared(deep_copy(&v))),
                            CaptureMode::Move => Value::Cell(shared(v.deref())),
                        }
                    })
                    .collect();
                let code = self.code_of(fi);
                Ok(Value::Closure(Rc::new(ClosureVal { func: fi, captures: cs, owned, code })))
            }
            ExprKind::Interpolate(parts) => {
                let mut s = String::new();
                for p in parts {
                    match p {
                        Part::Lit(t) => s.push_str(t),
                        Part::Show(x) => {
                            let v = self.eval(fr, x)?;
                            let shown = self.show(&v)?;
                            s.push_str(&shown);
                        }
                    }
                }
                Ok(Value::Str(Rc::from(s)))
            }
            ExprKind::Field(a, f) => {
                let v = self.eval(fr, a)?;
                self.field(&v.deref(), f)
            }
            ExprKind::TupleField(a, i) => {
                let v = self.eval(fr, a)?;
                self.project(v, &Proj::Tuple(*i))
            }
            ExprKind::Index(a, i) => {
                let v = self.eval(fr, a)?.deref();
                let i = self.eval(fr, i)?.as_int().ok_or(Exit::Halt(Halt::Dynamic("index is not an integer".into())))?;
                match &v {
                    Value::List(items) | Value::Array(items) => {
                        let b = items.borrow();
                        if i < 0 || i as usize >= b.len() {
                            return dynamic(format!("index {i} out of range 0..{} (proof-required)", b.len()));
                        }
                        Ok(b[i as usize].clone())
                    }
                    Value::Tuple(items) => items.get(i as usize).cloned().ok_or(Exit::Halt(Halt::Dynamic("tuple index".into()))),
                    other => dynamic(format!("indexing a {}", other.type_name())),
                }
            }
            ExprKind::OptField(a, f) => {
                let v = self.eval(fr, a)?.deref();
                if matches!(v, Value::Null) {
                    return Ok(Value::Null);
                }
                self.field(&v, f)
            }
            ExprKind::Present(a) | ExprKind::Success(a) => self.eval(fr, a),
            ExprKind::Coalesce(a, b) => {
                let v = self.eval(fr, a)?;
                if matches!(v.deref(), Value::Null) {
                    self.eval(fr, b)
                } else {
                    Ok(v)
                }
            }
            ExprKind::OrFail(a) => {
                let v = self.eval(fr, a)?;
                if v.is_failure() {
                    return Err(Exit::Flow(Flow::Fail(v)));
                }
                Ok(v)
            }
            ExprKind::OrRecover(a, b) => {
                let v = self.eval(fr, a)?;
                self.hit(a.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if v.is_failure() {
                    return self.eval(fr, b);
                }
                Ok(v)
            }
            ExprKind::Catch { value, bind, handler } => {
                let v = self.eval(fr, value)?;
                self.hit(value.as_ref() as *const Expr as usize, v.is_failure() as u32);
                if !v.is_failure() {
                    return Ok(v);
                }
                fr.locals[*bind as usize] = v;
                match self.block(fr, handler) {
                    Err(Exit::Flow(Flow::Recover(r))) => Ok(r),
                    Ok(()) => dynamic("catch block fell through without recover/return/fail"),
                    Err(e) => Err(e),
                }
            }
            ExprKind::If { cond, then, els } => {
                let c = self.eval(fr, cond)?;
                let t = self.truth(&c)?;
                self.hit(cond.as_ref() as *const Expr as usize, t as u32);
                let b = if t { then } else { els };
                self.block_value(fr, b)
            }
            ExprKind::Match { scrutinee, arms } => {
                let mut v = self.eval(fr, scrutinee)?;
                let any_binds = arms.iter().any(|a| !a.locals.is_empty());
                let temp_local = match &scrutinee.kind {
                    ExprKind::Temp { local, .. } => Some(*local),
                    _ => None,
                };
                // `match move o` of a local that holds a loan (`let o = xs.get(0)`, the `Outcome` of a `try` call whose
                // result is a loan) moves the loan: the arm's bindings hold loans of the owner's parts and destroy
                // nothing when the arm ends (7.9; D-394)
                let moved_loan = matches!(&scrutinee.kind, ExprKind::Place(p, Use::Move) if p.proj.is_empty() && self.holds_loan(fr, p.local));
                let owning = temp_local.is_some() || (matches!(&scrutinee.kind, ExprKind::Place(_, Use::Move)) && !moved_loan);
                for (ai, a) in arms.iter().enumerate() {
                    if !self.pattern(fr, &a.pattern, &v)? {
                        Self::end_loans(fr, &a.locals);
                        continue;
                    }
                    if let Some(g) = &a.guard {
                        let gv = self.eval(fr, g)?;
                        if !self.truth(&gv)? {
                            Self::end_loans(fr, &a.locals);
                            continue;
                        }
                    }
                    self.hit(scrutinee.as_ref() as *const Expr as usize, ai as u32);
                    // a binding of a part the owned scrutinee held as its own owns it, as in the statement form
                    // (7.9; D-366, D-372)
                    self.claim_owned_parts(fr, &a.pattern, &v, owning);
                    let rest = self.claim_scrutinee(fr, temp_local, owning, &mut v, a)?;
                    let r = match &a.body {
                        // an expression arm gives its value as a `give` would, a loan or not (D-338)
                        ArmBody::Expr(x) => self.eval_held(fr, x).map(|(v, lent)| {
                            self.gave_lent = lent;
                            v
                        }),
                        ArmBody::Block(b) => self.block_value(fr, b),
                    };
                    let halt = matches!(r, Err(Exit::Halt(Halt::Defect(_) | Halt::Shutdown)));
                    if owning && !halt {
                        let carried = match &r {
                            Ok(rv) => Self::value_id(rv),
                            Err(Exit::Flow(Flow::Give(rv) | Flow::Return(rv) | Flow::Recover(rv))) => Self::value_id(rv),
                            _ => 0,
                        };
                        self.drop_arm_bindings(fr, &a.locals, carried)?;
                    } else {
                        Self::end_loans(fr, &a.locals);
                    }
                    if let Some(rest) = rest {
                        if !halt {
                            self.destroy_typed(rest, &scrutinee.ty)?;
                        }
                    }
                    self.finish_unbound_scrutinee(fr, scrutinee, &mut v, owning, a.locals.is_empty(), any_binds, temp_local, halt)?;
                    // the arm's value is the same object the temporary still holds: the result owns it (7.9)
                    if let (Some(t), Ok(rv)) = (temp_local, &r) {
                        if Self::value_id(&fr.locals[t as usize]) == Self::value_id(rv) && Self::value_id(rv) != 0 {
                            fr.locals[t as usize] = Value::Moved;
                        }
                    }
                    return r;
                }
                dynamic("no match arm matched")
            }
            ExprKind::Block(b) => self.block_value(fr, b),
            ExprKind::Fail(a) => {
                let v = self.eval(fr, a)?;
                if !v.is_failure() {
                    return dynamic("fail of a non-failure value");
                }
                Err(Exit::Flow(Flow::Fail(v)))
            }
            ExprKind::Validate { value, lo, hi, length } => {
                let v = self.eval(fr, value)?;
                let n = if *length {
                    // `scalars` counts a text's Unicode scalars, `length` a collection's elements or a byte string's
                    // bytes (6.3), as native code measures them; a list was once returned unchecked (D-200)
                    match v.deref() {
                        Value::Str(s) => s.chars().count() as i128,
                        Value::List(l) | Value::Array(l) => l.borrow().len() as i128,
                        Value::Bytes(b) => b.len() as i128,
                        _ => return dynamic("validate of a length refinement on a value without a length"),
                    }
                } else {
                    match v.as_int() {
                        Some(n) => n,
                        None => return dynamic("validate of a non-integer"),
                    }
                };
                if lo.map_or(false, |l| n < l) || hi.map_or(false, |h| n > h) {
                    // A newtype validator takes its representation owned (or the legacy read form's copy).
                    // Failure must discharge that ownership before its caller handles the outcome (X-552).
                    if let fortis_types::types::Type::Named(outcome) = &e.ty {
                        if let Some(fortis_types::types::Type::Named(target)) = outcome.args.first() {
                            if self.prog.type_def(&target.name).map_or(false, |d| matches!(d.kind, TypeKind::Newtype { .. })) {
                                self.destroy_typed(v, &value.ty)?;
                            }
                        }
                    }
                    let origin = self.stack.last().cloned().unwrap_or_else(|| "main".into());
                    let b = |x: &Option<i128>| x.map_or(Value::Null, Value::Int);
                    return Ok(Value::Failure(shared(FailureVal {
                        code: Rc::from("refinement.out_of_range"),
                        payload: vec![(Rc::from("value"), Value::Int(n)), (Rc::from("low"), b(lo)), (Rc::from("high"), b(hi))],
                        origin: Rc::from(origin),
                        message: Rc::from(""),
                        cause: None,
                        annotations: vec![],
                    })));
                }
                Ok(v)
            }
            ExprKind::Temp { local, value } => {
                // a statement temporary: the value lives in its local until the end of the statement, and the
                // expression carries on with it (5.7, 7.9). A temporary that holds a loan result is not the
                // value's owner: the statement's end destroys nothing, and only a made replacement is destroyed
                // there (7.9; D-451)
                let (v, lent) = self.eval_temp(fr, value)?;
                Self::hold(fr, *local, lent);
                let boxed = self.prog.functions[fr.func].locals[*local as usize].boxed;
                fr.locals[*local as usize] = if boxed { Value::Cell(shared(v.clone())) } else { v.clone() };
                Ok(v)
            }
            ExprKind::Wrap { value, .. } | ExprKind::ToDynamic { value, .. } => self.eval(fr, value),
            ExprKind::Codec { op, target, arg } => {
                let v = self.eval(fr, arg)?.deref();
                match op {
                    CodecOp::ToText => Ok(Value::str(&crate::codec::to_text(self.prog, &v)?)),
                    CodecOp::ToBytes => Ok(Value::Bytes(Rc::new(crate::codec::to_bytes(self.prog, &v)?))),
                    CodecOp::FromText => self.decode(true, target, &v),
                    CodecOp::FromBytes => self.decode(false, target, &v),
                }
            }
            ExprKind::Await(x) => {
                let v = self.eval(fr, x)?;
                self.await_value(v)
            }
        }
    }

    /// A block used as a value: its `give`.
    fn block_value(&mut self, fr: &mut Frame, b: &Block) -> R<Value> {
        match self.block(fr, b) {
            Err(Exit::Flow(Flow::Give(v))) => Ok(v),
            Ok(()) => Ok(Value::Void),
            Err(e) => Err(e),
        }
    }

    fn global(&mut self, g: &str) -> R<Value> {
        // a session's tasks of several generations share the constant cache: each generation has its own entries
        let key: std::borrow::Cow<str> = match &self.gen {
            Some(_) if crate::session::fault("consts-shared") => g.into(),
            Some(gen) => format!("{}#{g}", gen.id).into(),
            None => g.into(),
        };
        if let Some(v) = self.consts.get(key.as_ref()) {
            return Ok(v.clone());
        }
        let (module, name) = g.split_once("::").unwrap_or(("", g));
        let c = self.prog.consts.iter().find(|c| c.module == module && c.name == name);
        let c = match c {
            Some(c) => c,
            None => return dynamic(format!("no constant `{g}`")),
        };
        let mut fr = Frame { locals: vec![], func: 0, consumed: None, lent: HashSet::new(), owned: HashSet::new(), ret_lent: true };
        let v = self.eval(&mut fr, &c.value)?;
        self.consts.insert(key.into_owned(), v.clone());
        Ok(v)
    }

    fn call_expr(&mut self, fr: &mut Frame, callee: &Callee, args: &[Arg]) -> R<Value> {
        let mut vals: Vec<(String, Value)> = Vec::with_capacity(args.len());
        // a closure value is evaluated before its arguments, as the interpreter's postfix chain does
        let fval = match callee {
            Callee::Value(f) => Some(self.eval(fr, f)?),
            _ => None,
        };
        for a in args {
            let v = self.eval(fr, &a.value)?;
            vals.push((a.param.clone(), v));
        }
        match callee {
            Callee::Func(name) => {
                let fi = self.func_index(name)?;
                self.call(fi, vals, &[])
            }
            Callee::Intrinsic(name) => {
                let mut vals = vals;
                // a member of `T` called on a `Shared[T]` receiver works on the value behind the handle (7.8)
                if !name.starts_with("Shared.") && !name.starts_with("Weak.") {
                    if let Some((_, Value::SharedRef(b))) = vals.first() {
                        let inner = b.value.borrow().clone();
                        vals[0].1 = inner;
                    }
                }
                let pos = crate::intrinsics::gather(vals);
                let r = crate::intrinsics::call(self, name, pos);
                self.ret_lent = true; // an intrinsic that lends (`get`, a cursor's `next`) lends (D-373)
                r
            }
            Callee::Virtual { method } => {
                let mut vals = vals;
                // a method of the value behind a `Shared` handle (7.8) is dispatched on that value
                if let Some((_, Value::SharedRef(b))) = vals.first() {
                    let inner = b.value.borrow().clone();
                    vals[0].1 = inner;
                }
                let recv = vals.first().map(|(_, v)| v.clone()).unwrap_or(Value::Void);
                let target = self.dispatch(&recv, method)?;
                match target {
                    Value::Func(fi) => self.call(fi, vals, &[]),
                    Value::Intrinsic(n) => {
                        let pos = crate::intrinsics::gather(vals);
                        let r = crate::intrinsics::call(self, &n, pos);
                        self.ret_lent = true; // an intrinsic that lends lends (D-373)
                        r
                    }
                    _ => dynamic("dispatch"),
                }
            }
            Callee::Value(fe) => {
                let f = fval.unwrap();
                let args = vals.into_iter().map(|(_, v)| v).collect();
                // `move f(…)` consumes the closure (8.2): the call owns it (7.9, X-250; D-30)
                if matches!(fe.kind, ExprKind::Place(_, Use::Move)) {
                    return self.call_consuming(f, args);
                }
                self.call_value(&f, args)
            }
        }
    }

    // ---- operators --------------------------------------------------------------------------------------------

    pub fn equal(&mut self, a: &Value, b: &Value) -> R<bool> {
        let (a, b) = (a.deref(), b.deref());
        Ok(match (&a, &b) {
            (Value::Enum(x), Value::Enum(y)) => {
                if let Some(t) = self.types.get(&*x.ty) {
                    if let Some((_, f)) = t.methods.iter().find(|(m, _)| m == "equals") {
                        let fi = self.func_index(f)?;
                        let params: Vec<String> = self.prog.functions[fi].params.iter().map(|p| p.name.clone()).collect();
                        let r = self.call(fi, vec![(params[0].clone(), a.clone()), (params.get(1).cloned().unwrap_or_default(), b.clone())], &[])?;
                        return self.truth(&r);                  // the enum's own equals (X-516; D-84)
                    }
                }
                if x.ty != y.ty || x.variant != y.variant {
                    return Ok(false);
                }
                for (k, v) in &x.fields {
                    let other = y.fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                    if !self.equal(v, &other)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Value::Obj(x), Value::Obj(y)) => {
                let ty = x.borrow().ty.clone();
                if let Some(t) = self.types.get(&*ty) {
                    if let Some((_, f)) = t.methods.iter().find(|(m, _)| m == "equals") {
                        let fi = self.func_index(f)?;
                        let params: Vec<String> = self.prog.functions[fi].params.iter().map(|p| p.name.clone()).collect();
                        let r = self.call(fi, vec![(params[0].clone(), a.clone()), (params.get(1).cloned().unwrap_or_default(), b.clone())], &[])?;
                        return self.truth(&r);
                    }
                }
                if ty != y.borrow().ty {
                    return Ok(false);
                }
                let xs: Vec<(Rc<str>, Value)> = x.borrow().fields.clone();
                for (k, v) in xs {
                    let other = y.borrow().get(&k).cloned().unwrap_or(Value::Null);
                    if !self.equal(&v, &other)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Value::List(x), Value::List(y)) | (Value::Array(x), Value::Array(y)) => {
                let (xs, ys) = (x.borrow().clone(), y.borrow().clone());
                if xs.len() != ys.len() {
                    return Ok(false);
                }
                for (p, q) in xs.iter().zip(ys.iter()) {
                    if !self.equal(p, q)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Value::Tuple(x), Value::Tuple(y)) => {
                if x.len() != y.len() {
                    return Ok(false);
                }
                for (p, q) in x.iter().zip(y.iter()) {
                    if !self.equal(p, q)? {
                        return Ok(false);
                    }
                }
                true
            }
            (Value::Null, Value::Null) | (Value::Void, Value::Void) => true,
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Int(x), Value::Int(y)) => x == y,
            (Value::Float(x), Value::Float(y)) => x == y,
            (Value::F32(x), Value::F32(y)) => x == y,
            (Value::Dec(x), Value::Dec(y)) => x.num_eq(y),
            (Value::Dur(x), Value::Dur(y)) | (Value::Time(x), Value::Time(y)) => x == y,
            (Value::Str(x), Value::Str(y)) | (Value::Scalar(x), Value::Scalar(y)) | (Value::Path(x), Value::Path(y)) => x == y,
            (Value::Str(x), Value::Scalar(y)) | (Value::Scalar(x), Value::Str(y)) => x == y,
            (Value::Bytes(x), Value::Bytes(y)) => x == y,
            (Value::Failure(x), Value::Failure(y)) => Rc::ptr_eq(x, y),
            _ => false,
        })
    }

    /// The order of two values (X-516; D-54.1, D-84): a type's own `compare` first, else declaration order (a
    /// class's fields, an enum's variants and then its payload's fields), else the primitives' order. The checker
    /// proved both sides are Comparable.
    pub fn compare(&mut self, a: &Value, b: &Value) -> R<std::cmp::Ordering> {
        use std::cmp::Ordering as O;
        let (a, b) = (a.deref(), b.deref());
        match (&a, &b) {
            (Value::Obj(x), Value::Obj(y)) => {
                let ty = x.borrow().ty.clone();
                if let Some(t) = self.types.get(&*ty) {
                    if let Some((_, f)) = t.methods.iter().find(|(m, _)| m == "compare") {
                        let fi = self.func_index(f)?;
                        let params: Vec<String> = self.prog.functions[fi].params.iter().map(|p| p.name.clone()).collect();
                        let r = self.call(fi, vec![(params[0].clone(), a.clone()), (params.get(1).cloned().unwrap_or_default(), b.clone())], &[])?;
                        return self.ordering_of(&r);
                    }
                }
                if ty != y.borrow().ty {
                    return dynamic("`compare` on values of different classes");
                }
                let order: Vec<String> = match self.types.get(&*ty).map(|t| &t.kind) {
                    Some(TypeKind::Class { fields }) => fields.iter().map(|f| f.name.clone()).collect(),
                    _ => x.borrow().fields.iter().map(|(k, _)| k.to_string()).collect(),
                };
                for k in order {
                    let va = x.borrow().get(&k).cloned().unwrap_or(Value::Null);
                    let vb = y.borrow().get(&k).cloned().unwrap_or(Value::Null);
                    if self.equal(&va, &vb)? {
                        continue;
                    }
                    return self.compare(&va, &vb);           // the first difference decides (R5)
                }
                Ok(O::Equal)
            }
            (Value::Enum(x), Value::Enum(y)) => {
                if let Some(t) = self.types.get(&*x.ty) {
                    if let Some((_, f)) = t.methods.iter().find(|(m, _)| m == "compare") {
                        let fi = self.func_index(f)?;
                        let params: Vec<String> = self.prog.functions[fi].params.iter().map(|p| p.name.clone()).collect();
                        let r = self.call(fi, vec![(params[0].clone(), a.clone()), (params.get(1).cloned().unwrap_or_default(), b.clone())], &[])?;
                        return self.ordering_of(&r);
                    }
                }
                if x.ty != y.ty {
                    return dynamic("`compare` on values of different enums");
                }
                // the variant order and this variant's payload fields, cloned out so the loop borrows only the
                // values, not the type table (R5)
                let (variant_order, payload): (Vec<String>, Vec<String>) = match self.types.get(&*x.ty).map(|t| &t.kind) {
                    Some(TypeKind::Enum { variants }) => {
                        let order: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                        let pay: Vec<String> = variants.iter().find(|v| v.name == &*x.variant).map(|v| v.fields.iter().map(|(n, _)| n.clone()).collect()).unwrap_or_default();
                        (order, pay)
                    }
                    _ => (vec![], vec![]),
                };
                let (ia, ib) = (variant_order.iter().position(|n| n == &*x.variant), variant_order.iter().position(|n| n == &*y.variant));
                match (ia, ib) {
                    (Some(i), Some(j)) if i != j => return Ok(i.cmp(&j)),   // the variant order decides first (R5)
                    (Some(_), Some(_)) => {}
                    _ => return dynamic("`compare` on non-comparable enum values"),
                }
                for n in &payload {
                    let va = x.fields.iter().find(|(k, _)| &**k == n.as_str()).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                    let vb = y.fields.iter().find(|(k, _)| &**k == n.as_str()).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
                    if self.equal(&va, &vb)? {
                        continue;
                    }
                    return self.compare(&va, &vb);           // then the payload's fields in order (R5)
                }
                Ok(O::Equal)
            }
            (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
            (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
            (Value::Float(x), Value::Float(y)) => match x.partial_cmp(y) {
                Some(o) => Ok(o),
                None => dynamic("`compare` on values that NaN makes non-comparable"),
            },
            (Value::F32(x), Value::F32(y)) => match x.partial_cmp(y) {
                Some(o) => Ok(o),
                None => dynamic("`compare` on values that NaN makes non-comparable"),
            },
            (Value::Str(x), Value::Str(y)) | (Value::Scalar(x), Value::Scalar(y)) | (Value::Path(x), Value::Path(y)) => Ok(x.as_ref().cmp(y.as_ref())),
            (Value::Dec(x), Value::Dec(y)) => Ok(x.cmp(y)),
            (Value::Dur(x), Value::Dur(y)) | (Value::Time(x), Value::Time(y)) => Ok(x.cmp(y)),
            _ => dynamic(format!("`compare` on non-comparable values ({}, {})", a.type_name(), b.type_name())),
        }
    }

    /// An `Ordering` value as its order (the result of a hand-written `compare`).
    fn ordering_of(&self, v: &Value) -> R<std::cmp::Ordering> {
        match v {
            Value::Enum(e) => match &*e.variant {
                "Less" => Ok(std::cmp::Ordering::Less),
                "Equal" => Ok(std::cmp::Ordering::Equal),
                "Greater" => Ok(std::cmp::Ordering::Greater),
                _ => dynamic("compare returned a variant that is not Ordering's"),
            },
            _ => dynamic("compare returned a non-Ordering value"),
        }
    }

    /// Whether `v`'s type writes its own `equals` or `hash`, or nests a value that does: a container then finds the
    /// key by that equality, not by a structural key, because a hand-written `hash` writes nothing (`Hasher` has no
    /// members in Language 1.0) and equal values must still be found by `equals` (6.10, X-516; D-84).
    pub fn custom_equality(&self, v: &Value) -> bool {
        let writes = |ty: &str| self.types.get(ty).map_or(false, |t| t.methods.iter().any(|(m, _)| m == "equals" || m == "hash"));
        match v {
            Value::Obj(o) => {
                let o = o.borrow();
                writes(&o.ty) || o.fields.iter().any(|(_, x)| self.custom_equality(x))
            }
            Value::Enum(e) => writes(&e.ty) || e.fields.iter().any(|(_, x)| self.custom_equality(x)),
            Value::List(items) | Value::Array(items) => items.borrow().iter().any(|x| self.custom_equality(x)),
            Value::Tuple(items) => items.iter().any(|x| self.custom_equality(x)),
            Value::Cell(c) => self.custom_equality(&c.borrow()),
            _ => false,
        }
    }

    pub fn binary(&mut self, op: BinOp, a: Value, b: Value) -> R<Value> {
        let (a, b) = (a.deref(), b.deref());
        if op == BinOp::Eq || op == BinOp::Ne {
            let eq = self.equal(&a, &b)?;
            return Ok(Value::Bool(if op == BinOp::Eq { eq } else { !eq }));
        }
        if op.is_comparison() {
            let ord = match (&a, &b) {
                (Value::Int(x), Value::Int(y)) => x.partial_cmp(y),
                (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
                (Value::F32(x), Value::F32(y)) => x.partial_cmp(y),
                (Value::Str(x), Value::Str(y)) => x.partial_cmp(y),
                (Value::Dec(x), Value::Dec(y)) => Some(x.cmp(y)),
                (Value::Dur(x), Value::Dur(y)) | (Value::Time(x), Value::Time(y)) => x.partial_cmp(y),
                // a derived Comparable's, or a type's own `compare` (X-516; D-54.1, D-84)
                (Value::Obj(_), Value::Obj(_)) | (Value::Enum(_), Value::Enum(_)) => Some(self.compare(&a, &b)?),
                _ => return dynamic(format!("`{}` on non-comparable values", op.symbol())),
            };
            let r = match ord {
                None => false,
                Some(o) => match op {
                    BinOp::Lt => o.is_lt(),
                    BinOp::Le => o.is_le(),
                    BinOp::Gt => o.is_gt(),
                    _ => o.is_ge(),
                },
            };
            return Ok(Value::Bool(r));
        }
        match (&a, &b) {
            (Value::Float(x), Value::Float(y)) if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div) => {
                Ok(Value::Float(fortis_runtime::float::f64_op(op.symbol().chars().next().unwrap(), *x, *y)))
            }
            (Value::F32(x), Value::F32(y)) if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div) => {
                Ok(Value::F32(fortis_runtime::float::f32_op(op.symbol().chars().next().unwrap(), *x, *y)))
            }
            (Value::Int(x), Value::Int(y)) => {
                let (x, y) = (*x, *y);
                match op {
                    BinOp::Add => check_int(x + y, "add"),
                    BinOp::Sub => check_int(x - y, "sub"),
                    BinOp::Mul => check_int(x.checked_mul(y).unwrap_or(i128::MAX), "mul"),
                    BinOp::Div => {
                        if y == 0 {
                            return dynamic("division by zero (proof-required; use Int.div)");
                        }
                        let q = x.abs() / y.abs();
                        check_int(if (x >= 0) == (y >= 0) { q } else { -q }, "div")
                    }
                    BinOp::Rem => {
                        if y == 0 {
                            return dynamic("remainder by zero");
                        }
                        let r = x.abs() % y.abs();
                        Ok(Value::Int(if x >= 0 { r } else { -r }))
                    }
                    BinOp::Shl => {
                        if !(0..128).contains(&y) {
                            return dynamic("shift out of range");
                        }
                        check_int(x.checked_shl(y as u32).unwrap_or(i128::MAX), "shiftLeft")
                    }
                    BinOp::Shr => Ok(Value::Int(x >> y.clamp(0, 127))),
                    BinOp::BitAnd => Ok(Value::Int(x & y)),
                    BinOp::BitOr => Ok(Value::Int(x | y)),
                    BinOp::BitXor => Ok(Value::Int(x ^ y)),
                    _ => dynamic("operator"),
                }
            }
            _ => dynamic(format!("`{}` on non-integer values ({} and {})", op.symbol(), a.type_name(), b.type_name())),
        }
    }

    // ---- patterns ---------------------------------------------------------------------------------------------

    fn bind(&self, fr: &mut Frame, l: LocalId, v: Value) {
        let boxed = self.prog.functions[fr.func].locals[l as usize].boxed;
        fr.locals[l as usize] = if boxed { Value::Cell(shared(v)) } else { v };
    }

    pub fn pattern(&mut self, fr: &mut Frame, p: &Pattern, v: &Value) -> R<bool> {
        let v = v.deref();
        Ok(match p {
            Pattern::Wildcard => true,
            Pattern::Bind(l) => {
                self.bind(fr, *l, v);
                true
            }
            Pattern::Literal(c) => {
                let k = konst(c);
                self.equal(&k, &v)?
            }
            Pattern::Null => matches!(v, Value::Null),
            Pattern::Present(inner) => !matches!(v, Value::Null) && self.pattern(fr, inner, &v)?,
            Pattern::Success(inner) => !v.is_failure() && self.pattern(fr, inner, &v)?,
            Pattern::Failure(inner) => v.is_failure() && self.pattern(fr, inner, &v)?,
            Pattern::Variant { variant, fields, .. } => match &v {
                Value::Enum(e) if &*e.variant == variant => {
                    for (n, sp) in fields {
                        let fv = e.fields.iter().find(|(k, _)| &**k == n).map(|(_, x)| x.clone()).unwrap_or(Value::Null);
                        if !self.pattern(fr, sp, &fv)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            Pattern::Class { ty, fields } => match &v {
                Value::Obj(o) if &*o.borrow().ty == ty => {
                    for (n, sp) in fields {
                        let fv = o.borrow().get(n).cloned().unwrap_or(Value::Null);
                        if !self.pattern(fr, sp, &fv)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            Pattern::Is { ty, fields, bind } => {
                let ok = match &v {
                    Value::Failure(f) => {
                        let code = match self.types.get(ty.as_str()).map(|t| &t.kind) {
                            Some(TypeKind::Error { code, .. }) => code.clone(),
                            _ => ty.clone(),
                        };
                        &*f.borrow().code == code
                    }
                    Value::Obj(o) => &*o.borrow().ty == ty,
                    _ => false,
                };
                if !ok {
                    return Ok(false);
                }
                for (n, sp) in fields {
                    let fv = self.field(&v, n)?;
                    if !self.pattern(fr, sp, &fv)? {
                        return Ok(false);
                    }
                }
                if let Some(b) = bind {
                    self.bind(fr, *b, v.clone());
                }
                true
            }
            Pattern::Tuple(ps) => match &v {
                Value::Tuple(items) if items.len() == ps.len() => {
                    for (sp, x) in ps.iter().zip(items.iter()) {
                        if !self.pattern(fr, sp, x)? {
                            return Ok(false);
                        }
                    }
                    true
                }
                _ => false,
            },
            Pattern::Record(fs) => {
                for (n, sp) in fs {
                    let fv = match self.field(&v, n) {
                        Ok(x) => x,
                        Err(_) => return Ok(false),
                    };
                    if !self.pattern(fr, sp, &fv)? {
                        return Ok(false);
                    }
                }
                true
            }
            Pattern::Otherwise(b) => {
                if let Some(b) = b {
                    self.bind(fr, *b, v.clone());
                }
                true
            }
            Pattern::Range { lo, hi } => match &v {
                // the folded inclusive interval of a range pattern (9.8, X-517); None leaves that end open
                Value::Int(n) => lo.map_or(true, |l| *n >= l) && hi.map_or(true, |h| *n <= h),
                _ => false,
            },
        })
    }
}

pub fn check_int(n: i128, what: &str) -> R<Value> {
    if n < i64::MIN as i128 || n > i64::MAX as i128 {
        return dynamic(format!("integer overflow in {what} (the operation is proof-required; use Int.{what})"));
    }
    Ok(Value::Int(n))
}

pub fn konst(c: &Const) -> Value {
    match c {
        Const::Void => Value::Void,
        Const::Bool(b) => Value::Bool(*b),
        Const::Int(n) => Value::Int(*n),
        Const::Float(b) => Value::Float(f64::from_bits(*b)),
        Const::Float32(b) => Value::F32(f32::from_bits(*b)),
        Const::Decimal(d) => match fortis_runtime::Decimal::parse(d) {
            Ok(x) => Value::Dec(x),
            Err(_) => Value::Null,
        },
        Const::Duration(ns) => Value::Dur(*ns),
        Const::Str(s) => Value::Str(Rc::from(s.as_str())),
        Const::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
        Const::Null => Value::Null,
    }
}
