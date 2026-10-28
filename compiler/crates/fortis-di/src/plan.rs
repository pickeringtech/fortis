//! Resolution (spikes/SP-08/design.md sections 3, 5-7): which binding fills each slot, where each instance lives,
//! the order instances are built in, and the graph rules. Everything here is a fact about the provider graph; no
//! rule here is about loans, moves or effects, which the core checks on the elaborated source.

use std::collections::{BTreeMap, BTreeSet};

use fortis_diag::Json;

use crate::scan::{Class, Func, Module};
use crate::schema::{Binding, Context, Source, Target, LEVELS};
use crate::Problem;

/// The container of each level (Platform 3.2); `application` has none, `transient` is built per use.
pub fn container(level: &str) -> Option<&'static str> {
    match level {
        "session" | "request" => Some("application"),
        "task" | "transaction" => Some("request"),
        _ => None,
    }
}

/// `outer` is `inner` or one of its containers.
pub fn encloses(outer: &str, inner: &str) -> bool {
    let mut l = Some(inner);
    while let Some(x) = l {
        if x == outer {
            return true;
        }
        l = container(x);
    }
    false
}

fn depth(level: &str) -> usize {
    container(level).map_or(0, |c| 1 + depth(c))
}

/// How a slot is filled (design section 5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Form {
    /// `Shared[…]` or `Weak[…]`: a held instance is `retain()`ed
    Shared,
    /// any other owned type: a held instance is copied, a transient one built
    Owned,
    /// a factory's plain parameter: a loan for the call
    Loan,
    /// a factory's `editable` parameter: an exclusive loan for the call
    Editable,
}

#[derive(Clone, Debug)]
pub struct Need {
    /// the consumer class or factory the slot belongs to
    pub owner: String,
    pub slot: String,
    pub key: String,
    pub form: Form,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub binding: usize,
    pub specificity: &'static str,
    pub reason: String,
    pub rejected: Vec<(usize, String)>,
}

#[derive(Debug, Default)]
pub struct Resolved {
    pub ctx: Context,
    /// the bindings of the context and its parents, nearest first
    pub bindings: Vec<Binding>,
    pub depth: Vec<usize>,
    /// application inputs and each contained level's inputs, parents' first
    pub inputs: BTreeMap<String, (String, Vec<String>)>,
    /// each binding's construction needs with their choices (instances have none)
    pub needs: BTreeMap<usize, Vec<(Need, Choice)>>,
    /// the levels with a class, in containment order
    pub levels: Vec<String>,
    /// held bindings per level in construction (dependency) order
    pub held: BTreeMap<String, Vec<usize>>,
    /// transient bindings per level: each at the outermost level it is resolvable from
    pub transient: BTreeMap<String, Vec<usize>>,
    /// a generated name per binding (field, method)
    pub names: BTreeMap<usize, String>,
    /// whether each binding is in effect (a parent's binding the child overrides entirely is not)
    pub effective: Vec<bool>,
    /// names the generated code may not declare: the inputs of every level
    pub reserved: BTreeSet<String>,
}

pub fn norm(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").replace("[ ", "[").replace(" ]", "]").replace(" ,", ",")
}

fn form_of(ty: &str) -> Form {
    if ty.starts_with("Shared[") || ty.starts_with("Weak[") {
        Form::Shared
    } else {
        Form::Owned
    }
}

/// `ProductionApplication`
pub fn level_class(ctx: &str, level: &str) -> String {
    let mut c = level.chars();
    format!("{ctx}{}{}", c.next().map(|x| x.to_ascii_uppercase()).unwrap_or_default(), c.as_str())
}

fn lower_camel(names: &[String]) -> String {
    let joined: String = names.concat();
    let mut c = joined.chars();
    match c.next() {
        Some(f) => format!("{}{}", f.to_ascii_lowercase(), c.as_str()),
        None => "instance".into(),
    }
}

fn upper_names(t: &str) -> Vec<String> {
    t.split(|c: char| !c.is_alphanumeric()).filter(|w| w.starts_with(|c: char| c.is_ascii_uppercase())).map(|w| w.to_string()).collect()
}

