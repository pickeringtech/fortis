//! The Fortis development VM (Delivery Programme 2.1, 6.1; DEL-35): executes the typed IR (fortis-mir) with tagged
//! values and reference counts, and reports a run in the form the conformance harness compares with the
//! definitional interpreter's.

pub mod codec;
pub mod executor;
pub mod generation;
pub mod intrinsics;
pub mod machine;
pub mod pyrandom;
pub mod session;
pub mod sync;
pub mod testing;
pub mod value;

use std::collections::HashMap;

use fortis_mir::model::Program;

use machine::{Exit, Flow, Halt, Vm};
use value::{display, Value};

/// What a run observed: console lines, then `fail:<code>`, `defect:<class>` or `dynamic:<message>`, joined with `|`
/// (conformance/tools/execute_check.py), with the trace and the emitted records.
pub struct Observed {
    pub text: String,
    pub out: Vec<String>,
    pub trace: Vec<String>,
    pub reports: Vec<machine::Record>,
    /// runtime-failure JSON lines, one per report, for stderr (the harness `record:` lines stay separate)
    pub failure_lines: Vec<String>,
    pub exit_code: i32,
    /// OS threads the run started for tasks (0 for a run that launched nothing)
    pub threads: usize,
}

/// The stack a VM run gets: the evaluator recurses several frames per Fortis call, and the depth bound (a
/// stack_exhaustion defect) must be reached before the thread's stack is.
const RUN_STACK: usize = 1 << 30;

/// The threads a run adds before any launch: the program runs on one runner thread while the caller's thread waits
/// for it, so one thread runs the program, as before (the thread probe counts it in the baseline, PROG-NFR-14).
pub const RUNNER_THREADS: usize = 1;

/// Run on a thread with a large reserved stack (reserved lazily by the platform).
pub(crate) fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| std::thread::Builder::new().name("fortis-vm".into()).stack_size(RUN_STACK).spawn_scoped(s, f).expect("spawn the VM thread").join().expect("the VM thread"))
}

pub fn run_main(prog: &Program, file: &str, args: &[String], decl_files: &HashMap<String, String>) -> Observed {
    on_big_stack(|| run_main_here(prog, file, args, decl_files))
}

fn run_main_here(prog: &Program, file: &str, args: &[String], decl_files: &HashMap<String, String>) -> Observed {
    let mut vm = Vm::new(prog, file);
    vm.decl_files = decl_files.clone();
    let entry = match &prog.entry {
        Some(e) => e.clone(),
        None => return Observed { text: "dynamic:no main".into(), out: vec![], trace: vec![], reports: vec![], failure_lines: vec![], exit_code: 2, threads: 0 },
    };
    let fi = match vm.func_index(&entry) {
        Ok(i) => i,
        Err(_) => return Observed { text: "dynamic:no main".into(), out: vec![], trace: vec![], reports: vec![], failure_lines: vec![], exit_code: 2, threads: 0 },
    };
    let pname = prog.functions[fi].params.first().map(|p| p.name.clone()).unwrap_or_else(|| "args".into());
    let argv = Value::list(args.iter().map(|a| Value::str(a)).collect());
    let r = vm.call(fi, if prog.functions[fi].params.is_empty() { vec![] } else { vec![(pname, argv)] }, &[]);
    // an async main runs as the main task (4.6)
    let r = match r {
        Ok(Value::Future(f)) => match f.work.borrow_mut().take() {
            Some(w) => vm.run_work(w),
            None => machine::dynamic("main's future was already polled"),
        },
        other => other,
    };
    let finish = |mut vm: Vm, text: String, code: i32| -> Observed {
        vm.flush_failures();
        let mut threads = 0;
        if let Some(s) = vm.sched.take() {
            threads = s.threads_started.load(std::sync::atomic::Ordering::Relaxed);
            s.shutdown();
        }
        Observed { text, out: vm.out, trace: vm.trace, reports: vm.reports, failure_lines: vm.failure_lines, exit_code: code, threads }
    };
    // the result of main: a failure value, a defect (no cleanup, no drain), or success
    let failure = match r {
        Ok(v) if v.is_failure() => Some(v),
        Ok(_) => None,
        Err(Exit::Flow(Flow::Fail(v))) => Some(v),
        Err(Exit::Halt(Halt::Defect(k))) => {
            vm.reports.push(vec![("kind".into(), "defect".into()), ("class".into(), k.clone()), ("task".into(), "main".into())]);
            let mut parts = vm.out.clone();
            parts.push(format!("defect:{k}"));
            return finish(vm, parts.join("|"), 101);
        }
        Err(Exit::Halt(Halt::Dynamic(m))) => return finish(vm, format!("dynamic:{m}"), 2),
        Err(Exit::Halt(Halt::Cancel)) => return finish(vm, "dynamic:the main task was cancelled".into(), 2),
        Err(Exit::Halt(Halt::Shutdown)) => return finish(vm, "dynamic:shutdown".into(), 2),
        Err(Exit::Flow(f)) => return finish(vm, format!("dynamic:{f:?} escaped main"), 2),
    };
    // detached tasks run to completion before main's own result is reported (their supervisor observes them)
    match vm.drain_detached() {
        Ok(()) => {}
        Err(Exit::Halt(Halt::Dynamic(m))) => return finish(vm, format!("dynamic:{m}"), 2),
        Err(Exit::Halt(Halt::Defect(k))) => {
            let mut parts = vm.out.clone();
            parts.push(format!("defect:{k}"));
            return finish(vm, parts.join("|"), 101);
        }
        Err(_) => return finish(vm, "dynamic:a detached task could not complete".into(), 2),
    }
    let (suffix, code) = match failure {
        Some(v) => {
            vm.emit_report(&v, "main");
            let c = match &v {
                Value::Failure(f) => f.borrow().code.to_string(),
                other => display(other),
            };
            (Some(format!("fail:{c}")), 1)
        }
        None => (None, 0),
    };
    let mut parts = vm.out.clone();
    if let Some(s) = suffix {
        parts.push(s);
    }
    finish(vm, parts.join("|"), code)
}

