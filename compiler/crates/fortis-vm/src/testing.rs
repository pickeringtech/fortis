//! The test runner (Language Reference 14; Programme 8.1 to 8.3; PROG-TST-01, PROG-TST-02; X-131).
//!
//! One runner serves every testing mode of Language 1.0:
//!
//! - **Example tests** (`test "…" { … }`): one case.
//! - **Table tests** (`test "…" cases { … } as row { … }`): one case per row. A row runs alone by its name
//!   (`case`), so a failing row reproduces exactly; a row is already the minimal failing input.
//! - **Properties** (`property "…" forall (…)`): inputs are generated from the parameter types and refinements. Case
//!   `k` draws from a generator seeded by the run's seed and `k`, so a failure replays from the seed and its case.
//!   A failing input is shrunk within the valid domain (each candidate keeps the refinement) while it still fails
//!   the same way.
//! - **Fuzzing** (`fuzz`): every property is also a fuzz target. Inputs are mutated from a corpus and guided by the
//!   branches the VM records; an input that takes a new branch joins the corpus. The corpus persists in a directory
//!   across runs and is minimised to the smallest inputs that keep its coverage. Each case has an execution budget,
//!   so a hang is a finding. A failure is shrunk, saved with its seed, and replays from the saved file (`replay`).
//!
//! Every outcome is a record (PROG-DIA-07): one `test-result` per case, and one `property-failure` for each
//! minimised failure, naming the seed, the original and minimal inputs and the command that replays it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use fortis_diag::Json;
use fortis_mir::model::{Program, TestDef};
use fortis_types::types::{strip, Type};

use crate::machine::{Exit, Flow, Frame, Halt, Vm, R};
use crate::pyrandom::PyRandom;
use crate::sync::Rc;
use crate::value::Value;

/// What to run and how.
#[derive(Clone, Debug)]
pub struct Options {
    /// only the declaration with this title
    pub test: Option<String>,
    /// only this case: a table row's name, or `case K` of a property
    pub case: Option<String>,
    /// the run's seed (by default each property's own, derived from its title)
    pub seed: Option<u64>,
    /// generated cases per property
    pub cases: usize,
    /// execution steps (calls and loop iterations) one case may take
    pub fuel: u64,
    /// steps of shrinking one failure may take
    pub shrink_budget: usize,
    pub fuzz: Option<Fuzz>,
    /// one saved input to run (a failure file written by fuzzing): `{"test": …, "inputs": {…}}`
    pub replay: Option<PathBuf>,
    /// the command a replay names (`fortis test FILE`)
    pub command: String,
}

impl Default for Options {
    fn default() -> Self {
        Options { test: None, case: None, seed: None, cases: 100, fuel: 5_000_000, shrink_budget: 2_000, fuzz: None, replay: None, command: "fortis test".into() }
    }
}

#[derive(Clone, Debug)]
pub struct Fuzz {
    /// executions per target
    pub budget: usize,
    /// where each target's corpus persists (`DIR/<target>/`), with its failures (`DIR/<target>/failures/`)
    pub corpus: Option<PathBuf>,
}

pub struct Outcome {
    pub records: Vec<Json>,
    pub passed: usize,
    pub failed: usize,
}

/// Run the program's tests as `opts` selects, on a thread with the VM's stack.
pub fn run(prog: &Program, file: &str, opts: &Options) -> Outcome {
    let (prog, file, opts) = (prog, file.to_string(), opts.clone());
    crate::on_big_stack(move || Runner { vm: Vm::new(prog, &file), prog, opts, records: vec![], passed: 0, failed: 0 }.all())
}

type Args = Vec<(String, Value)>;

struct Runner<'p> {
    vm: Vm<'p>,
    prog: &'p Program,
    opts: Options,
    records: Vec<Json>,
    passed: usize,
    failed: usize,
}

/// FNV-1a: a stable 64-bit hash for seeds and corpus file names.
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A property's default seed: its title's hash, so a run is reproducible without naming one.
pub fn title_seed(title: &str) -> u64 {
    fnv(title.as_bytes())
}

fn case_seed(seed: u64, k: usize) -> u128 {
    fnv(format!("{seed}/{k}").as_bytes()) as u128
}

fn slug(title: &str) -> String {
    let s: String = title.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    format!("{}-{:08x}", s.chars().take(40).collect::<String>(), fnv(title.as_bytes()) as u32)
}