pub struct World<'a> {
    pub classes: BTreeMap<String, &'a Class>,
    pub funcs: BTreeMap<String, &'a Func>,
}

impl<'a> World<'a> {
    /// What the context's module can name: its own declarations, then the members its imports name from the
    /// package's other modules (`import pkg.module { Name }`), each found in the module the import names.
    pub fn new(here: &'a Module, others: &'a [Module], package: &str) -> World<'a> {
        let mut classes = BTreeMap::new();
        let mut funcs = BTreeMap::new();
        for imp in &here.imports {
            let path = imp.path.join(".");
            let Some(rest) = path.strip_prefix(&format!("{package}.")) else { continue };
            let file = format!("/src/{}.ft", rest.replace('.', "/"));
            let Some(m) = others.iter().find(|m| m.file.replace('\\', "/").ends_with(&file)) else { continue };
            for name in &imp.members {
                if let Some(c) = m.classes.iter().find(|c| c.name == *name) {
                    classes.insert(name.clone(), c);
                }
                for f in m.funcs.iter().filter(|f| f.path == *name || f.path.starts_with(&format!("{name}."))) {
                    funcs.insert(f.path.clone(), f);
                }
            }
        }
        for c in &here.classes {
            classes.insert(c.name.clone(), c);
        }
        for f in &here.funcs {
            funcs.insert(f.path.clone(), f);
        }
        World { classes, funcs }
    }
}

fn mode_split(ty: &str) -> (Form, String) {
    let t = ty.trim();
    if let Some(r) = t.strip_prefix("owned ") {
        (form_of(r.trim()), norm(r))
    } else if let Some(r) = t.strip_prefix("editable ") {
        (Form::Editable, norm(r))
    } else {
        (Form::Loan, norm(t))
    }
}

/// The slots a binding's construction fills, or a problem naming why it cannot be constructed.
fn needs_of(b: &Binding, w: &World) -> Result<Vec<Need>, Problem> {
    match &b.source {
        Source::Instance(_) => Ok(vec![]),
        Source::Class(name) => {
            let base = name.split('[').next().unwrap_or(name).trim();
            let c = w.classes.get(base).ok_or_else(|| Problem::new("E-DI-009", format!("`{base}` is not a class of this package: bind it with `instance` or `factory`"), "PLAT-03-03", b.line, b.col))?;
            if c.generic {
                return Err(Problem::new("E-DI-009", format!("{base} is generic: bind it with `instance` or `factory`"), "PLAT-03-03", b.line, b.col));
            }
            if let Some(f) = c.fields.iter().find(|f| !f.default) {
                return Err(Problem::new("E-DI-009", format!("{base}.{} has no default, so `using {base}` cannot construct it: bind {base} with `instance` or `factory`", f.name), "PLAT-03-03", b.line, b.col));
            }
            Ok(c.slots.iter().map(|s| Need { owner: base.to_string(), slot: s.name.clone(), key: norm(&s.ty), form: form_of(&norm(&s.ty)), line: s.line }).collect())
        }
        Source::Factory(path) => {
            let f = w.funcs.get(path.as_str()).ok_or_else(|| Problem::new("E-DI-009", format!("`{path}` is not a function of this package"), "PLAT-03-03", b.line, b.col))?;
            Ok(f.params
                .iter()
                .filter(|p| !p.default)
                .map(|p| {
                    let (form, key) = mode_split(&p.ty);
                    Need { owner: path.clone(), slot: p.name.clone(), key, form, line: f.line }
                })
                .collect())
        }
    }
}

impl Resolved {
    fn group_members(&self, g: &str) -> Option<Vec<Target>> {
        // the nearest context that declares the group
        self.ctx.groups.iter().find(|(n, _)| n == g).map(|(_, t)| t.clone())
    }

