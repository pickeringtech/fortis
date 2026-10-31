//! Generations of a program in a development session (Platform 11.2; SP-07's design, sections 3, 4 and 6): the
//! identities that stay stable across edits, the fingerprints a reload compares, the classification of a reload
//! into changes, and the sets (Rep, Dep) that decide which scopes it restarts.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};

use fortis_diag::Json;
use fortis_mir::json;
use fortis_mir::model::*;
use fortis_types::types::Type;

use crate::sync::Rc;

/// Generations alive in this process (built and not yet dropped): the retention checks read it.
pub static LIVE: AtomicUsize = AtomicUsize::new(0);

/// What a code value (a closure, a function value, an unstarted future) carries so that a frame of another
/// generation can resolve it (design 5). It never refers to the generation itself, so it pins nothing.
#[derive(Debug)]
pub struct CodeId {
    pub gen: u32,
    pub ident: Rc<str>,
    /// the deep code fingerprint (design 3)
    pub deep: u64,
    /// the creating generation's parameter locals, then its capture locals: a future's frame is rebound by position
    pub slots: Vec<u32>,
}

/// What a function's IR refers to (Dep, design 6).
#[derive(Clone, Debug, Default)]
pub struct Refs {
    /// functions called directly, taken as values, or created as lambdas (qualified IR names)
    pub funcs: Vec<String>,
    /// methods called through an interface or a type parameter
    pub virtuals: Vec<String>,
    /// the function types of calls through values
    pub values: Vec<(Vec<Type>, Type)>,
    pub consts: Vec<String>,
    /// type names mentioned anywhere
    pub types: Vec<String>,
}

