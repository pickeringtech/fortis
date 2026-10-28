//! The source quotas (PROG-SEC-01): compiler inputs are untrusted, so every parser has a budget, and an input past
//! it is an `E-LIM-002` record, never a crash or an unbounded run. The parser and the checker recurse on nesting,
//! so nesting is bounded before either runs.

use crate::lexer::{Kind, Token};

/// The largest source file, in bytes.
pub const MAX_BYTES: usize = 16 << 20;
/// The most tokens in one file.
pub const MAX_TOKENS: usize = 2_000_000;
/// The deepest nesting of brackets and blocks, and the longest run of prefix operators.
pub const MAX_NESTING: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct Exceeded {
    pub budget: String,
    pub operation: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for Exceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} exceeds the budget of {}", self.operation, self.budget)
    }
}

/// The size quota, before lexing.
pub fn source(src: &str) -> Result<(), Exceeded> {
    if src.len() > MAX_BYTES {
        return Err(Exceeded { budget: format!("{MAX_BYTES} bytes per source file"), operation: format!("a source file of {} bytes", src.len()), line: 1, col: 1 });
    }
    Ok(())
}

/// The token and nesting quotas, after lexing and before parsing.
pub fn tokens(toks: &[Token]) -> Result<(), Exceeded> {
    if toks.len() > MAX_TOKENS {
        return Err(Exceeded { budget: format!("{MAX_TOKENS} tokens per source file"), operation: format!("a source file of {} tokens", toks.len()), line: 1, col: 1 });
    }
    let (mut depth, mut run) = (0usize, 0usize);
    let mut prev: Option<&Token> = None;
    for t in toks {
        if t.kind == Kind::Punct {
            match t.text.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        // a prefix operator follows an operator, an opening bracket or nothing
        let prefix = matches!(t.text.as_str(), "!" | "-" | "~") && prev.map_or(true, |p| p.kind == Kind::Punct && !matches!(p.text.as_str(), ")" | "]" | "}"));
        run = if prefix { run + 1 } else { 0 };
        if depth > MAX_NESTING || run > MAX_NESTING {
            let what = if run > MAX_NESTING { "a run of prefix operators" } else { "nesting" };
            return Err(Exceeded { budget: format!("nesting depth {MAX_NESTING}"), operation: format!("{what} {} deep", depth.max(run)), line: t.line, col: t.col });
        }
        prev = Some(t);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_nesting_is_refused_before_parsing() {
        let src = format!("func f() Int {{\n  return {}1{}\n}}\n", "(".repeat(300), ")".repeat(300));
        let toks = crate::lex(&src).unwrap();
        let e = tokens(&toks).unwrap_err();
        assert_eq!((e.line, e.budget.as_str()), (2, "nesting depth 256"));
        assert!(tokens(&crate::lex("func f() Int {\n  return ((1))\n}\n").unwrap()).is_ok());
    }

    #[test]
    fn a_long_run_of_prefix_operators_is_refused() {
        let src = format!("func f() Bool {{\n  return {}true\n}}\n", "!".repeat(300));
        assert!(tokens(&crate::lex(&src).unwrap()).is_err());
    }
}
