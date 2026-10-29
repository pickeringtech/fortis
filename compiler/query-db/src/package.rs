//! The package query database (SP-12's design, section 3): DEL-46's queries for every module of a package,
//! with dependencies across modules on single declarations, the package resolver as a query, dirty propagation,
//! collection, and one checker context per request.
//!
//! - **Across modules** a unit's pass reads `Foreign(g, n)`, the head of one declaration of another module, for every
//!   pair of its foreign closure (design 3.2): heads carry no positions, so a body edit reruns nothing outside its
//!   module, and a signature edit reruns the passes that name the declaration.
//! - **Dirty propagation.** Every memo knows its dependents. Setting a source marks everything that can reach it as
//!   possibly stale; a request verifies only those, and trusts the rest.
//! - **Collection.** `Package` and every `Module(f)` are roots; a memo no other memo depends on is dropped, so the
//!   live memos are those the roots reach.
//! - **The checker context.** A request builds one `Session` over the package's `Libraries`; each module whose queries
//!   run gets one checker after `load` and `check`, and each unit's run starts a fresh `Typer` from a clone of it.
//!
//! `check` answers with exactly the records of `fortisc check DIR` (fortis_types::check::check_package_dir and the
//! Dependency injection kit's report).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use fortis_diag::{Json, Record};
use fortis_names::resolve::{rel, PackageSet};
use fortis_names::sha256::sha256;
use fortis_names::{Libraries, Library, LibraryModule, Resolver};
use fortis_syntax::lexer::Kind;
use fortis_syntax::view::{C, N};
use fortis_syntax::Parsed;
use fortis_types::decls::{DeclChecker, Session};
use fortis_types::effects::Summary;
use fortis_types::incremental as inc;
use fortis_types::typer::{Typer, Unit};

/// A value's fingerprint: the SHA-256 of its canonical form.
pub type Fp = [u8; 32];

fn fp(data: &[u8]) -> Fp {
    let hex = sha256(data);
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap_or(0);
    }
    out
}

/// The fault switches of the apparatus check (SP-12's design, section 10); never in a default build.
#[cfg(feature = "faults")]
pub fn fault(name: &str) -> bool {
    std::env::var("FORTIS_QDB_FAULT").map_or(false, |v| v.split(',').any(|x| x == name))
}

#[cfg(not(feature = "faults"))]
#[inline(always)]
pub fn fault(_: &str) -> bool {
    false
}

/// A module, by its index in the package's module order (the batch check's order).
pub type F = u32;

/// A query of the package database.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum K {
    /// a module's text (an input)
    Source(F),
    /// the import cycles a module's import closure meets, with the module whose load enters each first in the
    /// batch check's order (an input: design 3.4)
    CyclePlan(F),
    /// each declaration's head without positions, the imports and the `implement` heads
    Heads(F),
    /// the declarations' names, kinds, export marks and members, and the imports: what the name resolver and the
    /// package resolver read of a module
    Shape(F),
    /// the names a module exports: what a candidate list (E-NAM-004) reads
    Exports(F),
    /// one declaration's head, or none
    Foreign(F, Rc<str>),
    /// a module's `implement` heads and the modules it imports
    ImplOf(F),
    /// the `implement` heads of every module a module imports, transitively
    Implements(F),
    /// the package resolver's import checks of one module
    ResolveMod(F),
    /// the package resolver's records
    Resolver,
    /// the name resolver's records of a module
    Names(F),
    Texts(F),
    Interface(F),
    UnitText(F, Rc<str>),
    Pass(F, Rc<str>, u8),
    Summary(F, Rc<str>, u8, Rc<str>),
    Module(F),
    /// the package's records: the answer
    Package,
}

impl K {
    pub fn kind(&self) -> &'static str {
        match self {
            K::Source(_) => "source",
            K::CyclePlan(_) => "cycle-plan",
            K::Heads(_) => "heads",
            K::Shape(_) => "shape",
            K::Exports(_) => "exports",
            K::Foreign(..) => "foreign",
            K::ImplOf(_) => "impl-of",
            K::Implements(_) => "implements",
            K::ResolveMod(_) => "resolve-module",
            K::Resolver => "resolver",
            K::Names(_) => "names",
            K::Texts(_) => "texts",
            K::Interface(_) => "interface",
            K::UnitText(..) => "unit-text",
            K::Pass(..) => "pass",
            K::Summary(..) => "summary",
            K::Module(_) => "module",
            K::Package => "package",
        }
    }

    fn is_root(&self) -> bool {
        matches!(self, K::Package | K::Module(_))
    }
}

/// One declaration's head: its tokens outside bodies, without positions, and the identifiers in it with the
/// modules that provide each through the declaring module's imports.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub text: String,
    pub refs: Vec<(String, Vec<F>)>,
    /// the aliases of the declaring module the head mentions, with the modules they name
    pub aliases_mentioned: Vec<(String, F)>,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: String,
    pub target: Option<F>,
    pub alias: Option<String>,
    pub members: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct HeadsV {
    /// declaration name -> (kind, exported, members, head, the head's identifiers), in source order for a name
    /// declared twice
    pub decls: BTreeMap<String, Vec<(String, bool, Vec<String>, String, Vec<String>)>>,
    pub imports: Vec<Import>,
    pub implements: String,
    /// identifier -> the modules a member import provides it from
    pub providers: BTreeMap<String, Vec<F>>,
    /// alias -> the module an alias import names
    pub aliases: BTreeMap<String, F>,
}