/// Run every test and property: one `test-result` record per case (14). The observed text is `N passed, M failed`.
pub fn run_tests(prog: &Program, file: &str, decl_files: &HashMap<String, String>) -> Observed {
    on_big_stack(|| run_tests_here(prog, file, decl_files))
}

fn run_tests_here(prog: &Program, file: &str, decl_files: &HashMap<String, String>) -> Observed {
    use fortis_types::types::{strip, Type};
    let mut vm = Vm::new(prog, file);
    vm.decl_files = decl_files.clone();
    let (mut passed, mut failed) = (0usize, 0usize);
    for t in &prog.tests {
        let fi = match vm.func_index(&t.function) {
            Ok(i) => i,
            Err(_) => continue,
        };
        let mut cases: Vec<(String, Vec<(String, Value)>)> = Vec::new();
        if t.kind == "test" {
            let row_param = prog.functions[fi].params.first().map(|p| p.name.clone());
            if t.cases.is_empty() {
                cases.push((String::new(), vec![]));
            }
            for (cname, rec) in &t.cases {
                let mut fr = machine::Frame { locals: vec![], func: fi, consumed: None };
                let v = vm.eval(&mut fr, rec).unwrap_or(Value::Null);
                cases.push((cname.clone(), row_param.clone().map(|p| vec![(p, v)]).unwrap_or_default()));
            }
        } else {
            let seed: u128 = t.title.chars().map(|c| c as u128).sum::<u128>() * 7919;
            let mut rng = pyrandom::PyRandom::new(seed);
            fn generate(rng: &mut pyrandom::PyRandom, ty: &Type) -> Value {
                let s = strip(ty);
                let name = match s {
                    Type::Named(n) => n.name.as_str(),
                    Type::Refined { base, .. } => base.name.as_str(),
                    _ => "",
                };
                match (name, s) {
                    ("Bool", _) => Value::Bool(rng.random() < 0.5),
                    ("String", _) => {
                        let n = rng.randint(0, 8);
                        let pool: Vec<char> = "abcxyz ".chars().collect();
                        let text: String = (0..n).map(|_| pool[rng.below(pool.len() as u128) as usize]).collect();
                        Value::str(&text)
                    }
                    ("List", Type::Named(n)) => {
                        let inner = n.args.first().cloned().unwrap_or_else(fortis_types::types::int);
                        let k = rng.randint(0, 8);
                        Value::list((0..k).map(|_| generate(rng, &inner)).collect())
                    }
                    (_, Type::Refined { lo, hi, .. }) => Value::Int(rng.randint(lo.unwrap_or(-1000), hi.unwrap_or(1000))),
                    _ => Value::Int(rng.randint(-1000, 1000)),
                }
            }
            for i in 0..25 {
                let args: Vec<(String, Value)> = t.generators.iter().map(|(n, ty)| (n.clone(), generate(&mut rng, ty))).collect();
                cases.push((format!("case {}", i + 1), args));
            }
        }
        for (cname, args) in cases {
            vm.obs.push(None);
            let r = vm.call(fi, args, &[]);
            vm.obs.pop();
            let (status, evidence) = match r {
                Ok(v) if v.is_failure() => {
                    let code = match &v {
                        Value::Failure(f) => f.borrow().code.to_string(),
                        _ => String::new(),
                    };
                    ("fail", format!("actual=failure {code} expected=success"))
                }
                Ok(_) | Err(Exit::Flow(Flow::Return(_))) => ("pass", String::new()),
                Err(Exit::Halt(Halt::Dynamic(m))) if m.starts_with("expect:") => ("fail", m[7..].to_string()),
                Err(Exit::Halt(Halt::Dynamic(m))) => ("error", m),
                Err(Exit::Halt(Halt::Cancel)) => ("error", "cancelled".to_string()),
                Err(Exit::Halt(Halt::Shutdown)) => ("error", "shutdown".to_string()),
                Err(Exit::Halt(Halt::Defect(k))) => ("error", format!("defect {k}")),
                Err(Exit::Flow(f)) => ("error", format!("{f:?}")),
            };
            vm.reports.push(vec![
                ("kind".into(), "test-result".into()),
                ("test".into(), t.title.clone()),
                ("case".into(), cname),
                ("status".into(), status.into()),
                ("evidence".into(), evidence),
            ]);
            if status == "pass" {
                passed += 1;
            } else {
                failed += 1;
            }
        }
    }
    for (title, reason) in &prog.unbuilt_tests {
        vm.reports.push(vec![
            ("kind".into(), "test-result".into()),
            ("test".into(), title.clone()),
            ("case".into(), String::new()),
            ("status".into(), "unbuilt".into()),
            ("evidence".into(), reason.clone()),
        ]);
    }
    let unbuilt = if prog.unbuilt_tests.is_empty() { String::new() } else { format!(", {} unbuilt", prog.unbuilt_tests.len()) };
    vm.flush_failures();
    Observed { text: format!("{passed} passed, {failed} failed{unbuilt}"), out: vm.out, trace: vm.trace, reports: vm.reports, failure_lines: vm.failure_lines, exit_code: if failed > 0 { 1 } else { 0 }, threads: 0 }
}