/// The status and evidence of one case's result.
fn classify(r: R<Value>) -> (&'static str, Json) {
    let ev = |actual: String, expected: &str| Json::obj([("actual", Json::str(&actual)), ("expected", Json::str(expected))]);
    match r {
        Ok(v) if v.is_failure() => {
            let code = match &v {
                Value::Failure(f) => f.borrow().code.to_string(),
                _ => String::new(),
            };
            ("fail", ev(format!("failure {code}"), "success"))
        }
        Ok(_) | Err(Exit::Flow(Flow::Return(_))) => ("pass", Json::Obj(BTreeMap::new())),
        Err(Exit::Flow(Flow::Fail(v))) => {
            let code = match &v {
                Value::Failure(f) => f.borrow().code.to_string(),
                _ => String::new(),
            };
            ("fail", ev(format!("failure {code}"), "success"))
        }
        Err(Exit::Halt(Halt::Dynamic(m))) if m.starts_with("expect:") => {
            let body = &m[7..];
            match body.strip_prefix("actual=").and_then(|b| b.split_once(" expected=")) {
                Some((a, e)) => ("fail", ev(a.to_string(), e)),
                None => ("fail", ev(body.to_string(), "true")),
            }
        }
        Err(Exit::Halt(Halt::Dynamic(m))) if m.starts_with("budget:") => ("error", ev(format!("hang: {}", &m[8..]), "the case completes within its budget")),
        Err(Exit::Halt(Halt::Dynamic(m))) => ("error", ev(m, "a well-formed run")),
        Err(Exit::Halt(Halt::Defect(k))) => ("error", ev(format!("defect {k}"), "no defect")),
        Err(Exit::Halt(Halt::Cancel)) => ("error", ev("cancelled".into(), "completion")),
        Err(Exit::Halt(Halt::Shutdown)) => ("error", ev("shutdown".into(), "completion")),
        Err(Exit::Flow(f)) => ("error", ev(format!("{f:?}"), "completion")),
    }
}

// ---- values from types -------------------------------------------------------------------------------------

fn type_name(ty: &Type) -> &str {
    match strip(ty) {
        Type::Named(n) => n.name.as_str(),
        Type::Refined { base, .. } => base.name.as_str(),
        _ => "",
    }
}

/// The integer range a type admits (an unrefined `Int` is 64-bit; `Byte` is 0..255).
fn int_range(ty: &Type) -> (i128, i128) {
    match strip(ty) {
        Type::Refined { base, lo, hi, .. } if base.name != "List" && base.name != "String" => (lo.unwrap_or(i64::MIN as i128), hi.unwrap_or(i64::MAX as i128)),
        Type::Named(n) if n.name == "Byte" => (0, 255),
        _ => (i64::MIN as i128, i64::MAX as i128),
    }
}

fn is_int(ty: &Type) -> bool {
    matches!(type_name(ty), "Int" | "Byte" | "Int8" | "Int16" | "Int32" | "Int64")
}

fn elem(ty: &Type) -> Option<Type> {
    match strip(ty) {
        Type::Named(n) if n.name == "List" => n.args.first().cloned(),
        _ => None,
    }
}

const POOL: &[char] = &['a', 'b', 'c', 'x', 'y', 'z', ' '];
const FUZZ_POOL: &[char] = &['a', 'Z', '0', ' ', '\n', '"', '\\', '$', '{', 'é', 'ß', '€', '😀', '\u{0}'];

/// A generated value of `ty`, or the reason there is no generator for it.
fn generate(rng: &mut PyRandom, ty: &Type, depth: usize) -> Result<Value, String> {
    if let Type::Optional(inner) = strip(ty) {
        return if rng.random() < 0.2 { Ok(Value::Null) } else { generate(rng, inner, depth) };
    }
    if let Type::Tuple(ts) = strip(ty) {
        return Ok(Value::Tuple(Rc::new(ts.iter().map(|t| generate(rng, t, depth + 1)).collect::<Result<Vec<_>, _>>()?)));
    }
    if is_int(ty) {
        let (lo, hi) = int_range(ty);
        // small values first, as the VM's generator draws them, within the refinement
        let (a, b) = (lo.max(-1000), hi.min(1000));
        return Ok(Value::Int(if a <= b { rng.randint(a, b) } else { rng.randint(lo, hi) }));
    }
    match type_name(ty) {
        "Bool" => Ok(Value::Bool(rng.random() < 0.5)),
        "String" => {
            let n = rng.randint(0, 8) as usize;
            Ok(Value::str(&(0..n).map(|_| POOL[rng.below(POOL.len() as u128) as usize]).collect::<String>()))
        }
        "Bytes" => {
            let n = rng.randint(0, 16) as usize;
            Ok(Value::Bytes(Rc::new((0..n).map(|_| rng.below(256) as u8).collect())))
        }
        "List" if depth < 4 => {
            let inner = elem(ty).unwrap_or_else(fortis_types::types::int);
            let n = rng.randint(0, 8) as usize;
            Ok(Value::list((0..n).map(|_| generate(rng, &inner, depth + 1)).collect::<Result<Vec<_>, _>>()?))
        }
        other => Err(format!("no generator for the type {}", if other.is_empty() { strip(ty).show() } else { other.to_string() })),
    }
}

