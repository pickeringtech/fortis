//! The runtime of the generated parser: a port of the runtime that `gen_parser.py` emits for Python. Decisions
//! consult the generated k-token tries over token classes; documented conflicts are resolved by ordered choice with
//! local backtracking and counted.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::generated::{dispatch, rule_id, KEYS, TABLES, TRIE_EDGES, TRIE_NODES};
use crate::lexer::{keywords, Kind, Token};
use crate::tree::{Child, Node};

pub type PResult = Result<(), Fail>;
pub type AltFn<'a> = fn(&mut Parser<'a>) -> PResult;

/// A failed attempt; the parser keeps the furthest failure and what was expected there.
#[derive(Debug)]
pub struct Fail;

#[derive(Clone, Debug)]
pub struct ParseError {
    pub token: usize,
    pub line: usize,
    pub col: usize,
    pub found_kind: &'static str,
    pub found_text: String,
    pub expected: Vec<String>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let exp: Vec<&str> = self.expected.iter().map(|s| s.as_str()).collect();
        let mut joined = exp.join(", ");
        joined.truncate(120);
        write!(f, "unexpected {} {} at {}:{}; expected one of: {}", self.found_kind, py_repr(&self.found_text), self.line, self.col, joined)
    }
}

pub fn py_repr(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

const ANY: &str = "$ANY";
const NO_KEY: u16 = u16::MAX;
const ACCEPT: u32 = u32::MAX;

fn key_index() -> &'static HashMap<&'static str, u16> {
    static M: OnceLock<HashMap<&'static str, u16>> = OnceLock::new();
    M.get_or_init(|| KEYS.iter().enumerate().map(|(i, k)| (*k, i as u16)).collect())
}

fn kind_only(kind: Kind) -> bool {
    matches!(kind, Kind::Str | Kind::Raw | Kind::Bytes | Kind::Eof | Kind::RefOpen | Kind::RefClose | Kind::Nl)
}

struct Frame {
    rule: u16,
    children: Vec<Child>,
}

pub struct Parser<'a> {
    pub toks: &'a [Token],
    keys: Vec<(u16, u16)>,
    pub pos: usize,
    /// undocumented conflicts resolved by backtracking (must be 0)
    pub fallbacks: u32,
    /// documented ordered-choice resolutions (policy P3)
    pub ordered: u32,
    stack: Vec<Frame>,
    furthest: usize,
    expected: BTreeSet<String>,
    any_key: u16,
    eof_key: u16,
}

