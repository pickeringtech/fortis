//! The formatter's rewrites into the v0.0.2 reading forms (register X-345), each applied only where it means the same:
//!
//! - **R9**, a pun: `wanted: wanted` is written `wanted` in a class constructor, after a named argument, or where the
//!   parameter in that position is `wanted` and every argument before it is positional (LR 5.8);
//! - **R12**, an or-pattern: adjacent single-line arms with no guard, no bindings and the same body are merged into one
//!   arm `A | B => body` (LR 9.8);
//! - **R14**, a checked operator: `Int.add(a, b) or fail` (and `sub`, `mul`, `div`, `rem`, under any outcome operator)
//!   is written `a + b or fail` where the operator would be unproved, so it is the same checked operation (LR 6.4);
//! - **R15**, a bare variant: `Severity.Error` is written `Error` where the expected type is `Severity` and no
//!   declaration is named `Error` (LR 9.7).
//!
//! The checker finds R9, R14 and R15 while typing (`Typer::migrations`); R12 is found here, from the tree and the
//! source. Only a module the checker accepts is rewritten, and the rewritten module must be accepted again
//! (`fortis_fmt` checks it); an edit overlapping one already taken waits for the next round.

use fortis_diag::Record;
use fortis_syntax::view::N;
use fortis_syntax::Parsed;

/// The source from `from` up to (not including) `to`, both (line, column) with columns counting characters as the
/// lexer's do, replaced by `text`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edit {
    pub from: (usize, usize),
    pub to: (usize, usize),
    pub text: String,
    pub rule: &'static str,
}

/// The rewrites of an accepted module, with its records; none when the module has records.
pub fn rewrites(parsed: &Parsed, file: &str, src: &str) -> (Vec<Record>, Vec<Edit>) {
    rewrites_in(parsed, src, crate::check::Unit::Alone(file))
}

/// `rewrites` for the module checked as `unit` says: on its own, or in its package (a package module whose imports
/// do not resolve alone).
pub fn rewrites_in(parsed: &Parsed, src: &str, unit: crate::check::Unit) -> (Vec<Record>, Vec<Edit>) {
    let lines: Vec<&str> = src.split('\n').collect();
    let (records, edits) = crate::check::check_unit_with(parsed, src, unit, |ty| {
        let mut out = ty.migrations.clone();
        let root = N::new(&parsed.root, &parsed.toks);
        for n in root.walk() {
            if n.name() == "matchExpr" {
                merge_arms(n, &lines, &mut out);
            }
        }
        out
    });
    let mut edits = edits.unwrap_or_default();
    if !records.is_empty() {
        edits.clear();
    }
    edits.sort();
    edits.dedup();
    (records, edits)
}

/// Runs of adjacent arms that may be one or-pattern arm (R12): each alone on its line, consecutive lines, no guard,
/// no binding, no catch-all, and the same body tokens.
fn merge_arms<'a>(m: N<'a>, lines: &[&str], out: &mut Vec<Edit>) {
    struct ArmInfo<'a> {
        line: usize,
        pats: Vec<N<'a>>,
        body: Vec<String>,
        ok: bool,
    }
    let mut infos: Vec<ArmInfo> = Vec::new();
    for arm in m.child_nodes("matchArm") {
        let toks = arm.tokens();
        let (first, last) = (toks[0], *toks.last().unwrap());
        let pats = arm.child_nodes("pattern");
        let guarded = arm.direct_tokens().iter().any(|t| t.text == "when");
        let body = arm.kids().into_iter().filter_map(|k| k.node()).filter(|k| k.name() == "expr" || k.name() == "block").last();
        let single = first.line == last.line;
        // alone on its line: nothing before the first token, nothing but spaces after the last
        let alone = single
            && lines.get(first.line - 1).map_or(false, |l| {
                let cs: Vec<char> = l.chars().collect();
                let before: String = cs[..(first.col - 1).min(cs.len())].iter().collect();
                let after: String = cs[(crate::decls::tok_end(last) - 1).min(cs.len())..].iter().collect();
                before.trim().is_empty() && after.trim().is_empty()
            });
        let binds = pats.iter().any(|p| binds(*p));
        let catch_all = pats.iter().any(|p| {
            let ts = p.direct_tokens();
            ts.first().map_or(false, |t| t.text == "_" || t.text == "otherwise") || (ts.len() == 1 && ts[0].kind == fortis_syntax::lexer::Kind::Lower)
        });
        let ok = alone && !guarded && !binds && !catch_all && body.map_or(false, |b| b.name() == "expr");
        let body_toks = body.map(|b| b.tokens().iter().map(|t| t.text.clone()).collect()).unwrap_or_default();
        infos.push(ArmInfo { line: first.line, pats, body: body_toks, ok });
    }
    let mut i = 0;
    while i < infos.len() {
        let mut j = i + 1;
        while j < infos.len() && infos[i].ok && infos[j].ok && infos[j].line == infos[j - 1].line + 1 && infos[j].body == infos[i].body {
            j += 1;
        }
        if j > i + 1 {
            let last_pat = infos[i].pats.last().unwrap().tokens();
            let end = *last_pat.last().unwrap();
            let mut text = String::new();
            for k in i + 1..j {
                for p in &infos[k].pats {
                    text.push_str(" | ");
                    text.push_str(&source_of(*p, lines));
                }
            }
            out.push(Edit { from: (end.line, crate::decls::tok_end(end)), to: (end.line, crate::decls::tok_end(end)), text, rule: "R12" });
            for k in i + 1..j {
                out.push(Edit { from: (infos[k].line, 1), to: (infos[k].line + 1, 1), text: String::new(), rule: "R12" });
            }
        }
        i = j;
    }
}