    /// How specifically binding `b` targets `owner.slot`: 4 slot, 3 consumer, 2 group, 1 untargeted, 0 not at all.
    fn specificity(&self, b: &Binding, owner: &str, slot: &str) -> u8 {
        if b.targets.is_empty() {
            return 1;
        }
        let mut best = 0;
        for t in &b.targets {
            let s = match t {
                Target::Slot(c, s) if c == owner && s == slot => 4,
                Target::Name(n) if n == owner => 3,
                Target::Name(n) => match self.group_members(n) {
                    Some(ms) if ms.iter().any(|m| matches!(m, Target::Name(x) if x == owner) || matches!(m, Target::Slot(c, s) if c == owner && s == slot)) => 2,
                    _ => 0,
                },
                _ => 0,
            };
            best = best.max(s);
        }
        best
    }

    /// Platform 3.3: nearest context first; within it slot, consumer, group, untargeted.
    fn choose(&self, need: &Need) -> Result<Choice, Problem> {
        let names = ["", "untargeted", "group", "consumer", "slot"];
        let mut rejected = Vec::new();
        let mut chosen: Option<(usize, u8)> = None;
        let max_depth = self.depth.iter().copied().max().unwrap_or(0);
        for d in 0..=max_depth {
            let mut here: Vec<(usize, u8)> = (0..self.bindings.len())
                .filter(|&i| self.depth[i] == d && norm(&self.bindings[i].key) == need.key)
                .map(|i| (i, self.specificity(&self.bindings[i], &need.owner, &need.slot)))
                .filter(|&(_, s)| s > 0)
                .collect();
            here.sort_by(|a, b| b.1.cmp(&a.1).then(self.bindings[a.0].identity().cmp(&self.bindings[b.0].identity())));
            if chosen.is_some() {
                for (i, _) in here {
                    rejected.push((i, format!("in {}, which {} shadows", self.bindings[i].context, self.ctx.name)));
                }
                continue;
            }
            if let Some(&(first, s)) = here.first() {
                let ties: Vec<usize> = here.iter().filter(|x| x.1 == s).map(|x| x.0).collect();
                if ties.len() > 1 {
                    let ids: Vec<String> = ties.iter().map(|&i| self.bindings[i].identity()).collect();
                    let b = &self.bindings[ties[1]];
                    return Err(Problem::new("E-DI-002", format!("{}.{} ({}) has {} equally specific ({}) bindings: {}", need.owner, need.slot, need.key, ties.len(), names[s as usize], ids.join("; ")), "PLAT-03-02", b.line, b.col)
                        .with(&need.owner, &need.slot, ids));
                }
                for &(i, si) in here.iter().skip(1) {
                    rejected.push((i, format!("less specific ({}) than the chosen {} binding", names[si as usize], names[s as usize])));
                }
                chosen = Some((first, s));
            }
        }
        match chosen {
            Some((i, s)) => Ok(Choice { binding: i, specificity: names[s as usize], reason: format!("the {} binding of {} in {}", names[s as usize], need.key, self.bindings[i].context), rejected }),
            None => Err(Problem::new("E-DI-001", format!("no binding provides {} for {}.{}", need.key, need.owner, need.slot), "PLAT-03-02", need.line, 1).with(&need.owner, &need.slot, vec![])),
        }
    }
}

