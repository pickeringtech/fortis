//! Tree helpers over a module's tree and tokens: the functions of conformance/tools/tree.py on a handle that pairs
//! a node with its module's token vector (a library module and a user module each have their own).

use crate::lexer::{Kind, Token};
use crate::tree::{Child, Node};

#[derive(Clone, Copy)]
pub struct N<'a> {
    pub node: &'a Node,
    pub toks: &'a [Token],
}

impl<'a> std::fmt::Debug for N<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{}>", self.name())
    }
}

#[derive(Clone, Copy)]
pub enum C<'a> {
    T(&'a Token),
    N(N<'a>),
}

impl<'a> C<'a> {
    pub fn name(self) -> Option<&'static str> {
        match self {
            C::N(n) => Some(n.name()),
            C::T(_) => None,
        }
    }
    pub fn tok(self) -> Option<&'a Token> {
        match self {
            C::T(t) => Some(t),
            C::N(_) => None,
        }
    }
    pub fn node(self) -> Option<N<'a>> {
        match self {
            C::N(n) => Some(n),
            C::T(_) => None,
        }
    }
    pub fn is(self, rule: &str) -> bool {
        self.name() == Some(rule)
    }
    pub fn text_is(self, text: &str) -> bool {
        matches!(self, C::T(t) if t.text == text)
    }
    pub fn tokens(self) -> Vec<&'a Token> {
        match self {
            C::T(t) => vec![t],
            C::N(n) => n.tokens(),
        }
    }
}

impl<'a> N<'a> {
    pub fn new(node: &'a Node, toks: &'a [Token]) -> N<'a> {
        N { node, toks }
    }
    pub fn name(self) -> &'static str {
        self.node.name()
    }
    pub fn id(self) -> usize {
        self.node as *const Node as usize
    }
    pub fn same(self, other: N<'a>) -> bool {
        std::ptr::eq(self.node, other.node)
    }
    pub fn kids(self) -> Vec<C<'a>> {
        let mut kids: Vec<_> = self.node
            .children
            .iter()
            .map(|c| match c {
                Child::Tok(i) => C::T(&self.toks[*i]),
                Child::Node(n) => C::N(N { node: n, toks: self.toks }),
            })
            .collect();
        if let Some(head) = self.node.inferred_head.get() {
            let head = N::new(&head.root, &head.toks);
            let at = kids.iter().position(|c| c.text_is("fn")).map(|i| i + 1).unwrap_or(0);
            let extra = head.kids().into_iter().filter(|c| c.is("captureList") || c.text_is("mut") || c.text_is("once"));
            kids.splice(at..at, extra);
        }
        kids
    }
    pub fn nkids(self) -> usize {
        self.node.children.len()
    }
    pub fn kid(self, i: usize) -> Option<C<'a>> {
        self.node.children.get(i).map(|c| match c {
            Child::Tok(i) => C::T(&self.toks[*i]),
            Child::Node(n) => C::N(N { node: n, toks: self.toks }),
        })
    }
    /// Direct child nodes with the given rule name.
    pub fn child_nodes(self, rule: &str) -> Vec<N<'a>> {
        self.kids().into_iter().filter_map(|c| c.node()).filter(|n| n.name() == rule).collect()
    }
    pub fn child_node(self, rule: &str) -> Option<N<'a>> {
        self.kids().into_iter().filter_map(|c| c.node()).find(|n| n.name() == rule)
    }
    /// Pre-order over nodes (not tokens), self first.
    pub fn walk(self) -> Vec<N<'a>> {
        let mut out = Vec::new();
        fn go<'a>(n: N<'a>, out: &mut Vec<N<'a>>) {
            out.push(n);
            for c in &n.node.children {
                if let Child::Node(m) = c {
                    go(N { node: m, toks: n.toks }, out);
                }
            }
        }
        go(self, &mut out);
        out
    }
    pub fn find(self, rule: &str) -> Option<N<'a>> {
        fn go<'a>(n: N<'a>, rule: &str) -> Option<N<'a>> {
            if n.name() == rule {
                return Some(n);
            }
            for c in n.kids() {
                if let C::N(m) = c {
                    if let Some(r) = go(m, rule) {
                        return Some(r);
                    }
                }
            }
            None
        }
        go(self, rule)
    }
    pub fn find_all(self, rule: &str) -> Vec<N<'a>> {
        self.walk().into_iter().filter(|n| n.name() == rule).collect()
    }
    pub fn tokens(self) -> Vec<&'a Token> {
        let mut out = Vec::new();
        fn go<'a>(n: N<'a>, out: &mut Vec<&'a Token>) {
            for c in &n.node.children {
                match c {
                    Child::Tok(i) => out.push(&n.toks[*i]),
                    Child::Node(m) => go(N { node: m, toks: n.toks }, out),
                }
            }
        }
        go(self, &mut out);
        out
    }
    pub fn direct_tokens(self) -> Vec<&'a Token> {
        self.kids().into_iter().filter_map(|c| c.tok()).collect()
    }
    pub fn first_token(self) -> Option<&'a Token> {
        fn go<'a>(n: N<'a>) -> Option<&'a Token> {
            for c in &n.node.children {
                match c {
                    Child::Tok(i) => return Some(&n.toks[*i]),
                    Child::Node(m) => {
                        if let Some(t) = go(N { node: m, toks: n.toks }) {
                            return Some(t);
                        }
                    }
                }
            }
            None
        }
        go(self)
    }
    pub fn ntokens(self) -> usize {
        self.tokens().len()
    }
    pub fn words(self) -> Vec<&'a str> {
        self.direct_tokens().into_iter().map(|t| t.text.as_str()).collect()
    }
}