/// A value as JSON, for records and the corpus; `from_json` reads it back given its type.
fn to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => i64::try_from(*i).map(Json::Int).unwrap_or_else(|_| Json::str(&i.to_string())),
        Value::Str(s) => Json::str(&s.to_string()),
        Value::Bytes(b) => Json::obj([("bytes", Json::str(&b.iter().map(|x| format!("{x:02x}")).collect::<String>()))]),
        Value::List(l) => Json::Arr(l.borrow().iter().map(to_json).collect()),
        Value::Tuple(t) => Json::obj([("tuple", Json::Arr(t.iter().map(to_json).collect()))]),
        other => Json::str(&crate::value::display(other)),
    }
}

fn from_json(j: &Json, ty: &Type) -> Option<Value> {
    if let Type::Optional(inner) = strip(ty) {
        return if *j == Json::Null { Some(Value::Null) } else { from_json(j, inner) };
    }
    if let (Type::Tuple(ts), Some(Json::Arr(items))) = (strip(ty), j.get("tuple")) {
        return Some(Value::Tuple(Rc::new(ts.iter().zip(items).map(|(t, x)| from_json(x, t)).collect::<Option<Vec<_>>>()?)));
    }
    if is_int(ty) {
        let i = match j {
            Json::Int(i) => *i as i128,
            Json::Str(s) => s.parse().ok()?,
            _ => return None,
        };
        let (lo, hi) = int_range(ty);
        return (lo..=hi).contains(&i).then_some(Value::Int(i));
    }
    match (type_name(ty), j) {
        ("Bool", Json::Bool(b)) => Some(Value::Bool(*b)),
        ("String", Json::Str(s)) => Some(Value::str(s)),
        ("Bytes", _) => match j.get("bytes") {
            Some(Json::Str(h)) => (0..h.len() / 2).map(|i| u8::from_str_radix(h.get(2 * i..2 * i + 2)?, 16).ok()).collect::<Option<Vec<u8>>>().map(|b| Value::Bytes(Rc::new(b))),
            _ => None,
        },
        ("List", Json::Arr(items)) => {
            let inner = elem(ty)?;
            Some(Value::list(items.iter().map(|x| from_json(x, &inner)).collect::<Option<Vec<_>>>()?))
        }
        _ => None,
    }
}

/// How big an input is: shrinking and corpus minimisation prefer smaller ones.
fn size(v: &Value) -> u128 {
    match v {
        Value::Int(i) => i.unsigned_abs(),
        Value::Bool(b) => *b as u128,
        Value::Str(s) => s.chars().count() as u128 * 4 + s.chars().map(|c| c as u128 - POOL.contains(&c) as u128 * c as u128).sum::<u128>().min(1_000_000),
        Value::Bytes(b) => b.len() as u128 * 4 + b.iter().map(|x| *x as u128).sum::<u128>(),
        Value::List(l) => {
            let l = l.borrow();
            1 + l.len() as u128 * 8 + l.iter().map(size).sum::<u128>()
        }
        Value::Tuple(t) => t.iter().map(size).sum(),
        _ => 0,
    }
}