/// Resolve one context against its parents and the package's declarations.
pub fn resolve(ctx: &Context, all: &BTreeMap<String, Context>, w: &World, out: &mut Vec<Problem>) -> Option<Resolved> {
    let mut r = Resolved { ctx: ctx.clone(), ..Default::default() };
    // the parent chain
    let mut chain = vec![ctx];
    let mut seen = BTreeSet::from([ctx.name.clone()]);
    let mut cur = ctx;
    while let Some((p, line, col)) = &cur.parent {
        match all.get(p) {
            Some(pc) if seen.insert(p.clone()) => {
                chain.push(pc);
                cur = pc;
            }
            Some(_) => {
                out.push(Problem::new("E-DI-008", format!("the parents of {} form a cycle through {p}", ctx.name), "PLAT-03-04", *line, *col));
                return None;
            }
            None => {
                out.push(Problem::new("E-DI-008", format!("`{p}` is not a context of this module"), "PLAT-03-04", *line, *col));
                return None;
            }
        }
    }
    for (d, c) in chain.iter().enumerate() {
        for b in &c.bindings {
            r.bindings.push(b.clone());
            r.depth.push(d);
        }
        if d > 0 {
            for g in &c.groups {
                if !r.ctx.groups.iter().any(|x| x.0 == g.0) {
                    r.ctx.groups.push(g.clone());
                }
            }
        }
    }
    // inputs: the parents' first
    for c in chain.iter().rev() {
        for (level, text, names) in &c.inputs {
            let e = r.inputs.entry(level.clone()).or_insert((String::new(), vec![]));
            for n in names {
                if e.1.contains(n) {
                    out.push(Problem::new("E-DI-010", format!("the {level} input `{n}` is declared twice in the contexts {} inherits", ctx.name), "PLAT-03-01", c.line, 1));
                }
            }
            if !text.is_empty() {
                e.0 = if e.0.is_empty() { text.clone() } else { format!("{}, {text}", e.0) };
            }
            e.1.extend(names.iter().cloned());
        }
    }
    // duplicate identities
    let mut ids = BTreeMap::new();
    for (i, b) in r.bindings.iter().enumerate() {
        if let Some(j) = ids.insert(b.identity(), i) {
            let _ = j;
            out.push(Problem::new("E-DI-002", format!("the binding `{}` is declared twice", b.identity()), "PLAT-03-02", b.line, b.col));
        }
    }
    // needs, which parent bindings are in effect, and choices
    let start = out.len();
    let mut all_needs: Vec<(usize, Vec<Need>)> = Vec::new();
    let mut unbuildable: Vec<(usize, Problem)> = Vec::new();
    for (i, b) in r.bindings.iter().enumerate() {
        match needs_of(b, w) {
            Ok(ns) => all_needs.push((i, ns)),
            Err(p) => unbuildable.push((i, p)),
        }
    }
    r.effective = (0..r.bindings.len()).map(|i| r.depth[i] == 0 || in_effect(&r, w, i)).collect();
    for (i, p) in unbuildable {
        if r.effective[i] {
            out.push(p);
        }
    }
    for (i, ns) in all_needs {
        let mut v = Vec::new();
        for n in ns {
            match r.choose(&n) {
                Ok(c) => v.push((n, c)),
                Err(p) => {
                    // a parent binding the child overrides entirely is never built
                    if r.effective[i] {
                        out.push(p)
                    }
                }
            }
        }
        r.needs.insert(i, v);
    }
    // `replaces` (Platform 3.3)
    for (i, b) in r.bindings.iter().enumerate().filter(|(i, _)| r.depth[*i] == 0) {
        let key = norm(&b.key);
        let slots = slots_with_key(&r, w, &key);
        let mut overlaps_any = false;
        for (j, p) in r.bindings.iter().enumerate().filter(|(j, x)| r.depth[*j] > 0 && norm(&x.key) == key) {
            let mut more_specific = false;
            for (owner, slot) in &slots {
                let si = r.specificity(b, owner, slot);
                let sj = r.specificity(p, owner, slot);
                if si > 0 && sj > 0 {
                    overlaps_any = true;
                    more_specific |= sj > si;
                }
            }
            if slots.is_empty() && b.targets.is_empty() && p.targets.is_empty() {
                overlaps_any = true;
            }
            if more_specific && !b.replaces {
                out.push(
                    Problem::new("E-DI-003", format!("`{}` overlaps the more specific parent binding `{}`: declare `replaces`", b.identity(), p.identity()), "PLAT-03-02", b.line, b.col)
                        .with("", "", vec![p.identity()])
                        .fix(b.replaces_at.0, b.replaces_at.1, " replaces", "declare the override"),
                );
            }
            let _ = (i, j);
        }
        if b.replaces && !overlaps_any {
            out.push(Problem::new("E-DI-004", format!("`{}` declares `replaces`, but no parent binding of {key} overlaps it", b.identity()), "PLAT-03-02", b.line, b.col));
        }
    }
    if out.len() > start {
        return None;
    }
    place(&mut r, out);
    if out.len() > start {
        return None;
    }
    name(&mut r);
    Some(r)
}

