//! The `context` body (DI schema v1, SP-08's design, section 2): `parent`, level inputs, groups and
//! `provide` bindings, parsed from the head body's statements.

use fortis_syntax::{Kind, Token};

use crate::scan::{closing, commas, Head, Stmt};
use crate::Problem;

pub const LEVELS: &[&str] = &["application", "session", "request", "task", "transaction", "transient"];

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Class(String),
    Instance(String),
    Factory(String),
}

impl Source {
    pub fn show(&self) -> String {
        match self {
            Source::Class(c) => c.clone(),
            Source::Instance(e) => format!("instance {e}"),
            Source::Factory(f) => format!("factory {f}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// a consumer, or a group name (told apart once the groups are known)
    Name(String),
    Slot(String, String),
}

impl Target {
    pub fn show(&self) -> String {
        match self {
            Target::Name(n) => n.clone(),
            Target::Slot(c, s) => format!("{c}.{s}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub context: String,
    pub key: String,
    pub source: Source,
    pub targets: Vec<Target>,
    pub replaces: bool,
    pub lifetime: Option<String>,
    pub line: usize,
    pub col: usize,
    /// where `replaces` is written in this statement: after its last token before a `{ lifetime: … }` block
    pub replaces_at: (usize, usize),
}

impl Binding {
    /// (context, key, source, sorted targets, lifetime): what a binding is, independent of where it is written
    pub fn identity(&self) -> String {
        let mut ts: Vec<String> = self.targets.iter().map(|t| t.show()).collect();
        ts.sort();
        let to = if ts.is_empty() { String::new() } else { format!(" to [{}]", ts.join(", ")) };
        let lt = self.lifetime.as_ref().map(|l| format!(" {{ lifetime: {l} }}")).unwrap_or_default();
        format!("{}: provide {} using {}{to}{lt}", self.context, self.key, self.source.show())
    }

    /// Where the instance lives: a level name, or `transient` (design section 3).
    pub fn placement(&self) -> &str {
        match (&self.source, self.lifetime.as_deref()) {
            (_, Some(l)) => l,
            (Source::Instance(_), None) => "application",
            (_, None) => "transient",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    pub name: String,
    pub parent: Option<(String, usize, usize)>,
    /// level -> (parameter text, parameter names); `application` holds the head parameters
    pub inputs: Vec<(String, String, Vec<String>)>,
    pub groups: Vec<(String, Vec<Target>)>,
    pub bindings: Vec<Binding>,
    pub line: usize,
    pub end_line: usize,
}

fn text(ts: &[Token]) -> String {
    ts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")
}

fn targets(ts: &[Token], from: usize, to: usize) -> Vec<Target> {
    let (a, b) = if from < to && ts[from].text == "[" { (from + 1, closing(ts, from).min(to)) } else { (from, to) };
    commas(ts, a, b)
        .into_iter()
        .map(|(x, y)| if y - x >= 3 && ts[x + 1].text == "." { Target::Slot(ts[x].text.clone(), ts[x + 2].text.clone()) } else { Target::Name(ts[x].text.clone()) })
        .collect()
}

fn provide(name: &str, s: &Stmt, out: &mut Vec<Problem>) -> Option<Binding> {
    let ts = &s.toks;
    let mut end = ts.len();
    let mut lifetime = None;
    // a trailing `{ lifetime: level }`
    if ts.last().map_or(false, |t| t.text == "}") {
        let open = (0..end).rev().find(|&i| ts[i].text == "{" && closing(ts, i) == end - 1);
        if let Some(o) = open {
            if o + 3 < end && ts[o + 1].text == "lifetime" && ts[o + 2].text == ":" {
                lifetime = Some(ts[o + 3].text.clone());
                end = o;
            }
        }
    }
    let mut replaces = false;
    if end > 0 && ts[end - 1].text == "replaces" {
        replaces = true;
        end -= 1;
    }
    let last = &ts[end.max(1) - 1];
    let replaces_at = (last.line, last.col + last.text.chars().count());
    let using = (1..end).find(|&i| ts[i].text == "using" && ts[i].kind == Kind::Lower);
    // `to` at depth 0, after `using` when there is one (`provide A to B` has none)
    let mut depth = 0i32;
    let mut to_at = None;
    for i in using.unwrap_or(1)..end {
        match ts[i].text.as_str() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            "to" if depth == 0 && ts[i].kind == Kind::Lower => {
                to_at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let src_end = to_at.unwrap_or(end);
    let key_end = using.unwrap_or(src_end);
    if key_end <= 1 {
        out.push(Problem::new("E-DI-010", "`provide` names no type", "PLAT-03-01", s.line, s.col));
        return None;
    }
    let key = crate::scan::Src::new(&s.text).span(&shift(&ts[1], s), &shift(&ts[key_end - 1], s));
    let source = match using {
        None => Source::Class(key.clone()),
        Some(u) if u + 1 >= src_end => {
            out.push(Problem::new("E-DI-010", "`using` names no source", "PLAT-03-01", s.line, s.col));
            return None;
        }
        Some(u) => {
            let span = |a: usize, b: usize| crate::scan::Src::new(&s.text).span(&shift(&ts[a], s), &shift(&ts[b - 1], s));
            match ts[u + 1].text.as_str() {
                "instance" if u + 2 < src_end => Source::Instance(span(u + 2, src_end)),
                "factory" if u + 2 < src_end => Source::Factory(text(&ts[u + 2..src_end]).replace(" . ", ".")),
                _ => Source::Class(span(u + 1, src_end)),
            }
        }
    };
    let tgts = match to_at {
        Some(t) => targets(ts, t + 1, end),
        None => vec![],
    };
    if let Some(l) = &lifetime {
        if !LEVELS.contains(&l.as_str()) {
            out.push(Problem::new("E-DI-008", format!("`{l}` is not a scope level (application, session, request, task, transaction, transient)"), "PLAT-03-04", s.line, s.col));
            return None;
        }
        if l == "transient" && matches!(source, Source::Instance(_)) {
            out.push(Problem::new("E-DI-010", "an instance binding is one value and cannot be transient", "PLAT-03-01", s.line, s.col));
            return None;
        }
    }
    Some(Binding { context: name.into(), key, source, targets: tgts, replaces, lifetime, line: s.line, col: s.col, replaces_at })
}

/// A token re-addressed relative to its statement's text (line 1 is the statement's first line).
fn shift(t: &Token, s: &Stmt) -> Token {
    let mut u = t.clone();
    if t.line == s.line {
        u.col = t.col - s.col + 1;
    }
    u.line = t.line - s.line + 1;
    u
}

pub fn parse(h: &Head, out: &mut Vec<Problem>) -> Context {
    let mut c = Context { name: h.name.clone(), line: h.line, end_line: h.end_line, ..Default::default() };
    c.inputs.push(("application".into(), h.params.clone(), h.param_names.clone()));
    for s in &h.body {
        let ts = &s.toks;
        let first = ts[0].text.as_str();
        match first {
            "provide" => {
                if let Some(b) = provide(&h.name, s, out) {
                    c.bindings.push(b);
                }
            }
            // a context's statements may come in any order (Platform 3.3): `parent` too
            "parent" if ts.len() == 2 => {
                if c.parent.is_some() {
                    out.push(Problem::new("E-DI-010", format!("{} declares a second parent", h.name), "PLAT-03-01", s.line, s.col));
                } else {
                    c.parent = Some((ts[1].text.clone(), s.line, s.col));
                }
            }
            "group" if ts.len() >= 4 && ts[2].text == "=" => {
                if c.groups.iter().any(|(g, _)| *g == ts[1].text) {
                    out.push(Problem::new("E-DI-010", format!("the group {} is declared twice in {}", ts[1].text, h.name), "PLAT-03-01", s.line, s.col));
                } else {
                    c.groups.push((ts[1].text.clone(), targets(ts, 3, ts.len())));
                }
            }
            l if ["session", "request", "task", "transaction"].contains(&l) && ts.len() >= 3 && ts[1].text == "(" => {
                let close = closing(ts, 1);
                let ptext = if close > 2 { crate::scan::Src::new(&s.text).span(&shift(&ts[2], s), &shift(&ts[close - 1], s)) } else { String::new() };
                let names = commas(ts, 2, close).into_iter().map(|(x, _)| ts[x].text.clone()).collect();
                if c.inputs.iter().any(|(x, _, _)| x == l) {
                    out.push(Problem::new("E-DI-010", format!("the {l} inputs of {} are declared twice", h.name), "PLAT-03-01", s.line, s.col));
                } else {
                    c.inputs.push((l.into(), ptext, names));
                }
            }
            _ => out.push(Problem::new("E-DI-010", format!("`{}` is not a statement of a context (DI schema v1: parent, a level's inputs, group, provide)", s.text), "PLAT-03-01", s.line, s.col)),
        }
    }
    c
}
