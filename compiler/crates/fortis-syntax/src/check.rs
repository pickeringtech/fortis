//! The syntax layer's judgement, a port of `try_parse` and `judge` in conformance/tools/run.py: lex (E-SYN-001 and
//! lexical E-SYN-000), parse (E-SYN-002 for a keyword standing where an identifier is required, else E-SYN-000 at the
//! furthest position any derivation reaches), E-SYN-004 for mixed operator families, then each interpolation parsed
//! as an expression that may neither await nor handle an outcome (LANG-03-06).

use std::collections::BTreeSet;

use fortis_diag::{Json, Record};

use crate::grammar::Recogniser;
use crate::lexer::{lex, Kind, Token};
use crate::runtime::{py_repr, Parser};
use crate::tree::{Child, Node, Parsed};

pub struct Outcome {
    pub ok: bool,
    pub code: Option<String>,
    pub line: usize,
    pub col: usize,
    pub records: Vec<Record>,
    pub parsed: Option<Parsed>,
    pub form: &'static str,
}

fn rec(file: &str, code: &str, message: String, rule: &str, line: usize, col: usize, expected: Vec<String>, found: &str) -> Record {
    Record::new(code, message, rule, file, line, col, Json::obj([("expected_tokens", Json::strs(expected)), ("found_token", Json::str(found))]))
}

fn edit(file: &str, line: usize, col: usize, end_line: usize, end_col: usize, replacement: &str) -> Json {
    Json::obj([
        ("file", Json::str(file)),
        ("line", Json::Int(line as i64)),
        ("column", Json::Int(col as i64)),
        ("end_line", Json::Int(end_line as i64)),
        ("end_column", Json::Int(end_col as i64)),
        ("replacement", Json::str(replacement)),
    ])
}

/// Parse with the generated parser; on failure, the reference's position comes from the recogniser.
fn parse_tokens(toks: &[Token], start: &'static str) -> Result<Node, (usize, BTreeSet<String>)> {
    match Parser::new(toks).parse(start) {
        Ok((node, _, _)) => Ok(node),
        Err(e) => match Recogniser::new(toks).recognise(start) {
            Err(f) => Err((f.token, f.expected)),
            Ok(()) => Err((e.token, e.expected.into_iter().collect())),
        },
    }
}

fn recognise_index(toks: &[Token], start: &'static str) -> usize {
    match Recogniser::new(toks).recognise(start) {
        Ok(()) => toks.len(),
        Err(f) => f.token,
    }
}

/// E-SYN-002 (LR 3.4): the first keyword of the failing statement that stands where an identifier is required.
fn keyword_as_identifier(toks: &[Token], fail_idx: usize, start: &'static str) -> Option<usize> {
    let stmt_end = (fail_idx..toks.len()).find(|&k| matches!(toks[k].kind, Kind::Nl | Kind::Eof)).unwrap_or(toks.len() - 1);
    let mut stmt_start = fail_idx;
    while stmt_start > 0 && toks[stmt_start - 1].kind != Kind::Nl {
        stmt_start -= 1;
    }
    let mut alt: Vec<Token> = toks.to_vec();
    let mut reclassified: Vec<usize> = Vec::new();
    let mut idx = fail_idx;
    loop {
        let mut progressed = false;
        let mut cand = idx as isize;
        while cand >= stmt_start as isize {
            let c = cand as usize;
            cand -= 1;
            if alt[c].kind != Kind::Kw {
                continue;
            }
            let saved = alt[c].kind;
            alt[c].kind = if alt[c].text.chars().next().map_or(false, |ch| ch.is_uppercase()) { Kind::Upper } else { Kind::Lower };
            let new_idx = recognise_index(&alt, start);
            if new_idx > idx {
                reclassified.push(c);
                idx = new_idx;
                progressed = true;
                break;
            }
            alt[c].kind = saved;
        }
        if !progressed {
            return None;
        }
        if idx > stmt_end {
            return reclassified.iter().min().copied();
        }
    }
}

const STOP: &[&str] = &["primaryExpr", "block", "argList", "headBody"];