/// Smaller candidates for a failing value, within its type's domain, most aggressive first.
fn shrinks(v: &Value, ty: &Type) -> Vec<Value> {
    let mut out = Vec::new();
    if let Type::Optional(inner) = strip(ty) {
        if !matches!(v, Value::Null) {
            out.push(Value::Null);
            out.extend(shrinks(v, inner));
        }
        return out;
    }
    match v {
        Value::Int(x) => {
            let (lo, hi) = int_range(ty);
            let target = 0i128.clamp(lo, hi);
            let mut d = *x - target;
            let mut seen = HashSet::new();
            out.push(Value::Int(target));
            while d != 0 {
                d /= 2;
                let c = *x - d;
                if c != *x && (lo..=hi).contains(&c) && seen.insert(c) {
                    out.push(Value::Int(c));
                }
            }
            if *x < 0 && (lo..=hi).contains(&-*x) {
                out.push(Value::Int(-*x));
            }
        }
        Value::Bool(true) => out.push(Value::Bool(false)),
        Value::Str(s) => {
            let cs: Vec<char> = s.chars().collect();
            let n = cs.len();
            if n > 0 {
                out.push(Value::str(""));
            }
            let mut chunk = n / 2;
            while chunk >= 1 {
                for start in (0..n).step_by(chunk) {
                    let t: String = cs[..start].iter().chain(cs[(start + chunk).min(n)..].iter()).collect();
                    out.push(Value::str(&t));
                }
                chunk /= 2;
            }
            for (i, c) in cs.iter().enumerate() {
                if *c != 'a' {
                    let mut t = cs.clone();
                    t[i] = 'a';
                    out.push(Value::str(&t.iter().collect::<String>()));
                }
            }
        }
        Value::Bytes(b) => {
            let n = b.len();
            if n > 0 {
                out.push(Value::Bytes(Rc::new(vec![])));
            }
            let mut chunk = n / 2;
            while chunk >= 1 {
                for start in (0..n).step_by(chunk) {
                    let t: Vec<u8> = b[..start].iter().chain(b[(start + chunk).min(n)..].iter()).copied().collect();
                    out.push(Value::Bytes(Rc::new(t)));
                }
                chunk /= 2;
            }
            for i in 0..n {
                if b[i] != 0 {
                    let mut t = (**b).clone();
                    t[i] = 0;
                    out.push(Value::Bytes(Rc::new(t)));
                }
            }
        }
        Value::List(l) => {
            let items = l.borrow().clone();
            let n = items.len();
            if n > 0 {
                out.push(Value::list(vec![]));
            }
            let mut chunk = n / 2;
            while chunk >= 1 {
                for start in (0..n).step_by(chunk) {
                    out.push(Value::list(items[..start].iter().chain(items[(start + chunk).min(n)..].iter()).cloned().collect()));
                }
                chunk /= 2;
            }
            if let Some(inner) = elem(ty) {
                for i in 0..n {
                    for c in shrinks(&items[i], &inner) {
                        let mut t = items.clone();
                        t[i] = c;
                        out.push(Value::list(t));
                    }
                }
            }
        }
        Value::Tuple(t) => {
            if let Type::Tuple(ts) = strip(ty) {
                for i in 0..t.len() {
                    for c in shrinks(&t[i], &ts[i]) {
                        let mut u = (**t).clone();
                        u[i] = c;
                        out.push(Value::Tuple(Rc::new(u)));
                    }
                }
            }
        }
        _ => {}
    }
    let s = size(v);
    out.retain(|c| size(c) < s || matches!(c, Value::Null));
    out
}

