//! The semantic query database (Programme 5.1, 13; PROG-CMP-01; DEL-46).
//!
//! The frontend's analyses of a module are queries over a database whose inputs are source texts. Each query is
//! keyed by stable IDs: a file, and a unit of the module's check, which is a declaration by name, an implement
//! block or a test (`fortis_types::typer::Unit`). A query's result is memoized with the fingerprint of every query
//! it read.
//!
//! - **Dependencies and invalidation.** A memo is reused while every query it read still has the fingerprint it
//!   had. A query re-runs only when one of them changed, and when its new result has the old fingerprint, the
//!   queries reading it do not re-run. A unit's check reads the module's interface (its declarations without
//!   bodies), its own text, and exactly the effect summaries it looked up. So after an edit to one function body,
//!   the queries that re-run are that unit's, plus those of units whose summaries changed.
//! - **Safe caching.** `save` writes every memo with a checksum, and `load` drops any entry that fails it or whose
//!   result no longer matches its fingerprint. A loaded entry is re-verified against the current sources before
//!   use, so a corrupted entry is recomputed, never served.
//! - **Cancellation.** A shared flag is checked at every query. A cancelled query stores nothing, so the database
//!   stays consistent.
//!
//! `module` gives exactly the batch check's records (`fortis_types::check::check_parsed`).

pub mod daemon;
pub mod alloc;
pub mod package;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fortis_diag::{Json, Record};
use fortis_names::sha256::sha256;
use fortis_syntax::Parsed;
use fortis_types::effects::Summary;
use fortis_types::incremental as inc;
use fortis_types::typer::Unit;

const MODULE: &str = "main";
pub const SCHEMA: &str = "fortis.querydb/v1";

/// A query: what it computes and for which stable IDs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Source(String),
    Parse(String),
    Resolve(String),
    Texts(String),
    Interface(String),
    UnitText(String, String),
    Pass(String, String, u8),
    /// one summary a unit's pass left (`file`, unit, pass, label): what another unit reads of it
    Summary(String, String, u8, String),
    Module(String),
}

impl Key {
    pub fn to_json(&self) -> Json {
        let s = |v: &str| Json::str(v);
        Json::Arr(match self {
            Key::Source(f) => vec![s("source"), s(f)],
            Key::Parse(f) => vec![s("parse"), s(f)],
            Key::Resolve(f) => vec![s("resolve"), s(f)],
            Key::Texts(f) => vec![s("texts"), s(f)],
            Key::Interface(f) => vec![s("interface"), s(f)],
            Key::UnitText(f, u) => vec![s("unit-text"), s(f), s(u)],
            Key::Pass(f, u, p) => vec![s("pass"), s(f), s(u), Json::Int(*p as i64)],
            Key::Summary(f, u, p, l) => vec![s("summary"), s(f), s(u), Json::Int(*p as i64), s(l)],
            Key::Module(f) => vec![s("module"), s(f)],
        })
    }

    pub fn from_json(j: &Json) -> Option<Key> {
        let Json::Arr(v) = j else { return None };
        let st = |i: usize| match v.get(i) {
            Some(Json::Str(x)) => Some(x.clone()),
            _ => None,
        };
        Some(match st(0)?.as_str() {
            "source" => Key::Source(st(1)?),
            "parse" => Key::Parse(st(1)?),
            "resolve" => Key::Resolve(st(1)?),
            "texts" => Key::Texts(st(1)?),
            "interface" => Key::Interface(st(1)?),
            "unit-text" => Key::UnitText(st(1)?, st(2)?),
            "pass" => match v.get(3) {
                Some(Json::Int(p)) if *p == 1 || *p == 2 => Key::Pass(st(1)?, st(2)?, *p as u8),
                _ => return None,
            },
            "summary" => match v.get(3) {
                Some(Json::Int(p)) if *p == 1 || *p == 2 => Key::Summary(st(1)?, st(2)?, *p as u8, st(4)?),
                _ => return None,
            },
            "module" => Key::Module(st(1)?),
            _ => return None,
        })
    }