/// A parent's binding is in effect in the child unless nearer bindings of its key cover everything it could fill:
/// every slot of the program it matches is matched by a nearer binding (selection then never reaches it), or, when it
/// matches no slot, a nearer binding of its key is untargeted or has its targets.
fn in_effect(r: &Resolved, w: &World, i: usize) -> bool {
    let b = &r.bindings[i];
    let key = norm(&b.key);
    let nearer: Vec<usize> = (0..r.bindings.len()).filter(|&j| r.depth[j] < r.depth[i] && norm(&r.bindings[j].key) == key).collect();
    let mine: Vec<(String, String)> = slots_with_key(r, w, &key).into_iter().filter(|(o, s)| r.specificity(b, o, s) > 0).collect();
    if !mine.is_empty() {
        return !mine.iter().all(|(o, s)| nearer.iter().any(|&j| r.specificity(&r.bindings[j], o, s) > 0));
    }
    let targets = |x: &Binding| {
        let mut t: Vec<String> = x.targets.iter().map(|t| t.show()).collect();
        t.sort();
        t
    };
    !nearer.iter().any(|&j| r.bindings[j].targets.is_empty() || targets(&r.bindings[j]) == targets(b))
}

fn slots_with_key(r: &Resolved, w: &World, key: &str) -> Vec<(String, String)> {
    let mut out = BTreeSet::new();
    for b in &r.bindings {
        if let Ok(ns) = needs_of(b, w) {
            for n in ns.into_iter().filter(|n| n.key == key) {
                out.insert((n.owner, n.slot));
            }
        }
    }
    out.into_iter().collect()
}

