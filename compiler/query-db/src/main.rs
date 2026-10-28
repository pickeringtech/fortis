//! The query database's evidence (DEL-46): `fortis-query-db equality|edits|corruption|cancel [FILES...]`, and
//! `records FILES...`: each module's records from the database as JSON lines, the surface DEL-43 compares the batch
//! commands with (PROG-DIA-07).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fortis_diag::Record;
use fortis_query_db::{Db, Key, Reason, Value};
use fortis_types::effects::Summary;
use fortis_types::incremental as inc;

fn lines(r: &[Record]) -> Vec<String> {
    r.iter().map(|x| x.to_json_line()).collect()
}

fn batch(file: &str, text: &str) -> Vec<String> {
    match fortis_syntax::parse_module(text) {
        Ok(p) => {
            let mut recs = fortis_types::check::check_parsed(&p, file);
            fortis_di::report(&mut recs, &[std::path::PathBuf::from(file)]);
            lines(&recs)
        }
        Err(_) => vec![],
    }
}

/// `equality`: the database's records for every file equal the batch check's.
fn equality(files: &[String]) -> bool {
    let mut db = Db::new();
    let mut differ = 0;
    let mut n = 0;
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        if fortis_syntax::parse_module(&text).is_err() {
            continue;
        }
        n += 1;
        let text = fortis_di::source_text(std::path::Path::new(f), text);
        db.set_source(f, &text);
        let mut recs = db.module(f).unwrap();
        fortis_di::report(&mut recs, &[std::path::PathBuf::from(f)]);
        let got = lines(&recs);
        if got != batch(f, &text) {
            differ += 1;
            println!("DIFFER {f}");
        }
    }
    println!("equality: {n} modules, {} answer as the batch check, {differ} differ", n - differ);
    differ == 0
}

/// The batch passes simulated unit by unit: for each unit and pass, the summaries it was given, and its output.
struct Reference {
    units: Vec<String>,
    texts: BTreeMap<String, String>,
    iface: String,
    given: BTreeMap<(String, u8), BTreeMap<String, String>>,
    out: BTreeMap<(String, u8), inc::UnitOut>,
}

fn sj(s: &Summary) -> String {
    inc::summary_json(s).to_line()
}

fn reference(file: &str, text: &str) -> Reference {
    let p = fortis_syntax::parse_module(text).expect("the edit script keeps the module parsing");
    let i = inc::interface(&p, file, "main");
    let t = inc::texts(&p, file, "main");
    let labels = inc::labels(&p, file, "main");
    let mut recs = i.declarations.clone();
    recs.extend(i.imports.clone());
    let iface = format!("{}|{:?}|{:?}|{}", lines(&recs).join("\n"), i.units.iter().map(|u| u.id()).collect::<Vec<_>>(), labels, t.skeleton);
    let mut state: BTreeMap<String, Summary> = BTreeMap::new();
    let mut given = BTreeMap::new();
    let mut out = BTreeMap::new();
    for pass in [1u8, 2] {
        for u in &i.units {
            given.insert((u.id(), pass), state.iter().map(|(k, v)| (k.clone(), sj(v))).collect());
            let o = inc::run_unit(&p, file, "main", u, &state, pass == 2);
            state.extend(o.produced.clone());
            out.insert((u.id(), pass), o);
        }
    }
    Reference { units: i.units.iter().map(|u| u.id()).collect(), texts: t.units, iface, given, out }
}

/// The base module of the edit script and the 50 edits, each a (find, replace) on the current text that keeps it
/// parsing; the kinds cycle: a body literal, a body that changes its summary, a type error in a body, whitespace
/// within a line, an added line, a signature, a test body, a class method, a reverted edit.
const BASE: &str = r#"import fortis.host { Host, Console }
export error Bad {
  code: "demo.bad"
}
func leaf(n: Int) Int {
  return Int.wrappingAdd(n, 1)
}
func twice(n: Int) Int {
  return leaf(leaf(n))
}
func noisy(console: Console, n: Int) Int {
  console.print("n ${n}")
  return n
}
func caller(console: Console, n: Int) Int {
  return noisy(console, twice(n))
}
try func checked(n: Int) Int {
  if n < 0 {
    fail Bad()
  }
  return n
}
recovers func useChecked(n: Int) Int {
  let v = checked(n) or recover 0
  return v
}
pure func square(n: Int) Int => Int.wrappingMul(n, n)
func total(xs: List[Int]) Int {
  let mut t = 0
  for x in xs {
    t = Int.wrappingAdd(t, square(x))
  }
  return t
}
export class Counter {
  export n: Int
  export pure func next(self) Int => Int.wrappingAdd(self.n, 1)
  export func bump(self, console: Console) Int {
    console.print("bump")
    return self.next()
  }
}
func useCounter(console: Console) Int {
  let c = Counter(n: 1)
  return c.bump(console)
}
test "leaf adds one" {
  expect leaf(1) == 2
}
export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  discard caller(caps.console, 3)
}
"#;

