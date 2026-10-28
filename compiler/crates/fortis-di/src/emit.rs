//! The generated declarations of one resolved context (spikes/SP-08/design.md section 4): `class C` with
//! `application(inputs)`, one class per level holding that level's instances, a factory method per transient
//! binding, an accessor per held instance and a creator method per contained level. Every expression is the most
//! direct ordinary one; the core judges it.

use std::collections::{BTreeMap, BTreeSet};

use crate::plan::{container, level_class, norm, Form, Resolved, World};
use crate::schema::Source;

/// Where generated code runs: the constructor of a level (inside its container's creator method, or
/// `C.application` for the application level), or a method of a level's class.
#[derive(Clone)]
enum Site {
    Ctor { level: String, locals: BTreeSet<usize> },
    Method { level: String },
}

#[derive(Default)]
struct Body {
    lines: Vec<String>,
    n: usize,
    fallible: bool,
    suspends: bool,
    /// a fill takes an exclusive loan of a field of `self`: the function's receiver is `editable self`
    editable_self: bool,
    /// locals a fill takes an exclusive loan of: declared `let mut`
    mutable: BTreeSet<String>,
    /// names a temporary may not take (the inputs and the generated names)
    reserved: BTreeSet<String>,
    /// lines that come from a declaration other than the function's own: line index -> source line
    origin: BTreeMap<usize, usize>,
}

impl Body {
    fn new(reserved: &BTreeSet<String>) -> Body {
        Body { reserved: reserved.clone(), ..Default::default() }
    }
    fn temp(&mut self) -> String {
        loop {
            self.n += 1;
            let t = format!("v{}", self.n);
            if !self.reserved.contains(&t) {
                return t;
            }
        }
    }
    /// The lines, with each local a fill borrows exclusively declared `let mut`.
    fn finished(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|l| match l.strip_prefix("let ").and_then(|r| r.split(':').next()) {
                Some(n) if self.mutable.contains(n.trim()) && !l.starts_with("let mut ") => format!("let mut {}", &l[4..]),
                _ => l.clone(),
            })
            .collect()
    }
    fn modifiers(&self) -> String {
        let mut m = String::new();
        if self.suspends {
            m.push_str("async ");
        }
        if self.fallible {
            m.push_str("try ");
        }
        m
    }
}

pub struct Gen<'a> {
    r: &'a Resolved,
    w: &'a World<'a>,
    /// live provenance (the module imports `fortis.di.provenance`, Platform 3.5): events are recorded
    provenance: bool,
    /// enclosing-level instances a level's class holds a handle of: level -> bindings
    imports: BTreeMap<String, BTreeSet<usize>>,
    grew: bool,
}

fn shared(ty: &str) -> bool {
    ty.starts_with("Shared[") || ty.starts_with("Weak[")
}

