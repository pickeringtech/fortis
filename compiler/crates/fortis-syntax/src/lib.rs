//! fortis-syntax: the lexer and the parser generated from reference/grammar/fortis.ebnf by the DEL-02 generator's
//! Rust backend (`conformance/tools/gen_parser.py --rust`), with the same decisions as the Python parser it emits.

pub mod generated;
pub mod grammar;
pub mod lexer;
pub mod limits;
pub mod runtime;
pub mod tree;
pub mod check;
pub mod view;

pub use lexer::{lex, Kind, LexError, Token};
pub use runtime::{ParseError, Parser};
pub use tree::{Child, Node, Parsed};

#[derive(Debug)]
pub enum SyntaxError {
    Lex(LexError),
    Parse(ParseError),
    /// a source quota is exceeded (PROG-SEC-01)
    Limit(limits::Exceeded),
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyntaxError::Lex(e) => write!(f, "{e}"),
            SyntaxError::Parse(e) => write!(f, "does not parse at {}:{}", e.line, e.col),
            SyntaxError::Limit(e) => write!(f, "E-LIM-002: {e}"),
        }
    }
}

/// Lex and parse a whole module.
pub fn parse_module(src: &str) -> Result<Parsed, SyntaxError> {
    parse_from(src, "module")
}

pub fn parse_from(src: &str, start: &str) -> Result<Parsed, SyntaxError> {
    limits::source(src).map_err(SyntaxError::Limit)?;
    let toks = lex(src).map_err(SyntaxError::Lex)?;
    limits::tokens(&toks).map_err(SyntaxError::Limit)?;
    let root = Parser::new(&toks).parse(start).map_err(SyntaxError::Parse)?.0;
    Ok(Parsed { toks, root })
}

/// Lex and parse a module; a syntax error is located where the reference recogniser locates it (its furthest
/// position), with the reference's message shape.
pub fn parse_module_located(src: &str) -> Result<Parsed, (String, usize, usize)> {
    limits::source(src).map_err(|e| (format!("E-LIM-002: {e}"), e.line, e.col))?;
    let toks = lex(src).map_err(|e| (e.to_string(), e.line, e.col))?;
    limits::tokens(&toks).map_err(|e| (format!("E-LIM-002: {e}"), e.line, e.col))?;
    match Parser::new(&toks).parse("module") {
        Ok((root, _, _)) => Ok(Parsed { toks, root }),
        Err(pe) => {
            let (idx, expected): (usize, Vec<String>) = match grammar::Recogniser::new(&toks).recognise("module") {
                Err(f) => (f.token, f.expected.into_iter().collect()),
                Ok(()) => (pe.token, pe.expected.clone()),
            };
            let t = &toks[idx];
            let exp: Vec<String> = expected.into_iter().take(12).collect();
            Err((format!("unexpected {} {} at {}:{}; expected one of: {}", t.kind.as_str(), runtime::py_repr(&t.text), t.line, t.col, exp.join(", ")), t.line, t.col))
        }
    }
}