fn script() -> Vec<(&'static str, &'static str, &'static str)> {
    let kinds: Vec<(&str, &str, &str)> = vec![
        ("a literal in leaf's body", "return Int.wrappingAdd(n, 1)", "return Int.wrappingAdd(n, 7)"),
        ("leaf's body allocates (its summary changes)", "return Int.wrappingAdd(n, 7)", "return \"${n}\".scalarCount()"),
        ("leaf's body no longer allocates", "return \"${n}\".scalarCount()", "return Int.wrappingAdd(n, 1)"),
        ("a type error in square's body", "Int.wrappingMul(n, n)", "Int.wrappingMul(n, true)"),
        ("square repaired", "Int.wrappingMul(n, true)", "Int.wrappingMul(n, n)"),
        ("whitespace within a line of total", "    t = Int.wrappingAdd(t, square(x))", "    t = Int.wrappingAdd(t,   square(x))"),
        ("whitespace removed", "    t = Int.wrappingAdd(t,   square(x))", "    t = Int.wrappingAdd(t, square(x))"),
        ("a line added in noisy's body", "  console.print(\"n ${n}\")\n", "  console.print(\"n ${n}\")\n  console.print(\"again\")\n"),
        ("the line removed", "  console.print(\"n ${n}\")\n  console.print(\"again\")\n", "  console.print(\"n ${n}\")\n"),
        ("a signature: leaf takes a second parameter", "func leaf(n: Int) Int {", "func leaf(n: Int, m: Int) Int {"),
        ("the signature restored", "func leaf(n: Int, m: Int) Int {", "func leaf(n: Int) Int {"),
        ("a test's body", "expect leaf(1) == 2", "expect leaf(2) == 3"),
        ("the test restored", "expect leaf(2) == 3", "expect leaf(1) == 2"),
        ("a class method's body", "console.print(\"bump\")", "console.print(\"bumped\")"),
        ("the method restored", "console.print(\"bumped\")", "console.print(\"bump\")"),
        ("checked fails differently", "if n < 0 {", "if n < 10 {"),
        ("checked restored", "if n < 10 {", "if n < 0 {"),
        ("useChecked recovers a different value", "or recover 0", "or recover 1"),
        ("useChecked restored", "or recover 1", "or recover 0"),
        ("caller's body changes its callee", "return noisy(console, twice(n))", "return noisy(console, leaf(n))"),
        ("caller restored", "return noisy(console, leaf(n))", "return noisy(console, twice(n))"),
        ("main's body", "discard caller(caps.console, 3)", "discard caller(caps.console, 4)"),
        ("main restored", "discard caller(caps.console, 4)", "discard caller(caps.console, 3)"),
        ("twice calls leaf three times", "return leaf(leaf(n))", "return leaf(leaf(leaf(n)))"),
        ("twice restored", "return leaf(leaf(leaf(n)))", "return leaf(leaf(n))"),
    ];
    kinds.iter().chain(kinds.iter()).cloned().collect()
}

/// `edits`: 50 scripted edits; after each, the pass queries that run are exactly those the reference predicts
/// (their own text, the interface, or a summary they read changed), and every result equals a fresh check.
/// Every pass of every unit of a file, demanded from the database.
fn demand_all(db: &Db, file: &str) -> BTreeMap<(String, u8), fortis_query_db::UnitV> {
    let mut out = BTreeMap::new();
    if let Ok(Some(iface)) = db.interface(file) {
        for u in &iface.units {
            for p in [1u8, 2] {
                if let Ok(Value::Unit(v)) = db.get(&Key::Pass(file.into(), u.id(), p)) {
                    out.insert((u.id(), p), (*v).clone());
                }
            }
        }
    }
    out
}