/// One structural mutation of a value, within its type's domain (the fuzzer's step).
fn mutate(rng: &mut PyRandom, v: &Value, ty: &Type, depth: usize) -> Value {
    if let Type::Optional(inner) = strip(ty) {
        return match v {
            Value::Null => generate(rng, inner, depth).unwrap_or(Value::Null),
            _ if rng.random() < 0.1 => Value::Null,
            _ => mutate(rng, v, inner, depth),
        };
    }
    match v {
        Value::Int(x) => {
            let (lo, hi) = int_range(ty);
            let c = match rng.randint(0, 6) {
                0 => x.saturating_add(1),
                1 => x.saturating_sub(1),
                2 => x.saturating_mul(2),
                3 => *x / 2,
                4 => [lo, hi, 0, -1, 1][rng.below(5) as usize],
                5 => x.saturating_neg(),
                _ => rng.randint(lo.max(-1_000_000), hi.min(1_000_000)),
            };
            Value::Int(c.clamp(lo, hi))
        }
        Value::Bool(b) => Value::Bool(!b),
        Value::Str(s) => {
            let mut cs: Vec<char> = s.chars().collect();
            let c = FUZZ_POOL[rng.below(FUZZ_POOL.len() as u128) as usize];
            match (rng.randint(0, 2), cs.is_empty()) {
                (0, _) | (_, true) => {
                    let at = rng.randint(0, cs.len() as i128) as usize;
                    cs.insert(at, c);
                }
                (1, false) => {
                    let at = rng.below(cs.len() as u128) as usize;
                    cs.remove(at);
                }
                _ => {
                    let at = rng.below(cs.len() as u128) as usize;
                    cs[at] = c;
                }
            }
            Value::str(&cs.into_iter().collect::<String>())
        }
        Value::Bytes(b) => {
            let mut t = (**b).clone();
            match (rng.randint(0, 2), t.is_empty()) {
                (0, _) | (_, true) => {
                    let at = rng.randint(0, t.len() as i128) as usize;
                    t.insert(at, rng.below(256) as u8);
                }
                (1, false) => {
                    let at = rng.below(t.len() as u128) as usize;
                    t.remove(at);
                }
                _ => {
                    let at = rng.below(t.len() as u128) as usize;
                    t[at] ^= 1 << rng.below(8);
                }
            }
            Value::Bytes(Rc::new(t))
        }
        Value::List(l) => {
            let mut items = l.borrow().clone();
            let inner = elem(ty).unwrap_or_else(fortis_types::types::int);
            match (rng.randint(0, 3), items.is_empty()) {
                (0, _) | (_, true) => {
                    if let Ok(x) = generate(rng, &inner, depth + 1) {
                        let at = rng.randint(0, items.len() as i128) as usize;
                        items.insert(at, x);
                    }
                }
                (1, false) => {
                    let at = rng.below(items.len() as u128) as usize;
                    items.remove(at);
                }
                (2, false) => {
                    let at = rng.below(items.len() as u128) as usize;
                    let x = items[at].clone();
                    items.insert(at, x);
                }
                _ => {
                    let at = rng.below(items.len() as u128) as usize;
                    items[at] = mutate(rng, &items[at], &inner, depth + 1);
                }
            }
            Value::list(items)
        }
        Value::Tuple(t) => {
            if let Type::Tuple(ts) = strip(ty) {
                if !t.is_empty() {
                    let mut u = (**t).clone();
                    let i = rng.below(u.len() as u128) as usize;
                    u[i] = mutate(rng, &u[i], &ts[i], depth + 1);
                    return Value::Tuple(Rc::new(u));
                }
            }
            v.clone()
        }
        other => other.clone(),
    }
}

fn args_json(args: &Args) -> Json {
    Json::Obj(args.iter().map(|(k, v)| (k.clone(), to_json(v))).collect::<BTreeMap<_, _>>())
}

fn args_size(args: &Args) -> u128 {
    args.iter().map(|(_, v)| size(v)).sum()
}

impl<'p> Runner<'p> {
    fn all(mut self) -> Outcome {
        if let Some(path) = self.opts.replay.clone() {
            self.replay(&path);
        } else {
            for t in &self.prog.tests {
                if self.opts.test.as_ref().map_or(false, |x| *x != t.title) {
                    continue;
                }
                let Ok(fi) = self.vm.func_index(&t.function) else { continue };
                if t.kind == "test" {
                    self.table(t, fi);
                } else if self.opts.fuzz.is_some() {
                    self.fuzz(t, fi);
                } else {
                    self.property(t, fi);
                }
            }
            for (title, reason) in &self.prog.unbuilt_tests {
                self.result(title, "", "error", Json::obj([("actual", Json::str(&format!("unbuilt: {reason}"))), ("expected", Json::str("a built test"))]));
            }
        }
        Outcome { records: self.records, passed: self.passed, failed: self.failed }
    }

