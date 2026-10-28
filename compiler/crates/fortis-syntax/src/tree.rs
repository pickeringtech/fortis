//! The concrete syntax tree: rule nodes whose children are nodes or indices into the token vector. The shape is
//! the recogniser's derivation (conformance/tools/parser.py), which the generated parser reproduces exactly.

use crate::generated::RULE_NAMES;
use crate::lexer::Token;

#[derive(Clone, Debug)]
pub enum Child {
    Tok(usize),
    Node(Node),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub rule: u16,
    pub children: Vec<Child>,
}

impl Node {
    pub fn name(&self) -> &'static str {
        RULE_NAMES.get(self.rule as usize).copied().unwrap_or("headBody")
    }
}

/// A parsed module: the tokens and the tree over them.
pub struct Parsed {
    pub toks: Vec<Token>,
    pub root: Node,
}

/// The tree as an S-expression with token positions, for comparing parsers.
pub fn dump(node: &Node, toks: &[Token], out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(node.name());
    out.push('\n');
    for c in &node.children {
        match c {
            Child::Node(n) => dump(n, toks, out, depth + 1),
            Child::Tok(i) => {
                let t = &toks[*i];
                for _ in 0..depth + 1 {
                    out.push_str("  ");
                }
                out.push_str(&format!("{} {} {}:{}\n", t.kind.as_str(), crate::runtime::py_repr(&t.text), t.line, t.col));
            }
        }
    }
}