/// Whether a pattern binds a name: a bare name anywhere in it, an `as` name, or `otherwise(name)` (9.8).
fn binds(p: N) -> bool {
    p.walk().into_iter().filter(|n| n.name() == "pattern").any(|q| {
        let ts = q.direct_tokens();
        (ts.len() == 1 && ts[0].kind == fortis_syntax::lexer::Kind::Lower && ts[0].text != "_") || ts.iter().any(|t| t.text == "as") || (ts.first().map_or(false, |t| t.text == "otherwise") && ts.len() > 1)
    })
}

/// The written text of a single-line node.
fn source_of(n: N, lines: &[&str]) -> String {
    let toks = n.tokens();
    let (first, last) = (toks[0], *toks.last().unwrap());
    let cs: Vec<char> = lines.get(first.line - 1).map(|l| l.chars().collect()).unwrap_or_default();
    cs[(first.col - 1).min(cs.len())..(crate::decls::tok_end(last) - 1).min(cs.len())].iter().collect()
}

/// The source with the edits applied; an edit that overlaps one before it is left for the next round.
pub fn apply(src: &str, edits: &[Edit]) -> (String, usize) {
    // character offset of each line start
    let mut starts = vec![0usize];
    let chars: Vec<char> = src.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            starts.push(i + 1);
        }
    }
    let off = |(l, c): (usize, usize)| -> usize {
        match starts.get(l.wrapping_sub(1)) {
            Some(s) => (s + c.saturating_sub(1)).min(chars.len()),
            None => chars.len(),
        }
    };
    let mut taken: Vec<(usize, usize, &Edit)> = Vec::new();
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| (off(e.from), std::cmp::Reverse(off(e.to))));
    for e in sorted {
        let (a, b) = (off(e.from), off(e.to));
        let clash = |x: usize, y: usize| if a == b { x < a && a < y } else if x == y { a < x && x < b } else { a < y && x < b };
        if taken.iter().any(|(x, y, _)| clash(*x, *y)) {
            continue;
        }
        taken.push((a, b, e));
    }
    taken.sort_by_key(|(a, b, _)| (std::cmp::Reverse(*a), std::cmp::Reverse(*b)));
    let mut out = chars;
    let n = taken.len();
    for (a, b, e) in taken {
        out.splice(a..b, e.text.chars());
    }
    (out.into_iter().collect(), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(src: &str) -> String {
        let p = fortis_syntax::parse_module(src).unwrap();
        let (records, edits) = rewrites(&p, "t.ft", src);
        assert!(records.is_empty(), "{records:?}");
        apply(src, &edits).0
    }

    #[test]
    fn a_constructor_puns() {
        let src = "class Item {\n  sku: Int\n  held: Int\n}\n\nfunc f(sku: Int, held: Int) Item => Item(sku: sku, held: held)\n";
        assert!(rewrite(src).contains("Item(sku, held)"), "{}", rewrite(src));
    }

    #[test]
    fn a_call_puns_only_where_the_meaning_stays() {
        let src = "func g(a: Int, b: Int) Int => a\n\nfunc f(a: Int, b: Int) Int => g(b: b, a: a)\n";
        assert!(rewrite(src).contains("g(b: b, a)"), "{}", rewrite(src));
    }

    #[test]
    fn a_qualified_variant_drops_its_prefix() {
        let src = "enum Level {\n  Low\n  High\n}\n\nfunc f() Level => Level.High\n";
        assert!(rewrite(src).contains("func f() Level => High"), "{}", rewrite(src));
    }

    #[test]
    fn a_checked_call_becomes_the_operator() {
        let src = "try func f(a: Int, b: Int) Int {\n  return Int.add(Int.wrappingMul(a, 2), b) or fail\n}\n";
        assert!(rewrite(src).contains("return Int.wrappingMul(a, 2) + b or fail"), "{}", rewrite(src));
    }

    #[test]
    fn a_proved_call_stays() {
        let src = "try func f(a: Int<0..10>, b: Int<0..10>) Int {\n  return Int.add(a, b) or fail\n}\n";
        assert!(rewrite(src).contains("Int.add(a, b) or fail"), "{}", rewrite(src));
    }

    #[test]
    fn equal_arms_merge() {
        let src = "enum L {\n  A\n  B\n  C\n}\n\nfunc f(l: L) Int {\n  return match l {\n    A => 1\n    B => 1\n    C => 2\n  }\n}\n";
        let out = rewrite(src);
        assert!(out.contains("    A | B => 1\n    C => 2\n"), "{out}");
    }
}
