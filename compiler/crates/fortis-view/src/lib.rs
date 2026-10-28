//! `fortis view` (Programme 9.2, DEL-41): the reader agents and people use instead of `cat` and `grep` on `.ft`
//! files. Every function here reads; nothing writes.
//!
//! - **Lines**: each source line with spans classed from the language's own lexer (keyword, type, name, string,
//!   number, comment, punctuation). The JSON lines are the primary form; the terminal highlighting is derived
//!   from the same spans.
//! - **Declarations**: kind, name, position, modifiers, named contracts, aspects, whether exported, the
//!   documentation comment above, and the head as written.
//! - **Symbols**: the full text of the declarations with a given name (`Name` or `Type.member`).
//! - **Queries**: structural questions text search cannot answer, each defined on the parse tree (`QUERIES`).

use fortis_syntax::lexer::{lex, Kind, Token};
use fortis_syntax::tree::Parsed;
use fortis_syntax::view::{C, N};

/// A module parsed as a module, or a fragment of statements parsed as a block (as the conformance harness parses
/// one); `shift` is the column offset the block wrapper added to line 1.
pub struct Source {
    pub parsed: Parsed,
    pub shift: usize,
}

pub fn parse(src: &str) -> Option<Source> {
    if let Ok(p) = fortis_syntax::parse_module(src) {
        return Some(Source { parsed: p, shift: 0 });
    }
    fortis_syntax::parse_from(&format!("{{ {src}\n}}\n"), "block").ok().map(|p| Source { parsed: p, shift: 2 })
}

fn pos(t: &Token, shift: usize) -> (usize, usize) {
    (t.line, if t.line == 1 { t.col.saturating_sub(shift) } else { t.col })
}

/// One answer to a query: where it is and what it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub line: usize,
    pub col: usize,
    pub text: String,
}

/// The queries, each with what it finds.
pub const QUERIES: &[(&str, &str)] = &[
    ("async", "every async function or method"),
    ("try", "every try function or method"),
    ("async-try", "every function or method that is both async and try"),
    ("pure", "every pure function or method"),
    ("noalloc", "every noalloc function or method"),
    ("trusted", "every trusted function or method"),
    ("recovers", "every function or method declared recovers"),
    ("editable-params", "every editable parameter, self included"),
    ("owned-params", "every owned parameter, self included"),
    ("or-fail", "every `or fail` site"),
    ("or-recover", "every `or recover` site"),
    ("catch", "every `catch` site"),
    ("fail", "every `fail` expression"),
    ("return", "every `return` statement"),
    ("defer", "every `defer` block"),
    ("scope", "every `scope` block"),
    ("capture", "every `capture` statement"),
    ("expect", "every `expect` statement"),
    ("move", "every `move` marker"),
    ("lambda", "every lambda"),
    ("tests", "every test declaration"),
    ("errors", "every error type"),
    ("classes", "every class"),
    ("aspect:NAME", "every use of the aspect NAME (`aspect:get` finds every @get route)"),
];

/// The name token of a function or method head: the first token after `func`.
fn head_name<'a>(head: N<'a>) -> Option<&'a Token> {
    let kids = head.kids();
    let at = kids.iter().position(|c| c.text_is("func"))?;
    kids.get(at + 1).and_then(|c| c.tokens().first().copied())
}

fn head_modifiers(head: N) -> Vec<String> {
    head.child_node("modifiers").map_or(vec![], |m| m.tokens().iter().map(|t| t.text.clone()).collect())
}