    fn result(&mut self, test: &str, case: &str, status: &str, evidence: Json) {
        if status == "pass" {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        self.records.push(Json::obj([("kind", Json::str("test-result")), ("test", Json::str(test)), ("case", Json::str(case)), ("status", Json::str(status)), ("evidence", evidence), ("rule", Json::str("LANG-14-02"))]));
    }

    /// One case: the function called with `args` under the case budget; with `cover`, the branches it took.
    fn case(&mut self, fi: usize, args: Args, cover: bool) -> (&'static str, Json, HashSet<(usize, u32)>) {
        self.vm.fuel = Some(self.opts.fuel);
        self.vm.coverage = cover.then(HashSet::new);
        self.vm.obs.push(None);
        let r = self.vm.call(fi, args, &[]);
        self.vm.obs.pop();
        self.vm.out.clear();
        self.vm.fuel = None;
        let cov = self.vm.coverage.take().unwrap_or_default();
        let (s, e) = classify(r);
        (s, e, cov)
    }

    fn table(&mut self, t: &TestDef, fi: usize) {
        let row_param = self.prog.functions[fi].params.first().map(|p| p.name.clone());
        let mut rows: Vec<(String, Args)> = Vec::new();
        if t.cases.is_empty() {
            rows.push((String::new(), vec![]));
        }
        for (name, rec) in &t.cases {
            let mut fr = Frame { locals: vec![], func: fi, consumed: None };
            let v = self.vm.eval(&mut fr, rec).unwrap_or(Value::Null);
            rows.push((name.clone(), row_param.clone().map(|p| vec![(p, v)]).unwrap_or_default()));
        }
        for (name, args) in rows {
            if self.opts.case.as_ref().map_or(false, |c| *c != name) {
                continue;
            }
            let (status, ev, _) = self.case(fi, args, false);
            self.result(&t.title, &name, status, ev);
        }
    }

    fn generated(&self, t: &TestDef, rng: &mut PyRandom) -> Result<Args, String> {
        t.generators.iter().map(|(n, ty)| generate(rng, ty, 0).map(|v| (n.clone(), v))).collect()
    }

    /// A property: one record for its passing cases together, or the failing case and its minimised input.
    fn property(&mut self, t: &TestDef, fi: usize) {
        let seed = self.opts.seed.unwrap_or_else(|| title_seed(&t.title));
        let mut ran = 0usize;
        for k in 0..self.opts.cases {
            let name = format!("case {}", k + 1);
            if self.opts.case.as_ref().map_or(false, |c| *c != name) {
                continue;
            }
            let mut rng = PyRandom::new(case_seed(seed, k));
            let args = match self.generated(t, &mut rng) {
                Ok(a) => a,
                Err(why) => {
                    self.result(&t.title, &name, "error", Json::obj([("actual", Json::str(&why)), ("expected", Json::str("a generator for every parameter"))]));
                    return;
                }
            };
            let (status, ev, _) = self.case(fi, args.clone(), false);
            ran += 1;
            if status != "pass" {
                self.result(&t.title, &name, status, ev);
                let replay = format!("{} --test {:?} --seed {seed} --case {:?}", self.opts.command, t.title, name);
                self.minimise(t, fi, args, status, "generated", seed, &name, &replay);
                return;
            }
        }
        if ran > 0 {
            let case = if ran == 1 && self.opts.case.is_some() { self.opts.case.clone().unwrap_or_default() } else { format!("{ran} cases") };
            self.result(&t.title, &case, "pass", Json::obj([("cases", Json::Int(ran as i64)), ("seed", Json::str(&seed.to_string()))]));
        }
    }

    /// Shrink a failing input while it fails the same way, then record it.
    #[allow(clippy::too_many_arguments)]
    fn minimise(&mut self, t: &TestDef, fi: usize, original: Args, status: &str, mode: &str, seed: u64, case: &str, replay: &str) -> Args {
        let types: Vec<Type> = t.generators.iter().map(|(_, ty)| ty.clone()).collect();
        let mut cur = original.clone();
        let mut steps = 0usize;
        let mut accepted = 0usize;
        let mut evidence = Json::Obj(BTreeMap::new());
        'outer: loop {
            for i in 0..cur.len() {
                for c in shrinks(&cur[i].1, &types[i]) {
                    if steps >= self.opts.shrink_budget {
                        break 'outer;
                    }
                    steps += 1;
                    let mut trial = cur.clone();
                    trial[i].1 = c;
                    let (s, ev, _) = self.case(fi, trial.clone(), false);
                    if s == status {
                        cur = trial;
                        evidence = ev;
                        accepted += 1;
                        continue 'outer;
                    }
                }
            }
            break;
        }
        if accepted == 0 {
            evidence = self.case(fi, cur.clone(), false).1;
        }
        self.records.push(Json::obj([
            ("kind", Json::str("property-failure")),
            ("test", Json::str(&t.title)),
            ("mode", Json::str(mode)),
            ("seed", Json::str(&seed.to_string())),
            ("case", Json::str(case)),
            ("original", args_json(&original)),
            ("minimal", args_json(&cur)),
            ("shrinks", Json::Int(accepted as i64)),
            ("evidence", evidence),
            ("replay", Json::str(replay)),
            ("rule", Json::str("PROG-TST-02")),
        ]));
        cur
    }

