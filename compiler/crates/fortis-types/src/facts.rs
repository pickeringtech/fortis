//! The inferred facts the formatter reprints (Programme 9.3, PROG-AGT-06, X-130).
//!
//! The checker enforces most of what 9.3 lists as always present in formatted source: a lambda's kind
//! (`E-FUN-010`), parameter-mode markers at call sites (`E-OWN`) and `from` regions are required in every accepted
//! program, so formatting cannot lose them. One fact may be left unwritten, and the formatter prints it back: the
//! single-word modifiers a named contract expands to, printed after the contract's name, which is kept (11.5: an
//! aspect may need the name).
//!
//! A lambda's inferred `pure` and `noalloc` are not printed back (8.2, X-345): the callee's parameter type states what
//! the lambda must be, and the checker enforces it there; `fortis view` shows the inferred summary. Written ones stay.
//!
//! `inferred` lists them as insertions at token positions, only for a module the checker accepts: inference over a
//! rejected module is not a fact.

use std::collections::BTreeSet;

use fortis_diag::Record;
use fortis_syntax::view::N;
use fortis_syntax::{Parsed, Token};


/// The modifier words in their canonical order (7.1; the grammar's `modifiers`).
pub const ORDER: &[&str] = &["trusted", "pure", "noalloc", "async", "try", "recovers"];

/// Text to insert before the token at (line, column), with the fact it prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Insertion {
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub fact: &'static str,
}

fn rank(w: &str) -> Option<usize> {
    ORDER.iter().position(|m| *m == w)
}

/// Insertions that complete a modifier list to `want`: each missing word goes before the first written word that
/// follows it in canonical order, or before `anchor` (`func` or `fn`).
fn complete(written: &[&Token], anchor: &Token, want: &BTreeSet<String>, fact: &'static str, out: &mut Vec<Insertion>) {
    let have: BTreeSet<&str> = written.iter().map(|t| t.text.as_str()).collect();
    for w in ORDER {
        if !want.contains(*w) || have.contains(w) {
            continue;
        }
        let r = rank(w).unwrap_or(0);
        let before = written.iter().find(|t| rank(&t.text).map_or(false, |x| x > r)).copied().unwrap_or(anchor);
        out.push(Insertion { line: before.line, column: before.col, text: format!("{w} "), fact });
    }
}

/// The facts to reprint, with the module's records; empty when the module has records.
pub fn inferred(parsed: &Parsed, file: &str) -> (Vec<Record>, Vec<Insertion>) {
    let (records, mut ins) = crate::check::check_parsed_with(parsed, file, |ty| {
        let mut out = Vec::new();
        let root = N::new(&parsed.root, &parsed.toks);
        for n in root.walk() {
            match n.name() {
                "funcHead" | "methodHead" => {
                    let written: Vec<&Token> = n.child_node("modifiers").map(|m| m.tokens()).unwrap_or_default();
                    let Some(func) = n.direct_tokens().into_iter().find(|t| t.text == "func") else { continue };
                    let mut want = BTreeSet::new();
                    for t in written.iter().filter(|t| t.text.starts_with(|c: char| c.is_uppercase())) {
                        if let Some(d) = ty.dc.table.get(&t.text) {
                            if d.kind == "contract" {
                                if let Some(m) = d.items.get("modifiers") {
                                    want.extend(m.iter().cloned());
                                }
                            }
                        }
                    }
                    let words: Vec<&Token> = written.iter().filter(|t| rank(&t.text).is_some()).copied().collect();
                    complete(&words, func, &want, "contract", &mut out);
                }
                _ => {}
            }
        }
        out
    });
    if !records.is_empty() {
        ins.clear();
    }
    (records, ins)
}

/// The source with the insertions applied (columns count characters, as the lexer's do).
pub fn apply(src: &str, ins: &[Insertion]) -> String {
    let mut lines: Vec<Vec<char>> = src.split('\n').map(|l| l.chars().collect()).collect();
    let mut sorted = ins.to_vec();
    // right to left within a line, so earlier columns stay valid; equal positions keep canonical order
    sorted.sort_by(|a, b| (b.line, b.column).cmp(&(a.line, a.column)).then(rank(a.text.trim()).cmp(&rank(b.text.trim())).reverse()));
    for i in sorted {
        if let Some(l) = lines.get_mut(i.line.wrapping_sub(1)) {
            let at = (i.column.saturating_sub(1)).min(l.len());
            l.splice(at..at, i.text.chars());
        }
    }
    lines.into_iter().map(|l| l.into_iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reprint(src: &str) -> String {
        let p = fortis_syntax::parse_module(src).unwrap();
        let (records, ins) = inferred(&p, "t.ft");
        assert!(records.is_empty(), "{records:?}");
        apply(src, &ins)
    }

    #[test]
    fn a_lambda_gets_no_claims_printed_back() {
        let src = "func f(xs: List[Int]) Int {\n  let ys = xs.map(fn(x: Int) Int => Int.wrappingAdd(x, 1))\n  return ys.count()\n}\n";
        assert_eq!(reprint(src), src);
    }

    #[test]
    fn a_written_lambda_claim_stays() {
        let src = "func f(xs: List[Int]) Int {\n  let ys = xs.map(pure fn(x: Int) Int => Int.wrappingAdd(x, 1))\n  return ys.count()\n}\n";
        assert_eq!(reprint(src), src);
    }

    #[test]
    fn a_contract_is_followed_by_its_modifiers() {
        let src = "export contract Fast {\n  pure\n  noalloc\n}\n\nexport Fast func sq(v: Int<0..100>) Int => v * v\n";
        assert!(reprint(src).contains("export Fast pure noalloc func sq"));
    }

    #[test]
    fn a_rejected_module_gets_no_facts() {
        let src = "export contract Fast {\n  pure\n}\n\nexport Fast func sq(v: Int) Int => Int.wrappingAdd(v, true)\n";
        let p = fortis_syntax::parse_module(src).unwrap();
        let (records, ins) = inferred(&p, "t.ft");
        assert!(!records.is_empty() && ins.is_empty());
    }

    #[test]
    fn insertions_at_one_position_keep_canonical_order() {
        let ins = vec![
            Insertion { line: 1, column: 1, text: "noalloc ".into(), fact: "lambda" },
            Insertion { line: 1, column: 1, text: "pure ".into(), fact: "lambda" },
        ];
        assert_eq!(apply("fn", &ins), "pure noalloc fn");
    }
}