/// The answers to one query over a parsed source.
pub fn query(q: &str, src: &Source) -> Result<Vec<Match>, String> {
    let root = N::new(&src.parsed.root, &src.parsed.toks);
    let shift = src.shift;
    let m = |t: &Token, text: &str| {
        let (line, col) = pos(t, shift);
        Match { line, col, text: text.to_string() }
    };
    let mut out = Vec::new();
    let heads = || root.walk().into_iter().filter(|n| matches!(n.name(), "funcHead" | "methodHead")).collect::<Vec<_>>();
    let modifier_query = |want: &[&str]| -> Vec<Match> {
        heads()
            .into_iter()
            .filter(|h| {
                let mods = head_modifiers(*h);
                want.iter().all(|w| mods.iter().any(|x| x == w))
            })
            .filter_map(|h| head_name(h).map(|t| m(t, &t.text)))
            .collect()
    };
    match q {
        "async" => out = modifier_query(&["async"]),
        "try" => out = modifier_query(&["try"]),
        "async-try" => out = modifier_query(&["async", "try"]),
        "pure" => out = modifier_query(&["pure"]),
        "noalloc" => out = modifier_query(&["noalloc"]),
        "trusted" => out = modifier_query(&["trusted"]),
        "recovers" => out = modifier_query(&["recovers"]),
        "editable-params" | "owned-params" => {
            let mode = if q == "editable-params" { "editable" } else { "owned" };
            for p in root.find_all("param") {
                if let Some(sp) = p.child_node("selfParam") {
                    if let Some(t) = sp.direct_tokens().into_iter().find(|t| t.text == mode) {
                        out.push(m(t, "self"));
                    }
                } else if let Some(pt) = p.child_node("paramType") {
                    if pt.direct_tokens().first().map_or(false, |t| t.text == mode) {
                        if let Some(name) = p.direct_tokens().first() {
                            out.push(m(name, &name.text));
                        }
                    }
                }
            }
        }
        "or-fail" | "or-recover" | "catch" => {
            for o in root.find_all("outcomeExpr") {
                let toks = o.direct_tokens();
                for (i, t) in toks.iter().enumerate() {
                    let hit = match q {
                        "catch" => t.text == "catch",
                        "or-fail" => t.text == "or" && toks.get(i + 1).map_or(false, |n| n.text == "fail"),
                        _ => t.text == "or" && toks.get(i + 1).map_or(false, |n| n.text == "recover"),
                    };
                    if hit {
                        out.push(m(t, &t.text));
                    }
                }
            }
        }
        "fail" => {
            for p in root.find_all("primaryExpr") {
                if let Some(t) = p.kids().first().and_then(|c| c.tok()).filter(|t| t.text == "fail" && t.kind == Kind::Kw) {
                    out.push(m(t, "fail"));
                }
            }
        }
        "return" => {
            for s in root.find_all("statement") {
                if let Some(t) = s.kids().first().and_then(|c| c.tok()).filter(|t| t.text == "return") {
                    out.push(m(t, "return"));
                }
            }
        }
        "defer" | "scope" | "capture" | "expect" => {
            let rule = format!("{q}Stmt");
            for s in root.find_all(&rule) {
                if let Some(t) = s.first_token() {
                    out.push(m(t, &t.text));
                }
            }
        }
        "move" => {
            for k in root.find_all("marker") {
                if let Some(t) = k.first_token().filter(|t| t.text == "move") {
                    out.push(m(t, "move"));
                }
            }
        }
        "lambda" => {
            for l in root.find_all("lambda") {
                if let Some(t) = l.first_token() {
                    out.push(m(t, &t.text));
                }
            }
        }
        "tests" => {
            for d in root.find_all("testDecl") {
                if let Some(t) = d.direct_tokens().into_iter().find(|t| t.kind == Kind::Str) {
                    out.push(m(t, &t.text));
                }
            }
        }
        "errors" | "classes" => {
            let (rule, kw) = if q == "errors" { ("errorDecl", "error") } else { ("classDecl", "class") };
            for d in root.find_all(rule) {
                let toks = d.direct_tokens();
                if let Some(i) = toks.iter().position(|t| t.text == kw) {
                    if let Some(t) = toks.get(i + 1) {
                        out.push(m(t, &t.text));
                    }
                }
            }
        }
        _ if q.starts_with("aspect:") => {
            let want = &q["aspect:".len()..];
            for a in root.find_all("aspectUse") {
                let toks = a.direct_tokens();
                let name = toks.iter().find(|t| t.kind == Kind::Lower).map(|t| t.text.as_str());
                if name == Some(want) {
                    if let Some(t) = toks.first() {
                        out.push(m(t, &format!("@{want}")));
                    }
                }
            }
        }
        _ => return Err(format!("unknown query `{q}`; `fortis view --queries` lists them")),
    }
    out.sort_by_key(|x| (x.line, x.col));
    out.dedup();
    Ok(out)
}