/// Levels, placement, construction order and the graph rules (design section 7).
fn place(r: &mut Resolved, out: &mut Vec<Problem>) {
    let start = out.len();
    let mut built: Vec<usize> = (0..r.bindings.len()).filter(|&i| r.effective[i]).collect();
    built.sort_by_key(|&i| r.bindings[i].identity());
    let rank: BTreeMap<usize, String> = (0..r.bindings.len()).map(|i| (i, r.bindings[i].identity())).collect();
    // cycles over every chosen edge, searched in identity order so the cycle reported never depends on declaration order
    let edges = |i: usize| -> Vec<usize> {
        let mut v: Vec<usize> = r.needs.get(&i).map(|v| v.iter().map(|(_, c)| c.binding).collect()).unwrap_or_default();
        v.sort_by_key(|j| rank[j].clone());
        v
    };
    let mut state = vec![0u8; r.bindings.len()];
    let mut stack = Vec::new();
    fn dfs(i: usize, edges: &dyn Fn(usize) -> Vec<usize>, state: &mut Vec<u8>, stack: &mut Vec<usize>) -> Option<Vec<usize>> {
        state[i] = 1;
        stack.push(i);
        for j in edges(i) {
            if state[j] == 1 {
                let at = stack.iter().position(|&x| x == j).unwrap();
                let mut cyc = stack[at..].to_vec();
                cyc.push(j);
                return Some(cyc);
            }
            if state[j] == 0 {
                if let Some(c) = dfs(j, edges, state, stack) {
                    return Some(c);
                }
            }
        }
        stack.pop();
        state[i] = 2;
        None
    }
    for &i in &built {
        if state[i] == 0 {
            if let Some(cyc) = dfs(i, &edges, &mut state, &mut stack) {
                let names: Vec<String> = cyc.iter().map(|&k| norm(&r.bindings[k].key)).collect();
                let b = &r.bindings[cyc[0]];
                out.push(Problem::new("E-DI-006", format!("construction cycle: {}", names.join(" -> ")), "PLAT-03-04", b.line, b.col));
                return;
            }
        }
    }
    // the held level of each binding, and the level-edge rule
    for &i in &built {
        let li = r.bindings[i].placement().to_string();
        if li == "transient" {
            continue;
        }
        for (n, c) in r.needs.get(&i).cloned().unwrap_or_default() {
            let lj = r.bindings[c.binding].placement().to_string();
            for held in held_reached(r, c.binding) {
                let lh = r.bindings[held].placement().to_string();
                if !encloses(&lh, &li) {
                    let b = &r.bindings[i];
                    out.push(Problem::new(
                        "E-DI-005",
                        format!("{} lives at {li} but {}.{} needs {} from {lh}, which {li} does not lie within (Platform 3.2)", norm(&b.key), n.owner, n.slot, norm(&r.bindings[held].key)),
                        "PLAT-03-04",
                        b.line,
                        b.col,
                    ));
                }
            }
            let _ = lj;
        }
    }
    if out.len() > start {
        return;
    }
    let mut levels: BTreeSet<String> = BTreeSet::from(["application".to_string()]);
    for &i in built.iter() {
        let l = r.bindings[i].placement();
        if l != "transient" {
            levels.insert(l.into());
        }
    }
    for l in r.inputs.keys() {
        levels.insert(l.clone());
    }
    for l in levels.clone() {
        let mut c = container(&l);
        while let Some(x) = c {
            levels.insert(x.into());
            c = container(x);
        }
    }
    let mut lv: Vec<String> = levels.into_iter().collect();
    lv.sort_by_key(|l| (depth(l), LEVELS.iter().position(|x| x == l)));
    r.levels = lv.clone();
    // held, in dependency order (ties by key)
    for l in &lv {
        let mut todo: Vec<usize> = built.iter().copied().filter(|&i| r.bindings[i].placement() == l).collect();
        todo.sort_by_key(|&i| r.bindings[i].identity());
        let mut order = Vec::new();
        while !todo.is_empty() {
            let pos = todo.iter().position(|&i| held_reached(r, i).iter().all(|h| *h == i || r.bindings[*h].placement() != l || order.contains(h))).unwrap_or(0);
            order.push(todo.remove(pos));
        }
        r.held.insert(l.clone(), order);
    }
    // transient bindings at the outermost level they are resolvable from
    for &i in &built {
        if r.bindings[i].placement() != "transient" {
            continue;
        }
        let reached = held_reached(r, i);
        match lv.iter().find(|l| reached.iter().all(|&h| encloses(r.bindings[h].placement(), l))) {
            Some(l) => r.transient.entry(l.clone()).or_default().push(i),
            None => {
                let b = &r.bindings[i];
                let from: BTreeSet<&str> = reached.iter().map(|&h| r.bindings[h].placement()).collect();
                out.push(Problem::new("E-DI-005", format!("{} needs instances of the levels {}, which no one scope lies within", norm(&b.key), from.into_iter().collect::<Vec<_>>().join(" and ")), "PLAT-03-04", b.line, b.col));
            }
        }
    }
    for v in r.transient.values_mut() {
        v.sort_by_key(|&i| r.bindings[i].identity());
    }
}

/// The held bindings a binding's construction reaches, through transient bindings (itself included when held).
pub fn held_reached(r: &Resolved, i: usize) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut todo = vec![i];
    let mut seen = BTreeSet::new();
    while let Some(k) = todo.pop() {
        if !seen.insert(k) {
            continue;
        }
        if r.bindings[k].placement() != "transient" && k != i {
            out.insert(k);
            continue;
        }
        if k == i && r.bindings[k].placement() != "transient" {
            out.insert(k);
        }
        for (_, c) in r.needs.get(&k).cloned().unwrap_or_default() {
            todo.push(c.binding);
        }
    }
    out
}

