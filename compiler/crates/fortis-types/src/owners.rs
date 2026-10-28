//! Lexical loans, moves and markers (DEL-14): a port of checker/owners.py. Loans are shared cells, as the reference's
//! loan objects are shared between the ownership states a branch copies (a binding later adopts a carried loan).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use fortis_syntax::Token;

#[derive(Clone, Debug, PartialEq)]
pub struct LoanData {
    pub owner: String,
    pub mode: String,
    pub holder: Option<String>,
    pub depth: i64,
    pub line: usize,
}

pub type Loan = Rc<RefCell<LoanData>>;

pub fn loan_eq(a: &Loan, b: &Loan) -> bool {
    *a.borrow() == *b.borrow()
}

pub fn overlaps(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

pub fn root_of(path: &str) -> String {
    path.split('.').next().unwrap_or("").split('[').next().unwrap_or("").to_string()
}

/// An insertion-ordered map with removal (the reference's dicts of obligations and holders).
#[derive(Clone, Debug)]
pub struct Ordered<V> {
    pub items: Vec<(String, V)>,
}

impl<V> Default for Ordered<V> {
    fn default() -> Self {
        Ordered { items: Vec::new() }
    }
}

impl<V: Clone> Ordered<V> {
    pub fn get(&self, k: &str) -> Option<&V> {
        self.items.iter().find(|(kk, _)| kk == k).map(|(_, v)| v)
    }
    pub fn contains(&self, k: &str) -> bool {
        self.items.iter().any(|(kk, _)| kk == k)
    }
    pub fn insert(&mut self, k: &str, v: V) {
        if let Some(e) = self.items.iter_mut().find(|(kk, _)| kk == k) {
            e.1 = v;
        } else {
            self.items.push((k.to_string(), v));
        }
    }
    pub fn set_default(&mut self, k: &str, v: V) {
        if !self.contains(k) {
            self.items.push((k.to_string(), v));
        }
    }
    pub fn remove(&mut self, k: &str) -> Option<V> {
        let i = self.items.iter().position(|(kk, _)| kk == k)?;
        Some(self.items.remove(i).1)
    }
    pub fn keys(&self) -> Vec<String> {
        self.items.iter().map(|(k, _)| k.clone()).collect()
    }
    pub fn retain<F: FnMut(&str, &V) -> bool>(&mut self, mut f: F) {
        self.items.retain(|(k, v)| f(k, v));
    }
}

#[derive(Clone, Debug, Default)]
pub struct Owners {
    pub moved: BTreeSet<String>,
    pub loans: Vec<Loan>,
    pub holders: Ordered<Loan>,
    pub reservations: Vec<(String, String)>,
    pub obligations: Ordered<(Token, i64)>,
    pub half: Vec<(String, Token)>,
    pub captured: Ordered<String>,
}

impl Owners {
    pub fn copy(&self) -> Owners {
        Owners {
            moved: self.moved.clone(),
            loans: self.loans.clone(),
            holders: self.holders.clone(),
            reservations: self.reservations.clone(),
            obligations: self.obligations.clone(),
            half: vec![],
            captured: self.captured.clone(),
        }
    }

    pub fn join(&self, other: &Owners) -> Owners {
        let mut loans = self.loans.clone();
        for l in &other.loans {
            let ld = l.borrow();
            if !loans.iter().any(|m| {
                let md = m.borrow();
                md.owner == ld.owner && md.mode == ld.mode && md.holder == ld.holder
            }) {
                drop(ld);
                loans.push(l.clone());
            }
        }
        let mut holders = self.holders.clone();
        for (k, v) in &other.holders.items {
            holders.insert(k, v.clone());
        }
        let mut obligations = Ordered::default();
        let mut half = Vec::new();
        let mut keys: Vec<String> = self.obligations.keys();
        for k in other.obligations.keys() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        for p in keys {
            match (self.obligations.get(&p), other.obligations.get(&p)) {
                (Some(a), Some(_)) => obligations.insert(&p, a.clone()),
                (a, b) => half.push((p.clone(), a.or(b).unwrap().0.clone())),
            }
        }
        let mut captured = Ordered::default();
        let mut fs: Vec<String> = self.captured.keys();
        for k in other.captured.keys() {
            if !fs.contains(&k) {
                fs.push(k);
            }
        }
        for f in fs {
            let all = self.captured.get(&f).map(|s| s == "all").unwrap_or(false) && other.captured.get(&f).map(|s| s == "all").unwrap_or(false);
            captured.insert(&f, if all { "all".into() } else { "some".into() });
        }
        Owners { moved: self.moved.union(&other.moved).cloned().collect(), loans, holders, reservations: vec![], obligations, half, captured }
    }

    pub fn owner_path(&self, path: &str) -> String {
        let root = root_of(path);
        if let Some(h) = self.holders.get(&root) {
            let rest = &path[root.len()..];
            return format!("{}{}", h.borrow().owner, rest);
        }
        path.to_string()
    }

    pub fn active(&self, path: &str) -> Vec<Loan> {
        let p = self.owner_path(path);
        self.loans
            .iter()
            .filter(|l| {
                let l = l.borrow();
                overlaps(&l.owner, &p) || (root_of(&l.owner) == root_of(&p) && l.mode == "editable")
            })
            .cloned()
            .collect()
    }

    pub fn is_moved(&self, path: &str) -> bool {
        self.moved.iter().any(|m| path == m || path.starts_with(&format!("{m}.")) || m.starts_with(&format!("{path}.")))
    }

    pub fn end_block(&mut self, depth: i64) {
        self.loans.retain(|l| {
            let l = l.borrow();
            l.holder.is_none() || l.depth < depth
        });
        self.holders.retain(|_, l| l.borrow().depth < depth);
    }

    pub fn end_statement(&mut self) {
        self.loans.retain(|l| l.borrow().holder.is_some());
        self.reservations.clear();
    }

    pub fn add_loan(&mut self, path: &str, mode: &str, holder: Option<&str>, depth: i64, line: usize) -> Loan {
        let owner = self.owner_path(path);
        crate::journal::loan(&owner, mode, holder, depth, line);
        let l = Rc::new(RefCell::new(LoanData { owner, mode: mode.into(), holder: holder.map(|s| s.to_string()), depth, line }));
        self.loans.push(l.clone());
        if let Some(h) = holder {
            self.holders.insert(h, l.clone());
        }
        l
    }

    pub fn remove_loan(&mut self, l: &Loan) {
        if let Some(i) = self.loans.iter().position(|x| loan_eq(x, l)) {
            self.loans.remove(i);
        }
    }

    pub fn has_loan(&self, l: &Loan) -> bool {
        self.loans.iter().any(|x| loan_eq(x, l))
    }

    pub fn move_(&mut self, path: &str) {
        self.moved.insert(path.to_string());
        if self.obligations.remove(path).is_some() {
            crate::journal::close_obligation(path, 0, "move");
        }
    }

    /// A linear place becomes owed (10.1).
    pub fn open_obligation(&mut self, path: &str, tok: Token, depth: i64) {
        crate::journal::open_obligation(path, tok.line);
        self.obligations.insert(path, (tok, depth));
    }

    /// An owed place is discharged (consumed, reported, dropped or forgotten at a scope end).
    pub fn close_obligation(&mut self, path: &str, line: usize, how: &str) -> Option<(Token, i64)> {
        let r = self.obligations.remove(path);
        if r.is_some() {
            crate::journal::close_obligation(path, line, how);
        }
        r
    }

    /// An owed place reported unconsumed: the finding's record journals it (decls::rec_corr), so it is forgotten here.
    pub fn forget(&mut self, path: &str) -> Option<(Token, i64)> {
        self.obligations.remove(path)
    }

    pub fn reinit(&mut self, path: &str) {
        self.moved.retain(|m| !(m == path || m.starts_with(&format!("{path}."))));
    }
}