/// A declaration as `--decls` lists it.
#[derive(Clone, Debug)]
pub struct Decl {
    pub kind: String,
    pub name: String,
    pub line: usize,
    pub col: usize,
    pub exported: bool,
    pub modifiers: Vec<String>,
    pub contracts: Vec<String>,
    pub aspects: Vec<String>,
    pub doc: String,
    pub head: String,
    pub member_of: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
}

const DECL_RULES: &[(&str, &str)] = &[
    ("funcDecl", "func"),
    ("classDecl", "class"),
    ("enumDecl", "enum"),
    ("errorDecl", "error"),
    ("interfaceDecl", "interface"),
    ("typeDecl", "type"),
    ("constDecl", "const"),
    ("aspectDecl", "aspect"),
    ("observationDecl", "observation"),
    ("contractDecl", "contract"),
    ("mixinDecl", "mixin"),
    ("implementDecl", "implement"),
    ("testDecl", "test"),
    ("propertyDecl", "property"),
];

fn line_span(n: N) -> (usize, usize) {
    let toks = n.tokens();
    let first = toks.iter().find(|t| !matches!(t.kind, Kind::Nl | Kind::Newline)).map_or(1, |t| t.line);
    let last = toks.iter().rev().find(|t| !matches!(t.kind, Kind::Nl | Kind::Newline)).map_or(first, |t| t.line + t.text.matches('\n').count());
    (first, last)
}

/// The comment lines directly above a line.
fn doc_above(lines: &[&str], line: usize) -> String {
    let mut out = Vec::new();
    let mut l = line;
    while l > 1 {
        let text = lines.get(l - 2).map_or("", |s| s.trim());
        if let Some(c) = text.strip_prefix("//") {
            out.push(c.trim().to_string());
            l -= 1;
        } else {
            break;
        }
    }
    out.reverse();
    out.join("\n")
}

fn join_tokens(toks: &[&Token]) -> String {
    fortis_fmt::render(toks)
}