    /// The kind of query, for logs and tests.
    pub fn kind(&self) -> &'static str {
        match self {
            Key::Source(_) => "source",
            Key::Parse(_) => "parse",
            Key::Resolve(_) => "resolve",
            Key::Texts(_) => "texts",
            Key::Interface(_) => "interface",
            Key::UnitText(..) => "unit-text",
            Key::Pass(..) => "pass",
            Key::Summary(..) => "summary",
            Key::Module(_) => "module",
        }
    }
}

/// A module's interface: the records before its units, the units in order, the summary labels each produces,
/// and its skeleton (declarations without bodies).
#[derive(Clone, Debug)]
pub struct Iface {
    pub records: Vec<Record>,
    pub units: Vec<Unit>,
    pub labels: BTreeMap<String, String>,
    pub skeleton: String,
}

/// One unit's check in one pass.
#[derive(Clone, Debug)]
pub struct UnitV {
    pub records: Vec<Record>,
    pub produced: BTreeMap<String, Summary>,
    pub requested: BTreeSet<String>,
}

#[derive(Clone)]
pub enum Value {
    Text(Rc<String>),
    Parsed(Option<Rc<Parsed>>, String),
    Records(Rc<Vec<Record>>),
    Texts(Rc<inc::Texts>),
    Interface(Rc<Iface>),
    Unit(Rc<UnitV>),
    Summary(Option<Rc<Summary>>),
}

fn unit_json(u: &Unit) -> Json {
    match u {
        Unit::Decl(n) => Json::Arr(vec![Json::str("decl"), Json::str(n)]),
        Unit::Implement(i, s) => Json::Arr(vec![Json::str("implement"), Json::Int(*i as i64), Json::str(s)]),
        Unit::Test(i, s) => Json::Arr(vec![Json::str("test"), Json::Int(*i as i64), Json::str(s)]),
    }
}

fn unit_from_json(j: &Json) -> Option<Unit> {
    let Json::Arr(v) = j else { return None };
    match (v.first()?, v.get(1), v.get(2)) {
        (Json::Str(k), Some(Json::Str(n)), None) if k == "decl" => Some(Unit::Decl(n.clone())),
        (Json::Str(k), Some(Json::Int(i)), Some(Json::Str(s))) if k == "implement" => Some(Unit::Implement(*i as usize, s.clone())),
        (Json::Str(k), Some(Json::Int(i)), Some(Json::Str(s))) if k == "test" => Some(Unit::Test(*i as usize, s.clone())),
        _ => None,
    }
}

fn records_json(r: &[Record]) -> Json {
    Json::Arr(r.iter().map(|x| x.to_json()).collect())
}

fn records_from(j: Option<&Json>) -> Option<Vec<Record>> {
    match j? {
        Json::Arr(v) => v.iter().map(Record::from_json).collect(),
        _ => None,
    }
}

fn strmap_json(m: &BTreeMap<String, String>) -> Json {
    Json::Obj(m.iter().map(|(k, v)| (k.clone(), Json::str(v))).collect())
}

impl Value {
    /// The canonical form: a persisted value, and the input of its fingerprint.
    pub fn to_json(&self) -> Json {
        match self {
            Value::Text(t) => Json::obj([("text", Json::str(t.as_str()))]),
            Value::Parsed(_, fp) => Json::obj([("parsed", Json::str(fp))]),
            Value::Records(r) => Json::obj([("records", records_json(r))]),
            Value::Texts(t) => Json::obj([("skeleton", Json::str(&t.skeleton)), ("units", strmap_json(&t.units))]),
            Value::Interface(i) => Json::obj([
                ("records", records_json(&i.records)),
                ("units", Json::Arr(i.units.iter().map(unit_json).collect())),
                ("labels", strmap_json(&i.labels)),
                ("skeleton", Json::str(&i.skeleton)),
            ]),
            Value::Unit(u) => Json::obj([
                ("records", records_json(&u.records)),
                ("produced", Json::Obj(u.produced.iter().map(|(k, s)| (k.clone(), inc::summary_json(s))).collect())),
                ("requested", Json::strs(u.requested.iter().cloned().collect::<Vec<String>>())),
            ]),
            Value::Summary(s) => Json::obj([("summary", s.as_ref().map_or(Json::Null, |x| inc::summary_json(x)))]),
        }
    }