    fn fuzz(&mut self, t: &TestDef, fi: usize) {
        let fz = self.opts.fuzz.clone().expect("fuzzing");
        let seed = self.opts.seed.unwrap_or_else(|| title_seed(&t.title));
        let dir = fz.corpus.as_ref().map(|d| d.join(slug(&t.title)));
        let types: Vec<Type> = t.generators.iter().map(|(_, ty)| ty.clone()).collect();
        let mut rng = PyRandom::new(case_seed(seed, usize::MAX));
        // the persisted corpus first, then generated inputs
        let mut corpus: Vec<(Args, HashSet<(usize, u32)>)> = Vec::new();
        let mut covered: HashSet<(usize, u32)> = HashSet::new();
        let mut seeds: Vec<Args> = load_inputs(dir.as_deref(), t);
        for _ in 0..8 {
            match self.generated(t, &mut rng) {
                Ok(a) => seeds.push(a),
                Err(why) => {
                    self.result(&t.title, "fuzz", "error", Json::obj([("actual", Json::str(&why)), ("expected", Json::str("a generator for every parameter"))]));
                    return;
                }
            }
        }
        let mut executions = 0usize;
        let mut finding: Option<(Args, &'static str, Json, usize)> = None;
        for a in seeds {
            let (s, ev, cov) = self.case(fi, a.clone(), true);
            executions += 1;
            if s != "pass" {
                finding = Some((a, s, ev, executions));
                break;
            }
            if !cov.is_subset(&covered) {
                covered.extend(cov.iter().copied());
                corpus.push((a, cov));
            }
        }
        while finding.is_none() && executions < fz.budget {
            let base = if !corpus.is_empty() && rng.random() < 0.85 { corpus[rng.below(corpus.len() as u128) as usize].0.clone() } else { self.generated(t, &mut rng).unwrap_or_default() };
            let mut input = base;
            for _ in 0..rng.randint(1, 3) {
                if input.is_empty() {
                    break;
                }
                let i = rng.below(input.len() as u128) as usize;
                input[i].1 = mutate(&mut rng, &input[i].1, &types[i], 0);
            }
            let (s, ev, cov) = self.case(fi, input.clone(), true);
            executions += 1;
            if s != "pass" {
                finding = Some((input, s, ev, executions));
                break;
            }
            if !cov.is_subset(&covered) {
                covered.extend(cov.iter().copied());
                corpus.push((input, cov));
            }
        }
        // corpus minimisation: the smallest inputs that keep the coverage (greedy set cover)
        corpus.sort_by_key(|(a, _)| args_size(a));
        let mut kept: Vec<Args> = Vec::new();
        let mut have: HashSet<(usize, u32)> = HashSet::new();
        for (a, cov) in &corpus {
            if !cov.is_subset(&have) {
                have.extend(cov.iter().copied());
                kept.push(a.clone());
            }
        }
        if let Some(d) = &dir {
            save_corpus(d, t, &kept);
        }
        match finding {
            None => self.result(
                &t.title,
                "fuzz",
                "pass",
                Json::obj([("executions", Json::Int(executions as i64)), ("corpus", Json::Int(kept.len() as i64)), ("branches", Json::Int(covered.len() as i64))]),
            ),
            Some((input, status, ev, n)) => {
                let case = format!("fuzz {n}");
                self.result(&t.title, &case, status, ev);
                let replay_file = dir.as_ref().map(|d| d.join("failures"));
                let replay = match &replay_file {
                    Some(_) => format!("{} --replay <the saved failure>", self.opts.command),
                    None => format!("{} --test {:?} --fuzz --seed {seed}", self.opts.command, t.title),
                };
                let minimal = self.minimise(t, fi, input, status, "fuzz", seed, &case, &replay);
                if let Some(f) = replay_file {
                    let _ = std::fs::create_dir_all(&f);
                    let body = Json::obj([("test", Json::str(&t.title)), ("inputs", args_json(&minimal))]).to_line();
                    let path = f.join(format!("{:016x}.json", fnv(body.as_bytes())));
                    let _ = std::fs::write(&path, format!("{body}\n"));
                    if let Some(Json::Obj(m)) = self.records.last_mut() {
                        m.insert("replay".into(), Json::str(&format!("{} --replay {}", self.opts.command, path.display())));
                    }
                }
            }
        }
    }

    fn replay(&mut self, path: &Path) {
        let text = read_input(path).unwrap_or_default();
        let parsed = match Json::parse(text.trim()) {
            Ok(j) => Some(j),
            Err(e) => {
                self.result("replay", &path.display().to_string(), "error", Json::obj([("actual", Json::str(&e)), ("expected", Json::str("a saved input"))]));
                return;
            }
        };
        let title = parsed.as_ref().and_then(|j| match j.get("test") {
            Some(Json::Str(s)) => Some(s.clone()),
            _ => None,
        });
        let Some(t) = title.and_then(|x| self.prog.tests.iter().find(|t| t.title == x)) else {
            self.result("replay", &path.display().to_string(), "error", Json::obj([("actual", Json::str("the file names no test of this program")), ("expected", Json::str("a saved input"))]));
            return;
        };
        let inputs = parsed.as_ref().and_then(|j| j.get("inputs")).cloned().unwrap_or(Json::Null);
        let args: Option<Args> = t.generators.iter().map(|(n, ty)| inputs.get(n).and_then(|x| from_json(x, ty)).map(|v| (n.clone(), v))).collect();
        let (Some(args), Ok(fi)) = (args, self.vm.func_index(&t.function)) else {
            self.result(&t.title, "replay", "error", Json::obj([("actual", Json::str("the saved inputs do not fit the property's parameters")), ("expected", Json::str("a saved input"))]));
            return;
        };
        let (status, ev, _) = self.case(fi, args, false);
        self.result(&t.title, "replay", status, ev);
    }
}

/// A saved input, within the quota of every input parser (PROG-SEC-01): at most 1 MiB.
fn read_input(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > 1 << 20 {
        return Some("E-LIM-005: a saved input over the budget of 1048576 bytes".into());
    }
    std::fs::read_to_string(path).ok()
}

fn load_inputs(dir: Option<&Path>, t: &TestDef) -> Vec<Args> {
    let Some(dir) = dir else { return vec![] };
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map_or(false, |x| x == "json")).collect()).unwrap_or_default();
    names.sort();
    names
        .iter()
        .filter_map(|p| {
            let j = Json::parse(read_input(p)?.trim()).ok()?;
            let inputs = j.get("inputs")?;
            t.generators.iter().map(|(n, ty)| inputs.get(n).and_then(|x| from_json(x, ty)).map(|v| (n.clone(), v))).collect()
        })
        .collect()
}