pub fn same_tok(a: &Token, b: &Token) -> bool {
    std::ptr::eq(a, b)
}

pub const DECL_KINDS: &[(&str, &str)] = &[
    ("constDecl", "const"),
    ("typeDecl", "type"),
    ("classDecl", "class"),
    ("enumDecl", "enum"),
    ("errorDecl", "error"),
    ("interfaceDecl", "interface"),
    ("implementDecl", "implement"),
    ("mixinDecl", "mixin"),
    ("contractDecl", "contract"),
    ("aspectDecl", "aspect"),
    ("observationDecl", "observation"),
    ("funcDecl", "func"),
    ("testDecl", "test"),
    ("propertyDecl", "property"),
    ("headBlock", "head"),
];

#[derive(Clone)]
pub struct DeclInfo<'a> {
    pub kind: &'static str,
    pub name: Option<String>,
    pub exported: bool,
    pub node: N<'a>,
    pub token: Option<&'a Token>,
    pub aspects: Vec<N<'a>>,
}

/// Every top-level declaration of a module tree (tree.declarations).
pub fn declarations<'a>(module: N<'a>) -> Vec<DeclInfo<'a>> {
    let mut out = Vec::new();
    for decl in module.kids() {
        let decl = match decl.node() {
            Some(d) if d.name() == "declaration" => d,
            _ => continue,
        };
        let exported = decl.kids().iter().any(|c| c.text_is("export"));
        let inner = decl.kids().into_iter().filter_map(|c| c.node()).find(|n| DECL_KINDS.iter().any(|(r, _)| *r == n.name()));
        let inner = match inner {
            Some(i) => i,
            None => continue,
        };
        let kind = DECL_KINDS.iter().find(|(r, _)| *r == inner.name()).unwrap().1;
        let nm: Option<&Token> = match kind {
            "func" => {
                let head = inner.find("funcHead").unwrap();
                let ts = head.direct_tokens();
                let idx = ts.iter().position(|t| t.text == "func").unwrap();
                Some(ts[idx + 1])
            }
            "implement" => None,
            "test" | "property" => inner.direct_tokens().get(1).copied(),
            _ => inner.direct_tokens().into_iter().find(|t| matches!(t.kind, Kind::Upper | Kind::Lower)),
        };
        let aspects = decl.child_nodes("aspectUse");
        out.push(DeclInfo { kind, name: nm.map(|t| t.text.clone()), exported, node: inner, token: nm, aspects });
    }
    out
}

#[derive(Clone)]
pub struct ImportInfo<'a> {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub members: Option<Vec<String>>,
    pub token: &'a Token,
}

/// Every import of a module tree (tree.imports).
pub fn imports<'a>(module: N<'a>) -> Vec<ImportInfo<'a>> {
    let mut out = Vec::new();
    for imp in module.kids() {
        let imp = match imp.node() {
            Some(i) if i.name() == "import" => i,
            _ => continue,
        };
        let dotted = imp.find("dottedPath").unwrap();
        let segs: Vec<String> = dotted.tokens().iter().filter(|t| t.text != ".").map(|t| t.text.clone()).collect();
        let ts = imp.direct_tokens();
        let alias = ts.iter().position(|t| t.text == "as").map(|i| ts[i + 1].text.clone());
        let members = imp.find("identList").map(|il| il.tokens().iter().filter(|t| t.text != ",").map(|t| t.text.clone()).collect());
        out.push(ImportInfo { path: segs, alias, members, token: dotted.tokens()[0] });
    }
    out
}
