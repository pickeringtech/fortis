//! The state of one function being built: its locals, its lexical scopes, and — for a lambda — the captures it
//! takes from the enclosing function.

use fortis_types::types::{NamedT, Type};

use crate::model::*;

pub struct Fb {
    pub name: String,
    pub label: String,
    pub module: String,
    pub ret: Type,
    pub is_try: bool,
    pub params: Vec<Param>,
    pub locals: Vec<LocalDecl>,
    /// lexical scopes, innermost last: (name, local) in binding order
    pub scopes: Vec<Vec<(String, LocalId)>>,
    /// the locals each open block declares, innermost last
    pub block_locals: Vec<Vec<LocalId>>,
    pub self_local: Option<LocalId>,
    /// a lambda: its kind and whether it has an explicit capture list (else it captures by reference on use)
    pub lambda: Option<LambdaCtx>,
    pub captures: Vec<CaptureDef>,
    /// the value each capture takes from the enclosing function, in capture order
    pub capture_values: Vec<Expr>,
    /// the statement temporaries of each open statement, innermost last, in creation order (5.7): the
    /// statement takes them when it is built, and they are destroyed at its end
    pub stmt_temps: Vec<Vec<LocalId>>,
    /// the `where` bounds in force in the function (its own and, for a method, its class's): (type parameter,
    /// the bound interface as the checker names it, with its type arguments)
    pub bounds: Vec<(String, NamedT)>,
}

pub struct LambdaCtx {
    pub kind: String,
    pub explicit: bool,
}

impl Fb {
    pub fn new(label: &str, module: &str, ret: Type, is_try: bool) -> Fb {
        Fb {
            name: format!("{module}::{label}"),
            label: label.to_string(),
            module: module.to_string(),
            ret,
            is_try,
            params: vec![],
            locals: vec![],
            scopes: vec![vec![]],
            block_locals: vec![vec![]],
            self_local: None,
            lambda: None,
            captures: vec![],
            capture_values: vec![],
            stmt_temps: vec![],
            bounds: vec![],
        }
    }

    pub fn add_local(&mut self, name: &str, ty: Type, mutable: bool, kind: LocalKind) -> LocalId {
        let id = self.locals.len() as LocalId;
        self.locals.push(LocalDecl { name: name.to_string(), ty, mutable, kind, boxed: false });
        id
    }

    /// Declare a local of the innermost open block and bind its name.
    pub fn declare(&mut self, name: &str, ty: Type, mutable: bool, kind: LocalKind) -> LocalId {
        let id = self.add_local(name, ty, mutable, kind);
        self.block_locals.last_mut().unwrap().push(id);
        if name != "_" {
            self.bind(name, id);
        }
        id
    }

    /// A temporary of the innermost open block.
    pub fn temp(&mut self, ty: Type) -> LocalId {
        let id = self.add_local(&format!("tmp{}", self.locals.len()), ty, true, LocalKind::Temp);
        self.block_locals.last_mut().unwrap().push(id);
        id
    }

    /// A statement temporary (5.7): destroyed at the end of the full statement, so it belongs to no block;
    /// it is recorded on the innermost open statement, or nowhere when no statement is open (a constant's
    /// value, a parameter's default: the interpreter tracks neither).
    pub fn stmt_temp(&mut self, ty: Type) -> LocalId {
        let id = self.add_local(&format!("tmp{}", self.locals.len()), ty, true, LocalKind::Temp);
        if let Some(v) = self.stmt_temps.last_mut() {
            v.push(id);
        }
        id
    }

    pub fn bind(&mut self, name: &str, id: LocalId) {
        self.scopes.last_mut().unwrap().push((name.to_string(), id));
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(vec![]);
        self.block_locals.push(vec![]);
    }

    /// Close a scope: the locals its block declared.
    pub fn pop_scope(&mut self) -> Vec<LocalId> {
        self.scopes.pop();
        self.block_locals.pop().unwrap_or_default()
    }

    pub fn lookup(&self, name: &str) -> Option<LocalId> {
        if name == "self" {
            return self.self_local.or_else(|| self.find(name));
        }
        self.find(name)
    }

    fn find(&self, name: &str) -> Option<LocalId> {
        for s in self.scopes.iter().rev() {
            if let Some((_, id)) = s.iter().rev().find(|(n, _)| n == name) {
                return Some(*id);
            }
        }
        None
    }

    pub fn finish(self, line: u32, body: Option<Block>, modifiers: Vec<String>, effects: Effects, observe: Option<String>, closure: Option<ClosureDef>) -> Function {
        Function { name: self.name, label: self.label, module: self.module, body_module: None, line, params: self.params, ret: self.ret, modifiers, locals: self.locals, body, effects, observe, closure, bounds: vec![] }
    }
}