/// A user function as a reload compares it.
#[derive(Clone, Debug)]
pub struct FnInfo {
    pub index: usize,
    pub label: String,
    pub summary: u64,
    /// (part, fingerprint, shown): what an explanation of a summary change names
    pub parts: Vec<(&'static str, u64, String)>,
    pub env: u64,
    pub env_shown: String,
    pub deep: u64,
    pub body: u64,
    pub generic: bool,
    /// the parent's identity, for a lambda
    pub parent: Option<String>,
    pub is_async: bool,
    pub fails: bool,
    pub params: Vec<Type>,
    pub ret: Type,
    pub captures: Vec<Type>,
    pub refs: Refs,
    /// the DI context that generated it
    pub di: Option<String>,
}

/// A user type as a reload compares it.
#[derive(Clone, Debug)]
pub struct TypeInfo {
    pub name: String,
    /// its type parameters, substituted by a use's type arguments when Rep walks its edges
    pub params: Vec<String>,
    pub layout: u64,
    pub shown: String,
    pub deep: u64,
    /// (edge label, type): fields, variant payloads, a newtype's representation, an error's payload
    pub edges: Vec<(String, Type)>,
    pub implements: Vec<String>,
    /// method name to the identity of the function implementing it
    pub methods: BTreeMap<String, String>,
    pub interface: bool,
    pub error: bool,
    pub di: Option<String>,
}

pub struct Generation {
    pub id: u32,
    pub program: Program,
    /// by function index
    pub code: Vec<Rc<CodeId>>,
    pub by_ident: HashMap<Rc<str>, usize>,
    /// user functions and types by identity
    pub funcs: BTreeMap<String, FnInfo>,
    pub types: BTreeMap<String, TypeInfo>,
    /// a qualified IR function name to its identity
    pub name_ident: HashMap<String, String>,
    /// a user type's name to its identity
    pub type_ident: HashMap<String, String>,
    pub consts: BTreeMap<String, (u64, String)>,
    pub const_refs: BTreeMap<String, Refs>,
    /// the length of the program's IR serialisation (the memory cap's measure, design 8)
    pub bytes: usize,
    pub source: String,
    pub sha256: String,
}

impl std::fmt::Debug for Generation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Generation({})", self.id)
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---- hashing and canonical forms ----------------------------------------------------------------------------------

pub fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn text(j: &Json) -> String {
    let mut s = String::new();
    j.write(&mut s);
    s
}

/// A node with its line information removed and every generated name replaced, so moving code changes nothing.
fn canon(j: &Json, names: &HashMap<String, String>) -> Json {
    match j {
        Json::Obj(m) => Json::Obj(
            m.iter()
                .filter(|(k, _)| k.as_str() != "line")
                .map(|(k, v)| {
                    // a closure identity is the lambda's source location (8.4). It is a checker fact, so a
                    // blank line must not change the function's body fingerprint.
                    let v = if k.as_str() == "identity" { Json::Str(String::new()) } else { canon(v, names) };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Json::Arr(xs) => Json::Arr(xs.iter().map(|x| canon(x, names)).collect()),
        Json::Str(s) => Json::Str(names.get(s).cloned().unwrap_or_else(|| s.clone())),
        other => other.clone(),
    }
}

/// Every node of a kind, in the serialisation's order.
fn each<'j>(j: &'j Json, kind: &str, out: &mut Vec<&'j Json>) {
    match j {
        Json::Obj(m) => {
            if matches!(m.get("k"), Some(Json::Str(k)) if k == kind) {
                out.push(j);
            }
            for v in m.values() {
                each(v, kind, out);
            }
        }
        Json::Arr(xs) => xs.iter().for_each(|x| each(x, kind, out)),
        _ => {}
    }
}

fn field<'j>(j: &'j Json, key: &str) -> Option<&'j str> {
    match j {
        Json::Obj(m) => match m.get(key) {
            Some(Json::Str(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

fn has_key(j: &Json, key: &str) -> bool {
    matches!(j, Json::Obj(m) if m.contains_key(key))
}

/// The user type names a type mentions.
fn mentioned(t: &Type, out: &mut Vec<String>) {
    match t {
        Type::Named(n) | Type::Dynamic(n) | Type::Refined { base: n, .. } => {
            out.push(n.name.clone());
            n.args.iter().for_each(|a| mentioned(a, out));
        }
        Type::Optional(i) | Type::Loan { inner: i, .. } => mentioned(i, out),
        Type::Tuple(xs) => xs.iter().for_each(|x| mentioned(x, out)),
        Type::Record(fs) => fs.iter().for_each(|(_, x)| mentioned(x, out)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|x| mentioned(x, out));
            mentioned(ret, out);
        }
        Type::TypeParam(_) | Type::Unknown(_) => {}
    }
}

/// The position in a label `…@L:C` or `…@L`, for ordering generated functions in source order.
fn at_pos(label: &str) -> (u32, u32) {
    let at = label.rsplit('@').next().unwrap_or("");
    let mut it = at.split(':').map(|x| x.parse::<u32>().unwrap_or(u32::MAX));
    (it.next().unwrap_or(u32::MAX), it.next().unwrap_or(u32::MAX))
}

/// A function's identity before lambdas: its qualified name, with an aspect hook's line replaced by its ordinal.
fn named_identities(p: &Program) -> Vec<String> {
    let mut ids: Vec<String> = p.functions.iter().map(|f| f.name.clone()).collect();
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, f) in p.functions.iter().enumerate() {
        if f.label.starts_with("aspect ") && f.label.contains('@') {
            let stem = f.label.rsplit_once('@').map(|(s, _)| s.to_string()).unwrap_or_default();
            groups.entry((f.module.clone(), stem)).or_default().push(i);
        }
    }
    for ((module, stem), mut members) in groups {
        members.sort_by_key(|&i| (at_pos(&p.functions[i].label), i));
        for (k, i) in members.into_iter().enumerate() {
            ids[i] = format!("{module}::{stem}#{}", k + 1);
        }
    }
    // the woven parts of an aspect-decorated function follow their hook's identity
    let base: HashMap<String, String> = p.functions.iter().zip(ids.iter()).filter(|(f, id)| f.name != **id).map(|(f, id)| (f.name.clone(), id.clone())).collect();
    for (i, f) in p.functions.iter().enumerate() {
        if let Some((stem, suffix)) = f.name.rsplit_once('#') {
            if let Some(b) = base.get(stem) {
                ids[i] = format!("{b}#{suffix}");
            }
        }
    }
    ids
}

// ---- building a generation ---------------------------------------------------------------------------------------

struct Draft {
    summary: u64,
    parts: Vec<(&'static str, u64, String)>,
    env: u64,
    env_shown: String,
    body: u64,
    generic: bool,
    params: Vec<Type>,
    captures: Vec<Type>,
    refs: Refs,
}

fn draft(f: &Function, names: &HashMap<String, String>) -> Draft {
    let j = json::function(f);
    let ty_of = |l: u32| f.locals.get(l as usize).map(|d| d.ty.clone()).unwrap_or(Type::Unknown(String::new()));
    let params: Vec<Type> = f.params.iter().map(|p| ty_of(p.local)).collect();
    let shown_params = f
        .params
        .iter()
        .map(|p| {
            let d = p.default.as_ref().map(|e| format!(" = {}", text(&canon(&json::expr(e), names)))).unwrap_or_default();
            format!("{}: {:?} {}{d}", p.name, p.mode, ty_of(p.local))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut mods = f.modifiers.clone();
    mods.sort();
    let effects = format!("{:?} {:?} {:?}", f.effects.effects, f.effects.footprint, f.effects.authorities);
    // generic: a type parameter anywhere in the function's IR (a type node `param` has no `local`)
    let mut tps = Vec::new();
    each(&j, "param", &mut tps);
    let generic = tps.iter().any(|n| !has_key(n, "local"));
    let mut parts: Vec<(&'static str, String)> = vec![
        ("parameters", format!("({shown_params})")),
        ("return", f.ret.to_string()),
        ("modifiers", mods.join(" ")),
        ("effects", effects),
        ("observation", f.observe.clone().unwrap_or_default()),
        ("generic", generic.to_string()),
    ];
    // a generic function's `where` bounds are part of its contract: changing one is not a compatible reload (only
    // a function that has bounds carries the part, so other summaries are unchanged)
    if !f.bounds.is_empty() {
        let mut bs: Vec<String> = f.bounds.iter().map(|(p, i, a)| format!("{p}: {i}[{}]", a.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", "))).collect();
        bs.sort();
        parts.push(("bounds", bs.join(", ")));
    }
    let parts: Vec<(&'static str, u64, String)> = parts.into_iter().map(|(k, s)| (k, fnv(&s), s)).collect();
    let summary = fnv(&parts.iter().map(|(k, h, _)| format!("{k}={h:x}")).collect::<Vec<_>>().join(";"));
    let (env_shown, captures) = match &f.closure {
        Some(c) => (
            format!(
                "{} [{}]",
                c.kind,
                c.captures.iter().map(|cd| format!("{} {:?} {}{}", cd.name, cd.mode, ty_of(cd.local), if f.locals.get(cd.local as usize).map(|d| d.boxed).unwrap_or(false) { " boxed" } else { "" })).collect::<Vec<_>>().join(", ")
            ),
            c.captures.iter().map(|cd| ty_of(cd.local)).collect(),
        ),
        None => (String::new(), vec![]),
    };
    let body = {
        let mut m = BTreeMap::new();
        if let Some(b) = &f.body {
            m.insert("body".to_string(), canon(&json::block(b), names));
        }
        m.insert("locals".into(), Json::Arr(f.locals.iter().map(|d| Json::Str(format!("{} {} {} {:?} {}", d.name, d.ty, d.mutable, d.kind, d.boxed))).collect()));
        fnv(&text(&Json::Obj(m)))
    };
    let mut refs = Refs::default();
    for kind in ["func", "funcref", "closure"] {
        let mut ns = Vec::new();
        each(&j, kind, &mut ns);
        for n in ns {
            if let Some(name) = field(n, "name").or_else(|| field(n, "func")) {
                refs.funcs.push(name.to_string());
            }
        }
    }
    let mut ns = Vec::new();
    each(&j, "virtual", &mut ns);
    refs.virtuals = ns.iter().filter_map(|n| field(n, "method")).map(str::to_string).collect();
    let mut ns = Vec::new();
    each(&j, "global", &mut ns);
    refs.consts = ns.iter().filter_map(|n| field(n, "name")).map(str::to_string).collect();
    // the types of calls through values, and every type mentioned
    if let Some(b) = &f.body {
        let mut calls = Vec::new();
        walk_value_calls(b, &mut calls);
        refs.values = calls;
    }
    let mut ns = Vec::new();
    each(&j, "named", &mut ns);
    refs.types = ns.iter().filter_map(|n| field(n, "name")).map(str::to_string).collect();
    refs.types.sort();
    refs.types.dedup();
    Draft { summary, parts, env: fnv(&env_shown), env_shown, body, generic, params, captures, refs }
}

/// The callee types of calls through values in a block (Dep's calls through values).
fn walk_value_calls(b: &Block, out: &mut Vec<(Vec<Type>, Type)>) {
    let j = json::block(b);
    let mut ns = Vec::new();
    each(&j, "value", &mut ns);
    for n in ns {
        if let Json::Obj(m) = n {
            if let Some(v) = m.get("value") {
                if let Json::Obj(vm) = v {
                    if let Some(t) = vm.get("ty") {
                        if let Ok(Type::Fn { params, ret, .. }) = json::de_ty(t) {
                            out.push((params, *ret));
                        }
                    }
                }
            }
        }
    }
}

impl Generation {
    /// Build a generation. `prev` is its predecessor, whose lambda identities it inherits (design 3); `di` names each
    /// DI context with the classes the elaborator generated for it.
    pub fn new(id: u32, program: Program, source: &str, sha256: &str, di: &[(String, Vec<String>)], generic: &BTreeSet<String>, prev: Option<&Generation>) -> Generation {
        LIVE.fetch_add(1, Ordering::SeqCst);
        let index: HashMap<&str, usize> = program.functions.iter().enumerate().map(|(i, f)| (f.name.as_str(), i)).collect();
        // lambda parents and their lambdas in source order
        let mut parent: HashMap<usize, usize> = HashMap::new();
        let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, f) in program.functions.iter().enumerate() {
            let fj = json::function(f);
            let mut nodes = Vec::new();
            each(&fj, "closure", &mut nodes);
            for n in nodes {
                if let Some(&li) = field(n, "func").and_then(|name| index.get(name)) {
                    if li != i && !parent.contains_key(&li) && program.functions[li].closure.is_some() {
                        parent.insert(li, i);
                        children.entry(i).or_default().push(li);
                    }
                }
            }
        }
        for kids in children.values_mut() {
            kids.sort_by_key(|&k| (at_pos(&program.functions[k].label), k));
        }
        let mut ids = named_identities(&program);
        // names in bodies: an aspect hook by its identity; a lambda by its structure (its environment and body,
        // its own lambdas named the same way), so two lambdas differing only in what they create never look alike
        let mut names: HashMap<String, String> = HashMap::new();
        for (i, f) in program.functions.iter().enumerate() {
            if f.closure.is_none() && ids[i] != f.name {
                names.insert(f.name.clone(), ids[i].clone());
            }
        }
        let depth = |mut i: usize| {
            let mut d = 0;
            while let Some(&p) = parent.get(&i) {
                d += 1;
                i = p;
                if d > 64 {
                    break;
                }
            }
            d
        };
        let mut lambdas: Vec<usize> = (0..program.functions.len()).filter(|i| parent.contains_key(i)).collect();
        lambdas.sort_by_key(|&i| std::cmp::Reverse(depth(i)));
        let mut drafts: Vec<Option<Draft>> = (0..program.functions.len()).map(|_| None).collect();
        for &i in &lambdas {
            let d = draft(&program.functions[i], &names);
            let shape = if crate::session::fault("nested-lambda-flat") { "λ".to_string() } else { format!("λ{:x}.{:x}", d.env, d.body) };
            names.insert(program.functions[i].name.clone(), shape);
            drafts[i] = Some(d);
        }
        for (i, f) in program.functions.iter().enumerate() {
            if drafts[i].is_none() {
                drafts[i] = Some(draft(f, &names));
            }
        }
        let mut drafts: Vec<Draft> = drafts.into_iter().map(|d| d.unwrap()).collect();
        // lambda identities, parents first: inherited from the predecessor's lambdas of the same parent
        let mut order: Vec<usize> = (0..program.functions.len()).filter(|i| !parent.contains_key(i)).collect();
        let mut k = 0;
        while k < order.len() {
            let p = order[k];
            k += 1;
            let kids = match children.get(&p) {
                Some(ks) => ks.clone(),
                None => continue,
            };
            let pid = ids[p].clone();
            let old: Vec<(String, u64, u64)> = match prev {
                Some(g) => {
                    let mut v: Vec<(&String, &FnInfo)> = g.funcs.iter().filter(|(_, fi)| fi.parent.as_deref() == Some(pid.as_str())).collect();
                    v.sort_by_key(|(_, fi)| fi.index);
                    v.into_iter().map(|(id, fi)| (id.clone(), fi.env, fi.body)).collect()
                }
                None => vec![],
            };
            let mut taken = vec![false; old.len()];
            let mut got: Vec<Option<String>> = vec![None; kids.len()];
            let ordinal = crate::session::fault("lambda-ordinal");
            for (ki, &c) in kids.iter().enumerate() {
                if ordinal {
                    if ki < old.len() {
                        taken[ki] = true;
                        got[ki] = Some(old[ki].0.clone());
                    }
                    continue;
                }
                if let Some(oi) = (0..old.len()).find(|&oi| !taken[oi] && old[oi].1 == drafts[c].env && old[oi].2 == drafts[c].body) {
                    taken[oi] = true;
                    got[ki] = Some(old[oi].0.clone());
                }
            }
            // a lambda left over whose environment is unique among the leftovers on both sides is a body edit of the
            // old lambda with that environment; leftovers sharing an environment cannot be told apart
            let free_new: Vec<usize> = (0..kids.len()).filter(|&ki| got[ki].is_none()).collect();
            let free_old: Vec<usize> = (0..old.len()).filter(|&oi| !taken[oi]).collect();
            if !ordinal {
                for &ki in &free_new {
                    let env = drafts[kids[ki]].env;
                    let news = free_new.iter().filter(|&&k| drafts[kids[k]].env == env).count();
                    let olds: Vec<usize> = free_old.iter().copied().filter(|&oi| old[oi].1 == env).collect();
                    if news == 1 && olds.len() == 1 {
                        got[ki] = Some(old[olds[0]].0.clone());
                    }
                }
                // a single lambda left on each side is the same lambda with a changed environment
                let free_new: Vec<usize> = (0..kids.len()).filter(|&ki| got[ki].is_none()).collect();
                let used: BTreeSet<&String> = got.iter().flatten().collect();
                let free_old: Vec<usize> = (0..old.len()).filter(|&oi| !taken[oi] && !used.contains(&old[oi].0)).collect();
                if free_new.len() == 1 && free_old.len() == 1 {
                    got[free_new[0]] = Some(old[free_old[0]].0.clone());
                }
            }
            // library code is the same in every generation: its lambdas keep their ordinal identities
            let user = is_user_module(&program.functions[p].module);
            for (ki, &c) in kids.iter().enumerate() {
                ids[c] = match &got[ki] {
                    Some(s) => s.clone(),
                    None if prev.is_some() && user => format!("{pid}/λ@{id}.{}", ki + 1),
                    None => format!("{pid}/λ{}", ki + 1),
                };
                order.push(c);
            }
        }
        // declared genericity (the frontend's declarations, design 3): a function declared with type parameters, a
        // method of a generic type, and the lambdas inside either, whether or not a type parameter survives in the IR
        for i in 0..program.functions.len() {
            let (mut j, mut declared, mut hops) = (i, false, 0);
            loop {
                if generic.contains(&ids[j]) {
                    declared = true;
                    break;
                }
                match parent.get(&j) {
                    Some(&p) if hops < 64 => {
                        j = p;
                        hops += 1;
                    }
                    _ => break,
                }
            }
            if declared && !drafts[i].generic && !crate::session::fault("generic-by-use") {
                let d = &mut drafts[i];
                d.generic = true;
                for part in d.parts.iter_mut().filter(|p| p.0 == "generic") {
                    part.2 = "true".into();
                    part.1 = fnv("true");
                }
                d.summary = fnv(&d.parts.iter().map(|(k, h, _)| format!("{k}={h:x}")).collect::<Vec<_>>().join(";"));
            }
        }
        let generated: HashMap<&str, &str> = di.iter().flat_map(|(c, names)| names.iter().map(move |n| (n.as_str(), c.as_str()))).collect();
        let name_ident: HashMap<String, String> = program.functions.iter().zip(ids.iter()).map(|(f, id)| (f.name.clone(), id.clone())).collect();
        // types: layouts, then deep fingerprints over the type graph
        let mut types = BTreeMap::new();
        let mut type_ident = HashMap::new();
        for t in program.types.iter().filter(|t| is_user_module(&t.module)) {
            let fields = |fs: &[FieldDef]| {
                let mut v: Vec<String> = fs.iter().map(|f| format!("{}: {}", f.name, f.ty)).collect();
                if crate::session::fault("layout-unordered") {
                    v.sort();
                }
                v.join(", ")
            };
            let (kind, edges): (String, Vec<(String, Type)>) = match &t.kind {
                TypeKind::Class { fields: fs } => (format!("class {{{}}}", fields(fs)), fs.iter().map(|f| (format!("{}.{}", t.name, f.name), f.ty.clone())).collect()),
                TypeKind::Enum { variants } => (
                    format!("enum {{{}}}", variants.iter().map(|v| format!("{}({})", v.name, v.fields.iter().map(|(n, ty)| format!("{n}: {ty}")).collect::<Vec<_>>().join(", "))).collect::<Vec<_>>().join(" | ")),
                    variants.iter().flat_map(|v| v.fields.iter().map(move |(n, ty)| (format!("{}.{}.{}", t.name, v.name, n), ty.clone()))).collect(),
                ),
                TypeKind::Newtype { repr } => (format!("newtype {repr}"), vec![(format!("{} (representation)", t.name), repr.clone())]),
                TypeKind::Error { code, payload, .. } => (format!("error {code} {{{}}}", fields(payload)), payload.iter().map(|f| (format!("{}.{}", t.name, f.name), f.ty.clone())).collect()),
                TypeKind::Interface { methods } => (format!("interface {methods:?}"), vec![]),
                TypeKind::Mixin { provides, requires } => (format!("mixin {provides:?} {requires:?}"), vec![]),
                TypeKind::Observation { fields: fs, version } => (format!("observation v{version} {fs:?}"), vec![]),
            };
            let shown = format!(
                "{kind} params {:?} lifetimes {:?}{}{}{}",
                t.type_params,
                t.lifetimes,
                if t.trivial { " trivial" } else { "" },
                if t.copyable { " copyable" } else { "" },
                if t.cleanup.is_some() { " cleanup" } else { "" }
            );
            let ident = format!("{}::{}", t.module, t.name);
            type_ident.insert(t.name.clone(), ident.clone());
            let methods = t.methods.iter().map(|(m, f)| (m.clone(), name_ident.get(f).cloned().unwrap_or_else(|| f.clone()))).collect();
            types.insert(
                ident,
                TypeInfo {
                    name: t.name.clone(),
                    params: t.type_params.clone(),
                    layout: fnv(&shown),
                    shown,
                    deep: 0,
                    edges,
                    implements: t.implements.clone(),
                    methods,
                    interface: matches!(t.kind, TypeKind::Interface { .. }),
                    error: matches!(t.kind, TypeKind::Error { .. }),
                    di: generated.get(t.name.as_str()).map(|c| c.to_string()),
                },
            );
        }
        deep_types(&mut types, &type_ident);
        let deep_of = |ts: &[Type]| -> String {
            let mut names = Vec::new();
            ts.iter().for_each(|t| mentioned(t, &mut names));
            names.sort();
            names.dedup();
            names.iter().filter_map(|n| type_ident.get(n)).map(|id| format!("{id}={:x}", types[id].deep)).collect::<Vec<_>>().join(",")
        };
        let mut code = Vec::with_capacity(program.functions.len());
        let mut funcs = BTreeMap::new();
        for (i, (f, d)) in program.functions.iter().zip(drafts.into_iter()).enumerate() {
            let mut all: Vec<Type> = d.params.clone();
            all.push(f.ret.clone());
            all.extend(d.captures.iter().cloned());
            let deep = fnv(&format!("{:x}/{:x}/{}", d.summary, d.env, deep_of(&all)));
            let mut slots: Vec<u32> = f.params.iter().map(|p| p.local).collect();
            if let Some(c) = &f.closure {
                slots.extend(c.captures.iter().map(|cd| cd.local));
            }
            code.push(Rc::new(CodeId { gen: id, ident: Rc::from(ids[i].as_str()), deep, slots }));
            if !is_user_module(&f.module) {
                continue;
            }
            let owner = f.label.strip_prefix("method ").and_then(|m| m.split('.').next()).or_else(|| f.label.strip_prefix("cleanup "));
            let di_ctx = owner.and_then(|o| generated.get(o)).map(|c| c.to_string());
            funcs.insert(
                ids[i].clone(),
                FnInfo {
                    index: i,
                    label: f.label.clone(),
                    summary: d.summary,
                    parts: d.parts,
                    env: d.env,
                    env_shown: d.env_shown,
                    deep,
                    body: d.body,
                    generic: d.generic,
                    parent: parent.get(&i).map(|&p| ids[p].clone()),
                    is_async: f.has("async"),
                    fails: f.has("try") || f.has("recovers"),
                    params: d.params,
                    ret: f.ret.clone(),
                    captures: d.captures,
                    refs: d.refs,
                    di: di_ctx,
                },
            );
        }
        let consts = program
            .consts
            .iter()
            .filter(|c| is_user_module(&c.module))
            .map(|c| {
                let shown = format!("{}: {} = {}", c.name, c.ty, text(&canon(&json::expr(&c.value), &names)));
                (format!("{}::{}", c.module, c.name), (fnv(&shown), shown))
            })
            .collect();
        // what each constant's initializer refers to (Dep follows it: a constant read through another, design 6)
        let const_refs = program
            .consts
            .iter()
            .filter(|c| is_user_module(&c.module))
            .map(|c| {
                let j = json::expr(&c.value);
                let mut r = Refs::default();
                for (kind, key, out) in [("global", "name", &mut r.consts), ("funcref", "name", &mut r.funcs), ("named", "name", &mut r.types)] {
                    let mut ns = Vec::new();
                    each(&j, kind, &mut ns);
                    out.extend(ns.iter().filter_map(|n| field(n, key)).map(str::to_string));
                }
                let mut ty = Vec::new();
                mentioned(&c.ty, &mut ty);
                r.types.extend(ty);
                (format!("{}::{}", c.module, c.name), r)
            })
            .collect();
        let by_ident = code.iter().enumerate().map(|(i, c)| (c.ident.clone(), i)).collect();
        let bytes = text(&json::program(&program)).len();
        Generation { id, program, code, by_ident, funcs, types, name_ident, type_ident, consts, const_refs, bytes, source: source.to_string(), sha256: sha256.to_string() }
    }

    /// The function of this generation a code value of another generation resolves to (design 5), or why it
    /// cannot.
    pub fn resolve(&self, c: &CodeId) -> Result<usize, String> {
        let i = match self.by_ident.get(&c.ident) {
            Some(i) => *i,
            None => return Err(format!("`{}` (generation {}) does not exist in generation {}", c.ident, c.gen, self.id)),
        };
        if self.code[i].deep != c.deep {
            return Err(format!("`{}` (generation {}) has a different summary, environment or captured layout in generation {}", c.ident, c.gen, self.id));
        }
        Ok(i)
    }

    /// A user function by its source name (`openShop`, `Shop.add`), as the script names factories and handlers.
    pub fn function_named(&self, name: &str) -> Option<(&String, &FnInfo)> {
        self.funcs.iter().find(|(_, f)| f.parent.is_none() && (f.label == format!("function {name}") || f.label == format!("method {name}")))
    }
}

/// Deep fingerprints over the type graph: each strongly connected component hashes its members' layouts and the
/// deep fingerprints of what it points to outside itself.
fn deep_types(types: &mut BTreeMap<String, TypeInfo>, type_ident: &HashMap<String, String>) {
    let ids: Vec<String> = types.keys().cloned().collect();
    let pos: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    let succ: Vec<Vec<usize>> = ids
        .iter()
        .map(|id| {
            let mut names = Vec::new();
            // the fault switch `r1-no-payloads` leaves variant payloads out of the deep fingerprint too
            types[id].edges.iter().filter(|(l, _)| !(crate::session::fault("r1-no-payloads") && l.matches('.').count() >= 2)).for_each(|(_, t)| mentioned(t, &mut names));
            let mut v: Vec<usize> = names.iter().filter_map(|n| type_ident.get(n)).filter_map(|i| pos.get(i.as_str()).copied()).collect();
            v.sort();
            v.dedup();
            v
        })
        .collect();
    // Tarjan's algorithm; components come out in reverse topological order (dependencies first)
    struct T<'a> {
        succ: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        comps: Vec<Vec<usize>>,
    }
    fn visit(t: &mut T, v: usize) {
        t.index[v] = Some(t.next);
        t.low[v] = t.next;
        t.next += 1;
        t.stack.push(v);
        t.on[v] = true;
        for &w in t.succ[v].clone().iter() {
            match t.index[w] {
                None => {
                    visit(t, w);
                    t.low[v] = t.low[v].min(t.low[w]);
                }
                Some(iw) if t.on[w] => t.low[v] = t.low[v].min(iw),
                _ => {}
            }
        }
        if Some(t.low[v]) == t.index[v] {
            let mut comp = Vec::new();
            while let Some(w) = t.stack.pop() {
                t.on[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            t.comps.push(comp);
        }
    }
    let n = ids.len();
    let mut t = T { succ: &succ, index: vec![None; n], low: vec![0; n], on: vec![false; n], stack: vec![], next: 0, comps: vec![] };
    for v in 0..n {
        if t.index[v].is_none() {
            visit(&mut t, v);
        }
    }
    let mut deep = vec![0u64; n];
    for comp in &t.comps {
        let inside: BTreeSet<usize> = comp.iter().copied().collect();
        let mut members: Vec<String> = comp.iter().map(|&v| format!("{}={:x}", ids[v], types[&ids[v]].layout)).collect();
        members.sort();
        let mut outside: Vec<String> = comp.iter().flat_map(|&v| succ[v].iter().copied()).filter(|w| !inside.contains(w)).map(|w| format!("{}={:x}", ids[w], deep[w])).collect();
        outside.sort();
        outside.dedup();
        let h = fnv(&format!("{}|{}", members.join(","), outside.join(",")));
        for &v in comp {
            deep[v] = fnv(&format!("{}:{h:x}", ids[v]));
        }
    }
    for (i, id) in ids.iter().enumerate() {
        if let Some(ti) = types.get_mut(id) {
            ti.deep = deep[i];
        }
    }
}

// ---- classification (design 4) -----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Change {
    pub ident: String,
    /// `body`, `added`, `generic-body`, `summary`, `environment`, `removed`, `layout`, `dispatch`, `constant` or `di`
    pub kind: &'static str,
    /// the kind without the DI attribution
    pub base: &'static str,
    pub compatible: bool,
    pub detail: String,
    /// what changed: `function`, `type`, `member` (a method or an interface of a type) or `constant`
    pub decl: &'static str,
}

impl Change {
    fn new(ident: &str, kind: &'static str, detail: String, decl: &'static str, di: Option<&String>) -> Change {
        let base = kind;
        let (kind, detail) = match di {
            Some(c) => ("di", format!("context {c}: {kind}{}{detail}", if detail.is_empty() { "" } else { ": " })),
            None => (kind, detail),
        };
        let compatible = matches!(kind, "body" | "added") || (kind == "di" && base == "body" && crate::session::fault("di-body-compatible"));
        Change { ident: ident.to_string(), kind, base, compatible, detail, decl }
    }
}

pub fn classify(old: &Generation, new: &Generation) -> Vec<Change> {
    let mut out = Vec::new();
    let idents: BTreeSet<&String> = old.funcs.keys().chain(new.funcs.keys()).collect();
    for id in idents {
        match (old.funcs.get(id), new.funcs.get(id)) {
            (None, Some(n)) => out.push(Change::new(id, "added", String::new(), "function", n.di.as_ref())),
            (Some(o), None) => out.push(Change::new(id, "removed", String::new(), "function", o.di.as_ref())),
            (Some(o), Some(n)) => {
                let di = n.di.as_ref().or(o.di.as_ref());
                if o.summary != n.summary {
                    let parts: Vec<String> = o.parts.iter().zip(n.parts.iter()).filter(|(a, b)| a.1 != b.1).map(|(a, b)| format!("{}: {} → {}", a.0, a.2, b.2)).collect();
                    out.push(Change::new(id, "summary", parts.join("; "), "function", di));
                } else if o.env != n.env && !crate::session::fault("env-ignored") {
                    out.push(Change::new(id, "environment", format!("{} → {}", o.env_shown, n.env_shown), "function", di));
                } else if o.body != n.body {
                    out.push(Change::new(id, if o.generic { "generic-body" } else { "body" }, String::new(), "function", di));
                } else if di.is_some() && o.deep != n.deep {
                    out.push(Change::new(id, "layout", "a type it mentions changed".into(), "function", di));
                }
            }
            (None, None) => {}
        }
    }
    let idents: BTreeSet<&String> = old.types.keys().chain(new.types.keys()).collect();
    for id in idents {
        match (old.types.get(id), new.types.get(id)) {
            (None, Some(n)) => out.push(Change::new(id, "added", String::new(), "type", n.di.as_ref())),
            (Some(o), None) => out.push(Change::new(id, "removed", String::new(), "type", o.di.as_ref())),
            (Some(o), Some(n)) => {
                let di = n.di.as_ref().or(o.di.as_ref());
                if o.layout != n.layout {
                    out.push(Change::new(id, "layout", format!("{} → {}", o.shown, n.shown), "type", di));
                }
                for (m, f) in o.methods.iter().filter(|_| !crate::session::fault("dispatch-ignored")) {
                    match n.methods.get(m) {
                        None => out.push(Change::new(id, "removed", format!("method {m}"), "member", di)),
                        Some(g) if g != f => out.push(Change::new(id, "dispatch", format!("method {m}: {f} → {g}"), "member", di)),
                        _ => {}
                    }
                }
                for m in n.methods.keys().filter(|m| !o.methods.contains_key(*m)) {
                    out.push(Change::new(id, "added", format!("method {m}"), "member", di));
                }
                for i in o.implements.iter().filter(|i| !n.implements.contains(i)) {
                    out.push(Change::new(id, "removed", format!("implements {i}"), "member", di));
                }
                for i in n.implements.iter().filter(|i| !o.implements.contains(i)) {
                    out.push(Change::new(id, "added", format!("implements {i}"), "member", di));
                }
            }
            (None, None) => {}
        }
    }
    let idents: BTreeSet<&String> = old.consts.keys().chain(new.consts.keys()).collect();
    for id in idents {
        match (old.consts.get(id), new.consts.get(id)) {
            (None, Some(_)) => out.push(Change::new(id, "added", String::new(), "constant", None)),
            (Some(_), None) => out.push(Change::new(id, "removed", String::new(), "constant", None)),
            (Some(o), Some(n)) if o.0 != n.0 => out.push(Change::new(id, "constant", format!("{} → {}", o.1, n.1), "constant", None)),
            _ => {}
        }
    }
    out
}

// ---- Rep and Dep (design 6) --------------------------------------------------------------------------------------

/// Structural equality with type parameters and unknowns as wildcards; closure kinds and effect modifiers ignored.
pub fn unify(a: &Type, b: &Type) -> bool {
    use Type::*;
    let base = |t: &Type| -> Option<(String, Vec<Type>)> {
        match t {
            Named(n) => Some((n.name.clone(), n.args.clone())),
            Refined { base, .. } => Some((base.name.clone(), base.args.clone())),
            _ => None,
        }
    };
    match (a, b) {
        (TypeParam(_), _) | (_, TypeParam(_)) | (Unknown(_), _) | (_, Unknown(_)) => true,
        (Loan { inner, .. }, o) | (o, Loan { inner, .. }) => unify(inner, o),
        (Optional(x), Optional(y)) => unify(x, y),
        (Tuple(xs), Tuple(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| unify(x, y)),
        (Record(xs), Record(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|((n, x), (m, y))| n == m && unify(x, y)),
        (Fn { params: p, ret: r, .. }, Fn { params: q, ret: s, .. }) => p.len() == q.len() && p.iter().zip(q).all(|(x, y)| unify(x, y)) && unify(r, s),
        (Dynamic(x), Dynamic(y)) => x.name == y.name,
        _ => match (base(a), base(b)) {
            (Some((n, xs)), Some((m, ys))) => n == m && xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| unify(x, y)),
            _ => false,
        },
    }
}

/// A declaration a state can hold values of, or depend on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Item {
    Type(String),
    Code(String),
    /// (interface, implementor): a value of the implementor held as the interface
    Impl(String, String),
    Const(String),
}

impl Item {
    pub fn ident(&self) -> &str {
        match self {
            Item::Type(s) | Item::Code(s) | Item::Const(s) => s,
            Item::Impl(_, t) => t,
        }
    }
}

/// A type with its type parameters replaced by a use's type arguments.
fn subst(t: &Type, m: &HashMap<&str, &Type>) -> Type {
    use fortis_types::types::NamedT;
    let named = |n: &NamedT| NamedT { name: n.name.clone(), args: n.args.iter().map(|a| subst(a, m)).collect(), package: n.package.clone(), origin: n.origin.clone() };
    match t {
        Type::TypeParam(p) => m.get(p.as_str()).map(|x| (*x).clone()).unwrap_or_else(|| t.clone()),
        Type::Named(n) => Type::Named(named(n)),
        Type::Dynamic(n) => Type::Dynamic(named(n)),
        Type::Refined { base, lo, hi, kind } => Type::Refined { base: named(base), lo: *lo, hi: *hi, kind: kind.clone() },
        Type::Optional(i) => Type::Optional(Box::new(subst(i, m))),
        Type::Tuple(xs) => Type::Tuple(xs.iter().map(|x| subst(x, m)).collect()),
        Type::Record(fs) => Type::Record(fs.iter().map(|(n, x)| (n.clone(), subst(x, m))).collect()),
        Type::Fn { capture_origins, param_modes, params, ret, kind, modifiers, origin, dynamic, identity } => Type::Fn {
            capture_origins: capture_origins.clone(),
            param_modes: param_modes.clone(),
            params: params.iter().map(|x| subst(x, m)).collect(),
            ret: Box::new(subst(ret, m)),
            kind: kind.clone(),
            modifiers: modifiers.clone(),
            origin: origin.clone(),
            dynamic: *dynamic,
            identity: identity.clone(),
        },
        Type::Loan { inner, mode, origin } => Type::Loan { inner: Box::new(subst(inner, m)), mode: mode.clone(), origin: origin.clone() },
        Type::Unknown(_) => t.clone(),
    }
}

/// The function type of a callback position (`fn(…) …`, `dynamic fn(…) …`).
fn callback(t: &Type) -> Option<(Vec<Type>, Type)> {
    match t {
        Type::Fn { params, ret, .. } => Some((params.clone(), (**ret).clone())),
        Type::Loan { inner, .. } => callback(inner),
        _ => None,
    }
}

/// Rep_g(τ) (design 6): every declaration whose values a state of type τ can hold under g, with a path to it.
pub fn rep(g: &Generation, root: &Type) -> BTreeMap<Item, Vec<String>> {
    rep_walk(g, root, false)
}

/// The part of Rep_g(τ) behind interior mutability (R2): a value made after the reload can enter carried state only
/// through a `Mutex`, since a `Shared` value is otherwise immutable and atomics hold integers and booleans. The walk
/// is Rep's own, through captures, implementors, future arguments and failures, marking what lies under a `Mutex`.
pub fn rep_mutable(g: &Generation, root: &Type) -> BTreeMap<Item, Vec<String>> {
    rep_walk(g, root, true)
}

fn rep_walk(g: &Generation, root: &Type, mutable_only: bool) -> BTreeMap<Item, Vec<String>> {
    let mut out: BTreeMap<Item, Vec<String>> = BTreeMap::new();
    // what has been walked, and whether under a Mutex: a declaration reached outside one may be reached inside later
    let mut seen: BTreeSet<(String, bool)> = BTreeSet::new();
    let mut q: VecDeque<(Type, Vec<String>, bool)> = VecDeque::new();
    q.push_back((root.clone(), vec![format!("state: {root}")], false));
    // the fault switch `r2-shallow`: R2's walk follows only fields and type arguments (the first version's)
    let shallow = mutable_only && crate::session::fault("r2-shallow");
    let record = |out: &mut BTreeMap<Item, Vec<String>>, item: Item, path: &Vec<String>, under: bool| {
        if !mutable_only || under {
            out.entry(item).or_insert_with(|| path.clone());
        }
    };
    let errors: Vec<(String, String)> = g.types.iter().filter(|(_, t)| t.error).map(|(id, t)| (id.clone(), t.name.clone())).collect();
    while let Some((t, path, under)) = q.pop_front() {
        let step = |p: &Vec<String>, s: String| {
            let mut x = p.clone();
            x.push(s);
            x
        };
        if let Some((ps, rt)) = callback(&t) {
            for (id, f) in &g.funcs {
                if f.params.len() == ps.len() && f.params.iter().zip(&ps).all(|(a, b)| unify(a, b)) && unify(&f.ret, &rt) && seen.insert((format!("C:{id}"), under)) {
                    let p = step(&path, format!("value of {id}"));
                    record(&mut out, Item::Code(id.clone()), &p, under);
                    if !shallow {
                        for (k, ct) in f.captures.iter().enumerate() {
                            q.push_back((ct.clone(), step(&p, format!("capture {}: {ct}", k + 1)), under));
                        }
                    }
                    if f.fails && seen.insert(("F".into(), under)) {
                        for (eid, en) in &errors {
                            q.push_back((fortis_types::types::named(en), step(&p, format!("failure {eid}")), under));
                        }
                    }
                }
            }
            continue;
        }
        match &t {
            Type::Named(n) | Type::Refined { base: n, .. } => {
                if let Some(id) = g.type_ident.get(&n.name) {
                    // an instantiation is walked with its arguments in place of the type's parameters, so a generic
                    // field (`Bag[T] { items: Mutex[List[T]] }`) reaches what `Bag[fn(Int) Int]` holds
                    let shown = format!("{t}");
                    if shown.len() < 2000 && seen.insert((format!("T:{shown}"), under)) {
                        record(&mut out, Item::Type(id.clone()), &path, under);
                        let ti = &g.types[id];
                        if ti.interface {
                            q.push_back((Type::Dynamic(n.clone()), path.clone(), under));
                        }
                        let args: HashMap<&str, &Type> = if crate::session::fault("r2-no-subst") { HashMap::new() } else { ti.params.iter().map(|p| p.as_str()).zip(n.args.iter()).collect() };
                        for (label, ft) in &ti.edges {
                            let ft = &subst(ft, &args);
                            // the fault switch `r1-no-payloads` skips variant payloads (`Type.Variant.field`)
                            if crate::session::fault("r1-no-payloads") && label.matches('.').count() >= 2 {
                                continue;
                            }
                            q.push_back((ft.clone(), step(&path, format!("{label}: {ft}")), under));
                        }
                    }
                } else if n.name == "Mutex" && !n.args.is_empty() {
                    q.push_back((n.args[0].clone(), step(&path, format!("Mutex[{}] (interior mutability)", n.args[0])), true));
                    continue;
                } else if (n.name == "Future" || n.name == "Task") && !n.args.is_empty() {
                    for (id, f) in &g.funcs {
                        if f.is_async && unify(&f.ret, &n.args[0]) && seen.insert((format!("C:{id}"), under)) {
                            let p = step(&path, format!("future of {id}"));
                            record(&mut out, Item::Code(id.clone()), &p, under);
                            if !shallow {
                                for (k, ct) in f.captures.iter().enumerate() {
                                    q.push_back((ct.clone(), step(&p, format!("capture {}: {ct}", k + 1)), under));
                                }
                                for at in &f.params {
                                    q.push_back((at.clone(), step(&p, format!("argument: {at}")), under));
                                }
                            }
                            if f.fails && seen.insert(("F".into(), under)) {
                                for (eid, en) in &errors {
                                    q.push_back((fortis_types::types::named(en), step(&p, format!("failure {eid}")), under));
                                }
                            }
                        }
                    }
                } else if n.name == "Failure" && seen.insert(("F".into(), under)) {
                    for (eid, en) in &errors {
                        q.push_back((fortis_types::types::named(en), step(&path, format!("failure {eid}")), under));
                    }
                }
                for a in &n.args {
                    q.push_back((a.clone(), path.clone(), under));
                }
            }
            Type::Dynamic(n) => {
                if let Some(id) = g.type_ident.get(&n.name) {
                    if seen.insert((format!("T:{id}"), under)) {
                        record(&mut out, Item::Type(id.clone()), &path, under);
                    }
                }
                let iface = g.type_ident.get(&n.name).cloned().unwrap_or_else(|| n.name.clone());
                for (id, ti) in &g.types {
                    if ti.implements.iter().any(|i| i == &n.name || i.rsplit(['.', ':']).next() == Some(n.name.as_str())) && seen.insert((format!("I:{iface}:{id}"), under)) {
                        let p = step(&path, format!("dynamic {} → {}", n.name, ti.name));
                        record(&mut out, Item::Impl(iface.clone(), id.clone()), &p, under);
                        if !shallow {
                            q.push_back((fortis_types::types::named(&ti.name), p, under));
                        }
                    }
                }
            }
            Type::Optional(i) | Type::Loan { inner: i, .. } => q.push_back(((**i).clone(), path.clone(), under)),
            Type::Tuple(xs) => xs.iter().for_each(|x| q.push_back((x.clone(), path.clone(), under))),
            Type::Record(fs) => fs.iter().for_each(|(_, x)| q.push_back((x.clone(), path.clone(), under))),
            Type::Fn { .. } | Type::TypeParam(_) | Type::Unknown(_) => {}
        }
    }
    out
}

/// The handlers of a scope under g: every user function whose first parameter matches the state type.
pub fn handlers<'g>(g: &'g Generation, state: &Type) -> Vec<&'g String> {
    g.funcs.iter().filter(|(_, f)| f.parent.is_none() && f.params.first().map(|p| unify(p, state)).unwrap_or(false)).map(|(id, _)| id).collect()
}

/// Dep_g(S) (design 6): what a scope's factory and handlers reach under g, with a path to each.
pub fn dep(g: &Generation, state: &Type, factory: &str) -> BTreeMap<Item, Vec<String>> {
    let mut out: BTreeMap<Item, Vec<String>> = BTreeMap::new();
    let mut q: VecDeque<(String, Vec<String>)> = VecDeque::new();
    let mut roots: Vec<(String, String)> = Vec::new();
    if g.funcs.contains_key(factory) {
        roots.push((factory.to_string(), format!("factory {factory}")));
    }
    for h in handlers(g, state) {
        roots.push((h.clone(), format!("handler {h}")));
    }
    for (id, label) in roots {
        if !out.contains_key(&Item::Code(id.clone())) {
            out.insert(Item::Code(id.clone()), vec![label.clone()]);
            q.push_back((id, vec![label]));
        }
    }
    let add_type = |out: &mut BTreeMap<Item, Vec<String>>, q: &mut VecDeque<(String, Vec<String>)>, name: &str, path: &Vec<String>| {
        if let Some(id) = g.type_ident.get(name) {
            let item = Item::Type(id.clone());
            if !out.contains_key(&item) {
                let mut p = path.clone();
                p.push(format!("type {id}"));
                out.insert(item, p.clone());
                // its cleanup and methods run on its values
                for f in g.types[id].methods.values() {
                    if g.funcs.contains_key(f) && !out.contains_key(&Item::Code(f.clone())) {
                        let mut pp = p.clone();
                        pp.push(format!("method {f}"));
                        out.insert(Item::Code(f.clone()), pp.clone());
                        q.push_back((f.clone(), pp));
                    }
                }
                let cleanup = format!("{}::cleanup {}", id.split("::").next().unwrap_or(""), g.types[id].name);
                if g.funcs.contains_key(&cleanup) && !out.contains_key(&Item::Code(cleanup.clone())) {
                    let mut pp = p.clone();
                    pp.push(format!("cleanup {cleanup}"));
                    out.insert(Item::Code(cleanup.clone()), pp.clone());
                    q.push_back((cleanup, pp));
                }
            }
        }
    };
    while let Some((id, path)) = q.pop_front() {
        let f = match g.funcs.get(&id) {
            Some(f) => f.clone(),
            None => continue,
        };
        let mut next: Vec<(String, String)> = Vec::new();
        for n in &f.refs.funcs {
            if let Some(nid) = g.name_ident.get(n) {
                if g.funcs.contains_key(nid) {
                    next.push((nid.clone(), format!("calls {nid}")));
                }
            }
        }
        for m in &f.refs.virtuals {
            for (nid, nf) in &g.funcs {
                if nf.label.starts_with("method ") && nf.label.rsplit('.').next() == Some(m.as_str()) {
                    next.push((nid.clone(), format!("may dispatch to {nid}")));
                }
            }
        }
        for (ps, rt) in &f.refs.values {
            for (nid, nf) in &g.funcs {
                if nf.params.len() == ps.len() && nf.params.iter().zip(ps).all(|(a, b)| unify(a, b)) && unify(&nf.ret, rt) {
                    next.push((nid.clone(), format!("may call {nid} through a value")));
                }
            }
        }
        // constants, and what their initializers read, transitively
        let mut cq: Vec<(String, Vec<String>)> = f.refs.consts.iter().map(|c| (c.clone(), path.clone())).collect();
        while let Some((c, from)) = cq.pop() {
            let item = Item::Const(c.clone());
            if g.consts.contains_key(&c) && !out.contains_key(&item) {
                let mut p = from.clone();
                p.push(format!("reads {c}"));
                out.insert(item, p.clone());
                if crate::session::fault("const-shallow") {
                    continue;
                }
                if let Some(r) = g.const_refs.get(&c) {
                    for d in &r.consts {
                        cq.push((d.clone(), p.clone()));
                    }
                    for t in &r.types {
                        add_type(&mut out, &mut q, t, &p);
                    }
                    for n in &r.funcs {
                        if let Some(nid) = g.name_ident.get(n) {
                            if g.funcs.contains_key(nid) && !out.contains_key(&Item::Code(nid.clone())) {
                                let mut pp = p.clone();
                                pp.push(format!("calls {nid}"));
                                out.insert(Item::Code(nid.clone()), pp.clone());
                                q.push_back((nid.clone(), pp));
                            }
                        }
                    }
                }
            }
        }
        for t in &f.refs.types {
            add_type(&mut out, &mut q, t, &path);
        }
        for (nid, why) in next {
            let item = Item::Code(nid.clone());
            if !out.contains_key(&item) {
                let mut p = path.clone();
                p.push(why);
                out.insert(item, p.clone());
                q.push_back((nid, p));
            }
        }
    }
    for (item, path) in rep(g, state) {
        out.entry(item).or_insert(path);
    }
    out
}

/// The DI context that generated an item's declaration, if one did.
pub fn di_context(g: &Generation, item: &Item) -> Option<String> {
    match item {
        Item::Type(id) | Item::Impl(_, id) => g.types.get(id).and_then(|t| t.di.clone()),
        Item::Code(id) => g.funcs.get(id).and_then(|f| f.di.clone()),
        Item::Const(_) => None,
    }
}

/// Whether an item of `g` is unusable in `new`: missing, or a different deep fingerprint, or a type that lost a
/// method or interface or changed a method's implementation (R1, R2).
pub fn differs(g: &Generation, new: &Generation, item: &Item) -> Option<String> {
    match item {
        Item::Type(id) => match (g.types.get(id), new.types.get(id)) {
            (Some(_), None) => Some(format!("type {id} is missing")),
            (None, Some(_)) => Some(format!("type {id} does not exist in generation {}", g.id)),
            (Some(a), Some(b)) => {
                if a.deep != b.deep {
                    return Some(format!("type {id} changed layout: {} → {}", a.shown, b.shown));
                }
                for (m, f) in &a.methods {
                    match b.methods.get(m) {
                        None => return Some(format!("type {id} lost method {m}")),
                        Some(h) if h != f => return Some(format!("type {id}: method {m} is implemented by {h}, not {f}")),
                        _ => {}
                    }
                }
                None
            }
            (None, None) => None,
        },
        Item::Code(id) => {
            let a = g.by_ident.get(id.as_str()).map(|&i| g.code[i].deep);
            let b = new.by_ident.get(id.as_str()).map(|&i| new.code[i].deep);
            match (a, b) {
                (Some(_), None) => Some(format!("{id} is missing")),
                (None, Some(_)) => Some(format!("{id} does not exist in generation {}", g.id)),
                (Some(x), Some(y)) if x != y => Some(format!("{id} changed its summary, environment or a captured layout")),
                _ => None,
            }
        }
        Item::Impl(i, t) => {
            let has = |x: &Generation| x.types.get(t).map(|ti| ti.implements.iter().any(|j| j == i || i.ends_with(&format!("::{j}")) || j.ends_with(i.rsplit("::").next().unwrap_or(i)))).unwrap_or(false);
            match (has(g), has(new)) {
                (true, false) => Some(format!("{t} no longer implements {i}")),
                (false, true) => Some(format!("{t} does not implement {i} in generation {}", g.id)),
                _ => None,
            }
        }
        Item::Const(c) => match (g.consts.get(c), new.consts.get(c)) {
            (Some(a), Some(b)) if a.0 != b.0 => Some(format!("{c} changed")),
            (Some(_), None) => Some(format!("{c} is missing")),
            _ => None,
        },
    }
}

#[cfg(test)]
mod canon_identity {
    use super::*;

    #[test]
    fn canon_blanks_closure_identity() {
        let shifted = |ident: &str, line: i64| {
            Json::obj([
                ("k", Json::str("fn")),
                ("identity", Json::str(ident)),
                ("line", Json::Int(line)),
                ("nested", Json::obj([("identity", Json::str(ident))])),
            ])
        };
        let names = HashMap::new();
        assert_eq!(text(&canon(&shifted("main:3:5", 3), &names)), text(&canon(&shifted("main:9:5", 9), &names)));
    }
}