/// Every declaration of a module and every member of its classes, in source order.
pub fn declarations(text: &str, src: &Source) -> Vec<Decl> {
    let root = N::new(&src.parsed.root, &src.parsed.toks);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    for d in root.child_nodes("declaration") {
        let exported = d.direct_tokens().iter().any(|t| t.text == "export");
        let aspects: Vec<String> = d.child_nodes("aspectUse").iter().map(|a| join_tokens(&a.tokens())).collect();
        let body = d.kids().into_iter().filter_map(|c| c.node()).find(|n| DECL_RULES.iter().any(|(r, _)| *r == n.name()));
        let Some(body) = body else { continue };
        let kind = DECL_RULES.iter().find(|(r, _)| *r == body.name()).map(|(_, k)| *k).unwrap_or("declaration");
        let (head_node, name_tok) = match body.child_node("funcHead") {
            Some(h) => (Some(h), head_name(h)),
            None => (None, {
                let toks = body.direct_tokens();
                let at = toks.iter().position(|t| t.text == kind || (kind == "func" && t.text == "func"));
                at.and_then(|i| toks.get(i + 1).copied()).or_else(|| toks.iter().find(|t| matches!(t.kind, Kind::Upper | Kind::Lower | Kind::Str)).copied())
            }),
        };
        let Some(name_tok) = name_tok else { continue };
        let modifiers = head_node.map_or(vec![], head_modifiers);
        let contracts: Vec<String> = modifiers.iter().filter(|m| m.chars().next().map_or(false, |c| c.is_uppercase())).cloned().collect();
        let modifiers: Vec<String> = modifiers.into_iter().filter(|m| !m.chars().next().map_or(false, |c| c.is_uppercase())).collect();
        let (first, last) = line_span(d);
        let head_toks: Vec<&Token> = match head_node {
            Some(h) => h.tokens().into_iter().filter(|t| !matches!(t.kind, Kind::Nl | Kind::Newline)).collect(),
            None => body.tokens().into_iter().take_while(|t| t.text != "{" && !matches!(t.kind, Kind::Nl | Kind::Newline)).collect(),
        };
        let (line, col) = pos(name_tok, src.shift);
        let name = name_tok.text.clone();
        out.push(Decl { kind: kind.into(), name: name.clone(), line, col, exported, modifiers, contracts, aspects, doc: doc_above(&lines, first), head: join_tokens(&head_toks), member_of: None, start_line: first, end_line: last });
        // the members of a class, an interface or a mixin
        for member in body.walk().into_iter().filter(|n| n.name() == "methodHead") {
            let Some(t) = head_name(member) else { continue };
            let mods = head_modifiers(member);
            let contracts: Vec<String> = mods.iter().filter(|m| m.chars().next().map_or(false, |c| c.is_uppercase())).cloned().collect();
            let modifiers: Vec<String> = mods.into_iter().filter(|m| !m.chars().next().map_or(false, |c| c.is_uppercase())).collect();
            let owner_kids: Vec<C> = body.kids();
            let _ = owner_kids;
            let (ml, mc) = pos(t, src.shift);
            let member_first = member.first_token().map_or(ml, |x| x.line);
            let decl_node = member;
            let (_, mlast) = line_span(decl_node);
            let exported_m = lines.get(member_first - 1).map_or(false, |l| l.trim_start().starts_with("export"));
            let head_toks: Vec<&Token> = member.tokens().into_iter().filter(|t| !matches!(t.kind, Kind::Nl | Kind::Newline)).collect();
            out.push(Decl {
                kind: "method".into(),
                name: t.text.clone(),
                line: ml,
                col: mc,
                exported: exported_m,
                modifiers,
                contracts,
                aspects: vec![],
                doc: doc_above(&lines, member_first),
                head: join_tokens(&head_toks),
                member_of: Some(name.clone()),
                start_line: member_first,
                end_line: mlast,
            });
        }
    }
    out
}

/// The span classes of the highlighting.
pub fn class_of(t: &Token) -> &'static str {
    match t.kind {
        Kind::Kw => "keyword",
        Kind::Upper => "type",
        Kind::Lower => "name",
        Kind::Str | Kind::Raw | Kind::Bytes | Kind::Template => "string",
        Kind::Int | Kind::Float | Kind::Dec | Kind::Dur => "number",
        _ => "punctuation",
    }
}