fn edits() -> bool {
    let file = "edit-script.ft";
    let mut db = Db::new();
    let mut text = BASE.to_string();
    db.set_source(file, &text);
    db.module(file).unwrap();
    demand_all(&db, file);
    db.take_log();
    let mut prev = reference(file, &text);
    let (mut over, mut under, mut stale, mut ran_total, mut possible_total) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for (k, (what, find, replace)) in script().into_iter().enumerate() {
        assert!(text.contains(find), "edit {k} ({what}): text not found");
        text = text.replacen(find, replace, 1);
        db.set_source(file, &text);
        let got = lines(&db.module(file).unwrap());
        // every pass of every unit is demanded, so each one's value is checked, not only those the module needs
        let values = demand_all(&db, file);
        let ran: BTreeSet<(String, u8)> = db.take_log().into_iter().filter_map(|(key, _)| if let Key::Pass(_, u, p) = key { Some((u, p)) } else { None }).collect();
        let now = reference(file, &text);
        let mut expected = BTreeSet::new();
        for u in &now.units {
            for p in [1u8, 2] {
                let key = (u.clone(), p);
                let changed_text = prev.texts.get(u) != now.texts.get(u);
                let changed_iface = prev.iface != now.iface;
                let read_changed = match prev.out.get(&key) {
                    None => true,
                    Some(o) => o.requested.iter().any(|l| prev.given.get(&key).and_then(|g| g.get(l)) != now.given.get(&key).and_then(|g| g.get(l))),
                };
                if changed_text || changed_iface || read_changed {
                    expected.insert(key);
                }
            }
        }
        let o = ran.difference(&expected).count();
        let u = expected.difference(&ran).count();
        if o + u > 0 {
            println!("    ran only {:?}; expected only {:?}", ran.difference(&expected).collect::<Vec<_>>(), expected.difference(&ran).collect::<Vec<_>>());
        }
        // every unit pass's value equals the reference's
        for ((uid, p), v) in &values {
            let r = &now.out[&(uid.clone(), *p)];
            // the summaries a unit leaves: the reference gives each unit the whole state, the database only what
            // it asked for, so an entry equal to what the reference gave is no change
            let given = &now.given[&(uid.clone(), *p)];
            let effective: BTreeMap<String, String> = v.produced.iter().map(|(k, x)| (k.clone(), sj(x))).filter(|(k, x)| given.get(k) != Some(x)).collect();
            let same = lines(&v.records) == lines(&r.records)
                && v.requested == r.requested
                && effective == r.produced.iter().map(|(k, x)| (k.clone(), sj(x))).collect::<BTreeMap<_, _>>();
            if !same {
                stale += 1;
                println!("    STALE pass {p} of {uid}");
            }
        }
        over += o;
        under += u;
        // every result equals the fresh one
        let fresh = batch(file, &text);
        let ok_out = got == fresh;
        stale += !ok_out as usize;
        ran_total += ran.len();
        possible_total += 2 * now.units.len();
        println!("{:>2} {:<46} ran {:>2} of {:>2} unit passes; expected {:>2}; over {o}, under {u}; records {}", k + 1, what, ran.len(), 2 * now.units.len(), expected.len(), if ok_out { "equal" } else { "DIFFER" });
        prev = now;
    }
    println!("edits: 50; unit passes run {ran_total} of {possible_total}; over-invalidated {over}, under-invalidated {under}; results differing from a fresh check {stale}");
    over == 0 && under == 0 && stale == 0
}