impl<'a> Gen<'a> {
    pub fn new(r: &'a Resolved, w: &'a World<'a>, provenance: bool) -> Gen<'a> {
        Gen { r, w, provenance, imports: BTreeMap::new(), grew: false }
    }

    fn schema(&self) -> String {
        format!("{}Provenance", self.r.ctx.name)
    }

    /// One live-provenance event (1 created, 2 reused, 3 disposed): the binding's place in the plan's order and
    /// the level, trivial values only, so the record allocates nothing and carries no field value.
    fn event(&self, kind: u8, i: usize) -> String {
        let idx = {
            let mut ids: Vec<String> = self.r.bindings.iter().map(|b| b.identity()).collect();
            ids.sort();
            ids.iter().position(|x| *x == self.r.bindings[i].identity()).unwrap_or(0)
        };
        let level = crate::schema::LEVELS.iter().position(|l| *l == self.level(i)).unwrap_or(0);
        format!("record {}(event: {kind}, binding: {idx}, level: {level})", self.schema())
    }

    fn key(&self, i: usize) -> String {
        norm(&self.r.bindings[i].key)
    }

    fn level(&self, i: usize) -> &str {
        self.r.bindings[i].placement()
    }

    /// The enclosing-level instances `level` holds a handle of, in name order (never declaration order).
    fn imported(&self, level: &str) -> Vec<usize> {
        let mut v: Vec<usize> = self.imports.get(level).map(|s| s.iter().copied().collect()).unwrap_or_default();
        v.sort_by(|a, b| self.r.names[a].cmp(&self.r.names[b]));
        v
    }

    /// Every name the generated code declares or reads as an input: a temporary never takes one.
    fn reserved(&self) -> BTreeSet<String> {
        self.r.reserved.iter().cloned().chain(self.r.names.values().cloned()).collect()
    }

    fn import(&mut self, level: &str, i: usize) {
        if self.imports.entry(level.to_string()).or_default().insert(i) {
            self.grew = true;
        }
    }

    /// The place of held instance `i` seen from `site`.
    fn path(&mut self, i: usize, site: &Site) -> String {
        let name = self.r.names[&i].clone();
        match site {
            Site::Ctor { level, locals } if self.level(i) == level && locals.contains(&i) => name,
            Site::Ctor { level, .. } => {
                let p = container(level).unwrap_or("application").to_string();
                if self.level(i) != p {
                    self.import(&p, i);
                }
                format!("self.{name}")
            }
            Site::Method { level } => {
                if self.level(i) != level {
                    let l = level.clone();
                    self.import(&l, i);
                }
                format!("self.{name}")
            }
        }
    }

    /// The expression filling one need.
    fn fill(&mut self, b: &mut Body, form: Form, chosen: usize, site: &Site) -> String {
        if self.level(chosen) == "transient" {
            let e = self.value(b, chosen, site);
            let v = b.temp();
            let k = self.key(chosen);
            let m = if form == Form::Editable { "mut " } else { "" };
            b.lines.push(format!("let {m}{v}: {k} = {e}"));
            if self.provenance {
                b.lines.push(self.event(1, chosen));
            }
            match form {
                Form::Editable => format!("editable {v}"),
                Form::Loan => format!("alias {v}"),
                _ => format!("move {v}"),
            }
        } else {
            let p = self.path(chosen, site);
            if self.provenance {
                b.lines.push(self.event(2, chosen));
            }
            if form == Form::Editable {
                match p.strip_prefix("self.") {
                    Some(_) => b.editable_self = true,
                    None => {
                        b.mutable.insert(p.clone());
                    }
                }
            }
            match form {
                Form::Shared => format!("{p}.retain()"),
                Form::Owned => format!("copy {p}"),
                Form::Loan => format!("alias {p}"),
                Form::Editable => format!("editable {p}"),
            }
        }
    }

    /// The expression constructing binding `i` (its key's type, a `Shared` key wrapping the source's value).
    fn value(&mut self, b: &mut Body, i: usize, site: &Site) -> String {
        let needs = self.r.needs.get(&i).cloned().unwrap_or_default();
        let mut args = Vec::new();
        for (n, c) in &needs {
            let f = self.fill(b, n.form, c.binding, site);
            args.push(format!("{}: {f}", n.slot));
        }
        let (e, produces) = match &self.r.bindings[i].source {
            Source::Instance(x) => return x.clone(),
            Source::Class(c) => (format!("{c}({})", args.join(", ")), c.clone()),
            Source::Factory(f) => {
                let func = self.w.funcs.get(f.as_str());
                let mut call = format!("{f}({})", args.join(", "));
                if func.map_or(false, |x| x.suspends) {
                    call = format!("await {call}");
                    b.suspends = true;
                }
                if func.map_or(false, |x| x.fallible) {
                    call = format!("{call} or fail");
                    b.fallible = true;
                }
                (call, func.map(|x| norm(&x.ret)).unwrap_or_default())
            }
        };
        let key = self.key(i);
        if shared(&key) && !shared(&produces) && key.starts_with("Shared[") {
            let inner = &key["Shared[".len()..key.len() - 1];
            let v = b.temp();
            b.lines.push(format!("let {v}: {inner} = {e}"));
            return format!("Shared.create(move {v})");
        }
        e
    }

    /// A generated function; every line carries the source line it was generated from.
    fn function(&self, head: &str, b: &Body, ret_expr: &str, at: usize) -> Vec<(String, usize)> {
        // an exclusive loan of one of the receiver's fields needs the receiver exclusively
        let head = if b.editable_self { head.replacen("(self", "(editable self", 1) } else { head.to_string() };
        let mut out = vec![(format!("  export {}{head} {{", b.modifiers()), at)];
        for (k, l) in b.finished().into_iter().enumerate() {
            out.push((format!("    {l}"), b.origin.get(&k).copied().unwrap_or(at)));
        }
        out.push((format!("    return {ret_expr}"), at));
        out.push(("  }".into(), at));
        out
    }

    fn line_of(&self, i: usize) -> usize {
        self.r.bindings[i].line
    }

    /// The constructor of `level`: (body, the class construction expression).
    fn constructor(&mut self, level: &str) -> (Body, String) {
        let mut b = Body::new(&self.reserved());
        let mut locals = BTreeSet::new();
        let order = self.r.held.get(level).cloned().unwrap_or_default();
        for &i in &order {
            let site = Site::Ctor { level: level.into(), locals: locals.clone() };
            let e = self.value(&mut b, i, &site);
            let n = self.r.names[&i].clone();
            b.lines.push(format!("let {n}: {} = {e}", self.key(i)));
            if self.provenance {
                b.lines.push(self.event(1, i));
            }
            locals.insert(i);
        }
        let mut fields: Vec<String> = order.iter().rev().map(|i| format!("{n}: move {n}", n = self.r.names[i])).collect();
        let site = Site::Ctor { level: level.into(), locals };
        for i in self.imported(level) {
            // the level's own handle of an enclosing instance, on a line of its own that a core diagnostic locates at
            // the binding of the instance it would copy
            let p = self.path(i, &site);
            if self.provenance {
                b.lines.push(self.event(2, i));
            }
            let n = self.r.names[&i].clone();
            let fill = if shared(&self.key(i)) { format!("{p}.retain()") } else { format!("copy {p}") };
            b.origin.insert(b.lines.len(), self.line_of(i));
            b.lines.push(format!("let {n}: {} = {fill}", self.key(i)));
            fields.push(format!("{n}: move {n}"));
        }
        (b, format!("{}({})", level_class(&self.r.ctx.name, level), fields.join(", ")))
    }

    fn class_of(&mut self, level: &str) -> Vec<(String, usize)> {
        let cname = level_class(&self.r.ctx.name, level);
        let at = self.r.ctx.line;
        let mut out = vec![(format!("class {cname} {{"), at)];
        let order = self.r.held.get(level).cloned().unwrap_or_default();
        for &i in order.iter().rev() {
            out.push((format!("  {}: {}", self.r.names[&i], self.key(i)), self.line_of(i)));
        }
        for i in self.imported(level) {
            out.push((format!("  {}: {}", self.r.names[&i], self.key(i)), self.line_of(i)));
        }
        // the held instances are read as fields by the module that declares the context (the classes are private)
        if self.provenance && !order.is_empty() {
            out.push(("  cleanup(editable self) Void {".into(), at));
            for &i in order.iter().rev() {
                out.push((format!("    {}", self.event(3, i)), self.line_of(i)));
            }
            out.push(("  }".into(), at));
        }
        // factory methods of the transient bindings resolvable here and not from a container
        for i in self.r.transient.get(level).cloned().unwrap_or_default() {
            let mut b = Body::new(&self.reserved());
            let site = Site::Method { level: level.into() };
            let e = self.value(&mut b, i, &site);
            let head = format!("func {}(self) {}", self.r.names[&i], self.key(i));
            let ret = if self.provenance {
                // created: recorded once the value exists
                b.lines.push(format!("let result: {} = {e}", self.key(i)));
                b.lines.push(self.event(1, i));
                "move result".to_string()
            } else {
                e
            };
            out.extend(self.function(&head, &b, &ret, self.line_of(i)));
        }
        // creators of the contained levels
        let children: Vec<String> = self.r.levels.iter().filter(|l| container(l) == Some(level)).cloned().collect();
        for c in children {
            let (b, e) = self.constructor(&c);
            let inputs = self.r.inputs.get(&c).map(|x| x.0.clone()).unwrap_or_default();
            let sep = if inputs.is_empty() { "" } else { ", " };
            let head = format!("func {c}(self{sep}{inputs}) {}", level_class(&self.r.ctx.name, &c));
            out.extend(self.function(&head, &b, &e, at));
        }
        out.push(("}".into(), at));
        out
    }

    fn once(&mut self) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let ctx = self.r.ctx.name.clone();
        let at = self.r.ctx.line;
        let (b, e) = self.constructor("application");
        let mut inputs = self.r.inputs.get("application").map(|x| x.0.clone()).unwrap_or_default();
        if self.provenance {
            // the Supervisor capability enables live provenance (Platform 3.5), under a name no input has
            let sup = ["supervisor", "provenanceSupervisor", "diSupervisor"].iter().find(|n| !self.reserved().contains(**n)).copied().unwrap_or("diProvenanceSupervisor");
            inputs = if inputs.is_empty() { format!("{sup}: Supervisor") } else { format!("{inputs}, {sup}: Supervisor") };
            out.push((format!("observation {} {{", self.schema()), at));
            for f in ["event", "binding", "level"] {
                out.push((format!("  {f}: Int  required: none"), at));
            }
            out.push(("  encoding: bounded(64)".into(), at));
            out.push(("}".into(), at));
            out.push((String::new(), at));
        }
        out.push((format!("class {ctx} {{"), at));
        out.extend(self.function(&format!("func application({inputs}) {}", level_class(&ctx, "application")), &b, &e, at));
        out.push(("}".into(), at));
        for l in self.r.levels.clone() {
            out.push((String::new(), at));
            out.extend(self.class_of(&l));
        }
        out
    }

    /// The generated lines with the source line each comes from; generation repeats until the imports each level
    /// needs are stable.
    pub fn lines(&mut self) -> Vec<(String, usize)> {
        for _ in 0..8 {
            self.grew = false;
            let out = self.once();
            if !self.grew {
                return out;
            }
        }
        self.once()
    }
}