/// One source line with its classed spans: (column, length in scalars, class), comments included.
pub struct Line {
    pub number: usize,
    pub text: String,
    pub spans: Vec<(usize, usize, &'static str)>,
}

pub fn lines(text: &str) -> Vec<Line> {
    let src_lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<Line> = src_lines.iter().enumerate().map(|(i, l)| Line { number: i + 1, text: l.to_string(), spans: vec![] }).collect();
    if text.ends_with('\n') {
        out.pop();
    }
    let toks = lex(text).unwrap_or_default();
    let mut last_end: Vec<usize> = vec![1; out.len() + 2];
    for t in toks.iter().filter(|t| !matches!(t.kind, Kind::Nl | Kind::Newline | Kind::Eof)) {
        let class = class_of(t);
        let mut line = t.line;
        let mut col = t.col;
        for (k, seg) in t.text.split('\n').enumerate() {
            if k > 0 {
                line += 1;
                col = 1;
            }
            let len = seg.chars().count();
            if let Some(l) = out.get_mut(line - 1) {
                if len > 0 {
                    l.spans.push((col, len, class));
                }
            }
            if let Some(e) = last_end.get_mut(line) {
                *e = (*e).max(col + len);
            }
        }
    }
    for l in out.iter_mut() {
        let from = last_end.get(l.number).copied().unwrap_or(1);
        let rest: String = l.text.chars().skip(from - 1).collect();
        if let Some(i) = rest.find("//") {
            let col = from + rest[..i].chars().count();
            l.spans.push((col, rest[i..].chars().count(), "comment"));
        }
        l.spans.sort();
    }
    out
}

/// The terminal rendering of a line: ANSI colours from its spans.
pub fn highlight(l: &Line) -> String {
    let chars: Vec<char> = l.text.chars().collect();
    let mut s = String::new();
    let mut at = 1usize;
    for &(col, len, class) in &l.spans {
        if col < at {
            continue;
        }
        s.extend(chars.iter().skip(at - 1).take(col - at));
        let code = match class {
            "keyword" => "1;35",
            "type" => "36",
            "string" => "32",
            "number" => "33",
            "comment" => "2;37",
            _ => "",
        };
        let piece: String = chars.iter().skip(col - 1).take(len).collect();
        if code.is_empty() {
            s.push_str(&piece);
        } else {
            s.push_str(&format!("\u{1b}[{code}m{piece}\u{1b}[0m"));
        }
        at = col + len;
    }
    s.extend(chars.iter().skip(at.saturating_sub(1)));
    s
}

/// A JSON string literal.
pub fn js(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A simple glob: `*` within a path segment, `**` across segments, `?` one character.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn seg(p: &[char], s: &[char]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some('*'), _) => seg(&p[1..], s) || (!s.is_empty() && seg(p, &s[1..])),
            (Some('?'), Some(_)) => seg(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => seg(&p[1..], &s[1..]),
            _ => false,
        }
    }
    fn parts(p: &[&str], s: &[&str]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(&"**"), _) => parts(&p[1..], s) || (!s.is_empty() && parts(p, &s[1..])),
            (Some(a), Some(b)) => seg(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>()) && parts(&p[1..], &s[1..]),
            _ => false,
        }
    }
    let p: Vec<&str> = pattern.split('/').filter(|x| !x.is_empty() && *x != ".").collect();
    let s: Vec<&str> = path.split('/').filter(|x| !x.is_empty() && *x != ".").collect();
    parts(&p, &s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "@get(\"/x\")\nexport async try func load(editable self, xs: owned List[Int]) Int {\n  defer { discard 1 }\n  let v = g() or fail\n  return v\n}\n";

    #[test]
    fn queries_find_their_sites() {
        let s = parse(SRC).unwrap();
        assert_eq!(query("async-try", &s).unwrap(), vec![Match { line: 2, col: 23, text: "load".into() }]);
        assert_eq!(query("or-fail", &s).unwrap().len(), 1);
        assert_eq!(query("defer", &s).unwrap().len(), 1);
        assert_eq!(query("owned-params", &s).unwrap()[0].text, "xs");
        assert_eq!(query("editable-params", &s).unwrap()[0].text, "self");
        assert_eq!(query("aspect:get", &s).unwrap()[0].text, "@get");
        assert!(query("nonsense", &s).is_err());
    }

    #[test]
    fn declarations_carry_heads_and_aspects() {
        let s = parse(SRC).unwrap();
        let d = declarations(SRC, &s);
        assert_eq!(d[0].name, "load");
        assert!(d[0].exported);
        assert_eq!(d[0].modifiers, vec!["async", "try"]);
        assert_eq!(d[0].aspects, vec!["@get(\"/x\")"]);
    }

    #[test]
    fn lines_class_comments_and_globs_match() {
        let ls = lines("let x = 1 // one\n");
        assert!(ls[0].spans.iter().any(|s| s.2 == "comment"));
        assert!(glob_match("src/**/*.ft", "src/a/b/c.ft"));
        assert!(!glob_match("src/*.ft", "src/a/b.ft"));
    }
}
