//! Facts of one function's final typed walk, journaled for the function IR (PROG-CMP-02): the loans it takes, the
//! obligations it opens and how each is closed, its observation operations and the cleanup edges of its exits.
//! The journal is active only while a function body is typed in the final pass.

use std::cell::RefCell;

use crate::effects::Summary;

#[derive(Clone, Debug)]
pub struct LoanFact {
    /// None in the function's own text; else the woven aspect block it arose in ("failure handler of @get")
    pub origin: Option<String>,
    pub owner: String,
    pub mode: String,
    pub holder: Option<String>,
    pub depth: i64,
    pub line: usize,
}

/// One linear place the walk owes: where it was opened and every event that closed it on some path (a branch can
/// consume it while another reports it unconsumed), in walk order.
#[derive(Clone, Debug)]
pub struct OblFact {
    pub origin: Option<String>,
    pub place: String,
    pub opened: usize,
    pub closes: Vec<(usize, String)>,
}

#[derive(Clone, Debug)]
pub struct ObsFact {
    pub origin: Option<String>,
    pub op: String,
    pub name: String,
    pub line: usize,
    pub required: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CleanupFact {
    pub origin: Option<String>,
    pub op: String,
    pub name: String,
    pub line: usize,
    pub runs: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct FnIr {
    pub function: String,
    pub line: usize,
    /// A contributed body's text belongs to this file and declaration, while `function` names the application.
    /// Ordinary bodies omit both fields from the JSON representation.
    pub source_file: Option<String>,
    pub source_function: Option<String>,
    pub loans: Vec<LoanFact>,
    pub obligations: Vec<OblFact>,
    pub observation: Vec<ObsFact>,
    pub cleanup: Vec<CleanupFact>,
    pub effects: Option<Summary>,
}

thread_local! {
    static J: RefCell<Option<FnIr>> = const { RefCell::new(None) };
    static AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ORIGIN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Facts recorded until the matching `leave` arose in a woven aspect block (its lines are the aspect's file's).
pub fn enter(origin: &str) {
    ORIGIN.with(|o| o.borrow_mut().push(origin.to_string()));
}

pub fn leave() {
    ORIGIN.with(|o| {
        o.borrow_mut().pop();
    });
}

fn origin() -> Option<String> {
    ORIGIN.with(|o| o.borrow().last().cloned())
}

/// The line of the statement being walked: the line of an event whose site has no token of its own (a move).
pub fn at(line: usize) {
    AT.with(|a| a.set(line));
}

pub fn begin(function: &str, line: usize) {
    J.with(|j| *j.borrow_mut() = Some(FnIr { function: function.to_string(), line, ..Default::default() }));
}

pub fn source(file: &str, function: &str) {
    with(|ir| {
        ir.source_file = Some(file.to_string());
        ir.source_function = Some(function.to_string());
    });
}

pub fn end() -> Option<FnIr> {
    J.with(|j| j.borrow_mut().take())
}

pub fn active() -> bool {
    J.with(|j| j.borrow().is_some())
}

fn with<F: FnOnce(&mut FnIr)>(f: F) {
    J.with(|j| {
        if let Some(ir) = j.borrow_mut().as_mut() {
            f(ir);
        }
    });
}

/// A call's loan kept by the binding of its result (7.6): the recorded loan gains its holder and its region.
pub fn kept(owner: &str, mode: &str, line: usize, holder: &str, depth: i64) {
    with(|ir| {
        if let Some(f) = ir.loans.iter_mut().rev().find(|f| f.owner == owner && f.mode == mode && f.line == line && f.holder.is_none()) {
            f.holder = Some(holder.into());
            f.depth = depth;
        }
    });
}

pub fn loan(owner: &str, mode: &str, holder: Option<&str>, depth: i64, line: usize) {
    with(|ir| ir.loans.push(LoanFact { origin: origin(), owner: owner.into(), mode: mode.into(), holder: holder.map(|h| h.into()), depth, line }));
}

pub fn open_obligation(place: &str, line: usize) {
    with(|ir| ir.obligations.push(OblFact { origin: origin(), place: place.into(), opened: line, closes: vec![] }));
}

pub fn close_obligation(place: &str, line: usize, how: &str) {
    let line = if line == 0 { AT.with(|a| a.get()) } else { line };
    with(|ir| {
        if let Some(o) = ir.obligations.iter_mut().rev().find(|o| o.place == place) {
            if o.closes.last() != Some(&(line, how.to_string())) {
                o.closes.push((line, how.into()));
            }
        }
    });
}

/// An E-OBL-001 finding: the place is reported unconsumed at `line` (its obligation opened at `origin`).
pub fn unconsumed(place: &str, origin: usize, line: usize) {
    with(|ir| {
        match ir.obligations.iter_mut().rev().find(|o| o.place == place) {
            Some(o) => o.closes.push((line, "unconsumed".into())),
            None => ir.obligations.push(OblFact { origin: self::origin(), place: place.into(), opened: origin, closes: vec![(line, "unconsumed".into())] }),
        }
    });
}

pub fn observe(op: &str, name: &str, line: usize, required: Vec<String>) {
    with(|ir| ir.observation.push(ObsFact { origin: origin(), op: op.into(), name: name.into(), line, required }));
}

pub fn cleanup(op: &str, name: &str, line: usize, runs: Vec<usize>) {
    with(|ir| ir.cleanup.push(CleanupFact { origin: origin(), op: op.into(), name: name.into(), line, runs }));
}

pub fn set_effects(s: &Summary) {
    with(|ir| ir.effects = Some(s.clone()));
}