fn save_corpus(dir: &Path, t: &TestDef, kept: &[Args]) {
    let _ = std::fs::create_dir_all(dir);
    let mut keep = HashSet::new();
    for a in kept {
        let body = Json::obj([("test", Json::str(&t.title)), ("inputs", args_json(a))]).to_line();
        let name = format!("{:016x}.json", fnv(body.as_bytes()));
        let _ = std::fs::write(dir.join(&name), format!("{body}\n"));
        keep.insert(name);
    }
    // entries the minimised corpus no longer needs are removed (failures stay in their own directory)
    if let Ok(d) = std::fs::read_dir(dir) {
        for e in d.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".json") && !keep.contains(&n) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortis_types::types::NamedT;

    fn int_in(lo: i128, hi: i128) -> Type {
        Type::Refined { base: NamedT { name: "Int".into(), args: vec![], package: String::new(), origin: String::new() }, lo: Some(lo), hi: Some(hi), kind: "range".into() }
    }

    #[test]
    fn shrinking_stays_in_the_refinement() {
        let ty = int_in(10, 100);
        for c in shrinks(&Value::Int(77), &ty) {
            let Value::Int(x) = c else { panic!() };
            assert!((10..=100).contains(&x) && x < 77);
        }
        assert!(matches!(shrinks(&Value::Int(77), &ty)[0], Value::Int(10)));
    }

    #[test]
    fn values_round_trip_through_json() {
        let ty = fortis_types::types::named_args("List", vec![int_in(0, 100)]);
        let v = Value::list(vec![Value::Int(3), Value::Int(99)]);
        let back = from_json(&to_json(&v), &ty).unwrap();
        assert_eq!(to_json(&back), to_json(&v));
        assert!(from_json(&Json::Arr(vec![Json::Int(101)]), &ty).is_none(), "a value outside the refinement is not read back");
    }

    #[test]
    fn mutation_stays_in_the_domain() {
        let ty = int_in(-5, 5);
        let mut rng = PyRandom::new(7);
        let mut v = Value::Int(0);
        for _ in 0..500 {
            v = mutate(&mut rng, &v, &ty, 0);
            let Value::Int(x) = v else { panic!() };
            assert!((-5..=5).contains(&x));
        }
    }

    #[test]
    fn generation_is_reproducible_from_the_seed() {
        let ty = fortis_types::types::named_args("List", vec![int_in(0, 100)]);
        let a = to_json(&generate(&mut PyRandom::new(case_seed(42, 3)), &ty, 0).unwrap());
        let b = to_json(&generate(&mut PyRandom::new(case_seed(42, 3)), &ty, 0).unwrap());
        assert_eq!(a, b);
    }
}