    pub fn from_json(key: &Key, j: &Json) -> Option<Value> {
        let strmap = |k: &str| -> Option<BTreeMap<String, String>> {
            match j.get(k)? {
                Json::Obj(m) => m.iter().map(|(a, b)| if let Json::Str(s) = b { Some((a.clone(), s.clone())) } else { None }).collect(),
                _ => None,
            }
        };
        let s = |k: &str| match j.get(k) {
            Some(Json::Str(v)) => Some(v.clone()),
            _ => None,
        };
        Some(match key {
            Key::Source(_) | Key::UnitText(..) => Value::Text(Rc::new(s("text")?)),
            Key::Parse(_) => return None,
            Key::Resolve(_) | Key::Module(_) => Value::Records(Rc::new(records_from(j.get("records"))?)),
            Key::Texts(_) => Value::Texts(Rc::new(inc::Texts { skeleton: s("skeleton")?, units: strmap("units")? })),
            Key::Interface(_) => {
                let units = match j.get("units")? {
                    Json::Arr(v) => v.iter().map(unit_from_json).collect::<Option<Vec<_>>>()?,
                    _ => return None,
                };
                Value::Interface(Rc::new(Iface { records: records_from(j.get("records"))?, units, labels: strmap("labels")?, skeleton: s("skeleton")? }))
            }
            Key::Pass(..) => {
                let produced = match j.get("produced")? {
                    Json::Obj(m) => m.iter().map(|(k, v)| inc::summary_from_json(v).map(|x| (k.clone(), x))).collect::<Option<BTreeMap<_, _>>>()?,
                    _ => return None,
                };
                let requested = match j.get("requested")? {
                    Json::Arr(v) => v.iter().map(|x| if let Json::Str(s) = x { Some(s.clone()) } else { None }).collect::<Option<BTreeSet<_>>>()?,
                    _ => return None,
                };
                Value::Unit(Rc::new(UnitV { records: records_from(j.get("records"))?, produced, requested }))
            }
            Key::Summary(..) => match j.get("summary")? {
                Json::Null => Value::Summary(None),
                x => Value::Summary(Some(Rc::new(inc::summary_from_json(x)?))),
            },
        })
    }

    pub fn fingerprint(&self) -> String {
        match self {
            Value::Parsed(_, fp) => fp.clone(),
            v => sha256(v.to_json().to_line().as_bytes()),
        }
    }
}

/// A query stopped by its cancellation flag.
#[derive(Clone, Debug, PartialEq)]
pub struct Cancelled;

struct Memo {
    value: Value,
    fp: String,
    verified: u64,
    deps: Vec<(Key, String)>,
}

/// Why a query ran.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// no memo
    New,
    /// this dependency's fingerprint changed
    Stale(Key),
}

pub struct Db {
    rev: u64,
    sources: HashMap<String, (Rc<String>, String)>,
    memo: RefCell<HashMap<Key, Memo>>,
    frames: RefCell<Vec<Vec<(Key, String)>>>,
    /// every query run since the last `take_log`, with why
    log: RefCell<Vec<(Key, Reason)>>,
    cancel: Arc<AtomicBool>,
    /// cache entries `load` dropped, with why
    pub dropped: Vec<String>,
}

impl Default for Db {
    fn default() -> Self {
        Self::new()
    }
}

impl Db {
    pub fn new() -> Db {
        Db { rev: 1, sources: HashMap::new(), memo: RefCell::new(HashMap::new()), frames: RefCell::new(vec![]), log: RefCell::new(vec![]), cancel: Arc::new(AtomicBool::new(false)), dropped: vec![] }
    }