/// `corruption`: ten corrupted caches; each corrupted entry is detected and recomputed, never served.
fn corruption(file: &str) -> bool {
    let text = std::fs::read_to_string(file).expect("a module");
    let mut base = Db::new();
    base.set_source(file, &text);
    let fresh = lines(&base.module(file).unwrap());
    let dir = std::env::temp_dir().join(format!("fortis-querydb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cache = dir.join("cache.jsonl");
    base.save(&cache).unwrap();
    let original = std::fs::read_to_string(&cache).unwrap();
    let entries: Vec<&str> = original.lines().skip(1).collect();
    let header = original.lines().next().unwrap().to_string();
    // a pass entry to corrupt (its value is a unit's result)
    let pick = entries.iter().position(|l| l.contains("[\"pass\"")).unwrap();
    let resum = |body: &str| -> String {
        let j = fortis_diag::Json::parse(body).unwrap();
        let line = j.to_line();
        fortis_diag::Json::obj([("entry", j), ("sum", fortis_diag::Json::str(&fortis_names::sha256::sha256(line.as_bytes())))]).to_line()
    };
    let body_of = |l: &str| -> String { fortis_diag::Json::parse(l).unwrap().get("entry").unwrap().to_line() };
    let with = |i: usize, new: String| -> String {
        let mut v: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
        v[i] = new;
        format!("{header}\n{}\n", v.join("\n"))
    };
    let target = entries[pick];
    let tbody = body_of(target);
    let mut cases: Vec<(&str, String, bool)> = vec![];
    // 1 a flipped character in a value
    let flipped = target.replacen("\"records\"", "\"recordz\"", 1);
    cases.push(("a flipped character in a value", with(pick, flipped), false));
    // 2 a truncated entry
    cases.push(("a truncated entry", with(pick, target[..target.len() / 2].to_string()), false));
    // 3 a wrong checksum
    let sum_at = target.rfind("\"sum\"").unwrap();
    cases.push(("a wrong checksum", with(pick, format!("{}\"sum\": \"{}\"}}", &target[..sum_at], "0".repeat(64))), false));
    // 4 a value changed under a recomputed checksum (it no longer has its fingerprint)
    let edit = |f: &dyn Fn(&mut fortis_diag::Json)| -> String {
        let mut j = fortis_diag::Json::parse(&tbody).unwrap();
        f(&mut j);
        j.to_line()
    };
    let changed_value = edit(&|j| {
        if let fortis_diag::Json::Obj(m) = j {
            if let Some(fortis_diag::Json::Obj(v)) = m.get_mut("value") {
                if let Some(fortis_diag::Json::Arr(r)) = v.get_mut("requested") {
                    r.push(fortis_diag::Json::str("function forged"));
                }
            }
        }
    });
    cases.push(("a changed value with a recomputed checksum", with(pick, resum(&changed_value)), false));
    // 5 an unknown key kind
    let unknown_key = edit(&|j| {
        if let fortis_diag::Json::Obj(m) = j {
            if let Some(fortis_diag::Json::Arr(k)) = m.get_mut("key") {
                k[0] = fortis_diag::Json::str("pazz");
            }
        }
    });
    cases.push(("an unknown key", with(pick, resum(&unknown_key)), false));
    // 6 an unreadable line
    cases.push(("an unreadable line", with(pick, "{broken".into()), false));
    // 7 a dependency fingerprint changed under a recomputed checksum: loads, fails verification, recomputed
    let dep_changed = edit(&|j| {
        if let fortis_diag::Json::Obj(m) = j {
            if let Some(fortis_diag::Json::Arr(d)) = m.get_mut("deps") {
                if let Some(fortis_diag::Json::Arr(first)) = d.first_mut() {
                    first[1] = fortis_diag::Json::str(&"e".repeat(64));
                }
            }
        }
    });
    cases.push(("a dependency fingerprint changed (recomputed checksum)", with(pick, resum(&dep_changed)), true));
    // 8 another schema
    cases.push(("another schema", format!("{{\"schema\": \"other/v9\"}}\n{}\n", entries.join("\n")), false));
    // 9 two entries for one key
    cases.push(("two entries for one key", format!("{header}\n{}\n{}\n", entries.join("\n"), resum(&changed_value)), false));
    // 10 a cache of an older source
    cases.push(("a cache saved for an older source", original.clone(), true));
    let mut ok_all = true;
    for (i, (what, cache_text, verified)) in cases.into_iter().enumerate() {
        std::fs::write(&cache, &cache_text).unwrap();
        let mut db = Db::new();
        let src = if i == 9 { text.replacen("\n", "\n\n", 1) } else { text.clone() };
        db.set_source(file, &src);
        db.load(&cache).unwrap();
        db.take_log();
        let got = lines(&db.module(file).unwrap());
        let want = if i == 9 { batch(file, &src) } else { fresh.clone() };
        let log = db.take_log();
        let detected = if verified { log.iter().any(|(_, r)| matches!(r, Reason::Stale(_))) } else { !db.dropped.is_empty() };
        let ok = got == want && detected;
        ok_all &= ok;
        println!("{:>2} {:<56} detected {:<5} recomputed {:>3} queries; result {}", i + 1, what, detected, log.len(), if got == want { "equals a fresh check" } else { "DIFFERS" });
    }
    let _ = std::fs::remove_dir_all(&dir);
    println!("corruption: 10 fixtures, {}", if ok_all { "every corrupted entry detected and recomputed, never served" } else { "FAILED" });
    ok_all
}

/// `cancel`: 20 runs; a running module check is cancelled at a random moment and returns within 50 ms; the
/// database stays consistent (the next check answers as a fresh one).
fn cancel(file: &str) -> bool {
    let text = std::fs::read_to_string(file).expect("a module");
    let want = batch(file, &text);
    let mut worst = Duration::ZERO;
    let mut ok_all = true;
    let mut seed: u64 = 0x5EED;
    // the full check's duration, to place the cancellations inside it
    let t0 = Instant::now();
    {
        let mut db = Db::new();
        db.set_source(file, &text);
        db.module(file).unwrap();
    }
    let full = t0.elapsed();
    for run in 0..20 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let delay = Duration::from_micros((seed >> 33) % (full.as_micros() as u64 * 8 / 10).max(1));
        let mut db = Db::new();
        db.set_source(file, &text);
        let flag = db.cancel_handle();
        let asked: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let a2 = asked.clone();
        let f2 = flag.clone();
        let timer = std::thread::spawn(move || {
            std::thread::sleep(delay);
            *a2.lock().unwrap() = Some(Instant::now());
            f2.store(true, Ordering::SeqCst);
        });
        let r = db.module(file);
        let returned = Instant::now();
        timer.join().unwrap();
        let latency = asked.lock().unwrap().map(|a| returned.saturating_duration_since(a)).unwrap_or(Duration::ZERO);
        let cancelled = r.is_err();
        flag.store(false, Ordering::SeqCst);
        let after = lines(&db.module(file).unwrap());
        let consistent = after == want;
        worst = worst.max(latency);
        let ok = latency <= Duration::from_millis(50) && consistent;
        ok_all &= ok;
        println!("{:>2} cancel after {:>6.1} ms: {} in {:>5.2} ms; then the check {}", run + 1, delay.as_secs_f64() * 1e3, if cancelled { "cancelled" } else { "had finished" }, latency.as_secs_f64() * 1e3, if consistent { "answers as a fresh one" } else { "DIFFERS" });
    }
    println!("cancel: 20 runs over a {:.0} ms check; worst latency {:.2} ms (budget 50 ms); {}", full.as_secs_f64() * 1e3, worst.as_secs_f64() * 1e3, if ok_all { "consistent after every cancellation" } else { "FAILED" });
    ok_all
}

/// `records`: one database, every file's records as JSON lines (a file that does not parse gives the syntax record
/// the batch check gives).
fn records(files: &[String]) -> bool {
    let mut db = Db::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        // a module is judged and held as the core reads it (the DI kit's elaboration), and every record, its
        // syntax record too, is reported as `check` reports it: a module whose elaboration failed by the kit's
        // findings alone (D-139)
        let src = fortis_di::source_text(std::path::Path::new(f), text);
        let judged = fortis_syntax::check::judge(f, &src);
        let mut recs = if !judged.ok {
            judged.records
        } else if judged.form != "module" {
            vec![]
        } else {
            db.set_source(f, &src);
            db.module(f).unwrap_or_default()
        };
        fortis_di::report(&mut recs, &[std::path::PathBuf::from(f)]);
        for r in recs {
            println!("{}", r.to_json_line());
        }
    }
    true
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ok = match args.first().map(|s| s.as_str()) {
        Some("records") => records(&args[1..]),
        Some("equality") => equality(&args[1..]),
        Some("edits") => edits(),
        Some("corruption") => corruption(&args[1]),
        Some("cancel") => cancel(&args[1]),
        _ => {
            eprintln!("usage: fortis-query-db equality FILE... | edits | corruption FILE | cancel FILE");
            std::process::exit(2);
        }
    };
    std::process::exit(if ok { 0 } else { 1 });
}