/// A unit's text with its positions, and its identifiers.
#[derive(Clone, Debug)]
pub struct UnitTextV {
    pub text: String,
    pub idents: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Iface {
    pub records: Vec<Record>,
    pub units: Vec<Unit>,
    pub labels: BTreeMap<String, String>,
    pub skeleton: String,
    pub skeleton_idents: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct UnitV {
    pub records: Vec<Record>,
    pub produced: BTreeMap<String, Summary>,
    pub requested: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub struct TextsV {
    pub texts: inc::Texts,
    pub idents: BTreeMap<String, Vec<String>>,
    pub skeleton_idents: Vec<String>,
}

#[derive(Clone)]
pub enum V {
    Heads(Rc<HeadsV>),
    Json(Rc<Json>),
    Head(Option<Rc<Head>>),
    Text(Rc<String>),
    Records(Rc<Vec<Record>>),
    Texts(Rc<TextsV>),
    UnitText(Rc<UnitTextV>),
    Interface(Rc<Iface>),
    Unit(Rc<UnitV>),
    Summary(Option<Rc<Summary>>),
}

fn records_json(r: &[Record]) -> Json {
    Json::Arr(r.iter().map(|x| x.to_json()).collect())
}

fn strs(v: &[String]) -> Json {
    Json::strs(v.to_vec())
}

impl V {
    /// The canonical form, the input of the fingerprint.
    fn canonical(&self) -> String {
        let j = match self {
            V::Heads(h) => Json::obj([
                (
                    "decls",
                    Json::Obj(h.decls.iter().map(|(n, ds)| (n.clone(), Json::Arr(ds.iter().map(|(k, e, m, t, _)| Json::Arr(vec![Json::str(k), Json::Bool(*e), strs(m), Json::str(t)])).collect()))).collect()),
                ),
                (
                    "imports",
                    Json::Arr(
                        h.imports
                            .iter()
                            .map(|i| {
                                Json::Arr(vec![
                                    Json::str(&i.path),
                                    i.target.map_or(Json::Null, |t| Json::Int(t as i64)),
                                    i.alias.as_ref().map_or(Json::Null, |a| Json::str(a)),
                                    i.members.as_ref().map_or(Json::Null, |m| strs(m)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("implements", Json::str(&h.implements)),
            ]),
            V::Json(j) => (**j).clone(),
            V::Head(h) => match h {
                None => Json::Null,
                Some(h) => Json::obj([
                    ("text", Json::str(&h.text)),
                    ("refs", Json::Arr(h.refs.iter().map(|(x, p)| Json::Arr(vec![Json::str(x), Json::Arr(p.iter().map(|f| Json::Int(*f as i64)).collect())])).collect())),
                    ("aliases", Json::Arr(h.aliases_mentioned.iter().map(|(a, t)| Json::Arr(vec![Json::str(a), Json::Int(*t as i64)])).collect())),
                ]),
            },
            V::Text(t) => Json::str(t.as_str()),
            V::Records(r) => records_json(r),
            V::Texts(t) => Json::obj([
                ("skeleton", Json::str(&t.texts.skeleton)),
                ("units", Json::Obj(t.texts.units.iter().map(|(k, v)| (k.clone(), Json::str(v))).collect())),
                ("idents", Json::Obj(t.idents.iter().map(|(k, v)| (k.clone(), strs(v))).collect())),
                ("skeleton_idents", strs(&t.skeleton_idents)),
            ]),
            V::UnitText(u) => Json::obj([("text", Json::str(&u.text)), ("idents", strs(&u.idents))]),
            V::Interface(i) => Json::obj([
                ("records", records_json(&i.records)),
                ("units", Json::strs(i.units.iter().map(|u| u.id()).collect::<Vec<String>>())),
                ("labels", Json::Obj(i.labels.iter().map(|(k, v)| (k.clone(), Json::str(v))).collect())),
                ("skeleton", Json::str(&i.skeleton)),
                ("skeleton_idents", strs(&i.skeleton_idents)),
            ]),
            V::Unit(u) => Json::obj([
                ("records", records_json(&u.records)),
                ("produced", Json::Obj(u.produced.iter().map(|(k, s)| (k.clone(), inc::summary_json(s))).collect())),
                ("requested", Json::strs(u.requested.iter().cloned().collect::<Vec<String>>())),
            ]),
            V::Summary(s) => s.as_ref().map_or(Json::Null, |x| inc::summary_json(x)),
        };
        j.to_line()
    }
}

/// Why a query ran.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    New,
    Stale(K),
}

/// A stopped request (the cancellation flag; unused by the daemon, kept for DEL-46's contract).
#[derive(Clone, Debug, PartialEq)]
pub struct Cancelled;

struct Memo {
    value: V,
    fp: Fp,
    deps: Vec<(K, Fp)>,
}

struct Mod {
    key: String,
    path: String,
    file: PathBuf,
    rel: String,
    text: Rc<String>,
    fp: Fp,
    parsed: Option<Arc<Parsed>>,
}

/// A module's checker after `load` and `check`, for one request.
struct Prep<'a> {
    dc: DeclChecker<'a>,
    tree: N<'a>,
}

/// What a request shares: the session over the package's libraries, and the prepared checkers.
struct Cx<'a> {
    sess: &'a Session<'a>,
    libs: &'a Libraries,
    preps: RefCell<HashMap<F, Rc<Prep<'a>>>>,
}

/// What one request did.
#[derive(Default, Clone, Debug)]
pub struct Ran {
    /// queries that ran, by kind
    pub by_kind: BTreeMap<String, usize>,
    /// every pass that ran: (module, unit, pass)
    pub passes: Vec<(F, String, u8)>,
    /// modules whose checker context was built
    pub prepared: usize,
}

pub struct Pkg {
    pub dir: PathBuf,
    identity: String,
    /// the root module, and the package name it is also known by
    root: Option<(F, String)>,
    mods: Vec<Mod>,
    by_key: HashMap<String, F>,
    ps: RefCell<PackageSet>,
    libs: Libraries,
    memo: RefCell<HashMap<K, Memo>>,
    rdeps: RefCell<HashMap<K, HashSet<K>>>,
    dirty: RefCell<HashSet<K>>,
    frames: RefCell<Vec<Vec<(K, Fp)>>>,
    /// the keys being computed, innermost last
    stack: RefCell<Vec<K>>,
    /// memos that lost their last dependent during the request, collected when it ends
    orphans: RefCell<Vec<K>>,
    log: RefCell<Vec<(K, Reason)>>,
    prepared: Cell<usize>,
    /// each module's imports (targets in import order), from its parse
    graph: Vec<Vec<F>>,
    /// each module's cycle plan and its fingerprint
    plans: Vec<(Vec<(F, Vec<F>)>, Fp)>,
    /// the modules whose main load the batch check makes first into each import cycle, in package order
    entries: Vec<F>,
    /// faults `leak-text` keeps every replaced text here
    leaked: Vec<Rc<String>>,
    /// fault `reply-stale`: the previous answer
    previous: RefCell<Option<Rc<Vec<Record>>>>,
    requests: Cell<u64>,
    changed: Cell<u64>,
    /// the string parts of the keys, one allocation per distinct string (X-141)
    strs: RefCell<HashSet<Rc<str>>>,
    /// the module key of the program's entry module (the manifest's `entry`, 4.6), whose `main` alone creates the
    /// capabilities (11.6); the manifest is read once, when the package is opened
    program_entry: Option<String>,
}

/// Keep a table at the room its length needs, so its memory is a function of its contents (X-141). `grew` says
/// whether the change that called it was an insertion. `shrink_to_fit` keeps a table whose buckets are already the
/// fewest its length needs, and costs nothing then; `capacity()` cannot decide it, because tombstones lower it below
/// what the buckets hold.
fn fit<T: std::hash::Hash + Eq>(s: &mut HashSet<T>, grew: bool) {
    // fault `fit-loose`: a set an insertion changed keeps room for twice its length, one a removal changed is
    // shrunk, so its capacity depends on its last change rather than on its contents
    if fault("fit-loose") {
        if !grew {
            s.shrink_to_fit();
        } else if s.capacity() < 2 * s.len() {
            s.reserve(s.len());
        }
        return;
    }
    s.shrink_to_fit();
}

fn idents_of(n: N) -> Vec<String> {
    let mut out = vec![];
    for t in n.tokens() {
        token_idents(t, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// A token's identifiers: itself, or the identifiers inside its interpolation holes (whose parses it keeps),
/// recursively (apparatus review 2).
fn token_idents(t: &fortis_syntax::lexer::Token, out: &mut Vec<String>) {
    if matches!(t.kind, Kind::Lower | Kind::Upper) {
        out.push(t.text.clone());
    }
    if fault("hole-idents") {
        return;
    }
    for (k, (src, _, _)) in t.interps.iter().enumerate() {
        if let Some(p) = fortis_types::typer::expr::hole_tree_at(t, k, src) {
            for u in &p.toks {
                token_idents(u, out);
            }
        }
    }
}

/// A declaration's tokens outside bodies (inc::Texts' skeleton, per declaration and without positions).
fn head_text(n: N, out: &mut String, idents: &mut BTreeSet<String>) {
    for c in n.kids() {
        match c {
            C::T(t) => {
                if !matches!(t.kind, Kind::Nl | Kind::Newline) {
                    out.push_str(&t.text);
                    out.push(' ');
                    let mut ids = vec![];
                    token_idents(t, &mut ids);
                    idents.extend(ids);
                }
            }
            C::N(m) => {
                let body = matches!(m.name(), "funcBody" | "cleanupDecl") || (m.name() == "block" && matches!(n.name(), "testDecl" | "propertyDecl"));
                if body {
                    out.push_str("{…} ");
                } else {
                    head_text(m, out, idents);
                }
            }
        }
    }
}

fn skeleton_idents(mtree: N) -> Vec<String> {
    let mut s = String::new();
    let mut ids = BTreeSet::new();
    head_text(mtree, &mut s, &mut ids);
    ids.into_iter().collect()
}

/// Parse every interpolation hole of a module now, rather than when a check first reaches it: a token keeps its
/// holes' parses (D-32), so a module's memory is then a function of its text, not of which units ran since it was
/// parsed (the soak's growth check compares the heap at states that are equal, design 7).
fn settle_holes(parsed: &Parsed) {
    for tk in &parsed.toks {
        for (k, (src, _, _)) in tk.interps.iter().enumerate() {
            let _ = fortis_types::typer::expr::hole_tree_at(tk, k, src);
        }
    }
}

fn read_status(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with(field)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

#[repr(C)]
struct Mallinfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

extern "C" {
    fn mallinfo2() -> Mallinfo2;
}

/// The bytes the allocator has handed out and not had back (glibc's `mallinfo2`: allocated chunks plus mapped
/// blocks): the retained-allocation count of the soak's growth check (design 7, G2).
pub fn heap() -> u64 {
    // SAFETY: mallinfo2 takes no arguments, returns a plain struct by value and is safe to call at any time.
    let m = unsafe { mallinfo2() };
    let _ = (m.arena, m.ordblks, m.smblks, m.hblks, m.usmblks, m.fsmblks, m.fordblks, m.keepcost);
    (m.uordblks + m.hblkhd) as u64
}

/// The resident set and its high-water mark, in KiB.
pub fn rss() -> (u64, u64) {
    (read_status("VmRSS:"), read_status("VmHWM:"))
}

impl Pkg {
    /// Open the package at `dir`: its manifest, its modules as they are on disk.
    pub fn open(dir: &Path) -> Result<Pkg, String> {
        let search = dir.parent().unwrap_or(Path::new("."));
        let ps = PackageSet::load(dir, search).map_err(|e| e.0)?;
        let declared: Vec<String> = ps.packages[0].1.dependencies.iter().filter(|(d, _)| !fortis_names::compat::is_standard(d)).map(|(d, _)| d.clone()).collect();
        if ps.packages.len() != 1 || !declared.is_empty() {
            return Err(format!("the daemon checks a package with no package dependencies (SP-12 design 13); this one declares {}", declared.join(", ")));
        }
        let p = &ps.packages[0].1;
        let identity = p.identity();
        let program_entry = ps.entry_key();
        let mut mods = Vec::new();
        let mut by_key = HashMap::new();
        for (mpath, m) in &p.modules {
            let text = std::fs::read_to_string(&m.file).map_err(|e| format!("{}: {e}", m.file.display()))?;
            if fortis_di::scan::uses_di(&text) {
                return Err(format!("{}: the Dependency injection kit is outside the daemon's scope (SP-12 design 13)", m.file.display()));
            }
            let key = format!("{}.{}", p.name, mpath);
            by_key.insert(key.clone(), mods.len() as F);
            if let Some(p) = &m.parsed {
                settle_holes(p);
            }
            mods.push(Mod { key, path: mpath.clone(), file: m.file.clone(), rel: rel(&m.file), fp: fp(text.as_bytes()), text: Rc::new(text), parsed: m.parsed.clone() });
        }
        let mut lib = Library::new();
        for m in &mods {
            if let Some(parsed) = &m.parsed {
                lib.modules.insert(m.key.clone(), Arc::new(LibraryModule::new(&m.file.display().to_string(), &identity, parsed.clone(), Some(0))));
            }
        }
        // the root module is also known by the package's name, to the libraries and to import resolution
        let mut root_f = None;
        if let Some(root) = &p.root {
            let key = format!("{}.{}", p.name, root);
            if let Some(f) = by_key.get(&key).copied() {
                root_f = Some(f);
                by_key.insert(p.name.clone(), f);
            }
            if let Some(lm) = lib.modules.get(&key).cloned() {
                lib.modules.insert(p.name.clone(), lm);
            }
        }
        let mut pkg = Pkg {
            dir: dir.to_path_buf(),
            identity,
            root: root_f.map(|f| (f, p.name.clone())),
            mods,
            by_key,
            ps: RefCell::new(ps),
            libs: Libraries { libs: vec![lib] },
            memo: RefCell::new(HashMap::new()),
            rdeps: RefCell::new(HashMap::new()),
            dirty: RefCell::new(HashSet::new()),
            frames: RefCell::new(vec![]),
            stack: RefCell::new(vec![]),
            orphans: RefCell::new(vec![]),
            log: RefCell::new(vec![]),
            prepared: Cell::new(0),
            graph: vec![],
            plans: vec![],
            entries: vec![],
            leaked: vec![],
            previous: RefCell::new(None),
            requests: Cell::new(0),
            changed: Cell::new(0),
            strs: RefCell::new(HashSet::new()),
            program_entry,
        };
        pkg.graph = (0..pkg.mods.len() as F).map(|f| pkg.imports_of(f)).collect();
        pkg.replan();
        Ok(pkg)
    }

    fn imports_of(&self, f: F) -> Vec<F> {
        let Some(parsed) = &self.mods[f as usize].parsed else { return vec![] };
        fortis_syntax::view::imports(N::new(&parsed.root, &parsed.toks)).iter().filter_map(|i| self.by_key.get(&i.path.join(".")).copied()).collect()
    }

    /// Recompute every module's cycle plan from the import graph, and mark the modules whose plan changed.
    ///
    /// The batch check builds an imported module's table once per session, and a module met again while its own
    /// table is being built is seen with no declarations (`Session::loading`). Inside an import cycle the tables
    /// therefore depend on which member the session enters first, which the batch check's module order decides. The
    /// plan simulates that order over the import graph: `entries` are the modules whose load first enters a cycle,
    /// and a request replays their loads, in order, before it builds any checker.
    fn replan(&mut self) {
        let n = self.mods.len();
        let g = &self.graph;
        // strongly connected components (iterative Tarjan)
        let mut index = vec![usize::MAX; n];
        let mut low = vec![0usize; n];
        let mut on = vec![false; n];
        let mut stack: Vec<usize> = vec![];
        let mut comp = vec![usize::MAX; n];
        let mut comps: Vec<Vec<usize>> = vec![];
        let mut next = 0usize;
        for root in 0..n {
            if index[root] != usize::MAX {
                continue;
            }
            let mut call: Vec<(usize, usize)> = vec![(root, 0)];
            index[root] = next;
            low[root] = next;
            next += 1;
            stack.push(root);
            on[root] = true;
            while let Some(&mut (v, ref mut i)) = call.last_mut() {
                if *i < g[v].len() {
                    let w = g[v][*i] as usize;
                    *i += 1;
                    if index[w] == usize::MAX {
                        index[w] = next;
                        low[w] = next;
                        next += 1;
                        stack.push(w);
                        on[w] = true;
                        call.push((w, 0));
                    } else if on[w] {
                        low[v] = low[v].min(index[w]);
                    }
                } else {
                    call.pop();
                    if let Some(&(u, _)) = call.last() {
                        low[u] = low[u].min(low[v]);
                    }
                    if low[v] == index[v] {
                        let mut c = vec![];
                        while let Some(w) = stack.pop() {
                            on[w] = false;
                            comp[w] = comps.len();
                            c.push(w);
                            if w == v {
                                break;
                            }
                        }
                        comps.push(c);
                    }
                }
            }
        }
        let cyclic: Vec<bool> = comps.iter().map(|c| c.len() > 1 || c.iter().any(|v| g[*v].contains(&(*v as F)))).collect();
        // the batch check's loads, in module order: the first module whose load enters each cycle
        let mut cached = vec![false; n];
        let mut loading = vec![false; n];
        let mut entered: Vec<Option<F>> = vec![None; comps.len()];
        // per cycle, the session's loads into it in order: (0 entered, 1 met while loading, 2 built; module; who
        // asked, a module or `n + m` for module m's own load)
        let mut events: Vec<Vec<(u8, usize, usize)>> = vec![vec![]; comps.len()];
        let mut entries = vec![];
        for m in 0..n {
            let mut first = false;
            // depth-first, as `DeclChecker::load` and `loaded` walk the imports
            let mut call: Vec<(usize, usize)> = vec![];
            for &i in &g[m] {
                call.push((i as usize, usize::MAX));
                while let Some(&(v, k)) = call.last() {
                    let top = call.len() - 1;
                    if k == usize::MAX {
                        let asker = if top >= 1 { call[top - 1].0 } else { n + m };
                        if cached[v] {
                            call.pop();
                            continue;
                        }
                        if loading[v] {
                            if cyclic[comp[v]] {
                                events[comp[v]].push((1, v, asker));
                            }
                            call.pop();
                            continue;
                        }
                        if cyclic[comp[v]] {
                            events[comp[v]].push((0, v, asker));
                        }
                        loading[v] = true;
                        if cyclic[comp[v]] && entered[comp[v]].is_none() {
                            entered[comp[v]] = Some(m as F);
                            first = true;
                        }
                        call[top].1 = 0;
                        continue;
                    }
                    if k < g[v].len() {
                        call[top].1 = k + 1;
                        call.push((g[v][k] as usize, usize::MAX));
                    } else {
                        loading[v] = false;
                        cached[v] = true;
                        if cyclic[comp[v]] {
                            events[comp[v]].push((2, v, 0));
                        }
                        call.pop();
                    }
                }
            }
            if first {
                entries.push(m as F);
            }
        }
        // a module's plan: the cycles its import closure meets, each with its entry and members
        let mut plans = vec![];
        for f in 0..n {
            let mut seen = vec![false; n];
            let mut work = vec![f];
            let mut met = BTreeSet::new();
            while let Some(v) = work.pop() {
                if seen[v] {
                    continue;
                }
                seen[v] = true;
                if cyclic[comp[v]] {
                    met.insert(comp[v]);
                }
                for &w in &g[v] {
                    work.push(w as usize);
                }
            }
            let plan: Vec<(F, Vec<F>)> = met
                .iter()
                .map(|c| {
                    let mut members: Vec<F> = comps[*c].iter().map(|v| *v as F).collect();
                    members.sort();
                    (entered[*c].unwrap_or(F::MAX), members)
                })
                .collect();
            // the order of the loads into each cycle, not only its entry and members (apparatus review 3)
            let order: Vec<&Vec<(u8, usize, usize)>> = met.iter().map(|c| &events[*c]).collect();
            let canon = format!("{plan:?}{order:?}");
            plans.push((plan, fp(canon.as_bytes())));
        }
        let old: Vec<Fp> = self.plans.iter().map(|p| p.1).collect();
        self.plans = plans;
        self.entries = entries;
        for f in 0..n {
            if old.get(f).map_or(false, |o| *o != self.plans[f].1) {
                self.mark(K::CyclePlan(f as F));
            }
        }
    }

    pub fn module_count(&self) -> usize {
        self.mods.len()
    }

    /// The module of a file path relative to the package directory (`src/l0/m00000.ft`).
    pub fn module_of(&self, file: &str) -> Option<F> {
        let want = self.dir.join(file);
        self.mods.iter().position(|m| m.file == want).map(|i| i as F)
    }

    pub fn module_file(&self, f: F) -> String {
        self.mods[f as usize].file.strip_prefix(&self.dir).map(|p| p.display().to_string()).unwrap_or_default()
    }

    /// Set a module's text: parse it, and mark everything that can reach it possibly stale. A text that uses the
    /// Dependency injection kit is refused (design 13).
    pub fn set_source(&mut self, f: F, text: &str) -> Result<(), String> {
        if fortis_di::scan::uses_di(text) {
            return Err("the Dependency injection kit is outside the daemon's scope (SP-12 design 13)".into());
        }
        let text = text.to_string();
        let nfp = fp(text.as_bytes());
        if nfp == self.mods[f as usize].fp {
            return Ok(());
        }
        let parsed = fortis_syntax::parse_module_located(&text);
        let m = &mut self.mods[f as usize];
        let old = std::mem::replace(&mut m.text, Rc::new(text));
        if fault("leak-text") {
            self.leaked.push(old);
        }
        // fault `leak-small`: 512 bytes kept per edit
        if fault("leak-small") {
            self.leaked.push(Rc::new("x".repeat(512)));
        }
        // fault `leak-uncounted`: 16 KiB per edit kept inside the daemon's allocator but outside its counts, so only its
        // footprint shows them (enough to pass G4's budget within the reduced soak)
        if fault("leak-uncounted") {
            crate::alloc::leak_uncounted(16 << 10);
        }
        m.fp = nfp;
        let (key, path) = (m.key.clone(), m.path.clone());
        {
            let mut ps = self.ps.borrow_mut();
            let pm = ps.packages[0].1.modules.get_mut(&path).expect("a module of the package");
            match parsed {
                Ok(p) => {
                    settle_holes(&p);
                    let p = Arc::new(p);
                    pm.parsed = Some(p.clone());
                    pm.error = None;
                    m.parsed = Some(p.clone());
                    let lm = Arc::new(LibraryModule::new(&m.file.display().to_string(), &self.identity, p, Some(0)));
                    // fault `root-stale`: the root's entry under the package's name keeps the old parse
                    if let Some((rf, name)) = &self.root {
                        if *rf == f && !fault("root-stale") {
                            self.libs.libs[0].modules.insert(name.clone(), lm.clone());
                        }
                    }
                    self.libs.libs[0].modules.insert(key, lm);
                }
                Err(e) => {
                    pm.parsed = None;
                    pm.error = Some(e);
                    m.parsed = None;
                }
            }
        }
        self.mark(K::Source(f));
        let imports = self.imports_of(f);
        if self.mods[f as usize].parsed.is_some() && imports != self.graph[f as usize] {
            self.graph[f as usize] = imports;
            self.replan();
        }
        Ok(())
    }

    /// Mark everything that reaches `start` through its dependencies as possibly stale.
    fn mark(&self, start: K) {
        let rdeps = self.rdeps.borrow();
        let mut dirty = self.dirty.borrow_mut();
        let mut work = vec![(start, 0usize)];
        while let Some((k, depth)) = work.pop() {
            if let Some(ds) = rdeps.get(&k) {
                for d in ds {
                    // fault `dirty-shallow`: the walk stops one step from the source
                    if fault("dirty-shallow") && depth >= 2 {
                        continue;
                    }
                    if dirty.insert(d.clone()) {
                        work.push((d.clone(), depth + 1));
                    }
                }
            }
        }
    }

    pub fn take_log(&self) -> Vec<(K, Reason)> {
        std::mem::take(&mut self.log.borrow_mut())
    }

    /// The one allocation of a key's string part.
    fn sym(&self, s: &str) -> Rc<str> {
        // fault `strings-fresh`: every key builds its strings afresh, as SP-12's daemon did
        if fault("strings-fresh") {
            return s.into();
        }
        let mut t = self.strs.borrow_mut();
        if let Some(x) = t.get(s) {
            return x.clone();
        }
        let x: Rc<str> = s.into();
        t.insert(x.clone());
        x
    }

    pub fn memo_count(&self) -> usize {
        self.memo.borrow().len()
    }

    pub fn memo_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut out = BTreeMap::new();
        for k in self.memo.borrow().keys() {
            *out.entry(k.kind()).or_insert(0) += 1;
        }
        out
    }

    /// The package's records, as `fortisc check DIR` gives them, and what ran.
    pub fn check(&self) -> (Rc<Vec<Record>>, Ran) {
        // fault `slow-check`: every request takes 150 ms longer
        if fault("slow-check") {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        self.prepared.set(0);
        self.take_log();
        let sess = Session::new(&self.libs);
        let cx = Cx { sess: &sess, libs: &self.libs, preps: RefCell::new(HashMap::new()) };
        // fault `cycle-unordered`: the batch check's order into import cycles is not replayed
        let entries: &[F] = if fault("cycle-unordered") { &[] } else { &self.entries };
        for e in entries {
            let m = &self.mods[*e as usize];
            if let Some(lm) = self.libs.libs[0].modules.get(&m.key) {
                let mut dc = DeclChecker::new(&sess, 0, &m.rel, &m.key, &self.identity);
                dc.load(lm.tree(), false);
            }
        }
        let recs = match self.get(&cx, &K::Package) {
            Ok(V::Records(r)) => r,
            _ => Rc::new(vec![]),
        };
        drop(cx);
        self.collect();
        // what the request ran is reported, and its log is released with the request (SP-12 closing review 1); fault
        // `log-kept`: the log stays until the next request, as in SP-12
        let log = if fault("log-kept") { self.log.borrow().clone() } else { self.take_log() };
        {
            let mut d = self.dirty.borrow_mut();
            // fault `dirty-kept`: the emptied possibly-stale set keeps the room of the largest invalidation
            if d.is_empty() && !fault("dirty-kept") {
                d.shrink_to_fit();
            }
            // a string no key holds any more leaves the table, and the tables keep the room their entries need
            let mut t = self.strs.borrow_mut();
            // fault `strs-kept`: strings no key holds stay in the table
            if !fault("strs-kept") {
                t.retain(|x| Rc::strong_count(x) > 1);
            }
            // fault `tables-kept`: the tables keep whatever room they once grew to
            if !fault("tables-kept") {
                fit(&mut t, false);
                self.memo.borrow_mut().shrink_to_fit();
                self.rdeps.borrow_mut().shrink_to_fit();
            }
            // the computation's scratch stacks are empty between requests, and keep no room from the deepest one
            // (SP-12b design review 3)
            // fault `scratch-kept`: the scratch stacks keep the room of the deepest computation
            if !fault("scratch-kept") {
                *self.frames.borrow_mut() = Vec::new();
                *self.stack.borrow_mut() = Vec::new();
            }
        }
        let mut ran = Ran { prepared: self.prepared.get(), ..Default::default() };
        for (k, _) in log.iter() {
            *ran.by_kind.entry(k.kind().to_string()).or_insert(0) += 1;
            if let K::Pass(f, u, p) = k {
                ran.passes.push((*f, u.to_string(), *p));
            }
        }
        let n = self.requests.get() + 1;
        self.requests.set(n);
        // fault `reply-stale`: one in five answers that differ from the previous one carries the previous one
        let prev = self.previous.replace(Some(recs.clone()));
        if fault("reply-stale") {
            if let Some(p) = prev {
                let differ = p.len() != recs.len() || p.iter().zip(recs.iter()).any(|(a, b)| a.to_json_line() != b.to_json_line());
                if differ {
                    self.changed.set(self.changed.get() + 1);
                    if self.changed.get() % 5 == 0 {
                        return (p, ran);
                    }
                }
            }
        }
        // faults `omit-own`, `omit-eff`, `omit-obl`, `omit-ref`: an analysis's records are left out, as if it were
        // skipped
        let omit: Vec<&str> = [("omit-own", "E-OWN"), ("omit-eff", "E-EFF"), ("omit-obl", "E-OBL"), ("omit-ref", "E-REF")].iter().filter(|(f, _)| fault(f)).map(|(_, c)| *c).collect();
        if !omit.is_empty() {
            return (Rc::new(recs.iter().filter(|r| !omit.iter().any(|c| r.code.starts_with(c))).cloned().collect()), ran);
        }
        (recs, ran)
    }

    fn get<'a>(&self, cx: &Cx<'a>, key: &K) -> Result<V, Cancelled> {
        let (v, f) = self.ensure(cx, key)?;
        if let Some(top) = self.frames.borrow_mut().last_mut() {
            top.push((key.clone(), f));
        }
        Ok(v)
    }

    fn ensure<'a>(&self, cx: &Cx<'a>, key: &K) -> Result<(V, Fp), Cancelled> {
        if let K::Source(f) = key {
            let m = &self.mods[*f as usize];
            return Ok((V::Text(m.text.clone()), m.fp));
        }
        if let K::CyclePlan(f) = key {
            return Ok((V::Text(Rc::new(String::new())), self.plans[*f as usize].1));
        }
        let cached = self.memo.borrow().get(key).map(|m| (m.value.clone(), m.fp, m.deps.clone()));
        let mut reason = Reason::New;
        let mut old_deps: Vec<(K, Fp)> = vec![];
        if let Some((value, f, deps)) = cached {
            if !self.dirty.borrow().contains(key) {
                return Ok((value, f));
            }
            // fault `skip-final`: a final pass, once computed, is never rerun
            if fault("skip-final") && matches!(key, K::Pass(_, _, 2)) {
                self.dirty.borrow_mut().remove(key);
                return Ok((value, f));
            }
            let mut stale = None;
            for (d, dfp) in &deps {
                let (_, now) = self.ensure(cx, d)?;
                // fault `summary-unchecked`: a changed summary does not make its reader stale
                if fault("summary-unchecked") && matches!(d, K::Summary(..)) {
                    continue;
                }
                if now != *dfp {
                    stale = Some(d.clone());
                    break;
                }
            }
            match stale {
                None => {
                    self.dirty.borrow_mut().remove(key);
                    return Ok((value, f));
                }
                Some(d) => reason = Reason::Stale(d),
            }
            old_deps = deps;
        }
        self.frames.borrow_mut().push(vec![]);
        self.stack.borrow_mut().push(key.clone());
        let result = self.compute(cx, key);
        self.stack.borrow_mut().pop();
        let deps = self.frames.borrow_mut().pop().unwrap_or_default();
        let value = result?;
        let vfp = fp(value.canonical().as_bytes());
        self.relink(key, &old_deps, &deps);
        self.memo.borrow_mut().insert(key.clone(), Memo { value: value.clone(), fp: vfp, deps });
        self.dirty.borrow_mut().remove(key);
        self.log.borrow_mut().push((key.clone(), reason));
        Ok((value, vfp))
    }

    /// A recomputed memo's dependent edges: add the new ones first, then remove the old ones. A memo whose last
    /// dependent left is collected when the request ends (design 3.3).
    fn relink(&self, key: &K, old: &[(K, Fp)], new: &[(K, Fp)]) {
        let mut rdeps = self.rdeps.borrow_mut();
        for (d, _) in new {
            let s = rdeps.entry(d.clone()).or_default();
            // `insert` reserves room before it looks, so inserting an element already there grows a full set: look
            // first. An insertion into a set whose free room tombstones used up doubles it: keep it at what its
            // length needs.
            // fault `insert-unchecked`: insert without looking, as before SP-12b's calibration found the growth
            if fault("insert-unchecked") {
                if s.insert(key.clone()) {
                    fit(s, true);
                }
            } else if !s.contains(key) {
                s.insert(key.clone());
                fit(s, true);
            }
        }
        let keep: HashSet<&K> = new.iter().map(|(d, _)| d).collect();
        for (d, _) in old {
            if keep.contains(d) {
                continue;
            }
            if let Some(s) = rdeps.get_mut(d) {
                s.remove(key);
                if s.is_empty() {
                    rdeps.remove(d);
                    self.orphans.borrow_mut().push(d.clone());
                } else {
                    // a set keeps exactly the room its dependents need: memory is a function of the state
                    fit(s, false);
                }
            }
        }
    }

    /// Drop every memo no other memo depends on, except the roots, and what only they reached.
    fn collect(&self) {
        let mut orphans = std::mem::take(&mut *self.orphans.borrow_mut());
        if fault("gc-off") {
            return;
        }
        let mut rdeps = self.rdeps.borrow_mut();
        let mut memo = self.memo.borrow_mut();
        while let Some(o) = orphans.pop() {
            if o.is_root() || matches!(o, K::Source(_) | K::CyclePlan(_)) || rdeps.get(&o).map_or(false, |s| !s.is_empty()) {
                continue;
            }
            if let Some(m) = memo.remove(&o) {
                self.dirty.borrow_mut().remove(&o);
                for (d, _) in &m.deps {
                    if let Some(s) = rdeps.get_mut(d) {
                        s.remove(&o);
                        if s.is_empty() {
                            rdeps.remove(d);
                            orphans.push(d.clone());
                        } else {
                            fit(s, false);
                        }
                    }
                }
            }
        }
    }

    fn on_stack(&self, k: &K) -> bool {
        self.stack.borrow().contains(k)
    }

    fn prep<'a>(&self, cx: &Cx<'a>, f: F) -> Rc<Prep<'a>> {
        if let Some(p) = cx.preps.borrow().get(&f) {
            return p.clone();
        }
        let m = &self.mods[f as usize];
        let lm: &'a LibraryModule = &cx.libs.libs[0].modules[&m.key];
        let tree = lm.tree();
        let mut dc = DeclChecker::new(cx.sess, 0, &m.rel, &m.key, &self.identity);
        dc.entry_module = self.program_entry.clone();
        dc.load(tree, false);
        dc.check();
        let p = Rc::new(Prep { dc, tree });
        cx.preps.borrow_mut().insert(f, p.clone());
        self.prepared.set(self.prepared.get() + 1);
        p
    }

    fn heads(&self, cx: &Cx, f: F) -> Result<Rc<HeadsV>, Cancelled> {
        match self.get(cx, &K::Heads(f))? {
            V::Heads(h) => Ok(h),
            _ => unreachable!(),
        }
    }

    fn interface(&self, cx: &Cx, f: F) -> Result<Rc<Iface>, Cancelled> {
        match self.get(cx, &K::Interface(f))? {
            V::Interface(i) => Ok(i),
            _ => unreachable!(),
        }
    }

    fn records(&self, cx: &Cx, k: &K) -> Result<Rc<Vec<Record>>, Cancelled> {
        match self.get(cx, k)? {
            V::Records(r) => Ok(r),
            _ => unreachable!(),
        }
    }

    fn pass(&self, cx: &Cx, f: F, uid: &Rc<str>, p: u8) -> Result<Rc<UnitV>, Cancelled> {
        match self.get(cx, &K::Pass(f, uid.clone(), p))? {
            V::Unit(u) => Ok(u),
            _ => unreachable!(),
        }
    }

    fn summary(&self, cx: &Cx, f: F, uid: &str, p: u8, label: &str) -> Result<Option<Summary>, Cancelled> {
        match self.get(cx, &K::Summary(f, self.sym(uid), p, self.sym(label)))? {
            V::Summary(s) => Ok(s.map(|x| (*x).clone())),
            _ => unreachable!(),
        }
    }

    /// The summary a unit at `idx` sees for `label` in pass `p`, as the batch check's state would hold it (DEL-46).
    fn provider(&self, cx: &Cx, f: F, iface: &Iface, idx: usize, label: &str, p: u8) -> Result<Option<Summary>, Cancelled> {
        let ids: Vec<String> = iface.units.iter().map(|u| u.id()).collect();
        if label == "closure" {
            for j in (0..idx).rev() {
                if let Some(s) = self.summary(cx, f, &ids[j], p, "closure")? {
                    return Ok(Some(s));
                }
            }
            if p == 2 {
                for j in (0..ids.len()).rev() {
                    if let Some(s) = self.summary(cx, f, &ids[j], 1, "closure")? {
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
                return self.summary(cx, f, owner, 1, label);
            }
            return Ok(None);
        }
        if o < idx {
            if let Some(s) = self.summary(cx, f, owner, 2, label)? {
                return Ok(Some(s));
            }
        }
        self.summary(cx, f, owner, 1, label)
    }

    /// Read the foreign closure of identifiers `start` used in module `f` (design 3.2), each pair as a dependency.
    fn closure(&self, cx: &Cx, f: F, start: &[String]) -> Result<(), Cancelled> {
        if fault("foreign-dropped") {
            return Ok(());
        }
        let own = self.heads(cx, f)?;
        let mut seen: HashSet<(F, String)> = HashSet::new();
        let mut work: Vec<(F, String)> = vec![];
        // modules named through an alias: a call through it may list the module's exports (E-NAM-004)
        let aliased: RefCell<BTreeSet<F>> = RefCell::new(BTreeSet::new());
        // the names a list of identifiers of the module reaches: its own, and through an alias it mentions, any of
        // that module's
        let expand = |ids: &[String], work: &mut Vec<(F, String)>| {
            for x in ids {
                work.push((f, x.clone()));
            }
            for (a, t) in &own.aliases {
                if ids.iter().any(|x| x == a) {
                    for x in ids {
                        work.push((*t, x.clone()));
                    }
                    aliased.borrow_mut().insert(*t);
                }
            }
        };
        expand(start, &mut work);
        let mut whole: BTreeSet<F> = BTreeSet::new();
        while let Some((g, t)) = work.pop() {
            if !seen.insert((g, t.clone())) {
                continue;
            }
            if g == f {
                for p in own.providers.get(&t).into_iter().flatten() {
                    work.push((*p, t.clone()));
                }
                for (_, _, _, _, ids) in own.decls.get(&t).into_iter().flatten() {
                    expand(ids, &mut work);
                }
                continue;
            }
            // fault `foreign-module`: the whole module's heads instead of the one declaration's
            if fault("foreign-module") {
                if whole.insert(g) {
                    self.heads(cx, g)?;
                }
            }
            if let V::Head(Some(h)) = self.get(cx, &K::Foreign(g, self.sym(t.as_str())))? {
                for (a, tg) in &h.aliases_mentioned {
                    let _ = a;
                    aliased.borrow_mut().insert(*tg);
                }
                for (x, provs) in &h.refs {
                    work.push((g, x.clone()));
                    for p in provs {
                        work.push((*p, x.clone()));
                    }
                    // a name in a foreign head may be looked up in the reading module's table (mixins, interfaces)
                    work.push((f, x.clone()));
                }
            }
        }
        // fault `alias-shape`: the export sets of aliased modules are not read
        if !fault("alias-shape") {
            for t in aliased.into_inner() {
                self.get(cx, &K::Exports(t))?;
            }
        }
        Ok(())
    }

    fn compute<'a>(&self, cx: &Cx<'a>, key: &K) -> Result<V, Cancelled> {
        match key {
            K::Source(_) | K::CyclePlan(_) => unreachable!(),
            K::Heads(f) => {
                self.get(cx, &K::Source(*f))?;
                Ok(V::Heads(Rc::new(self.compute_heads(*f))))
            }
            K::Shape(f) => {
                let h = self.heads(cx, *f)?;
                let decls = Json::Obj(h.decls.iter().map(|(n, ds)| (n.clone(), Json::Arr(ds.iter().map(|(k, e, m, _, _)| Json::Arr(vec![Json::str(k), Json::Bool(*e), strs(m)])).collect()))).collect());
                let imports = Json::Arr(h.imports.iter().map(|i| Json::Arr(vec![Json::str(&i.path), i.alias.as_ref().map_or(Json::Null, |a| Json::str(a)), i.members.as_ref().map_or(Json::Null, |m| strs(m))])).collect());
                Ok(V::Json(Rc::new(Json::obj([("decls", decls), ("imports", imports)]))))
            }
            K::Exports(f) => {
                let h = self.heads(cx, *f)?;
                let names: Vec<String> = h.decls.iter().filter(|(_, ds)| ds.iter().any(|d| d.1)).map(|(n, _)| n.clone()).collect();
                Ok(V::Json(Rc::new(Json::strs(names))))
            }
            K::Foreign(g, n) => {
                let h = self.heads(cx, *g)?;
                let Some(ds) = h.decls.get(n.as_ref()) else { return Ok(V::Head(None)) };
                let mut text = String::new();
                let mut ids = BTreeSet::new();
                for (k, e, _, t, xs) in ds {
                    text.push_str(&format!("{k} {e} {t}\n"));
                    ids.extend(xs.iter().cloned());
                }
                // a name qualified by an alias the head mentions may be any of that module's declarations
                let mentioned: Vec<(String, F)> = h.aliases.iter().filter(|(a, _)| ids.contains(*a)).map(|(a, t)| (a.clone(), *t)).collect();
                let through: Vec<F> = mentioned.iter().map(|(_, t)| *t).collect();
                let refs = ids.into_iter().map(|x| {
                    let mut p = h.providers.get(&x).cloned().unwrap_or_default();
                    p.extend(through.iter().copied());
                    (x, p)
                });
                Ok(V::Head(Some(Rc::new(Head { text, refs: refs.collect(), aliases_mentioned: mentioned }))))
            }
            K::ImplOf(f) => {
                let h = self.heads(cx, *f)?;
                let targets: BTreeSet<F> = h.imports.iter().filter_map(|i| i.target).collect();
                Ok(V::Json(Rc::new(Json::obj([("implements", Json::str(&h.implements)), ("imports", Json::Arr(targets.iter().map(|t| Json::Int(*t as i64)).collect()))]))))
            }
            K::Implements(f) => {
                let V::Json(own) = self.get(cx, &K::ImplOf(*f))? else { unreachable!() };
                let mut out = String::new();
                if let Some(Json::Arr(ts)) = own.get("imports") {
                    for t in ts {
                        let Json::Int(g) = t else { continue };
                        let g = *g as F;
                        // an import cycle reads the module's own part only
                        if g == *f {
                            continue;
                        }
                        let V::Json(theirs) = self.get(cx, &K::ImplOf(g))? else { unreachable!() };
                        if let Some(Json::Str(i)) = theirs.get("implements") {
                            out.push_str(i);
                        }
                        if !self.on_stack(&K::Implements(g)) {
                            let V::Text(rest) = self.get(cx, &K::Implements(g))? else { unreachable!() };
                            out.push_str(&rest);
                        }
                    }
                }
                Ok(V::Text(Rc::new(out)))
            }
            K::ResolveMod(f) => {
                // the records carry the import tokens' positions
                self.get(cx, &K::Source(*f))?;
                let h = self.heads(cx, *f)?;
                self.get(cx, &K::Shape(*f))?;
                for i in &h.imports {
                    if let Some(t) = i.target {
                        self.get(cx, &K::Shape(t))?;
                    }
                }
                let path = self.mods[*f as usize].path.clone();
                Ok(V::Records(Rc::new(self.ps.borrow_mut().module_records(0, &path))))
            }
            K::Resolver => {
                for f in 0..self.mods.len() as F {
                    self.get(cx, &K::Source(f))?;
                }
                // the set's own records (the load's) come back with the opening ones
                let mut recs = self.ps.borrow_mut().opening_records();
                // fault `load-twice`: the load's records are added again
                if fault("load-twice") {
                    let mut again = self.ps.borrow().records.clone();
                    again.extend(recs);
                    recs = again;
                }
                if !recs.is_empty() {
                    return Ok(V::Records(Rc::new(recs)));
                }
                // fault `resolver-stale`: the import checks are not rerun after `open`
                let stale = fault("resolver-stale") && self.requests.get() > 0;
                for f in 0..self.mods.len() as F {
                    if stale {
                        if let Some(m) = self.memo.borrow().get(&K::ResolveMod(f)) {
                            if let V::Records(r) = &m.value {
                                recs.extend(r.iter().cloned());
                                continue;
                            }
                        }
                    }
                    recs.extend(self.records(cx, &K::ResolveMod(f))?.iter().cloned());
                }
                recs.extend(self.ps.borrow_mut().entry_records());
                Ok(V::Records(Rc::new(recs)))
            }
            K::Names(f) => {
                self.get(cx, &K::Source(*f))?;
                let h = self.heads(cx, *f)?;
                for i in &h.imports {
                    if let Some(t) = i.target {
                        self.get(cx, &K::Shape(t))?;
                    }
                }
                let m = &self.mods[*f as usize];
                let lm: &LibraryModule = &cx.libs.libs[0].modules[&m.key];
                // a module of the package: the manifest's `entry` judges its `main` (`entry_records`), not this resolve
                Ok(V::Records(Rc::new(Resolver::new(&cx.libs.libs[0], &m.rel, false).resolve_module(lm.tree()))))
            }
            K::Texts(f) => {
                self.get(cx, &K::Source(*f))?;
                let p = self.prep(cx, *f);
                let ty = Typer::new(p.dc.clone());
                let texts = inc::texts_of(&ty, p.tree);
                // a declaration's aspects sit beside its node: what they name is read by the unit too
                let decls = fortis_syntax::view::declarations(p.tree);
                let idents = inc::unit_nodes(&ty, p.tree)
                    .into_iter()
                    .map(|(u, n)| {
                        let mut ids = n.map(idents_of).unwrap_or_default();
                        if let Unit::Decl(name) = &u {
                            for d in decls.iter().filter(|d| d.name.as_deref() == Some(name.as_str())) {
                                for a in &d.aspects {
                                    ids.extend(idents_of(*a));
                                }
                            }
                            ids.sort();
                            ids.dedup();
                        }
                        (u.id(), ids)
                    })
                    .collect();
                Ok(V::Texts(Rc::new(TextsV { texts, idents, skeleton_idents: skeleton_idents(p.tree) })))
            }
            K::UnitText(f, u) => {
                let V::Texts(t) = self.get(cx, &K::Texts(*f))? else { unreachable!() };
                let text = t.texts.units.get(u.as_ref()).cloned().unwrap_or_default();
                let idents = t.idents.get(u.as_ref()).cloned().unwrap_or_default();
                Ok(V::UnitText(Rc::new(UnitTextV { text, idents })))
            }
            K::Interface(f) => {
                self.get(cx, &K::Source(*f))?;
                let V::Texts(t) = self.get(cx, &K::Texts(*f))? else { unreachable!() };
                let h = self.heads(cx, *f)?;
                if !fault("interface-local") {
                    for i in &h.imports {
                        if let Some(g) = i.target {
                            self.get(cx, &K::Shape(g))?;
                        }
                    }
                    self.closure(cx, *f, &t.skeleton_idents)?;
                }
                self.get(cx, &K::Implements(*f))?;
                self.get(cx, &K::CyclePlan(*f))?;
                let p = self.prep(cx, *f);
                let mut ty = Typer::new(p.dc.clone());
                let labels = inc::labels_of(&mut ty, p.tree);
                let mut ty = Typer::new(p.dc.clone());
                let (declarations, imports, units) = inc::interface_of(&mut ty, p.tree);
                let mut records = declarations;
                records.extend(imports);
                Ok(V::Interface(Rc::new(Iface { records, units, labels, skeleton: t.texts.skeleton.clone(), skeleton_idents: t.skeleton_idents.clone() })))
            }
            K::Pass(f, uid, pn) => {
                let iface = self.interface(cx, *f)?;
                let V::UnitText(ut) = self.get(cx, &K::UnitText(*f, uid.clone()))? else { unreachable!() };
                self.closure(cx, *f, &ut.idents)?;
                self.get(cx, &K::Implements(*f))?;
                self.get(cx, &K::CyclePlan(*f))?;
                let Some(idx) = iface.units.iter().position(|u| u.id() == **uid) else {
                    return Ok(V::Unit(Rc::new(UnitV { records: vec![], produced: BTreeMap::new(), requested: BTreeSet::new() })));
                };
                let unit = iface.units[idx].clone();
                let p = self.prep(cx, *f);
                let mut given: BTreeMap<String, Summary> = BTreeMap::new();
                let mut asked: BTreeSet<String> = BTreeSet::new();

                loop {
                    let mut ty = Typer::new(p.dc.clone());
                    let out = inc::run_unit_of(&mut ty, p.tree, &unit, &given, *pn == 2);
                    let mut added = false;
                    for label in out.requested.iter() {
                        if asked.insert(label.clone()) {
                            if let Some(s) = self.provider(cx, *f, &iface, idx, label, *pn)? {
                                given.insert(label.clone(), s);
                                added = true;
                            }
                        }
                    }
                    if !added {
                        return Ok(V::Unit(Rc::new(UnitV { records: out.records, produced: out.produced, requested: out.requested })));
                    }
                }
            }
            K::Summary(f, uid, p, label) => Ok(V::Summary(self.pass(cx, *f, uid, *p)?.produced.get(label.as_ref()).cloned().map(Rc::new))),
            K::Module(f) => {
                let names = self.records(cx, &K::Names(*f))?;
                let iface = self.interface(cx, *f)?;
                let mut out = (*names).clone();
                let mut body = iface.records.clone();
                for u in &iface.units {
                    let uid: Rc<str> = self.sym(&u.id());
                    body.extend(self.pass(cx, *f, &uid, 2)?.records.iter().cloned());
                }
                out.extend(inc::dedupe(body));
                Ok(V::Records(Rc::new(out)))
            }
            K::Package => {
                let r = self.records(cx, &K::Resolver)?;
                let mut recs = if !r.is_empty() {
                    (*r).clone()
                } else {
                    let mut out = vec![];
                    for f in 0..self.mods.len() as F {
                        out.extend(self.records(cx, &K::Module(f))?.iter().cloned());
                        // the module's checker is done with for this request
                        cx.preps.borrow_mut().remove(&f);
                    }
                    out
                };
                let files: Vec<PathBuf> = self.mods.iter().map(|m| m.file.clone()).collect();
                fortis_di::report(&mut recs, &files);
                Ok(V::Records(Rc::new(recs)))
            }
        }
    }

    /// A module's heads, read from its parse alone (no checker).
    fn compute_heads(&self, f: F) -> HeadsV {
        let m = &self.mods[f as usize];
        let mut out = HeadsV { decls: BTreeMap::new(), imports: vec![], implements: String::new(), providers: BTreeMap::new(), aliases: BTreeMap::new() };
        let Some(parsed) = &m.parsed else { return out };
        let tree = N::new(&parsed.root, &parsed.toks);
        for d in fortis_syntax::view::declarations(tree) {
            let mut text = String::new();
            let mut ids = BTreeSet::new();
            head_text(d.node, &mut text, &mut ids);
            // an aspect's hooks are checked inside the functions that use it, and their records carry the hook's
            // positions: its head is its whole text, with positions (design 3.2)
            if d.kind == "aspect" {
                text.clear();
                for t in d.node.tokens() {
                    text.push_str(&format!("{}:{}:{} ", t.line, t.col, t.text));
                }
            }
            if fault("positional-heads") {
                if let Some(t) = d.node.first_token() {
                    text.push_str(&format!("@{}:{}", t.line, t.col));
                }
            }
            if d.kind == "implement" {
                out.implements.push_str(&text);
                out.implements.push('\n');
                continue;
            }
            let Some(name) = d.name.clone() else { continue };
            let members: Vec<String> = fortis_names::library::members_of(d.node).into_iter().collect();
            out.decls.entry(name).or_default().push((d.kind.to_string(), d.exported, members, text, ids.into_iter().collect()));
        }
        for i in fortis_syntax::view::imports(tree) {
            let path = i.path.join(".");
            let target = self.by_key.get(&path).copied();
            if let Some(t) = target {
                match &i.members {
                    Some(ms) => {
                        for x in ms {
                            out.providers.entry(x.clone()).or_default().push(t);
                        }
                    }
                    None => {
                        let a = i.alias.clone().unwrap_or_else(|| i.path.last().cloned().unwrap_or_default());
                        out.aliases.insert(a, t);
                    }
                }
            }
            out.imports.push(Import { path, target, alias: i.alias.clone(), members: i.members.clone() });
        }
        out
    }

    /// The deep comparison (design 9): a fresh database on the same sources must hold the same live keys, each
    /// with the same value and dependencies. Returns the differences (at most 20).
    pub fn verify(&self) -> Vec<String> {
        let mut fresh = match Pkg::open(&self.dir) {
            Ok(p) => p,
            Err(e) => return vec![format!("the fresh database did not open: {e}")],
        };
        for f in 0..self.mods.len() as F {
            let text = self.mods[f as usize].text.clone();
            let _ = fresh.set_source(f, &text);
        }
        fresh.check();
        let mine = self.memo.borrow();
        let theirs = fresh.memo.borrow();
        let dirty = self.dirty.borrow();
        let mut out = vec![];
        for (k, m) in mine.iter() {
            if dirty.contains(k) {
                continue;
            }
            match theirs.get(k) {
                None => out.push(format!("{k:?}: live here, not in a fresh database")),
                Some(t) if t.fp != m.fp => out.push(format!("{k:?}: value differs from a fresh database's")),
                Some(t) if t.deps != m.deps => out.push(format!("{k:?}: dependencies differ from a fresh database's")),
                _ => {}
            }
            if out.len() >= 20 {
                return out;
            }
        }
        for k in theirs.keys() {
            if !mine.contains_key(k) || dirty.contains(k) {
                out.push(format!("{k:?}: in a fresh database, not live here"));
                if out.len() >= 20 {
                    break;
                }
            }
        }
        out
    }
}