    /// Set a file's text; a new revision when it changed.
    pub fn set_source(&mut self, file: &str, text: &str) {
        let fp = sha256(text.as_bytes());
        if self.sources.get(file).map_or(true, |(_, f)| *f != fp) {
            self.sources.insert(file.to_string(), (Rc::new(text.to_string()), fp));
            self.rev += 1;
        }
    }

    /// The flag that cancels running queries (set it from any thread).
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn take_log(&self) -> Vec<(Key, Reason)> {
        std::mem::take(&mut self.log.borrow_mut())
    }

    pub fn memo_count(&self) -> usize {
        self.memo.borrow().len()
    }

    /// A query's value, recorded as a dependency of the query that asks.
    pub fn get(&self, key: &Key) -> Result<Value, Cancelled> {
        let (v, fp) = self.ensure(key)?;
        if let Some(top) = self.frames.borrow_mut().last_mut() {
            top.push((key.clone(), fp));
        }
        Ok(v)
    }

    /// A query's value without recording a dependency (a unit's check reads the parsed module through it; what it
    /// depends on is the interface, its own text and its summaries, which it reads with `get`).
    fn peek(&self, key: &Key) -> Result<Value, Cancelled> {
        self.ensure(key).map(|x| x.0)
    }

    fn ensure(&self, key: &Key) -> Result<(Value, String), Cancelled> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        if let Key::Source(f) = key {
            let (t, fp) = self.sources.get(f).cloned().unwrap_or_else(|| (Rc::new(String::new()), sha256(b"")));
            return Ok((Value::Text(t), fp));
        }
        let cached = self.memo.borrow().get(key).map(|m| (m.verified, m.deps.clone(), m.value.clone(), m.fp.clone()));
        let mut reason = Reason::New;
        if let Some((verified, deps, value, fp)) = cached {
            if verified == self.rev {
                return Ok((value, fp));
            }
            let mut stale = None;
            for (d, dfp) in &deps {
                let (_, now) = self.ensure(d)?;
                if now != *dfp {
                    stale = Some(d.clone());
                    break;
                }
            }
            match stale {
                None => {
                    if let Some(m) = self.memo.borrow_mut().get_mut(key) {
                        m.verified = self.rev;
                    }
                    return Ok((value, fp));
                }
                Some(d) => reason = Reason::Stale(d),
            }
        }
        self.frames.borrow_mut().push(vec![]);
        let result = self.compute(key);
        let deps = self.frames.borrow_mut().pop().unwrap_or_default();
        let value = result?;
        let fp = value.fingerprint();
        self.memo.borrow_mut().insert(key.clone(), Memo { value: value.clone(), fp: fp.clone(), verified: self.rev, deps });
        self.log.borrow_mut().push((key.clone(), reason));
        Ok((value, fp))
    }

    fn parsed(&self, file: &str, tracked: bool) -> Result<Option<Rc<Parsed>>, Cancelled> {
        let k = Key::Parse(file.into());
        match if tracked { self.get(&k)? } else { self.peek(&k)? } {
            Value::Parsed(p, _) => Ok(p),
            _ => Ok(None),
        }
    }

    pub fn interface(&self, file: &str) -> Result<Option<Rc<Iface>>, Cancelled> {
        match self.get(&Key::Interface(file.into()))? {
            Value::Interface(i) => Ok(Some(i)),
            _ => Ok(None),
        }
    }

    fn pass(&self, file: &str, uid: &str, p: u8) -> Result<Rc<UnitV>, Cancelled> {
        match self.get(&Key::Pass(file.into(), uid.into(), p))? {
            Value::Unit(u) => Ok(u),
            _ => Ok(Rc::new(UnitV { records: vec![], produced: BTreeMap::new(), requested: BTreeSet::new() })),
        }
    }

    /// One summary a unit's pass left, through its own query (so a reader depends on that summary alone).
    fn summary(&self, file: &str, uid: &str, p: u8, label: &str) -> Result<Option<Summary>, Cancelled> {
        match self.get(&Key::Summary(file.into(), uid.into(), p, label.into()))? {
            Value::Summary(s) => Ok(s.map(|x| (*x).clone())),
            _ => Ok(None),
        }
    }

    /// The summary a unit at `idx` sees for `label` in pass `p`, as the batch check's state would hold it.
    fn provider(&self, file: &str, iface: &Iface, idx: usize, label: &str, p: u8) -> Result<Option<Summary>, Cancelled> {
        let ids: Vec<String> = iface.units.iter().map(|u| u.id()).collect();
        if label == "closure" {
            for j in (0..idx).rev() {
                if let Some(s) = self.summary(file, &ids[j], p, "closure")? {
                    return Ok(Some(s));
                }
            }
            if p == 2 {
                for j in (0..ids.len()).rev() {
                    if let Some(s) = self.summary(file, &ids[j], 1, "closure")? {
                        return Ok(Some(s));
                    }
                }
            }
            return Ok(None);
        }
        let Some(owner) = iface.labels.get(label) else { return Ok(None) };
        let Some(o) = ids.iter().position(|x| x == owner) else { return Ok(None) };
        if p == 1 {
            if o < idx {
                return self.summary(file, owner, 1, label);
            }
            return Ok(None);
        }
        if o < idx {
            if let Some(s) = self.summary(file, owner, 2, label)? {
                return Ok(Some(s));
            }
        }
        self.summary(file, owner, 1, label)
    }

    fn compute(&self, key: &Key) -> Result<Value, Cancelled> {
        match key {
            Key::Source(_) => unreachable!(),
            Key::Parse(f) => {
                let Value::Text(t) = self.get(&Key::Source(f.clone()))? else { unreachable!() };
                let parsed = fortis_syntax::parse_module(&t).ok().map(Rc::new);
                Ok(Value::Parsed(parsed, sha256(t.as_bytes())))
            }
            Key::Resolve(f) => {
                let recs = match self.parsed(f, true)? {
                    Some(p) => inc::interface(&p, f, MODULE).resolver,
                    None => vec![],
                };
                Ok(Value::Records(Rc::new(recs)))
            }
            Key::Texts(f) => {
                let t = match self.parsed(f, true)? {
                    Some(p) => inc::texts(&p, f, MODULE),
                    None => inc::Texts { skeleton: String::new(), units: BTreeMap::new() },
                };
                Ok(Value::Texts(Rc::new(t)))
            }
            Key::Interface(f) => {
                let parsed = self.parsed(f, true)?;
                let Value::Texts(t) = self.get(&Key::Texts(f.clone()))? else { unreachable!() };
                let iface = match parsed {
                    Some(p) => {
                        let i = inc::interface(&p, f, MODULE);
                        let mut records = i.declarations;
                        records.extend(i.imports);
                        Iface { records, units: i.units, labels: inc::labels(&p, f, MODULE), skeleton: t.skeleton.clone() }
                    }
                    None => Iface { records: vec![], units: vec![], labels: BTreeMap::new(), skeleton: String::new() },
                };
                Ok(Value::Interface(Rc::new(iface)))
            }
            Key::UnitText(f, u) => {
                let Value::Texts(t) = self.get(&Key::Texts(f.clone()))? else { unreachable!() };
                Ok(Value::Text(Rc::new(t.units.get(u).cloned().unwrap_or_default())))
            }
            Key::Pass(f, uid, p) => {
                let Some(iface) = self.interface(f)? else { return Ok(Value::Unit(Rc::new(UnitV { records: vec![], produced: BTreeMap::new(), requested: BTreeSet::new() }))) };
                self.get(&Key::UnitText(f.clone(), uid.clone()))?;
                let Some(parsed) = self.parsed(f, false)? else { return Ok(Value::Unit(Rc::new(UnitV { records: vec![], produced: BTreeMap::new(), requested: BTreeSet::new() }))) };
                let idx = iface.units.iter().position(|u| u.id() == *uid).unwrap_or(0);
                let unit = iface.units[idx].clone();
                let mut given: BTreeMap<String, Summary> = BTreeMap::new();
                let mut asked: BTreeSet<String> = BTreeSet::new();
                loop {
                    let out = inc::run_unit(&parsed, f, MODULE, &unit, &given, *p == 2);
                    let mut added = false;
                    for label in out.requested.iter() {
                        if asked.insert(label.clone()) {
                            if let Some(s) = self.provider(f, &iface, idx, label, *p)? {
                                given.insert(label.clone(), s);
                                added = true;
                            }
                        }
                    }
                    if !added {
                        return Ok(Value::Unit(Rc::new(UnitV { records: out.records, produced: out.produced, requested: out.requested })));
                    }
                }
            }
            Key::Summary(f, uid, p, label) => Ok(Value::Summary(self.pass(f, uid, *p)?.produced.get(label).cloned().map(Rc::new))),
            Key::Module(f) => {
                let Value::Records(resolve) = self.get(&Key::Resolve(f.clone()))? else { unreachable!() };
                let mut out = (*resolve).clone();
                if let Some(iface) = self.interface(f)? {
                    let mut body = iface.records.clone();
                    for u in &iface.units {
                        body.extend(self.pass(f, &u.id(), 2)?.records.iter().cloned());
                    }
                    out.extend(inc::dedupe(body));
                }
                Ok(Value::Records(Rc::new(out)))
            }
        }
    }

    /// A module's records, exactly as the batch check gives them.
    pub fn module(&self, file: &str) -> Result<Vec<Record>, Cancelled> {
        match self.get(&Key::Module(file.into()))? {
            Value::Records(r) => Ok((*r).clone()),
            _ => Ok(vec![]),
        }
    }

    /// Every memo, with a checksum per entry, for a later process to `load`.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut out = format!("{}\n", Json::obj([("schema", Json::str(SCHEMA))]).to_line());
        let memo = self.memo.borrow();
        let mut keys: Vec<&Key> = memo.keys().filter(|k| !matches!(k, Key::Parse(_))).collect();
        keys.sort();
        for k in keys {
            let m = &memo[k];
            let body = Json::obj([
                ("key", k.to_json()),
                ("fp", Json::str(&m.fp)),
                ("deps", Json::Arr(m.deps.iter().map(|(d, f)| Json::Arr(vec![d.to_json(), Json::str(f)])).collect())),
                ("value", m.value.to_json()),
            ]);
            let line = body.to_line();
            out.push_str(&Json::obj([("entry", body), ("sum", Json::str(&sha256(line.as_bytes())))]).to_line());
            out.push('\n');
        }
        std::fs::write(path, out)
    }

    /// Read a saved cache. An entry whose checksum fails, whose key or value cannot be read, or whose value no
    /// longer has its fingerprint is dropped (and recomputed when asked for); a cache of another schema is dropped
    /// whole. Loaded entries are re-verified against the current sources before use.
    pub fn load(&mut self, path: &std::path::Path) -> std::io::Result<usize> {
        // the quota of every input parser (PROG-SEC-01): a cache over it is dropped whole, unread
        if std::fs::metadata(path)?.len() > fortis_diag::MAX_BYTES as u64 {
            self.dropped.push("the whole cache: over the budget of the JSON quota".into());
            return Ok(0);
        }
        let text = std::fs::read_to_string(path)?;
        let mut lines = text.lines();
        let header_ok = lines.next().and_then(|h| Json::parse(h).ok()).map_or(false, |h| h.get("schema") == Some(&Json::str(SCHEMA)));
        if !header_ok {
            self.dropped.push("the whole cache: not a fortis.querydb/v1 cache".into());
            return Ok(0);
        }
        let mut loaded: HashMap<Key, Memo> = HashMap::new();
        let mut conflicting: BTreeSet<Key> = BTreeSet::new();
        for (n, line) in lines.enumerate() {
            let entry = (|| -> Result<(Key, Memo), String> {
                let j = Json::parse(line).map_err(|e| format!("unreadable: {e}"))?;
                let body = j.get("entry").ok_or("no entry")?;
                let sum = match j.get("sum") {
                    Some(Json::Str(s)) => s.clone(),
                    _ => return Err("no checksum".into()),
                };
                if sha256(body.to_line().as_bytes()) != sum {
                    return Err("checksum mismatch".into());
                }
                let key = Key::from_json(body.get("key").ok_or("no key")?).ok_or("unknown key")?;
                let fp = match body.get("fp") {
                    Some(Json::Str(s)) => s.clone(),
                    _ => return Err("no fingerprint".into()),
                };
                let value = Value::from_json(&key, body.get("value").ok_or("no value")?).ok_or("unreadable value")?;
                if value.fingerprint() != fp {
                    return Err("the value does not have its fingerprint".into());
                }
                let deps = match body.get("deps") {
                    Some(Json::Arr(v)) => v
                        .iter()
                        .map(|d| match d {
                            Json::Arr(p) => match (p.first().and_then(Key::from_json), p.get(1)) {
                                (Some(k), Some(Json::Str(f))) => Some((k, f.clone())),
                                _ => None,
                            },
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()
                        .ok_or("unreadable dependencies")?,
                    _ => return Err("no dependencies".into()),
                };
                Ok((key, Memo { value, fp, verified: 0, deps }))
            })();
            match entry {
                Ok((k, m)) => {
                    if loaded.contains_key(&k) {
                        conflicting.insert(k.clone());
                        self.dropped.push(format!("entry {}: a second entry for {:?}", n + 1, k));
                    } else {
                        loaded.insert(k, m);
                    }
                }
                Err(e) => self.dropped.push(format!("entry {}: {e}", n + 1)),
            }
        }
        for k in conflicting {
            loaded.remove(&k);
        }
        let n = loaded.len();
        self.memo.borrow_mut().extend(loaded);
        self.rev += 1;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "func leaf(n: Int) Int {\n  return Int.wrappingAdd(n, 1)\n}\nfunc twice(n: Int) Int {\n  return leaf(leaf(n))\n}\n";

    #[test]
    fn keys_round_trip() {
        for k in [Key::Source("a".into()), Key::Pass("a".into(), "decl f".into(), 2), Key::Summary("a".into(), "decl f".into(), 1, "function f".into()), Key::Module("a".into())] {
            assert_eq!(Key::from_json(&k.to_json()), Some(k));
        }
    }

    #[test]
    fn a_body_edit_reruns_only_its_unit() {
        let mut db = Db::new();
        db.set_source("m.ft", SRC);
        let first = db.module("m.ft").unwrap();
        db.take_log();
        db.set_source("m.ft", &SRC.replace("wrappingAdd(n, 1)", "wrappingAdd(n, 2)"));
        let again = db.module("m.ft").unwrap();
        let passes: Vec<String> = db.take_log().into_iter().filter_map(|(k, _)| if let Key::Pass(_, u, _) = k { Some(u) } else { None }).collect();
        assert_eq!(passes, vec!["decl leaf"]);
        assert_eq!(first.len(), again.len());
    }

    #[test]
    fn values_round_trip_through_a_saved_cache() {
        let mut db = Db::new();
        db.set_source("m.ft", SRC);
        let want = db.module("m.ft").unwrap();
        let path = std::env::temp_dir().join(format!("qdb-test-{}.jsonl", std::process::id()));
        db.save(&path).unwrap();
        let mut db2 = Db::new();
        db2.set_source("m.ft", SRC);
        assert!(db2.load(&path).unwrap() > 0);
        db2.take_log();
        assert_eq!(db2.module("m.ft").unwrap().len(), want.len());
        assert!(db2.take_log().iter().all(|(k, _)| matches!(k, Key::Parse(_))), "only the parse, which is never persisted, is recomputed");
        let _ = std::fs::remove_file(path);
    }
}
