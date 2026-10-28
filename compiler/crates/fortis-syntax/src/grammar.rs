//! The grammar as data and the memoised recogniser of `conformance/tools/parser.py`: alternatives are explored
//! exhaustively (sets of end positions), so the furthest position any derivation reaches is where a syntax error is
//! reported, exactly as the reference runner reports it. Trees come from the generated parser; this only recognises.

use std::collections::{BTreeSet, HashMap};

use crate::generated::{GRAMMAR, RULE_BODIES};
use crate::lexer::{keywords, Kind, Token};
use crate::runtime::py_repr;

#[derive(Debug)]
pub enum G {
    Lit(&'static str),
    Special(&'static str),
    Ref(&'static str),
    Opt(u32),
    Rep(u32),
    Seq(&'static [u32]),
    Alt(&'static [u32]),
}

type Ends = BTreeSet<usize>;

fn special_kinds(name: &str) -> Option<Kind> {
    Some(match name {
        "lowerIdent" => Kind::Lower,
        "upperIdent" => Kind::Upper,
        "INTEGER" => Kind::Int,
        "FLOAT" => Kind::Float,
        "DECIMAL" => Kind::Dec,
        "DURATION" => Kind::Dur,
        "STRING" | "TEMPLATE" => Kind::Str,
        "RAWSTRING" => Kind::Raw,
        "BYTESTRING" => Kind::Bytes,
        "NL" => Kind::Nl,
        "keywordName" => Kind::Kw,
        _ => return None,
    })
}

fn placeholder(src: &str) -> bool {
    let s = src.trim();
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => cs.all(|c| c.is_ascii_alphanumeric() || c == '_'),
        _ => false,
    }
}

pub struct Recogniser<'a> {
    toks: &'a [Token],
    bodies: HashMap<&'static str, u32>,
    memo: HashMap<(&'static str, usize), Ends>,
    node_memo: HashMap<(u32, usize), Ends>,
    pub furthest: usize,
    pub expected: BTreeSet<String>,
}

/// A recognition failure: the token index where it is reported and what was expected there.
pub struct Failure {
    pub token: usize,
    pub expected: BTreeSet<String>,
}

impl<'a> Recogniser<'a> {
    pub fn new(toks: &'a [Token]) -> Recogniser<'a> {
        Recogniser { toks, bodies: RULE_BODIES.iter().copied().collect(), memo: HashMap::new(), node_memo: HashMap::new(), furthest: 0, expected: BTreeSet::new() }
    }

    fn terminal(&mut self, pos: usize, ok: bool, desc: &str) -> Ends {
        if ok {
            if pos + 1 > self.furthest {
                self.furthest = pos + 1;
                self.expected.clear();
            }
            return [pos + 1].into_iter().collect();
        }
        if pos == self.furthest {
            self.expected.insert(desc.to_string());
        } else if pos > self.furthest {
            self.furthest = pos;
            self.expected = [desc.to_string()].into_iter().collect();
        }
        Ends::new()
    }

    fn ends(&mut self, node: u32, pos: usize) -> Ends {
        if let Some(r) = self.node_memo.get(&(node, pos)) {
            return r.clone();
        }
        let r = self.ends_raw(node, pos);
        self.node_memo.insert((node, pos), r.clone());
        r
    }

    fn ends_raw(&mut self, node: u32, pos: usize) -> Ends {
        match &GRAMMAR[node as usize] {
            G::Lit(text) => {
                let t = &self.toks[pos];
                let ok = !matches!(t.kind, Kind::Str | Kind::Raw | Kind::Bytes | Kind::Eof | Kind::RefOpen | Kind::RefClose) && t.text == *text;
                self.terminal(pos, ok, &py_repr(text))
            }
            G::Special(text) => {
                let (kind, desc) = match *text {
                    "REF_OPEN" => (Kind::RefOpen, "<"),
                    "REF_CLOSE" => (Kind::RefClose, ">"),
                    other => panic!("unmapped special sequence: {other}"),
                };
                let ok = self.toks[pos].kind == kind;
                self.terminal(pos, ok, desc)
            }
            G::Ref(name) => self.rule(name, pos),
            G::Opt(body) => {
                let mut r = self.ends(*body, pos);
                r.insert(pos);
                r
            }
            G::Rep(body) => {
                let mut result: Ends = [pos].into_iter().collect();
                let mut frontier: Ends = [pos].into_iter().collect();
                while !frontier.is_empty() {
                    let mut new = Ends::new();
                    for p in frontier.iter().copied().collect::<Vec<_>>() {
                        new.extend(self.ends(*body, p));
                    }
                    frontier = new.difference(&result).copied().collect();
                    result.extend(new);
                }
                result
            }
            G::Seq(items) => {
                let mut current: Ends = [pos].into_iter().collect();
                for item in items.iter() {
                    let mut nxt = Ends::new();
                    for p in current.iter().copied().collect::<Vec<_>>() {
                        nxt.extend(self.ends(*item, p));
                    }
                    current = nxt;
                    if current.is_empty() {
                        break;
                    }
                }
                current
            }
            G::Alt(opts) => {
                let mut result = Ends::new();
                for o in opts.iter() {
                    result.extend(self.ends(*o, pos));
                }
                result
            }
        }
    }

    pub fn rule(&mut self, name: &'static str, pos: usize) -> Ends {
        if let Some(r) = self.memo.get(&(name, pos)) {
            return r.clone();
        }
        let r = if let Some(kind) = special_kinds(name) {
            let t = &self.toks[pos];
            let mut ok = t.kind == kind;
            let desc = if ok && name == "TEMPLATE" {
                ok = t.interps.iter().all(|(src, _, _)| placeholder(src) && !keywords().contains(src.trim()));
                "TEMPLATE (placeholders name payload fields: ${name})"
            } else {
                name
            };
            self.terminal(pos, ok, desc)
        } else if name == "headBody" {
            self.head_body(pos)
        } else {
            self.memo.insert((name, pos), Ends::new()); // guard against left recursion
            let body = self.bodies[name];
            self.ends(body, pos)
        };
        self.memo.insert((name, pos), r.clone());
        r
    }

    fn head_body(&mut self, pos: usize) -> Ends {
        let mut stack: Vec<&str> = Vec::new();
        let mut i = pos;
        while i < self.toks.len() {
            let t = &self.toks[i];
            match t.kind {
                Kind::Eof => return Ends::new(),
                Kind::RefOpen => stack.push("REF_CLOSE"),
                Kind::RefClose => {
                    if stack.pop() != Some("REF_CLOSE") {
                        return self.terminal(i, false, "matching refinement delimiter in head body");
                    }
                }
                Kind::Punct if t.text == "(" => stack.push(")"),
                Kind::Punct if t.text == "[" => stack.push("]"),
                Kind::Punct if t.text == "{" => stack.push("}"),
                Kind::Punct if t.text == ")" || t.text == "]" || t.text == "}" => {
                    if stack.is_empty() {
                        if t.text == "}" {
                            self.furthest = self.furthest.max(i);
                            return [i].into_iter().collect();
                        }
                        return self.terminal(i, false, "balanced head body");
                    }
                    let want = stack.pop();
                    if want != Some(t.text.as_str()) {
                        return self.terminal(i, false, "matching delimiter in head body");
                    }
                }
                _ => {}
            }
            i += 1;
        }
        Ends::new()
    }

    /// Recognise the tokens from `start`; Err names where the furthest derivation stopped.
    pub fn recognise(mut self, start: &'static str) -> Result<(), Failure> {
        let end = self.toks.len() - 1;
        let ends = self.rule(start, 0);
        if ends.contains(&end) {
            return Ok(());
        }
        if start != "module" {
            for e in &ends {
                if self.toks[*e..end].iter().all(|t| t.kind == Kind::Nl) {
                    return Ok(());
                }
            }
        }
        Err(Failure { token: self.furthest.min(end), expected: self.expected })
    }
}
