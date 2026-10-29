//! What the reader can already see, for the formatter to drop (v0.0.2, X-345).
//!
//! Three old forms stay accepted and the formatter rewrites them into the new ones:
//!
//! - a `Void` return type on a function or method head, or after `cleanup(editable self)` (Language Reference 8.1):
//!   purely syntactic, so it is dropped from any module that parses;
//! - an `editable` before a receiver of an `editable self` method, or before a `fn mut` callee, whose place is rooted
//!   in a `let mut` binding, an `editable` parameter, an `editable` capture or the method's own `editable self` (7.5);
//! - the `move` of `return move x` when `x` is a local `let` binding (7.5).
//!
//! The last two come from the checker (`Typer::redundant_markers`), only for a module it accepts: the checker
//! checks the unmarked form exactly as the marked one, so dropping the marker changes nothing it decides.

use fortis_diag::Record;
use fortis_syntax::view::N;
use fortis_syntax::Parsed;

pub use crate::check::Unit;

/// A token to delete at (line, column), with the old form it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deletion {
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub form: &'static str,
}

/// Every `Void` written as the return type of a function or method head, or of `cleanup`.
pub fn void_returns(parsed: &Parsed) -> Vec<Deletion> {
    let mut out = Vec::new();
    let root = N::new(&parsed.root, &parsed.toks);
    for n in root.walk() {
        let void = match n.name() {
            "funcSignature" => n.child_node("returnType").and_then(|r| {
                let ts = r.tokens();
                (ts.len() == 1 && ts[0].text == "Void").then(|| ts[0])
            }),
            "cleanupDecl" => n.direct_tokens().into_iter().find(|t| t.text == "Void"),
            _ => None,
        };
        if let Some(t) = void {
            out.push(Deletion { line: t.line, column: t.col, text: t.text.clone(), form: "Void return type" });
        }
    }
    out
}

/// The markers the checker found the declaration already shows, with the module's records; none when the module
/// has records.
pub fn redundant_markers(parsed: &Parsed, file: &str) -> (Vec<Record>, Vec<Deletion>) {
    redundant_markers_in(parsed, "", Unit::Alone(file))
}

/// `redundant_markers` for the module whose text is `src`, checked as `unit` says: on its own, or in its package
/// (a package module whose imports do not resolve alone).
pub fn redundant_markers_in(parsed: &Parsed, src: &str, unit: Unit) -> (Vec<Record>, Vec<Deletion>) {
    let (records, out) = crate::check::check_unit_with(parsed, src, unit, |ty| ty.redundant_markers.iter().copied().collect::<Vec<_>>());
    let mut out = out.unwrap_or_default();
    let mut dels = Vec::new();
    if records.is_empty() {
        for (line, column) in out.drain(..) {
            if let Some(t) = parsed.toks.iter().find(|t| t.line == line && t.col == column) {
                let form = if t.text == "move" { "return move of a local" } else { "editable receiver" };
                dels.push(Deletion { line, column, text: t.text.clone(), form });
            }
        }
    }
    (records, dels)
}

/// The (code, line) of each record: what a rewrite must leave as it was.
pub fn seen(records: &[Record]) -> std::collections::BTreeSet<(String, usize)> {
    records.iter().map(|r| (r.code.to_string(), r.line)).collect()
}

/// The source with the deletions applied: each token's text is removed, with one space after it, or before it when
/// nothing but the line's end follows (columns count characters, as the lexer's do).
pub fn apply(src: &str, dels: &[Deletion]) -> String {
    let mut lines: Vec<Vec<char>> = src.split('\n').map(|l| l.chars().collect()).collect();
    let mut sorted = dels.to_vec();
    sorted.sort_by(|a, b| (b.line, b.column).cmp(&(a.line, a.column))); // right to left: earlier columns stay valid
    sorted.dedup();
    for d in sorted {
        let Some(l) = lines.get_mut(d.line.wrapping_sub(1)) else { continue };
        let at = d.column.saturating_sub(1);
        let n = d.text.chars().count();
        if at + n > l.len() || l[at..at + n].iter().collect::<String>() != d.text {
            continue;
        }
        let mut end = at + n;
        let mut start = at;
        if end < l.len() && l[end] == ' ' {
            end += 1;
        } else if start > 0 && l[start - 1] == ' ' {
            start -= 1;
        }
        l.drain(start..end);
    }
    lines.into_iter().map(|l| l.into_iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(src: &str) -> String {
        let p = fortis_syntax::parse_module(src).unwrap();
        let mut dels = void_returns(&p);
        let (records, markers) = redundant_markers(&p, "t.ft");
        assert!(records.is_empty(), "{records:?}");
        dels.extend(markers);
        apply(src, &dels)
    }

    #[test]
    fn void_goes_from_heads_but_not_from_types() {
        let src = "func f(g: fn(Int) Void) Void {\n  g(1)\n}\n";
        assert_eq!(rewrite(src), "func f(g: fn(Int) Void) {\n  g(1)\n}\n");
    }

    #[test]
    fn a_receiver_marker_goes_where_the_binding_shows_it() {
        let src = "func f() Int {\n  let mut xs = List.of(1)\n  editable xs.push(2)\n  return xs.count()\n}\n";
        assert_eq!(rewrite(src), "func f() Int {\n  let mut xs = List.of(1)\n  xs.push(2)\n  return xs.count()\n}\n");
    }

    #[test]
    fn return_move_of_a_local_loses_its_marker() {
        let src = "func f() List[Int] {\n  let xs = List.of(1)\n  return move xs\n}\n";
        assert_eq!(rewrite(src), "func f() List[Int] {\n  let xs = List.of(1)\n  return xs\n}\n");
    }

    #[test]
    fn a_fn_mut_call_loses_its_marker_and_an_argument_keeps_its_own() {
        let src = "func g(xs: editable List[Int]) {\n  xs.push(1)\n}\nfunc f() Int {\n  let mut n = 0\n  {\n    let mut bump = fn mut[editable n]() Void {\n      n = 2\n    }\n    editable bump()\n  }\n  let mut ys = List.of(n)\n  g(editable ys)\n  return ys.count()\n}\n";
        let out = rewrite(src);
        assert!(out.contains("\n    bump()\n"), "{out}");
        assert!(out.contains("g(editable ys)") && out.contains("fn mut[editable n]() Void {"), "{out}");
    }

    #[test]
    fn return_move_of_a_parameter_keeps_it() {
        let src = "func f(xs: owned List[Int]) List[Int] {\n  return move xs\n}\n";
        assert_eq!(rewrite(src), src);
    }
}