impl<'a> Parser<'a> {
    pub fn new(toks: &'a [Token]) -> Parser<'a> {
        let idx = key_index();
        // each kind's key once, not a formatted string per token
        let mut kind_keys: HashMap<&'static str, u16> = HashMap::new();
        let keys = toks
            .iter()
            .map(|t| {
                let name = t.kind.as_str();
                let kk = *kind_keys.entry(name).or_insert_with(|| idx.get(format!("KIND:{name}").as_str()).copied().unwrap_or(NO_KEY));
                let tk = if kind_only(t.kind) { NO_KEY } else { idx.get(t.text.as_str()).copied().unwrap_or(NO_KEY) };
                (kk, tk)
            })
            .collect();
        Parser {
            toks,
            keys,
            pos: 0,
            fallbacks: 0,
            ordered: 0,
            stack: Vec::new(),
            furthest: 0,
            expected: BTreeSet::new(),
            any_key: idx.get(ANY).copied().unwrap_or(NO_KEY),
            eof_key: idx.get("KIND:EOF").copied().unwrap_or(NO_KEY),
        }
    }

    // ---- tables -------------------------------------------------------------------------
    fn edge(&self, node: u32, key: u16) -> Option<u32> {
        if key == NO_KEY {
            return None;
        }
        let (start, len) = TRIE_NODES[node as usize];
        let edges = &TRIE_EDGES[start as usize..(start + len) as usize];
        edges.binary_search_by_key(&key, |e| e.0).ok().map(|i| edges[i].1)
    }

    fn matches_node(&self, node: u32, pos: usize) -> bool {
        if node == ACCEPT {
            return true;
        }
        let (_, len) = TRIE_NODES[node as usize];
        if len == 0 {
            return false;
        }
        if pos >= self.toks.len() {
            return match self.edge(node, self.eof_key) {
                Some(sub) => self.matches_node(sub, pos + 1),
                None => false,
            };
        }
        let (kk, tk) = self.keys[pos];
        for key in [kk, tk, self.any_key] {
            if let Some(sub) = self.edge(node, key) {
                if self.matches_node(sub, pos + 1) {
                    return true;
                }
            }
        }
        false
    }

    pub fn matches(&self, table: usize) -> bool {
        self.matches_node(TABLES[table], self.pos)
    }

    fn first_key(table: usize) -> String {
        let (start, len) = TRIE_NODES[TABLES[table] as usize];
        if len == 0 {
            return "?".into();
        }
        KEYS[TRIE_EDGES[start as usize].0 as usize].to_string()
    }

    // ---- the tree -------------------------------------------------------------------------
    pub fn enter(&mut self, rule: u16) {
        self.stack.push(Frame { rule, children: Vec::new() });
    }

    pub fn leave(&mut self) {
        let f = self.stack.pop().expect("frame");
        let node = Node { rule: f.rule, children: f.children };
        if let Some(top) = self.stack.last_mut() {
            top.children.push(Child::Node(node));
        }
    }

    fn tok(&self) -> usize {
        if self.pos < self.toks.len() {
            self.pos
        } else {
            self.toks.len() - 1
        }
    }

    pub fn fail<I: IntoIterator<Item = String>>(&mut self, expected: I) -> PResult {
        if self.pos < self.furthest {
            return Err(Fail);
        }
        if self.pos > self.furthest {
            self.furthest = self.pos;
            self.expected = expected.into_iter().collect();
        } else if self.pos == self.furthest {
            self.expected.extend(expected);
        }
        Err(Fail)
    }

    fn push_tok(&mut self, i: usize) {
        if let Some(top) = self.stack.last_mut() {
            top.children.push(Child::Tok(i));
        }
    }

    pub fn lit(&mut self, text: &str) -> PResult {
        let i = self.tok();
        let t = &self.toks[i];
        if matches!(t.kind, Kind::Str | Kind::Raw | Kind::Bytes | Kind::Eof | Kind::RefOpen | Kind::RefClose) || t.text != text {
            return self.fail([py_repr(text)]);
        }
        self.pos += 1;
        self.push_tok(i);
        Ok(())
    }

    pub fn kind(&mut self, kind: &str, desc: &str) -> PResult {
        let i = self.tok();
        let t = &self.toks[i];
        if t.kind.as_str() != kind {
            return self.fail([desc.to_string()]);
        }
        if desc == "TEMPLATE" && !t.interps.iter().all(|(src, _, _)| placeholder(src) && !keywords().contains(src.trim())) {
            return self.fail(["TEMPLATE (placeholders name payload fields)".to_string()]);
        }
        self.pos += 1;
        self.push_tok(i);
        Ok(())
    }

    /// A balanced token sequence up to the "}" that closes a platform head (Language Reference 16).
    pub fn head_body(&mut self) -> PResult {
        match head_body_end(self.toks, self.pos) {
            Some(end) => {
                let node = Node { rule: rule_id("headBody").unwrap_or(u16::MAX), children: (self.pos..end).map(Child::Tok).collect() };
                if let Some(top) = self.stack.last_mut() {
                    top.children.push(Child::Node(node));
                }
                self.pos = end;
                Ok(())
            }
            None => self.fail(["balanced head body".to_string()]),
        }
    }

    /// Ordered choice over (table, branch) pairs; `exclusive` means the tables are disjoint (LL(k)).
    pub fn choose(&mut self, tries: &[(usize, AltFn<'a>)], exclusive: bool, documented: bool) -> PResult {
        // the disjoint (LL(k)) case, the common one, needs no list of candidates
        if exclusive {
            for (t, f) in tries {
                if self.matches(*t) {
                    return f(self);
                }
            }
            let exp: Vec<String> = tries.iter().map(|(t, _)| Self::first_key(*t)).collect();
            return self.fail(exp);
        }
        let cands: Vec<AltFn<'a>> = tries.iter().filter(|(t, _)| self.matches(*t)).map(|(_, f)| *f).collect();
        if cands.is_empty() {
            let exp: Vec<String> = tries.iter().map(|(t, _)| Self::first_key(*t)).collect();
            return self.fail(exp);
        }
        if exclusive || cands.len() == 1 {
            return cands[0](self);
        }
        if documented {
            self.ordered += 1;
        } else {
            self.fallbacks += 1;
        }
        let save = self.pos;
        let depth = self.stack.len();
        let kept = self.stack.last().map_or(0, |f| f.children.len());
        let mut last = Err(Fail);
        for f in cands {
            match f(self) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last = Err(e);
                    self.pos = save;
                    self.stack.truncate(depth);
                    if let Some(top) = self.stack.last_mut() {
                        top.children.truncate(kept);
                    }
                }
            }
        }
        last
    }

    /// Parse from a start rule; the tree's root is the start rule's node.
    pub fn parse(mut self, start: &str) -> Result<(Node, u32, u32), ParseError> {
        let rid = rule_id(start).expect("start rule");
        self.enter(u16::MAX);
        let mut r = dispatch(&mut self, rid);
        if r.is_ok() {
            while self.tok_kind() == Kind::Nl {
                self.pos += 1;
            }
            if self.tok_kind() != Kind::Eof {
                r = self.fail(["end of input".to_string()]);
            }
        }
        match r {
            Ok(()) => {
                let mut root = self.stack.pop().expect("root frame");
                match root.children.pop() {
                    Some(Child::Node(n)) => Ok((n, self.fallbacks, self.ordered)),
                    _ => unreachable!("the start rule produced no node"),
                }
            }
            Err(_) => {
                let i = self.furthest.min(self.toks.len() - 1);
                let t = &self.toks[i];
                Err(ParseError {
                    token: i,
                    line: t.line,
                    col: t.col,
                    found_kind: t.kind.as_str(),
                    found_text: t.text.clone(),
                    expected: self.expected.iter().cloned().collect(),
                })
            }
        }
    }

    fn tok_kind(&self) -> Kind {
        self.toks[self.tok()].kind
    }
}

/// `^\s*[a-z_][A-Za-z0-9_]*\s*$`: a template placeholder names a payload field.
fn placeholder(src: &str) -> bool {
    let s = src.trim();
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => cs.all(|c| c.is_ascii_alphanumeric() || c == '_'),
        _ => false,
    }
}

/// The recogniser's balanced scan: the index of the "}" that closes the head, if the tokens balance.
pub fn head_body_end(toks: &[Token], pos: usize) -> Option<usize> {
    let mut stack: Vec<&str> = Vec::new();
    let mut i = pos;
    while i < toks.len() {
        let t = &toks[i];
        match t.kind {
            Kind::Eof => return None,
            Kind::RefOpen => stack.push("REF_CLOSE"),
            Kind::RefClose => {
                if stack.pop() != Some("REF_CLOSE") {
                    return None;
                }
            }
            Kind::Punct if t.text == "(" => stack.push(")"),
            Kind::Punct if t.text == "[" => stack.push("]"),
            Kind::Punct if t.text == "{" => stack.push("}"),
            Kind::Punct if t.text == ")" || t.text == "]" || t.text == "}" => {
                if stack.is_empty() {
                    return if t.text == "}" { Some(i) } else { None };
                }
                if stack.pop() != Some(t.text.as_str()) {
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}