/// Names from types (design section 6): unique within the context, independent of declaration order.
fn name(r: &mut Resolved) {
    let mut ids: Vec<usize> = r.held.values().flatten().chain(r.transient.values().flatten()).copied().collect();
    ids.sort_by_key(|&i| r.bindings[i].identity());
    let base = |r: &Resolved, i: usize| lower_camel(&upper_names(&norm(&r.bindings[i].key)));
    // the level names and every input are taken: a generated local of the same name would capture an input
    r.reserved = r.inputs.values().flat_map(|(_, ns)| ns.iter().cloned()).collect();
    let mut taken: BTreeSet<String> = ["parent", "application", "session", "request", "task", "transaction", "result", "self"].iter().map(|s| s.to_string()).chain(r.reserved.iter().cloned()).collect();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for &i in &ids {
        *counts.entry(base(r, i)).or_default() += 1;
    }
    for &i in &ids {
        let b = base(r, i);
        let mut n = if counts[&b] > 1 || taken.contains(&b) {
            let src = match &r.bindings[i].source {
                Source::Class(c) => upper_names(c).concat(),
                Source::Factory(f) => f.split('.').map(|p| { let mut c = p.chars(); c.next().map(|x| x.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default() }).collect(),
                Source::Instance(_) => "Instance".into(),
            };
            format!("{b}{src}")
        } else {
            b.clone()
        };
        let mut k = 2;
        while taken.contains(&n) {
            n = format!("{b}{k}");
            k += 1;
        }
        taken.insert(n.clone());
        r.names.insert(i, n);
    }
}

/// The plan (`fortis.di.plan/v1`): static provenance, with no line numbers (design section 6).
pub fn plan_json(r: &Resolved, package: &str) -> Json {
    let id = |i: usize| Json::str(r.bindings[i].identity());
    let bindings: Vec<Json> = {
        let mut v: Vec<usize> = (0..r.bindings.len()).collect();
        v.sort_by_key(|&i| r.bindings[i].identity());
        v.into_iter()
            .map(|i| {
                let b = &r.bindings[i];
                Json::obj([
                    ("identity", id(i)),
                    ("context", Json::str(&b.context)),
                    ("key", Json::str(norm(&b.key))),
                    ("source", Json::str(b.source.show())),
                    ("placement", Json::str(b.placement())),
                    ("name", r.names.get(&i).map(|n| Json::str(n)).unwrap_or(Json::Null)),
                ])
            })
            .collect()
    };
    let levels: Vec<Json> = r
        .levels
        .iter()
        .map(|l| {
            Json::obj([
                ("level", Json::str(l)),
                ("class", Json::str(level_class(&r.ctx.name, l))),
                ("inputs", Json::str(r.inputs.get(l).map(|x| x.0.clone()).unwrap_or_default())),
                ("held", Json::Arr(r.held.get(l).map(|v| v.iter().map(|&i| id(i)).collect()).unwrap_or_default())),
                ("transient", Json::Arr(r.transient.get(l).map(|v| v.iter().map(|&i| id(i)).collect()).unwrap_or_default())),
            ])
        })
        .collect();
    let mut slots: Vec<(String, Json)> = Vec::new();
    for (_, v) in &r.needs {
        for (n, c) in v {
            let rejected: Vec<Json> = c.rejected.iter().map(|(i, why)| Json::obj([("binding", id(*i)), ("reason", Json::str(why))])).collect();
            slots.push((
                format!("{}.{}", n.owner, n.slot),
                Json::obj([
                    ("consumer", Json::str(&n.owner)),
                    ("slot", Json::str(&n.slot)),
                    ("key", Json::str(&n.key)),
                    ("form", Json::str(format!("{:?}", n.form).to_lowercase())),
                    ("chosen", id(c.binding)),
                    ("context", Json::str(&r.bindings[c.binding].context)),
                    ("level", Json::str(r.bindings[c.binding].placement())),
                    ("specificity", Json::str(c.specificity)),
                    ("reason", Json::str(&c.reason)),
                    ("rejected", Json::Arr(rejected)),
                ]),
            ));
        }
    }
    slots.sort_by(|a, b| a.0.cmp(&b.0));
    slots.dedup_by(|a, b| a.0 == b.0);
    Json::obj([
        ("context", Json::str(&r.ctx.name)),
        ("package", Json::str(package)),
        ("parent", r.ctx.parent.as_ref().map(|p| Json::str(&p.0)).unwrap_or(Json::Null)),
        ("bindings", Json::Arr(bindings)),
        ("levels", Json::Arr(levels)),
        ("slots", Json::Arr(slots.into_iter().map(|s| s.1).collect())),
    ])
}