fn families(node: &Node, toks: &[Token], found: &mut Vec<(&'static str, usize)>, root: bool) {
    let name = node.name();
    if STOP.contains(&name) || (name == "expr" && !root) {
        return;
    }
    let fam = match name {
        "outcomeExpr" => Some("outcome"),
        "coalesceExpr" => Some("coalesce"),
        "orExpr" | "andExpr" => Some("logical"),
        _ => None,
    };
    if let Some(fam) = fam {
        let op = node.children.iter().find_map(|c| match c {
            Child::Tok(i) => {
                let t = &toks[*i].text;
                let hit = (fam == "outcome" && (t == "or" || t == "catch")) || (fam == "coalesce" && t == "??") || (fam == "logical" && (t == "||" || t == "&&"));
                if hit {
                    Some(*i)
                } else {
                    None
                }
            }
            _ => None,
        });
        if let Some(i) = op {
            if !found.iter().any(|(f, _)| *f == fam) {
                found.push((fam, i));
            }
        }
    }
    for c in &node.children {
        if let Child::Node(n) = c {
            families(n, toks, found, false);
        }
    }
}

fn first_token(node: &Node) -> Option<usize> {
    for c in &node.children {
        match c {
            Child::Tok(i) => return Some(*i),
            Child::Node(n) => {
                if let Some(i) = first_token(n) {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// LR 5.6: the second operator family's token and the expression's first token, if families are mixed.
fn operator_mixing(node: &Node, toks: &[Token]) -> Option<(usize, usize)> {
    if node.name() == "expr" {
        let mut found = Vec::new();
        families(node, toks, &mut found, true);
        if found.len() > 1 {
            let mut ts: Vec<usize> = found.iter().map(|(_, i)| *i).collect();
            ts.sort_by_key(|i| (toks[*i].line, toks[*i].col));
            return Some((ts[1], first_token(node).unwrap_or(ts[1])));
        }
    }
    for c in &node.children {
        if let Child::Node(n) = c {
            if let Some(r) = operator_mixing(n, toks) {
                return Some(r);
            }
        }
    }
    None
}

/// An `await` or an outcome operator in an interpolated expression (closures excluded).
fn suspend_or_outcome(node: &Node, toks: &[Token]) -> Option<usize> {
    let name = node.name();
    if name == "lambda" {
        return None;
    }
    if name == "unaryExpr" {
        if let Some(Child::Tok(i)) = node.children.first() {
            if toks[*i].kind == Kind::Kw && toks[*i].text == "await" {
                return Some(*i);
            }
        }
    }
    if name == "outcomeExpr" && node.children.len() > 1 {
        if let Some(i) = node.children.iter().find_map(|c| if let Child::Tok(i) = c { Some(*i) } else { None }) {
            return Some(i);
        }
    }
    for c in &node.children {
        if let Child::Node(n) = c {
            if let Some(r) = suspend_or_outcome(n, toks) {
                return Some(r);
            }
        }
    }
    None
}

pub fn try_parse(file: &str, src: &str, start: &'static str, line_offset: usize) -> Outcome {
    let fail = |code: &str, line: usize, col: usize, records: Vec<Record>| Outcome { ok: false, code: Some(code.into()), line, col, records, parsed: None, form: "module" };
    // the source quotas come first: past them nothing is lexed or parsed (PROG-SEC-01)
    let limit = |e: crate::limits::Exceeded| {
        let r = Record::new("E-LIM-002", format!("{e}: the input is not parsed"), "PROG-SEC-01", file, e.line + line_offset, e.col, Json::obj([("budget", Json::str(&e.budget)), ("operation", Json::str(&e.operation))]));
        Outcome { ok: false, code: Some("E-LIM-002".into()), line: e.line + line_offset, col: e.col, records: vec![r], parsed: None, form: "module" }
    };
    if let Err(e) = crate::limits::source(src) {
        return limit(e);
    }
    let toks = match lex(src) {
        Ok(t) => t,
        Err(e) if e.code == "E-LIM-002" => {
            return limit(crate::limits::Exceeded { budget: format!("nesting depth {}", crate::limits::MAX_NESTING), operation: e.message.split(" exceeds").next().unwrap_or(&e.message).to_string(), line: e.line, col: e.col });
        }
        Err(e) => {
            let rule = match e.code {
                "E-SYN-001" => "LANG-03-01",
                "E-SYN-002" => "LANG-03-04",
                _ => "LANG-03-02",
            };
            let found = if e.message.contains('\'') {
                let parts: Vec<&str> = e.message.split('\'').collect();
                parts[parts.len() - 2].to_string()
            } else {
                String::new()
            };
            let mut r = rec(file, e.code, e.message.clone(), rule, e.line + line_offset, e.col, vec![], &found);
            if e.code == "E-SYN-001" {
                r.correction = Some(Json::obj([
                    ("kind", Json::str("delete the semicolon")),
                    ("edits", Json::Arr(vec![edit(file, e.line + line_offset, e.col, e.line + line_offset, e.col + 1, "")])),
                ]));
            }
            return fail(e.code, e.line + line_offset, e.col, vec![r]);
        }
    };
    if let Err(e) = crate::limits::tokens(&toks) {
        return limit(e);
    }
    let root = match parse_tokens(&toks, start) {
        Ok(n) => n,
        Err((idx, expected)) => {
            if let Some(kw) = keyword_as_identifier(&toks, idx, start) {
                let t = &toks[kw];
                let r = rec(file, "E-SYN-002", format!("keyword {} used as an identifier", py_repr(&t.text)), "LANG-03-04", t.line + line_offset, t.col, vec!["lowerIdent".into()], &t.text);
                return fail("E-SYN-002", t.line + line_offset, t.col, vec![r]);
            }
            let t = &toks[idx];
            let exp: Vec<String> = expected.into_iter().take(12).collect();
            let r = rec(file, "E-SYN-000", format!("unexpected {} {}", t.kind.as_str(), py_repr(&t.text)), "LANG-03-02", t.line + line_offset, t.col, exp, &t.text);
            return fail("E-SYN-000", t.line + line_offset, t.col, vec![r]);
        }
    };
    if let Some((bad, first)) = operator_mixing(&root, &toks) {
        let (b, f) = (&toks[bad], &toks[first]);
        let mut r = rec(file, "E-SYN-004", format!("operator {} mixes operator families in one unparenthesised expression", py_repr(&b.text)), "LANG-05-06", b.line + line_offset, b.col, vec!["parentheses around one operand".into()], &b.text);
        r.correction = Some(Json::obj([
            ("kind", Json::str("parenthesise the left operand")),
            ("edits", Json::Arr(vec![edit(file, f.line + line_offset, f.col, f.line + line_offset, f.col, "("), edit(file, b.line + line_offset, b.col, b.line + line_offset, b.col, ") ")])),
        ]));
        return fail("E-SYN-004", b.line + line_offset, b.col, vec![r]);
    }
    for t in &toks {
        if t.kind != Kind::Str {
            continue;
        }
        for (isrc, iline, icol) in &t.interps {
            let o = try_parse(file, isrc, "expr", line_offset + iline - 1);
            if !o.ok {
                return o;
            }
            if let Some(p) = &o.parsed {
                if suspend_or_outcome(&p.root, &p.toks).is_some() {
                    let r = rec(file, "E-SYN-000", "an interpolated expression may not await or handle an outcome".into(), "LANG-03-06", iline + line_offset, *icol, vec![], "");
                    return fail("E-SYN-000", iline + line_offset, *icol, vec![r]);
                }
            }
        }
    }
    Outcome { ok: true, code: None, line: 0, col: 0, records: vec![], parsed: Some(Parsed { toks, root }), form: "module" }
}

/// Module first, then as a function body (the reference runner's judgement); the interpretation that got further
/// supplies the records.
pub fn judge(file: &str, src: &str) -> Outcome {
    let m = try_parse(file, src, "module", 0);
    if m.ok {
        return m;
    }
    let wrapped = format!("{{ {}\n}}\n", src);
    let mut b = try_parse(file, &wrapped, "block", 0);
    if b.ok {
        b.form = "statements";
        b.records.clear();
        return b;
    }
    if (b.line, b.col) > (m.line, m.col) {
        b.form = "statements";
        return b;
    }
    m
}
