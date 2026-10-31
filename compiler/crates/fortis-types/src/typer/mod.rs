//! Statement and expression typing over declaration tables (DEL-07, DEL-11..DEL-25 analyses): a port of
//! checker/typer.py. One walk per body threads types, interval facts, ownership state, the effect summary and open
//! obligations; what a rule cannot decide is `Unknown` and produces no record (PROG-DIA-02).

mod aspects;
pub mod expr;
mod patterns;
mod stmts;
mod storage;
mod temporaries;

#[cfg(test)]
mod test_ownership;

#[cfg(test)]
mod test_obligations;

#[cfg(test)]
mod test_effects;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use fortis_diag::Json;
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{C, N};

use crate::decls::{Decl, DeclChecker, DeclRef, Sig};
use crate::effects::{library_summary, Summary, AUTHORITIES};
use crate::intervals::{self as iv, Facts, Interval};
use crate::literals::{is_interp, is_strlit};
use crate::owners::{loan_eq, overlaps, root_of, Loan, LoanData, Owners};
use crate::types::*;

#[derive(Clone, Debug)]
pub struct Binding {
    pub ty: Type,
    pub mutable: bool,
    pub kind: String,
    pub depth: i64,
}

pub struct EnvFrame {
    parent: Option<Env>,
    vars: RefCell<HashMap<String, Binding>>,
}
pub type Env = Rc<EnvFrame>;

pub fn env_new(parent: Option<&Env>) -> Env {
    Rc::new(EnvFrame { parent: parent.cloned(), vars: RefCell::new(HashMap::new()) })
}
pub fn env_lookup(env: &Env, n: &str) -> Option<Binding> {
    let mut e = Some(env.clone());
    while let Some(f) = e {
        if let Some(b) = f.vars.borrow().get(n) {
            return Some(b.clone());
        }
        e = f.parent.clone();
    }
    None
}
pub fn env_get(env: &Env, n: &str) -> Option<Type> {
    env_lookup(env, n).map(|b| b.ty)
}
pub fn env_set(env: &Env, n: &str, t: Type, mutable: bool, kind: &str) {
    env.vars.borrow_mut().insert(n.to_string(), Binding { ty: t, mutable, kind: kind.to_string(), depth: 0 });
}
/// Mutate the binding where it is found (the reference mutates the Binding object).
pub fn env_update<F: FnOnce(&mut Binding)>(env: &Env, n: &str, f: F) {
    let mut e = Some(env.clone());
    while let Some(fr) = e {
        if let Some(b) = fr.vars.borrow_mut().get_mut(n) {
            f(b);
            return;
        }
        e = fr.parent.clone();
    }
}
pub fn env_local(env: &Env, n: &str) -> Option<Binding> {
    env.vars.borrow().get(n).cloned()
}

pub const OPEN_SCRUTINEE: &[&str] = &["Int", "String", "StringView", "Failure", "Scalar", "Byte", "Decimal", "Duration", "Timestamp"];

/// An expression as the source spells it, for a message: `move d`, `make(console, "x")`.
pub fn source_text(n: N<'_>) -> String {
    let mut s = String::new();
    for t in n.tokens() {
        let text = t.text.as_str();
        let glue = matches!(text, "," | ")" | "]" | ".")
            || (matches!(text, "(" | "[") && s.chars().last().map_or(false, |c| c.is_alphanumeric() || c == '_'))
            || s.ends_with(['(', '[', '.']);
        if !s.is_empty() && !glue {
            s.push(' ');
        }
        s.push_str(text);
    }
    s
}

pub fn ftok<'a>(n: N<'a>) -> &'a Token {
    n.first_token().expect("a node with a token")
}
pub fn ctok<'a>(c: C<'a>) -> &'a Token {
    match c {
        C::T(t) => t,
        C::N(n) => ftok(n),
    }
}
pub fn synth(text: &str, line: usize, col: usize) -> Token {
    Token { kind: Kind::Punct, text: text.into(), line, col, interps: vec![], closes_type_args: false, holes: Default::default() }
}

#[derive(Clone, Debug, Default)]
pub struct ClosureInfo {
    pub kind: String,
    pub captures: Vec<(String, String)>,
    pub linear: bool,
    pub eff: Option<Summary>,
}

/// A unit of a module's pass (`Typer::units`): a declaration by name, an implement block (its index and
/// `Interface for Target`), a test or property (its index among the module's declarations and `test "name"`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Unit {
    Decl(String),
    Implement(usize, String),
    Test(usize, String),
}

impl Unit {
    /// The unit's stable ID.
    pub fn id(&self) -> String {
        match self {
            Unit::Decl(n) => format!("decl {n}"),
            Unit::Implement(_, s) => format!("implement {s}"),
            Unit::Test(_, s) => s.clone(),
        }
    }
}

/// Lowering facts for one applied mixin method. The same syntax nodes can have different types at two
/// applications, and source positions belong to the mixin's file rather than the applying class's file.
#[derive(Default, Clone)]
pub struct MixinBodyFacts {
    pub legacy_conversions: HashMap<(usize, usize), crate::migrate::LegacyConversion>,
    pub legacy_conversion_sources: HashMap<(usize, usize), Type>,
    pub node_types: HashMap<usize, Type>,
    pub closure_info: HashMap<usize, ClosureInfo>,
    pub implicit_moves: BTreeSet<usize>,
    pub checked_ops: BTreeSet<(usize, usize)>,
    pub implied_fails: BTreeSet<(usize, usize)>,
    pub implied_regions: BTreeSet<(usize, usize)>,
}

pub struct Typer<'a> {
    pub dc: DeclChecker<'a>,
    pub module: String,
    pub package: String,
    pub aliases: BTreeMap<String, String>,
    pub self_type: Option<Type>,
    pub self_mode: String,
    pub ret_type: Option<Type>,
    pub in_try: bool,
    pub ctx: Vec<String>,
    pub arm_types: Vec<Vec<(Type, Token)>>,
    pub loop_labels: Vec<Option<String>>,
    pub give_seen: Vec<bool>,
    pub facts: Facts,
    pub coll_path: Option<String>,
    pub own: Owners,
    pub depth: i64,
    pub arg_mode: Option<String>,
    pub receiver: Option<(String, String)>,
    pub in_marker: bool,
    pub pending: Option<(String, String, Token)>,
    /// `copy` markers already E-OWN-002, by line and column (D-341)
    pub copy_refused: std::collections::HashSet<(usize, usize)>,
    pub res_stack: Vec<Vec<(String, String)>>,
    pub infer_capture_node: Option<usize>,
    pub capture_probe_node: Option<usize>,
    pub capture_uses: Vec<(String, String, Token)>,
    pub lambda_kind: Vec<String>,
    pub last_closure: Option<ClosureInfo>,
    pub last_closure_eff: Option<Summary>,
    pub closures: BTreeMap<String, ClosureInfo>,
    pub discharges: Vec<(usize, String)>,
    pub eff: Summary,
    pub fresh: BTreeSet<String>,
    pub editable_params: BTreeSet<String>,
    pub summaries: BTreeMap<String, Summary>,
    /// the summary labels the pass looked up (an incremental check depends on exactly these, DEL-46)
    pub requested: RefCell<BTreeSet<String>>,
    pub storage_substitution: BTreeMap<String, Type>,
    pub storage_calls: BTreeSet<String>,
    pub storage_failed: bool,
    pub storage_expected: Option<Box<Type>>,
    pub current_label: String,
    pub call_decl: Option<DeclRef<'a>>,
    pub call_label: String,
    pub final_pass: bool,
    pub reported: BTreeSet<String>,
    pub schema: Option<DeclRef<'a>>,
    pub schema_unknown: bool,
    pub current_aspects: Vec<N<'a>>,
    pub caps_calls: usize,
    /// the type parameters of the body being typed, and its owner's and its own `where` bounds with arguments (6.10)
    pub tparams_in_scope: BTreeSet<String>,
    pub where_in_scope: Vec<(String, NamedT)>,
    /// the body's own `X: linear` parameters (6.12, 10.7): each is a linear type inside its body
    pub linear_in_scope: BTreeSet<String>,
    /// a generic class's bounds, for the method call checked next (D-171)
    pub pending_owner_where: Vec<(String, NamedT)>,
    /// constants whose value is being typed (a cycle guard; D-149)
    pub const_stack: BTreeSet<String>,
    pub elaborations: Vec<String>,
    pub defer_depth: i64,
    /// the type of every expression node (and of each postfix step) of the final pass, when `record_types` is set:
    /// the typed IR builder reads it (fortis-mir)
    pub node_types: Option<HashMap<usize, Type>>,
    /// the type a `give`/`recover` value joins, when known
    pub branch_expected: Vec<Option<Type>>,
    /// typing a branch value whose target type is known
    pub branch_target: bool,
    pub record_types: bool,
    pub closure_info: HashMap<usize, ClosureInfo>,
    /// the returned expressions that move a local `let` binding without `move` (`return x`, X-345): the typed IR
    /// builder makes them moves, as it does `return move x`
    pub implicit_moves: BTreeSet<usize>,
    /// the markers the declaration already shows (X-345), by (line, column): an `editable` before a receiver or a
    /// `fn mut` callee whose root may change, and a `move` in `return move x` of a local. The formatter drops them;
    /// the unmarked form is checked exactly as the marked one
    pub redundant_markers: BTreeSet<(usize, usize)>,
    /// the checked-arithmetic regions being typed: their operators' (line, column), the checked ones, and whether one
    /// was rejected with `E-REF-002` (6.4, X-345)
    pub regions: Vec<(BTreeSet<(usize, usize)>, Vec<Token>, bool)>,
    /// (line, column) of every operator typed as the checked operation (6.4, X-345); the IR builder lowers these
    pub checked_ops: BTreeSet<(usize, usize)>,
    /// > 0 while typing the arguments of a call the checker could not resolve (D-304)
    pub unresolved_args: usize,
    /// rewrites into the v0.0.2 forms that mean the same, found while typing (X-345; `crate::migrate`)
    pub migrations: Vec<crate::migrate::Edit>,
    /// Retired conversions resolved by identity, for the formatter (X-552).
    pub legacy_conversions: HashMap<(usize, usize), crate::migrate::LegacyConversion>,
    /// Checked sources for retired to and validate calls; unknown is never proof of a total rewrite (X-542).
    pub legacy_conversion_sources: HashMap<(usize, usize), Type>,
    /// A call argument whose expected target comes from an inferred type parameter, not a written type.
    pub inferred_conversion_target: bool,
    /// `Int.add(a, b)` and its family by call node: whether the operator form would be unproved (6.4, X-345)
    pub int_calls: HashMap<usize, bool>,
    pub guards: Vec<(String, i64, Token)>,
    pub scope_depths: Vec<(String, i64)>,
    pub deferred: Vec<(i64, BTreeSet<String>, Token)>,
    /// (block depth, deferred block) registered in the blocks being walked (D-75)
    pub defer_bodies: Vec<(i64, N<'a>)>,
    /// (depth, states) per if-expression branch, match arm and catch block: the states at each `give`/`recover`, after
    /// the cleanup they run (D-76)
    pub exit_frames: Vec<(i64, Vec<Facts>)>,
    /// (label, depth, states, break owners, continue owners) per loop: the states at each `break` (D-72), and the
    /// ownership at each `break` and each `continue`, whose loans are live at the loop's exit (D-362)
    pub break_frames: Vec<(Option<String>, i64, Vec<Facts>, Vec<Owners>, Vec<Owners>)>,
    /// Hidden owned linear remainders: loop depth and item token (D-588).
    pub linear_loops: Vec<(i64, Token)>,
    /// per if-expression branch, match arm and catch block: the loans each `give`/`recover` value carries, handed to
    /// the expression's consumer (7.3; D-355, D-395)
    pub arm_loans: Vec<Vec<Loan>>,
    /// per branch: whether a `give`/`recover` value is one of the function's own (D-389)
    pub arm_owned: Vec<bool>,
    /// the id of each expression node whose value owns a bearing part beside a loan it holds (7.9; D-369)
    pub owned_parts: std::collections::HashSet<usize>,
    /// the bindings that own such a part: a loan drawn from one is a loan of it (D-369, D-388)
    pub part_owners: BTreeSet<String>,
    /// the id of each `p ?? v` node whose value may be the bare owned place `p`'s, with `p` (D-357)
    pub coalesced_places: HashMap<usize, String>,
    /// the statements after the one being walked, in its block (D-75)
    pub block_rest: Vec<N<'a>>,
    /// every place whose facts were discarded, in order, for the module check (D-74, D-77)
    pub inval_log: Vec<String>,
    pub catch_bound: Vec<String>,
    pub carried: Vec<Loan>,
    /// temporaries an async call's future lends, until their statement ends (12.3; D-31)
    pub carried_temps: Vec<(Token, usize)>,
    /// calls being checked, innermost last: a temporary is recorded with the depth of the call it is lent to
    pub temporary_values: std::collections::HashSet<usize>,
    pub temporary_borrows: Vec<Token>,
    pub temporary_receiver: bool,
    pub optional_arguments: bool,
    pub optional_scopes: Vec<(BTreeMap<String, (Token, i64)>, Token)>,
    pub call_depth: usize,
    pub use_ok: bool,
    /// expected type of the call about to be checked, consumed by it
    pub pending_call_expected: Option<Type>,
    /// expected type of a chain's last call, copied across at the call itself
    pub result_expected: Option<Type>,
    pub level_values: Vec<BTreeMap<String, (Option<N<'a>>, Env)>>,
    /// the function IR of every body typed in the final pass (fortis-ir adds the control-flow graphs)
    pub ir_out: Vec<crate::journal::FnIr>,
    /// the head tokens of the function being typed (corrections insert modifiers before `func`)
    pub head_toks: Vec<Token>,
    /// the `fn` (and `mut`) tokens of the lambdas being typed (corrections strengthen a closure's kind)
    pub lambda_toks: Vec<(Token, Option<Token>)>,
    pub hole_trees: Vec<Rc<fortis_syntax::Parsed>>,
    /// each call's callee by its name token's position: (`try`, declared `@ignorable`) (5.5, 14; X-345)
    pub call_sigs: HashMap<(usize, usize), (bool, bool)>,
    /// typing a test or property body: `or fail` is implied (14, X-345)
    pub test_body: bool,
    /// inside a test or property body, lambdas included: `expect` is allowed (14)
    pub in_test_code: bool,
    /// typing a mixin's body at an application: only the class's exported members (9.6)
    pub mixin_view: Option<String>,
    /// (applying class identity, class name, method name) -> facts for this application (9.6, X-476).
    pub mixin_body_facts: BTreeMap<(String, String, String), MixinBodyFacts>,
    /// Ordinary lexical bodies' inferred effects, shared by this module's foreign mixin applications.
    pub mixin_home_summaries: BTreeMap<(usize, String), BTreeMap<String, Summary>>,
    /// False only while seeding a lexical module's ordinary bodies, so mixin applications cannot recurse.
    pub check_mixin_applications: bool,
    /// the `as` binding of a `cases` table being typed, and the types the body expects of each row field (14, D-50.3)
    pub row_name: Option<String>,
    pub row_uses: BTreeMap<String, Vec<Type>>,
    /// nodes that `or`, `catch` or `match` consume (14)
    pub consumed_ops: std::collections::HashSet<usize>,
    /// the name-token positions of the calls of a test body with an implied `or fail` (14); the IR builder reads them
    pub implied_fails: BTreeSet<(usize, usize)>,
    /// the first operator of each checked region a test body's arithmetic forms, with an implied `or fail` (14, 6.4)
    pub implied_regions: BTreeSet<(usize, usize)>,
}

impl<'a> Typer<'a> {
    pub fn new(dc: DeclChecker<'a>) -> Typer<'a> {
        let module = dc.module_name.clone();
        let package = dc.package.clone();
        Typer {
            dc,
            module,
            package,
            aliases: BTreeMap::new(),
            self_type: None,
            self_mode: "self".into(),
            ret_type: None,
            in_try: false,
            ctx: vec![],
            arm_types: vec![],
            loop_labels: vec![],
            give_seen: vec![],
            facts: Facts::default(),
            coll_path: None,
            own: Owners::default(),
            depth: 0,
            arg_mode: None,
            receiver: None,
            in_marker: false,
            pending: None,
            copy_refused: Default::default(),
            res_stack: vec![],
            infer_capture_node: None,
            capture_probe_node: None,
            capture_uses: vec![],
            lambda_kind: vec![],
            last_closure: None,
            last_closure_eff: None,
            closures: BTreeMap::new(),
            discharges: vec![],
            eff: Summary::default(),
            fresh: BTreeSet::new(),
            editable_params: BTreeSet::new(),
            summaries: BTreeMap::new(),
            requested: RefCell::new(BTreeSet::new()),
            storage_substitution: BTreeMap::new(),
            storage_calls: BTreeSet::new(),
            storage_failed: false,
            storage_expected: None,
            current_label: String::new(),
            call_decl: None,
            call_label: String::new(),
            final_pass: true,
            reported: BTreeSet::new(),
            schema: None,
            schema_unknown: false,
            current_aspects: vec![],
            caps_calls: 0,
            tparams_in_scope: BTreeSet::new(),
            where_in_scope: vec![],
            linear_in_scope: BTreeSet::new(),
            pending_owner_where: vec![],
            const_stack: BTreeSet::new(),
            elaborations: vec![],
            defer_depth: 0,
            node_types: None,
            branch_expected: Vec::new(),
            branch_target: false,
            record_types: false,
            closure_info: HashMap::new(),
            implicit_moves: BTreeSet::new(),
            redundant_markers: BTreeSet::new(),
            regions: Vec::new(),
            checked_ops: BTreeSet::new(),
            unresolved_args: 0,
            migrations: Vec::new(),
            legacy_conversions: HashMap::new(),
            legacy_conversion_sources: HashMap::new(),
            inferred_conversion_target: false,
            int_calls: HashMap::new(),
            guards: vec![],
            scope_depths: vec![],
            deferred: vec![],
            defer_bodies: vec![],
            exit_frames: vec![],
            break_frames: vec![],
            linear_loops: vec![],
            arm_loans: vec![],
            arm_owned: vec![],
            owned_parts: std::collections::HashSet::new(),
            part_owners: BTreeSet::new(),
            coalesced_places: HashMap::new(),
            block_rest: vec![],
            inval_log: vec![],
            catch_bound: vec![],
            carried: vec![],
            carried_temps: vec![],
            temporary_values: std::collections::HashSet::new(),
            temporary_borrows: Vec::new(),
            temporary_receiver: false,
            optional_arguments: false,
            optional_scopes: vec![],
            call_depth: 0,
            use_ok: true,
            pending_call_expected: None,
            result_expected: None,
            level_values: vec![],
            ir_out: vec![],
            head_toks: vec![],
            lambda_toks: vec![],
            hole_trees: vec![],
            call_sigs: HashMap::new(),
            test_body: false,
            in_test_code: false,
            mixin_view: None,
            mixin_body_facts: BTreeMap::new(),
            mixin_home_summaries: BTreeMap::new(),
            check_mixin_applications: true,
            row_name: None,
            row_uses: BTreeMap::new(),
            consumed_ops: std::collections::HashSet::new(),
            implied_fails: BTreeSet::new(),
            implied_regions: BTreeSet::new(),
        }
    }

    // ---- records ----------------------------------------------------------------------
    pub fn rec(&mut self, code: &str, msg: String, rule: &str, tok: &Token, fields: Json) {
        self.dc.rec(code, msg, rule, Some(tok), fields, "violated");
    }

    pub fn typ(&mut self, code: &str, msg: String, tok: &Token, expected: &str, actual: &str, span: &str, rule: &str) {
        self.rec(code, msg, rule, tok, DeclChecker::typ_fields(expected, actual, span));
    }

    pub fn rec_c(&mut self, code: &str, msg: String, rule: &str, tok: &Token, fields: Json, corr: Option<Json>) {
        self.dc.rec_corr(code, msg, rule, Some(tok), fields, "violated", corr);
    }

    /// Insert text before a token: one edit of a correction.
    pub fn insert_before(&self, kind: &str, t: &Token, text: &str) -> Json {
        let f = self.dc.file.clone();
        crate::decls::correction(kind, vec![crate::decls::edit(&f, t.line, t.col, t.col, text)])
    }

    /// A modifier inserted into the head of the function being typed, before the first of `before` it has.
    pub fn insert_modifier(&self, kind: &str, word: &str, before: &[&str]) -> Option<Json> {
        let t = self.head_toks.iter().find(|t| before.contains(&t.text.as_str()))?;
        Some(self.insert_before(kind, t, &format!("{word} ")))
    }

    pub fn nam(name: &str, cands: Vec<String>) -> Json {
        Json::obj([("name", Json::str(name)), ("candidates", Json::strs(cands))])
    }

    pub fn nrecords(&self) -> usize {
        self.dc.records.len()
    }
    pub fn truncate_records(&mut self, n: usize) {
        self.dc.records.truncate(n);
    }

    /// The declaration of a named type. A type is known by its identity (package@version and module, LANG-04-04),
    /// computed in the module that declares it, not by a same-named declaration this module happens to import:
    /// `import acme.left.items as left` enters `left.Item`, whose values have type `Item`.
    pub fn decl_of(&self, t: &Type) -> Option<DeclRef<'a>> {
        let t = strip(t);
        match t {
            Type::Refined { .. } | Type::Named(_) => self.dc.resolve_type(t, None),
            _ => None,
        }
    }

    /// Remember an unannotated constant's initialiser type (`module\0name`) for later uses and the IR builder.
    pub fn note_inferred_const(&self, module: &str, name: &str, ty: &Type) {
        if known(ty) {
            self.dc.sess.inferred_consts.borrow_mut().insert(format!("{module}\0{name}"), ty.clone());
        }
    }

    pub fn inferred_const(&self, module: &str, name: &str) -> Option<Type> {
        self.dc.sess.inferred_consts.borrow().get(&format!("{module}\0{name}")).cloned()
    }

    pub fn foreign(&self, d: &Decl<'a>) -> bool {
        d.module != self.module
    }

    // ---- visibility (4.4) -------------------------------------------------------------
    pub fn member_visible(&self, d: &Decl<'a>, member: &str) -> bool {
        if let Some(f) = d.fields.get(member) {
            if f.1 {
                return true;
            }
        } else if let Some(m) = d.methods.get(member) {
            if m.exported {
                return true;
            }
        } else {
            // A contributed method retains its visibility on the applying class. The absence of a method
            // in the class's own declarations does not make a private mixin implementation public (4.4, 9.6).
            let provided = d.mixins.iter().filter_map(|mx| self.dc.resolve_named(mx, None))
                .filter_map(|md| md.methods.get(member).cloned()).last();
            match provided {
                Some(m) if !m.exported => {},
                _ => return true,
            }
        }
        for iface in &d.implements {
            if let Some(id) = self.dc.resolve_named(iface, None) {
                if id.methods.contains(member) || id.constants.contains(member) {
                    return true;
                }
            }
            if iface.interface_key() == "Abandonable" && member == "abandon" {
                return true;
            }
        }
        // A requirement grants this mixin access to its applying class's method. It neither exports that
        // method to foreign callers nor grants another mixin access through a sibling's requirement (9.6).
        let owner = self.self_type.as_ref().and_then(|t| self.decl_of(t));
        if owner.as_ref().map_or(false, |o| o.name == d.name && o.type_identity() == d.type_identity()) {
            if let Some(md) = self.mixin_view.as_ref().and_then(|name| self.dc.table.get(name)) {
                if md.kind == "mixin" && md.module == self.module && md.methods.contains(&format!("requires:{member}")) {
                    return true;
                }
            }
        }
        false
    }

    pub fn check_member(&mut self, d: &Decl<'a>, member: &str, tok: &Token, what: &str) {
        // a mixin body sees only the exported members of the class it is applied to, in the same module too (9.6)
        let through_mixin = self.mixin_view.is_some() && self.self_type.as_ref().and_then(|t| self.decl_of(t)).map_or(false, |owner| owner.name == d.name && owner.type_identity() == d.type_identity());
        if (self.foreign(d) || through_mixin) && !self.member_visible(d, member) {
            let mut cands: Vec<String> = d.fields.iter().filter(|(_, v)| v.1).map(|(k, _)| k.clone()).collect();
            cands.sort();
            let mut ms: Vec<String> = d.methods.iter().filter(|(_, s)| s.exported).map(|(k, _)| k.clone()).collect();
            ms.sort();
            cands.extend(ms);
            self.rec(
                "E-NAM-004",
                if through_mixin { format!("{what} `{member}` of {} is not exported; a mixin ({}) sees only the exported members of {} (9.6)", d.name, self.mixin_view.clone().unwrap_or_default(), d.name) } else { format!("{what} `{member}` of {} is not exported; {} is declared in {}", d.name, d.name, d.module) },
                if what != "field" { "LANG-04-05" } else { "LANG-09-02" },
                tok,
                Self::nam(member, cands),
            );
        }
    }

    // ---- the interval domain (6.3) ------------------------------------------------------
    pub fn path_of_place(&self, pl: N<'a>) -> String {
        let ts = pl.direct_tokens();
        let mut parts = vec![ts[0].text.clone()];
        for k in pl.kids().into_iter().skip(1) {
            match k {
                C::N(n) if n.name() == "memberName" => parts.push(n.tokens()[0].text.clone()),
                C::T(t) if t.kind == Kind::Int => parts.push(format!("[{}]", t.text)),
                _ => {}
            }
        }
        parts.join(".")
    }

    pub fn path_of_expr(&self, e: Option<N<'a>>) -> Option<String> {
        let e = e?;
        let n = e.name();
        let kids = e.kids();
        const PASS: &[&str] = &[
            "constExpr", "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr", "asExpr", "unaryExpr",
            "exprStmt",
        ];
        if PASS.contains(&n) && kids.len() == 1 {
            return match kids[0] {
                C::N(k) => self.path_of_expr(Some(k)),
                C::T(_) => None,
            };
        }
        if n == "place" {
            return Some(self.path_of_place(e));
        }
        if n == "markerExpr" && kids.len() == 2 {
            return e.find("place").map(|p| self.path_of_place(p));
        }
        if n == "primaryExpr" {
            if kids.len() == 1 {
                if let C::T(t) = kids[0] {
                    if t.kind == Kind::Lower || t.text == "self" {
                        return Some(t.text.clone());
                    }
                }
            }
            if kids.len() == 3 && kids[0].text_is("(") {
                return self.path_of_expr(kids[1].node());
            }
            return None;
        }
        if n == "postfixExpr" {
            let mut root = self.path_of_expr(kids[0].node())?;
            for pf in &kids[1..] {
                let k = pf.node().map(|p| p.kids()).unwrap_or_default();
                if !k.is_empty() && k[0].text_is(".") && k.len() > 1 && k[1].is("memberName") {
                    root = format!("{root}.{}", k[1].node().unwrap().tokens()[0].text);
                } else {
                    return None;
                }
            }
            return Some(root);
        }
        None
    }

    pub fn interval_of(&self, t: &Type) -> Option<Interval> {
        int_bounds(strip(t))
    }

    pub fn fact_type(&self, path: Option<&str>, t: Type) -> Type {
        let path = match path {
            Some(p) => p,
            None => return t,
        };
        let s = strip(&t).clone();
        if let Type::Optional(i) = &s {
            if self.facts.present.get(path) == Some(&true) {
                return (**i).clone();
            }
        }
        if let Some(&(lo, hi)) = self.facts.ints.get(path) {
            if self.interval_of(&s).is_some() {
                let base = match &s {
                    Type::Refined { base, .. } => base.clone(),
                    Type::Named(n) => n.clone(),
                    _ => return t,
                };
                return refined(base, Some(lo), Some(hi), "range");
            }
        }
        t
    }

    pub fn is_int(&self, t: &Type) -> bool {
        self.interval_of(t).is_some()
    }

    pub fn ref_(&mut self, code: &str, msg: String, tok: &Token, status: &'static str, required: &str, proved: &str, widening: &str) {
        self.rec(code, msg, "LANG-06-02", tok, Json::obj([("required_range", Json::str(required)), ("proved_interval", Json::str(proved)), ("widening_point", Json::str(widening))]));
        if let Some(r) = self.dc.records.last_mut() {
            r.status = status;
        }
    }

    pub fn show_iv(i: Option<Interval>) -> String {
        match i {
            None => "?".into(),
            Some((a, b)) => format!("[{a}, {b}]"),
        }
    }

    pub fn set_fact(&mut self, path: Option<&str>, t: &Type) {
        let path = match path {
            Some(p) => p,
            None => return,
        };
        self.inval_log.push(path.to_string());
        self.facts.invalidate(path);
        if let Some(i) = self.interval_of(t) {
            self.facts.ints.insert(path.to_string(), i);
        }
    }

    pub fn assigned_places(&self, block: N<'a>) -> BTreeSet<String> {
        self.writes_in(block).into_iter().filter(|o| !o.is_empty()).collect()
    }

    /// Every place the node assigns or loans exclusively, in order and with repeats. A lambda body is a separate
    /// activation and is not entered: the lambda writes outside itself only through its `editable` captures, which count
    /// as writes where it is made; a marked use of a closure binding counts its `editable` captures as written there; an
    /// interpolation hole is an expression evaluated where its string is (3.6), so its writes count there.
    pub fn writes_in(&self, node: N<'a>) -> Vec<String> {
        let mut out = Vec::new();
        for c in node.kids() {
            out.extend(self.writes_of(c));
        }
        out
    }

    /// The writes of one child: the node itself (see `writes_in`), then what it contains.
    pub fn writes_of(&self, c: C<'a>) -> Vec<String> {
        let mut out = Vec::new();
        let c = match c {
            C::T(tk) => {
                if tk.kind == Kind::Str && tk.text.contains("${") {
                    for (k, src) in expr::hole_sources(tk).into_iter().enumerate() {
                        if let Some(p) = expr::hole_tree_at(tk, k, &src) {
                            out.extend(self.writes_of(C::N(N::new(&p.root, &p.toks))));
                        }
                    }
                }
                return out;
            }
            C::N(n) => n,
        };
        {
            match c.name() {
                "lambda" => {
                    if let Some(cl) = c.find("captureList") {
                        for cap in cl.find_all("capture") {
                            if let Some(pl) = cap.find("place") {
                                if ftok(cap).text == "editable" {
                                    out.push(self.path_of_place(pl));
                                }
                            }
                        }
                    }
                    return out;
                }
                "assignStmt" => {
                    if let Some(p) = c.find("place") {
                        out.push(self.path_of_place(p));
                    }
                }
                "markerExpr" => {
                    if let Some(p) = self.marked_path(c) {
                        if let Some(info) = self.closures.get(&root_of(&p)) {
                            for (cp, cm) in &info.captures {
                                if cm == "editable" {
                                    out.push(cp.clone());
                                }
                            }
                        }
                        if c.tokens()[0].text == "editable" {
                            out.push(p);
                        }
                    }
                }
                _ => {}
            }
            out.extend(self.writes_in(c));
        }
        out
    }

    /// The place a marker loans: in `editable p.m(...)` the greedy place took the method name, so the loaned place is the
    /// receiver `p` (D-71: `editable xs.pop()` loans `xs`, not a place `xs.pop`).
    pub fn marked_path(&self, m: N<'a>) -> Option<String> {
        let pl = m.find("place")?;
        let kids = m.kids();
        let members: Vec<N<'a>> = pl.child_nodes("memberName");
        if kids.len() > 2 && kids[2].text_is("(") && !members.is_empty() {
            let mut parts = vec![pl.direct_tokens()[0].text.clone()];
            for k in &members[..members.len() - 1] {
                parts.push(k.tokens()[0].text.clone());
            }
            return Some(parts.join("."));
        }
        Some(self.path_of_place(pl))
    }

    /// Every `move` marker the node contains, in source order: the moved place and the marker's token. A lambda body
    /// is a separate activation and is not entered; an interpolation hole is an expression evaluated where its string
    /// is (3.6), so its markers count there; `match move p` moves `p` as a marker does (9.8). Bindings an arm's
    /// pattern makes are fresh on every evaluation, so moves of them do not count (the `skip` names).
    pub fn move_markers(&self, node: N<'a>) -> Vec<(String, Token)> {
        let mut out = Vec::new();
        for c in node.kids() {
            self.move_markers_of(c, &mut out, &BTreeSet::new());
        }
        out
    }

    fn move_markers_of(&self, c: C<'a>, out: &mut Vec<(String, Token)>, skip: &BTreeSet<String>) {
        let n = match c {
            C::T(tk) => {
                if tk.kind == Kind::Str && tk.text.contains("${") {
                    for (k, src) in expr::hole_sources(tk).into_iter().enumerate() {
                        if let Some(p) = expr::hole_tree_at(tk, k, &src) {
                            self.move_markers_of(C::N(N::new(&p.root, &p.toks)), out, skip);
                        }
                    }
                }
                return;
            }
            C::N(n) => n,
        };
        if n.name() == "lambda" {
            return;
        }
        if n.name() == "markerExpr" && n.tokens()[0].text == "move" {
            if let Some(p) = self.marked_path(n) {
                if !skip.contains(&root_of(&p)) {
                    out.push((p, n.tokens()[0].clone()));
                }
            }
            return;
        }
        if n.name() == "matchExpr" {
            let kids = n.kids();
            if let Some(mt) = kids.iter().find_map(|k| match *k {
                C::T(t) if t.text == "move" => Some((*t).clone()),
                _ => None,
            }) {
                let scrut = kids.iter().filter_map(|k| k.node()).find(|k| k.name() == "expr" || k.name() == "place");
                let sp = match scrut {
                    Some(s) if s.name() == "place" => Some(self.path_of_place(s)),
                    other => self.path_of_expr(other),
                };
                if let Some(p) = sp {
                    if !skip.contains(&root_of(&p)) {
                        out.push((p, mt));
                    }
                }
            }
            let mut inner = skip.clone();
            for arm in n.child_nodes("matchArm") {
                for pat in arm.child_nodes("pattern") {
                    for t in pat.tokens() {
                        if t.kind == Kind::Lower {
                            inner.insert(t.text.clone());
                        }
                    }
                }
            }
            for k in kids {
                self.move_markers_of(k, out, &inner);
            }
            return;
        }
        for k in n.kids() {
            self.move_markers_of(k, out, skip);
        }
    }

    /// A deferred body's effect on the facts where it runs (D-75): every place it assigns or loans exclusively loses its
    /// facts, except that a place written once in the body, by a top-level `place = <integer literal>` (or its negation)
    /// with no other write to it, a prefix of it or a field of it, holds the literal afterwards (the body has no exits,
    /// E-DEF-001).
    pub fn deferred_effects(&mut self, facts: &mut Facts, blk: N<'a>) {
        let writes: Vec<String> = self.writes_in(blk).into_iter().filter(|w| !w.is_empty()).collect();
        let places: BTreeSet<String> = writes.iter().cloned().collect();
        for p in &places {
            self.inval_log.push(p.clone());
            facts.invalidate(p);
        }
        for st in blk.child_nodes("statement") {
            let Some(C::N(inner)) = st.kid(0) else { continue };
            if inner.name() != "assignStmt" {
                continue;
            }
            let Some(pl) = inner.find("place") else { continue };
            let path = self.path_of_place(pl);
            let ets = inner.find("expr").map(|e| e.tokens()).unwrap_or_default();
            // the literal holds only if this is the body's one write to the place, a prefix of it or a field of it
            if path.is_empty() || writes.iter().filter(|q| iv::overlaps(q, &path)).count() != 1 || !inner.direct_tokens().iter().any(|t| t.text == "=") {
                continue;
            }
            let negated = ets.len() == 2 && ets[0].text == "-";
            let lit = if negated { &ets[1..] } else { &ets[..] };
            if lit.len() == 1 && lit[0].kind == fortis_syntax::lexer::Kind::Int {
                let mut i = self.interval_of(&crate::literals::literal_type(lit[0]));
                if negated {
                    i = i.and_then(|i| iv::arith("-", (0, 0), i, 64));
                }
                if let Some(i) = i {
                    facts.ints.insert(path, i);
                }
            }
        }
    }

    /// The state at an exit that leaves every block deeper than `depth`: their pending deferred bodies run (D-75).
    pub fn cleanup_state(&mut self, depth: i64) -> Facts {
        let mut s = self.facts.clone();
        let bodies: Vec<(i64, N<'a>)> = self.defer_bodies.iter().rev().cloned().collect();
        for (d, blk) in bodies {
            if d > depth {
                self.deferred_effects(&mut s, blk);
            }
        }
        s
    }

    /// Type a condition and return (true state, false state, exact): the condition's effects are applied once, from the
    /// state before it, and both states carry them (D-73).
    pub fn condition_states(&mut self, cond: N<'a>, env: &Env) -> (Facts, Facts, bool) {
        let pre = self.facts.clone();
        self.condition(cond, env);
        self.facts = pre;
        self.narrow(cond, env)
    }

    /// (facts when true, facts when false, whether the false side is the exact negation).
    pub fn narrow(&mut self, cond: N<'a>, env: &Env) -> (Facts, Facts, bool) {
        let base = self.facts.clone();
        let n = cond.name();
        let kids = cond.kids();
        const PASS: &[&str] = &[
            "constExpr", "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr", "asExpr", "unaryExpr",
            "postfixExpr", "exprStmt",
        ];
        if PASS.contains(&n) && kids.len() == 1 {
            if let C::N(k) = kids[0] {
                return self.narrow(k, env);
            }
            return (base.clone(), base, false);
        }
        if n == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") {
            return self.narrow(kids[1].node().unwrap(), env);
        }
        if n == "andExpr" && kids.len() > 1 {
            // each conjunct is narrowed in the state where the earlier ones held, with its own effects; the expression is
            // false when some conjunct is false, so the false state joins each conjunct's false state (D-73)
            let mut t = base.clone();
            let mut falses: Vec<Facts> = vec![];
            for k in kids.iter().step_by(2) {
                self.facts = t;
                let (y, f, _) = self.narrow(k.node().unwrap(), env);
                t = y;
                falses.push(f);
            }
            self.facts = base;
            return (t, Facts::join_all(&falses), false);
        }
        if (n == "equalityExpr" || n == "relationalExpr") && kids.len() == 3 {
            let op = kids[1].tok().unwrap().text.clone();
            let mut lp = self.path_of_expr(kids[0].node());
            let rp = self.path_of_expr(kids[2].node());
            let before = self.nrecords();
            let lt = self.infer_c(kids[0], env, None);
            let mark = self.inval_log.len();
            let want = self.enum_operand(&lt); // `s == Error` (9.7, X-345)
            let rt = self.infer_c(kids[2], env, want.as_ref());
            // the operands are typed again for the narrowing; their diagnostics were reported when the condition was
            // typed, so a second `move d` here is not a second move to report (D-448)
            self.truncate_records(before);
            // D-74, D-77: the left operand's value was read before the right operand ran; if the right operand discarded
            // the left place's facts (a write, an editable loan), the comparison says nothing about its current value
            if let Some(p) = &lp {
                if self.inval_log[mark..].iter().any(|q| iv::overlaps(q, p)) {
                    lp = None;
                }
            }
            let cur = self.facts.clone(); // the state after both operands' effects
            if n == "equalityExpr" {
                for (p, t, other) in [(lp.clone(), strip(&lt).clone(), rt.clone()), (rp.clone(), strip(&rt).clone(), lt.clone())] {
                    if let Some(p) = p {
                        if t.is_optional() && *strip(&other) == null_t() {
                            let (mut yes, mut no) = (cur.clone(), cur.clone());
                            if op == "==" {
                                no.present.insert(p, true);
                            } else {
                                yes.present.insert(p, true);
                            }
                            return (yes, no, true);
                        }
                    }
                }
            }
            // length facts (6.7): the else branch of `i >= p.count()` gives `i < p`; the else branch of `i > p.count()`
            // gives only `i <= p`, which does not permit indexing (D-70)
            if n == "relationalExpr" && lp.is_some() && rp.is_none() && self.is_int(&lt) {
                if let Some(cp) = self.count_receiver(kids[2].node().unwrap()) {
                    let (mut yes, mut no) = (cur.clone(), cur.clone());
                    if op == "<" {
                        yes.below.insert(lp.clone().unwrap(), cp);
                    } else if op == ">=" {
                        no.below.insert(lp.clone().unwrap(), cp);
                    }
                    return (yes, no, true);
                }
            }
            let (li, ri) = (self.interval_of(&lt), self.interval_of(&rt));
            let (li, ri) = match (li, ri) {
                (Some(a), Some(b)) => (a, b),
                _ => return (cur.clone(), cur, false),
            };
            let (mut yes, mut no) = (cur.clone(), cur.clone());
            let nop = iv::negate_op(&op);
            if let Some(lp) = &lp {
                if let Some(a) = iv::narrow(&op, li, ri) {
                    yes.ints.insert(lp.clone(), a);
                }
                if let Some(b) = iv::narrow(nop, li, ri) {
                    no.ints.insert(lp.clone(), b);
                }
            }
            if let Some(rp) = &rp {
                let fop = iv::flip_op(&op);
                if let Some(a) = iv::narrow(fop, ri, li) {
                    yes.ints.insert(rp.clone(), a);
                }
                if let Some(b) = iv::narrow(iv::negate_op(fop), ri, li) {
                    no.ints.insert(rp.clone(), b);
                }
            }
            return (yes, no, lp.is_some() || rp.is_some());
        }
        // no narrowing form: the condition still runs, so both branches see its effects (D-73); its diagnostics were
        // reported when the condition was typed
        let before = self.nrecords();
        self.infer_c(C::N(cond), env, None);
        self.truncate_records(before);
        let cur = std::mem::replace(&mut self.facts, base);
        (cur.clone(), cur, false)
    }

    /// The collection place of `p.count()`.
    pub fn count_receiver(&self, e: N<'a>) -> Option<String> {
        let pe = e.find("postfixExpr")?;
        if e.ntokens() != pe.ntokens() {
            return None;
        }
        let kids = pe.kids();
        if kids.len() < 2 {
            return None;
        }
        let last = kids.last().unwrap().node()?;
        let mc = last.find("methodCall")?;
        if mc.find("memberName")?.tokens()[0].text != "count" || mc.find("argList").is_some() {
            return None;
        }
        if kids.len() == 2 {
            return self.path_of_expr(kids[0].node());
        }
        let mut root = self.path_of_expr(kids[0].node())?;
        for pf in &kids[1..kids.len() - 1] {
            let k = pf.node().map(|p| p.kids()).unwrap_or_default();
            if !k.is_empty() && k[0].text_is(".") && k.len() > 1 && k[1].is("memberName") {
                root = format!("{root}.{}", k[1].node().unwrap().tokens()[0].text);
            } else {
                return None;
            }
        }
        Some(root)
    }

    // ---- ownership (7.2-7.6, 5.7) ----------------------------------------------------------
    pub fn own_err(&mut self, code: &str, msg: String, tok: &Token, place: &str, conflict: &str, rule: &str, region: &str) {
        self.rec(code, msg, rule, tok, Json::obj([("place", Json::str(place)), ("conflict", Json::str(conflict)), ("spans", Json::str(place)), ("loan_region", Json::str(region))]));
    }

    /// The origin of a `??` operand that is a place read through a loan, written bare: a read or `editable`
    /// parameter, an `alias` capture, a binding that holds a loan, or `self` in a method that borrows it.
    pub fn lent_operand(&self, node: Option<N<'a>>, env: &Env) -> Option<String> {
        let n = node?;
        let path = self.path_of_expr(Some(n))?;
        if n.find("markerExpr").is_some() {
            return None;
        }
        let root = root_of(&path);
        let kind = self.binding_kind(&path, env).unwrap_or_default();
        if ["param", "editable", "capture-alias"].contains(&kind.as_str()) || self.own.holders.contains(&root) {
            return Some(root);
        }
        if root == "self" && kind == "self" && (self.self_mode == "self" || self.self_mode == "editable self") {
            return Some(root);
        }
        None
    }

    /// The type of `a ?? b`: `a`'s present type, and a loan of it when the result can be a loan (D-341): `a` lends,
    /// the fallback is a loan (a nested lent `??`, at any depth), or either operand is a place read through a loan.
    /// An owned value is then made from it only by `copy`, wherever it meets an owned expectation.
    pub fn coalesce_result(&mut self, inner: &Type, right: &Type, left_node: Option<N<'a>>, right_node: Option<N<'a>>, env: &Env) -> Type {
        let merged = self.with_closure(inner, right);
        let inner = &merged;
        if inner.is_loan() || !self.is_bearing(inner) {
            return inner.clone();
        }
        if let Type::Loan { mode, origin, .. } = right {
            return Type::Loan { inner: Box::new(inner.clone()), mode: mode.clone(), origin: origin.clone() };
        }
        match self.lent_operand(left_node, env).or_else(|| self.lent_operand(right_node, env)) {
            Some(origin) => Type::Loan { inner: Box::new(inner.clone()), mode: "alias".into(), origin },
            None => inner.clone(),
        }
    }

    /// Whether `e`, of type `t`, is a bearing value of its own where it stands: not a loan, nor a place read through
    /// one (a fresh value, `move p`, `copy p`, a bare owned place). The part of a `??`, a tuple, an `if` or a `match`
    /// that makes its binding own something beside the loans it holds (7.9; D-366, D-369).
    pub fn owns_value(&self, e: Option<N<'a>>, t: &Type, env: &Env) -> bool {
        self.is_bearing(t) && Self::lent(t).is_none() && self.lent_operand(e, env).is_none()
    }

    /// Whether the value of `e` owns a bearing part beside a loan it holds (`owned_parts`), looking through the
    /// wrappers of one expression and parentheses.
    pub fn has_owned_part(&self, e: Option<N<'a>>) -> bool {
        let mut n = match e {
            Some(n) => n,
            None => return false,
        };
        loop {
            if self.owned_parts.contains(&n.id()) {
                return true;
            }
            let kids = n.kids();
            if n.name() == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") {
                match kids[1].node() {
                    Some(k) => n = k,
                    None => return false,
                }
            } else if kids.len() == 1 {
                match kids[0].node() {
                    Some(k) => n = k,
                    None => return false,
                }
            } else {
                return false;
            }
        }
    }

    /// Record that `node`'s value, of type `t`, owns a bearing part beside the loans it holds (7.9).
    pub fn note_owned_part(&mut self, node: N<'a>, t: &Type, owned: bool) {
        if owned && (t.is_loan() || self.holds_loan(t)) {
            self.owned_parts.insert(node.id());
        }
    }

    /// The loans a `give` or `recover` value carries to the expression it ends (7.3; D-355): those its calls carry,
    /// and those held by the bindings it reads (a pattern binding of the arm, a binding of the branch), as
    /// `keep_nested` passes them to a binding. They are handed to the expression's consumer when the expression ends.
    pub fn handed_on(&self, e: Option<N<'a>>, t: &Type) -> Vec<Loan> {
        let mut out = self.carried.clone();
        let e = match e {
            Some(e) => e,
            None => return out,
        };
        if t.is_loan() || self.holds_loan(t) {
            let names: BTreeSet<String> = e.tokens().iter().filter(|tk| tk.kind == Kind::Lower).map(|tk| tk.text.clone()).collect();
            for l in &self.own.loans {
                let ld = l.borrow();
                if let Some(h) = &ld.holder {
                    if names.contains(h) && root_of(&ld.owner) != *h {
                        let via = ld.via.clone().or_else(|| if ld.mode == "alias" { Some(h.clone()) } else { None });
                        out.push(Rc::new(RefCell::new(LoanData { owner: ld.owner.clone(), mode: ld.mode.clone(), holder: None, depth: self.depth, line: ld.line, nested: true, via, parent: ld.parent.clone() })));
                    }
                }
            }
            let line = e.tokens().first().map_or(0, |t| t.line);
            for h in names.intersection(&self.part_owners) {
                // a loan of what the binding owns (D-369)
                out.push(Rc::new(RefCell::new(LoanData { owner: h.clone(), mode: "alias".into(), holder: None, depth: self.depth, line, nested: true, via: None, parent: None })));
            }
        }
        out
    }

    /// The expression has ended: the loans its branches handed on are carried to its consumer (7.3; D-355).
    pub fn take_handed(&mut self, loans: Vec<Loan>) {
        for l in loans {
            if !self.own.has_loan(&l) {
                self.own.loans.push(l.clone());
            }
            if !self.carried.iter().any(|c| loan_eq(c, &l)) {
                self.carried.push(l);
            }
        }
    }

    /// The bare owned place `p` of a `p ?? v` whose result `e` is, through the wrappers of one expression, parentheses
    /// and `present(…)`; None otherwise. Where `e` is given to a position that takes ownership, `p` needs the marker
    /// `let x = p` would (7.5; D-357).
    pub fn coalesced_place(&self, e: Option<N<'a>>) -> Option<String> {
        let mut n = e?;
        loop {
            if let Some(p) = self.coalesced_places.get(&n.id()) {
                return Some(p.clone());
            }
            let kids = n.kids();
            if n.name() == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") {
                n = kids[1].node()?;
            } else if n.name() == "primaryExpr" && kids.len() == 4 && kids[0].text_is("present") {
                n = kids[2].node()?;
            } else if kids.len() == 1 {
                n = kids[0].node()?;
            } else {
                return None;
            }
        }
    }

    /// E-OWN-001 where a `p ?? v` whose value may be the owned place `p`'s is given to `what`, a position that takes
    /// ownership (7.5; D-357).
    pub fn coalesced_marker(&mut self, e: Option<N<'a>>, t: &Type, what: &str) {
        let Some(lp) = self.coalesced_place(e) else { return };
        if self.is_bearing(t) {
            let tok = ftok(e.unwrap()).clone();
            self.own_err(
                "E-OWN-001",
                format!("{what} `{lp} ?? …`: `{lp}` is an existing ownership-bearing place and the result is owned, so the value would have two owners: write `move {lp}` or `copy {lp}`"),
                &tok,
                &lp,
                "marker required",
                "LANG-07-01",
                "",
            );
        }
    }

    /// The function may drop an owned value of type `t` here (a binding's end, an `owned` parameter, a temporary, a
    /// discarded value): a class's `cleanup` contributes its effects to every function that may drop its type (11.3;
    /// D-393). A loan destroys nothing (7.9), so what is lent is not charged (D-400).
    pub fn charge_drop(&mut self, t: &Type, tok: &Token) {
        if t.is_loan() {
            return;
        }
        let mut s = strip(t).clone();
        while let Type::Optional(i) = s {
            s = (*i).clone();
            if s.is_loan() {
                return;
            }
        }
        match &s {
            Type::Tuple(items) => {
                for x in items {
                    self.charge_drop(x, tok);
                }
                return;
            }
            Type::Record(fs) => {
                for (_, x) in fs {
                    self.charge_drop(x, tok);
                }
                return;
            }
            _ => {}
        }
        let cd = if known(&s) { self.decl_of(&s) } else { None };
        if let Some(cd) = cd {
            if cd.has_cleanup {
                self.requested.borrow_mut().insert(format!("cleanup {}", cd.name));
                let sm = self.summaries.get(&format!("cleanup {}", cd.name)).cloned().unwrap_or_default();
                self.eff.imports(&sm, Some(tok), &[]);
            }
        }
    }

    /// The type of a field read through a loan: a loan of the field, in the loan's mode and origin, since a field
    /// reached through a loan is lent too (7.9), where the receiver is a loan and the field bearing (D-356). A trivial
    /// field is its value.
    pub fn lent_member(&self, recv: &Type, r: Type) -> Type {
        let (mode, origin) = match recv {
            Type::Loan { mode, origin, .. } => (mode.clone(), origin.clone()),
            _ => return r,
        };
        if !known(&r) {
            return r;
        }
        let (core, opt) = match &r {
            Type::Optional(i) => ((**i).clone(), true),
            _ => (r.clone(), false),
        };
        if core.is_loan() || !self.is_bearing(&core) {
            return r;
        }
        let lent = Type::Loan { inner: Box::new(core), mode, origin };
        if opt { optional(lent) } else { lent }
    }

    /// Whether a method's result lends from its receiver: a loan `from self`, whole or inside an optional, a tuple or
    /// a record (`(alias T from self)?`, `ListCursor[T] from self`).
    pub fn lends_from_self(sig: &Sig<'a>) -> bool {
        if let Type::Loan { origin, .. } = &sig.ret {
            return origin == "self";
        }
        let mut origins = Vec::new();
        Self::nested_origins(&sig.ret, true, &mut origins);
        origins.iter().any(|(o, _)| o == "self")
    }

    /// Whether `t` is a standard cursor type (5.3), seen through any loan or optional around it. A binding that holds
    /// a cursor owns the value (7.7), so a `let mut` cursor binding may be reassigned; the loan the old cursor stored
    /// ends with it (7.9; D-401).
    pub fn cursor_type(t: &Type) -> bool {
        let mut s = strip(t);
        while let Type::Optional(i) = s {
            s = i;
        }
        s.is_named("ListCursor")
    }

    /// A loan-carrying value (7.7), the standard cursors included: it is owned by its binding, and what it lends is
    /// its origin's, so a loan drawn through it outlives the value itself (7.9).
    pub fn loan_carrying_or_cursor(&self, t: &Type) -> bool {
        self.loan_carrying(t) || Self::cursor_type(t)
    }

    /// Whether assigning the field path `pl` of the `let mut` binding `b` writes through a read loan: a step of the path
    /// before the field assigned reaches a lent value (`g.n = 9` on `let mut g = first(alias xs)`, or on a narrowed
    /// `let mut t = xs.get(0)`), which would change what its owner lends (7.3; D-356). Assigning a field that itself
    /// holds a loan (`rec.item = …` on a record the binding owns) replaces the loan and writes through none (7.9).
    pub fn writes_through_read_loan(&mut self, pl: N<'a>, b: &Binding) -> bool {
        let mut t = b.ty.clone();
        let n_records = self.nrecords();
        let mut out = false;
        for k in pl.kids().into_iter().skip(1) {
            let mut core = t.clone();
            while let Type::Optional(i) = core {
                core = (*i).clone();
            }
            if let Type::Loan { mode, .. } = &core {
                out = mode == "alias";
                break;
            }
            if k.is("memberName") {
                let mt = k.node().unwrap().tokens()[0];
                t = self.member_type(&core, mt);
            } else if let C::T(st) = k {
                if st.kind != Kind::Int {
                    continue;
                }
                match &core {
                    Type::Tuple(items) => {
                        let i: usize = st.text.parse().unwrap_or(usize::MAX);
                        if i >= items.len() {
                            break;
                        }
                        t = items[i].clone();
                    }
                    _ => break,
                }
            } else {
                continue;
            }
            if !known(&t) {
                break;
            }
        }
        self.truncate_records(n_records); // the place was typed already; this walk reports nothing
        out
    }

    /// A value of the function's own where the result is declared a loan (7.6; D-373, D-377, D-381). A `from` result
    /// is a loan, so the expression that produces it must lend: what does not would be held by the caller as a loan
    /// of what ends with the call, as a loan of a local or a temporary returned is: E-OWN-051.
    pub fn owned_into_lent(&mut self, have: &Type, ret: Option<&Type>, e: N<'a>, env: &Env) {
        let Some(owned) = ret.and_then(Self::lent) else { return };
        let Some(what) = self.unlent_in(e, ret.unwrap(), env, Some(have)) else { return };
        self.own_err(
            "E-OWN-051",
            format!("the result is declared as a loan, but `{what}` is a value of the function's own, which ends with the call: return an owned value from a function returning `{}` (bind the loan first, `let v = …`, then `return copy v ?? …`), or `match` the loan", owned.show()),
            ftok(e),
            &what,
            "owned value in a lent result",
            "LANG-07-06",
            "",
        );
    }

    /// The first part of `e` that is a value of the function's own where `want`, a result declared `from` a
    /// parameter, is a loan, as the source spells it; None when the expression lends (7.6; D-381). What lends: a
    /// place read through a loan (a parameter, `self`, a binding that holds a loan, a field or element path through
    /// one), `alias p`, a call whose result lends, a `??` both of whose sides lend, an `if` or `match` every one of
    /// whose results lends, an outcome expression whose value side and every `recover` lend (D-385), `present` of
    /// one (an optional holds nothing of its own), parentheses, and a string literal without interpolation where a
    /// view is declared, a view of static storage (6.6). What does not: an interpolated string, a literal where a
    /// `String` loan is declared (it converts by allocating; D-386), a tuple or record literal, a constructor,
    /// `move x`, `copy x`, a call whose result is owned, a lambda, an arithmetic or string result. A trivial value
    /// holds nothing and is exempt. `t` is `e`'s type where the caller has it; otherwise the type recorded while `e`
    /// was inferred says.
    fn unlent_in(&self, e: N<'a>, want: &Type, env: &Env, t: Option<&Type>) -> Option<String> {
        const WRAP: &[&str] = &[
            "constExpr", "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr", "asExpr", "unaryExpr", "postfixExpr", "primaryExpr", "exprStmt",
        ];
        const OPS: &[&str] = &["orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr"];
        let mut w = want;
        while let Type::Optional(i) = w {
            w = i;
        }
        if !w.is_loan() {
            return None; // an owned result: a loan there is loan_into_owned's
        }
        let mut n = e;
        loop {
            let kids = n.kids();
            if WRAP.contains(&n.name()) && kids.len() == 1 && kids[0].node().is_some() {
                n = kids[0].node()?;
            } else if n.name() == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") {
                n = kids[1].node()?;
            } else {
                break;
            }
        }
        let kids = n.kids();
        match n.name() {
            "outcomeExpr" if kids.len() > 1 => {
                // an outcome expression produces its value side, or what its handler recovers, so both must lend;
                // `or fail`, and a handler that fails or returns, produce nothing (10.2; D-385)
                let hit = self.unlent_in(kids[0].node()?, want, env, None);
                if hit.is_some() {
                    return hit;
                }
                if kids.iter().any(|k| k.text_is("recover")) {
                    return self.unlent_in(kids.last()?.node()?, want, env, None);
                }
                if kids.iter().any(|k| k.text_is("catch")) {
                    return self.unlent_recovered(n.child_node("block")?, want, env);
                }
                return None;
            }
            "coalesceExpr" if kids.len() == 3 => {
                return self.unlent_in(kids[0].node()?, want, env, None).or_else(|| self.unlent_in(kids[2].node()?, want, env, None));
            }
            "primaryExpr" if kids.len() == 4 && kids[0].text_is("present") => return self.unlent_in(kids[2].node()?, want, env, None),
            "matchExpr" => {
                for arm in n.child_nodes("matchArm") {
                    let body = arm.kids().into_iter().filter_map(|c| c.node()).filter(|c| c.name() == "expr" || c.name() == "block").last()?;
                    let hit = if body.name() == "expr" { self.unlent_in(body, want, env, None) } else { self.unlent_given(body, want, env) };
                    if hit.is_some() {
                        return hit;
                    }
                }
                return None;
            }
            "ifExpr" => {
                for c in kids.iter().filter_map(|c| c.node()) {
                    // a condition decides nothing
                    let hit = match c.name() {
                        "block" => self.unlent_given(c, want, env),
                        "ifExpr" => self.unlent_in(c, want, env, None),
                        _ => None,
                    };
                    if hit.is_some() {
                        return hit;
                    }
                }
                return None;
            }
            _ => {}
        }
        let recorded;
        let t = match t {
            Some(t) => Some(t),
            None => {
                recorded = self.recorded_type(e);
                recorded.as_ref()
            }
        };
        // a `try` call produces its success value
        let t = t
            .and_then(|t| match strip(t) {
                Type::Named(nt) if nt.package.is_empty() && nt.name == "Outcome" && !nt.args.is_empty() => Some(&nt.args[0]),
                _ => None,
            })
            .or(t);
        if n.name() == "literal" {
            if let Some(tok) = kids.first().and_then(|k| k.tok()) {
                if matches!(tok.kind, Kind::Str | Kind::Raw | Kind::Bytes) {
                    // text: a literal without interpolation is a view of static storage, which lends where a view is
                    // declared (6.6); an interpolation allocates a fresh String, and a literal converts to one where
                    // a `String` loan is declared: the function's own (D-386)
                    let mut core = want;
                    loop {
                        match core {
                            Type::Optional(i) => core = i,
                            Type::Loan { inner, .. } => core = inner,
                            _ => break,
                        }
                    }
                    // "without interpolation" is decided by the lexer's recorded holes, as `lower_names` decides it:
                    // an escaped `\${` opens no hole and prints literally on every backend (D-412)
                    let is_static = tok.kind != Kind::Str || expr::hole_sources(tok).is_empty();
                    if is_static && matches!(strip(core), Type::Named(nt) if nt.package.is_empty() && ["StringView", "BytesView"].contains(&nt.name.as_str())) {
                        return None;
                    }
                    return Some(source_text(n));
                }
            }
        }
        if OPS.contains(&n.name()) && kids.len() > 1 {
            // computed here: a trivial result holds nothing, any other is the function's own
            return match t {
                Some(t) if !self.dc.is_trivial(strip(t)) => Some(source_text(n)),
                _ => None,
            };
        }
        if t.map_or(false, |t| !self.is_bearing(&Self::lent(t).unwrap_or_else(|| t.clone()))) {
            return None; // trivial, a literal or unknown: it holds nothing
        }
        if matches!(n.name(), "tupleLit" | "recordLit" | "lambda") {
            return Some(source_text(n)); // constructed here, whatever its parts
        }
        if let Some(path) = self.path_of_expr(Some(n)) {
            if self.part_owners.contains(&root_of(&path)) {
                // the binding owns a part of what it holds (a `??` fallback, a tuple's own element, a branch's own
                // value): its value is the function's own on that path, lent or not (7.9; D-369, D-388, D-389)
                return Some(source_text(n));
            }
        }
        let t = t?;
        if Self::lent(t).is_some() || self.lent_operand(Some(n), env).is_some() {
            return None;
        }
        Some(source_text(n))
    }

    /// `unlent_in` over every `recover` of the handler `node`, at any depth of its statements: not one of a nested
    /// `catch`, whose handler recovers its own value, nor one inside a lambda (10.2; D-385).
    fn unlent_recovered(&self, node: N<'a>, want: &Type, env: &Env) -> Option<String> {
        for c in node.kids().into_iter().filter_map(|c| c.node()) {
            let hit = if c.name() == "statement" && c.kid(0).map_or(false, |k| k.text_is("recover")) {
                c.child_node("expr").and_then(|e| self.unlent_in(e, want, env, None))
            } else if c.name() == "lambda" || (c.name() == "outcomeExpr" && c.kids().iter().any(|k| k.text_is("catch"))) {
                None
            } else {
                self.unlent_recovered(c, want, env)
            };
            if hit.is_some() {
                return hit;
            }
        }
        None
    }

    /// `unlent_in` over every `give` that ends the arm or branch `node` is in, not one nested in an inner `if` or
    /// `match` expression (5.5).
    fn unlent_given(&self, node: N<'a>, want: &Type, env: &Env) -> Option<String> {
        for c in node.kids().into_iter().filter_map(|c| c.node()) {
            let hit = if c.name() == "statement" && c.kid(0).map_or(false, |k| k.text_is("give")) {
                c.child_node("expr").and_then(|e| self.unlent_in(e, want, env, None))
            } else if !matches!(c.name(), "matchExpr" | "ifExpr" | "lambda") {
                self.unlent_given(c, want, env)
            } else {
                None
            };
            if hit.is_some() {
                return hit;
            }
        }
        None
    }

    /// Start recording the types of the expressions inferred next when the result they produce is declared a loan,
    /// for `unlent_in`; true when this call started it, so its caller stops it. A recording already running (a
    /// lowering's, or an enclosing result's) is kept.
    pub fn lends_result(&mut self, ret: Option<&Type>) -> bool {
        if self.node_types.is_some() || ret.and_then(Self::lent).is_none() {
            return false;
        }
        self.node_types = Some(HashMap::new());
        true
    }

    /// The type recorded for `e` while it was inferred, or for the one expression it wraps; None when none was.
    fn recorded_type(&self, e: N<'a>) -> Option<Type> {
        let recorded = self.node_types.as_ref()?;
        let mut n = e;
        loop {
            if let Some(t) = recorded.get(&n.id()) {
                return Some(t.clone());
            }
            let kids = n.kids();
            if kids.len() != 1 {
                return None;
            }
            n = kids[0].node()?;
        }
    }

    /// What a loan lends, as the owned value a copy of it is: `alias T` gives `T` and `(alias T)?` gives `T?`; None for
    /// a type that is not a loan.
    pub fn lent(t: &Type) -> Option<Type> {
        match t {
            Type::Loan { inner, .. } => Some((**inner).clone()),
            Type::Optional(i) => Self::lent(i).map(|x| Type::Optional(Box::new(x))),
            Type::Tuple(items) => {
                let ls: Vec<Option<Type>> = items.iter().map(Self::lent).collect();
                if ls.iter().all(|l| l.is_none()) {
                    return None;
                }
                Some(Type::Tuple(items.iter().zip(ls).map(|(x, l)| l.unwrap_or_else(|| x.clone())).collect()))
            }
            Type::Record(fs) => {
                let ls: Vec<Option<Type>> = fs.iter().map(|(_, x)| Self::lent(x)).collect();
                if ls.iter().all(|l| l.is_none()) {
                    return None;
                }
                Some(Type::Record(fs.iter().zip(ls).map(|((k, x), l)| (k.clone(), l.unwrap_or_else(|| x.clone()))).collect()))
            }
            _ => None,
        }
    }

    /// A loan where an owned value is expected (a return, an annotated binding, an assignment, an `owned` argument or
    /// field, a payload, a recovered value) stays a loan (X-95): an owned value is made from it only by
    /// `copy` (7.5, 7.6; D-341). A trivial value needs no marker. Without one, a copyable value is E-OWN-001 (write
    /// `copy`); a value that is not Copyable has no owned copy at all, E-OWN-002. `want` is the expected type, or None
    /// where the position itself is owned; a loan or a view expected is lent on.
    pub fn loan_into_owned(&mut self, have: &Type, want: Option<&Type>, tok: &Token, what: &str) {
        let Some(lent) = Self::lent(have) else { return };
        if let Some(mut w) = want {
            while let Type::Optional(i) = w {
                w = i;
            }
            match w {
                Type::Loan { .. } | Type::Unknown(_) => return,
                Type::Named(n) if (n.package.is_empty() && ["StringView", "BytesView", "ListView", "ListCursor"].contains(&n.name.as_str())) => return,
                _ => {}
            }
        }
        let mut core = &lent;
        while let Type::Optional(i) = core {
            core = i;
        }
        if matches!(core, Type::Unknown(_)) || self.dc.is_trivial(&lent) || self.copy_refused.contains(&(tok.line, tok.col)) {
            return;
        }
        if matches!(core, Type::Named(n) if (n.package.is_empty() && ["StringView", "BytesView", "ListView", "ListCursor"].contains(&n.name.as_str()))) {
            return; // a view held whole is a stored loan (E-OWN-053, E-OWN-090), not a copy
        }
        let place = tok.text.clone();
        if self.dc.is_copyable(&lent) {
            self.own_err(
                "E-OWN-001",
                format!("{what} must be owned, but it is a loan of {}: write `copy` to make an owned copy (a call's result is bound first, `let v = …`, then `copy v`)", lent.show()),
                tok,
                &place,
                "copy required",
                "LANG-07-01",
                "",
            );
            return;
        }
        let dup = if matches!(core, Type::Named(n) if (n.package.is_empty() && n.name == "Shared") || (n.package.is_empty() && n.name == "Weak")) { " (a `Shared` or `Weak` is duplicated with `retain()`)" } else { "" };
        self.own_err(
            "E-OWN-002",
            format!("{what} must be owned, but it is a loan of {}, which is not Copyable (6.11), so no owned copy can be made{dup}: keep the loan (a read parameter or a `from` result), or take the value out of its owner", lent.show()),
            tok,
            &place,
            "loan where an owned value is required",
            "LANG-07-01",
            "",
        );
    }

    pub fn is_bearing(&self, t: &Type) -> bool {
        if t.is_loan() {
            return false;
        }
        let base = base_type(strip(t));
        let s = &base;
        if matches!(s, Type::TypeParam(_)) {
            return true; // a type parameter is never trivial: a loan of a `T` stays a loan (6.12, X-513; D-352)
        }
        if !known(s) || is_strlit(s) || is_interp(s) {
            return false;
        }
        if let Type::Named(n) = s {
            if n.package.is_empty() && ["StringView", "BytesView", "ListView"].contains(&n.name.as_str()) {
                return false;
            }
        }
        !self.dc.is_trivial(s)
    }

    /// Whether `editable p` may be written: `p` rooted in a `let mut` binding, an `editable` parameter, an `editable`
    /// or `move` capture, a binding that holds an exclusive loan, or the method's own `editable self`.
    pub fn editable_root(&self, path: &str, env: &Env) -> bool {
        let kind = self.binding_kind(path, env).unwrap_or_default();
        let holder = self.own.holders.get(&root_of(path)).cloned();
        ["mut", "editable", "capture-editable", "capture-move", "capture-probe"].contains(&kind.as_str())
            || holder.as_ref().map_or(false, |h| h.borrow().mode == "editable")
            || (root_of(path) == "self" && self.self_mode == "editable self")
    }

    pub fn binding_kind(&self, path: &str, env: &Env) -> Option<String> {
        let r = root_of(path);
        match env_lookup(env, &r) {
            Some(b) => Some(b.kind),
            None => {
                if r == "self" {
                    Some("self".into())
                } else {
                    None
                }
            }
        }
    }

    /// The status of the record just made (it was deduplicated away if the last record is another).
    pub fn set_last_status(&mut self, status: &'static str) {
        if let Some(r) = self.dc.records.last_mut() {
            if r.code.starts_with("E-EFF") {
                r.status = status;
            }
        }
    }

    pub fn eff_err(&mut self, code: &str, msg: String, tok: &Token, required: &str, effect: &str, witness: &str, rule: &str) {
        self.rec(code, msg, rule, tok, Json::obj([("required_contract", Json::str(required)), ("violating_effect", Json::str(effect)), ("witness_path", Json::str(witness))]));
    }

    pub fn provenance(&self, path: &str, env: &Env) -> String {
        let root = root_of(path);
        if root == "self" {
            return if self.self_mode == "editable self" { "footprint:self".into() } else { "none".into() };
        }
        if let Some(h) = self.own.holders.get(&root) {
            let owner = h.borrow().owner.clone();
            if root_of(&owner) != root {
                return self.provenance(&owner, env);
            }
        }
        let b = match env_lookup(env, &root) {
            Some(b) => b,
            None => return "none".into(),
        };
        if b.kind == "editable" {
            return format!("footprint:{root}");
        }
        if b.kind.starts_with("capture-") {
            return if b.kind == "capture-editable" { "mutate".into() } else { "fresh".into() };
        }
        if self.fresh.contains(&root) || ["mut", "let", "owned", "for", "pattern", "pattern-loan"].contains(&b.kind.as_str()) {
            return "fresh".into();
        }
        "mutate".into()
    }

    pub fn note_write(&mut self, path: &str, tok: &Token, env: &Env) {
        let prov = self.provenance(path, env);
        if let Some(p) = prov.strip_prefix("footprint:") {
            self.eff.footprint.insert(p.to_string());
        } else if prov == "mutate" {
            self.eff.add("mutate", "proved", Some(tok));
        }
    }

    pub fn import_call(&mut self, callee: &Summary, tok: &Token, receiver_path: Option<String>, arg_places: &[(String, String)], env: &Env, is_async_callee: bool) {
        let skip: &[&str] = if is_async_callee { &["suspend", "fail"] } else { &["fail"] };
        self.eff.imports(callee, Some(tok), skip);
        for fp in callee.footprint.clone() {
            let place = if fp == "self" { receiver_path.clone() } else { arg_places.iter().find(|(p, _)| *p == fp).map(|(_, pl)| pl.clone()) };
            if let Some(place) = place {
                self.note_write(&place, tok, env);
            }
        }
    }

    /// The summary label of a call of function `d`: `function f` for the module's own function, which a unit of the
    /// module may have summarised; qualified by its module for another module's, whose summary its modifiers give
    /// (SP-12: the labels of two modules never meet).
    pub fn function_label(&self, d: &DeclRef<'a>) -> String {
        if d.module == self.module || crate::decls::is_standard(&d.module) || d.kind == "unknown" {
            format!("function {}", d.name)
        } else {
            format!("function {}::{}", d.module, d.name)
        }
    }

    /// As `function_label`, for method `m` of type `d`.
    pub fn method_label(&self, d: &DeclRef<'a>, m: &str) -> String {
        if d.module == self.module || crate::decls::is_standard(&d.module) || d.kind == "unknown" {
            format!("method {}.{m}", d.name)
        } else {
            format!("method {}::{}.{m}", d.module, d.name)
        }
    }

    fn derived_conversion_summary(&self, d: &DeclRef<'a>, sig: &Sig<'a>, seen: &mut BTreeSet<String>) -> Summary {
        let mut out = Summary::default();
        if d.kind == "enum" { out.add("alloc", "proved", None); return out; }
        let key = format!("{}:{}:{}", d.module, d.name, sig.name);
        if !seen.insert(key) { return out; }
        let Some(source) = self.decl_of(&sig.params[0].1) else { return out };
        for (name, (ft, _, _)) in d.fields.iter() {
            let Some((st, _, _)) = source.fields.get(name) else { continue };
            let st = &self.dc.subst(st, &self.dc.type_substitution(&source, &sig.params[0].1));
            let ft = &self.dc.subst(ft, &self.dc.type_substitution(d, &sig.ret));
            if !self.dc.is_trivial(st) { out.add("alloc", "proved", None); }
            if st == ft { continue; }
            if let Some(owner) = self.decl_of(st).filter(|o| o.has_cleanup) {
                let label = format!("cleanup {}", owner.name);
                self.requested.borrow_mut().insert(label.clone());
                out.imports(&self.summaries.get(&label).cloned().unwrap_or_default(), None, &[]);
            }
            if let (Some(sb), Some(tb)) = (crate::conversions::base_of(st), crate::conversions::base_of(ft)) {
                if self.dc.is_core_type(st) && self.dc.is_core_type(ft) {
                    if let Some(row) = crate::conversions::resolve(sb, tb) {
                        if row.allocates() { out.add("alloc", "proved", None); }
                        continue;
                    }
                }
            }
            for (t, direction, other) in [(st, "to", ft), (ft, "from", st)] {
                let Some(owner) = self.decl_of(t) else { continue };
                for member in self.dc.instantiated_conversions(&owner, t) {
                    if member.direction == direction && member.other == *other && (member.exported || owner.module == d.module) {
                        let sub = if member.derived { self.derived_conversion_summary(&owner, &member.sig, seen) } else { self.callee_summary(Some(&owner), &member.sig, &self.method_label(&owner, &member.sig.name)) };
                        out.imports(&sub, None, &[]);
                    }
                }
            }
        }
        out
    }

    pub fn callee_summary(&self, d: Option<&DeclRef<'a>>, sig: &Sig<'a>, label: &str) -> Summary {
        if let Some(d) = d.filter(|_| sig.name.starts_with("derive:from:")) {
            return self.derived_conversion_summary(d, sig, &mut BTreeSet::new());
        }
        if label == "closure" {
            self.requested.borrow_mut().insert("closure".into());
            return self.summaries.get("closure").cloned().unwrap_or_default();
        }
        if sig.body.is_none() || sig.has_mod("trusted") {
            // the declaration the effect data names the operation by: the owning type of a method or associated
            // function, else the free function itself (an alias call names it; otherwise its label does)
            let owner = d.filter(|d| d.kind != "func");
            let fd = d.filter(|d| d.kind == "func").cloned().or_else(|| {
                if owner.is_some() {
                    return None;
                }
                label.strip_prefix("function ").and_then(|n| self.dc.table.get(n)).cloned()
            });
            let home = owner.cloned().or(fd);
            let auth = owner.map_or(false, |d| self.dc.implements_core_authority(d) || (AUTHORITIES.contains(&d.name.as_str()) && d.module.starts_with("fortis")));
            let mut s = library_summary(home.as_ref().map(|h| h.module.as_str()), owner.map(|d| d.name.as_str()), &sig.name, &sig.modifiers, auth, sig.receiver.as_deref(), &self.authority_params(sig));
            // Callback effects follow the declared type, including pure callbacks' allocation (11.3, X-610).
            for p in &sig.params {
                if let Type::Fn { modifiers, .. } = strip(&p.1) {
                    if !modifiers.iter().any(|m| m == "pure") {
                        for e in ["io", "mutate", "nondet"] {
                            s.add_uncertain(e, "proved", "possible", None);
                        }
                    }
                    if !modifiers.iter().any(|m| m == "noalloc") {
                        s.add_uncertain("alloc", "proved", "possible", None);
                    }
                }
            }
            // a foreign `trusted` function of the program (not the standard library, whose summaries are tabled):
            // what it does is behind a trust boundary except for the claims its signature writes (X-132)
            let user = |d: &DeclRef<'a>| !crate::decls::is_standard(&d.module) && d.kind != "unknown";
            let foreign = sig.has_mod("trusted")
                && match d {
                    Some(d) => user(d),
                    None => label.strip_prefix("function ").map(|n| n.rsplit("::").next().unwrap_or(n)).and_then(|n| self.dc.table.get(n)).map_or(false, |f| f.kind == "func" && user(f)),
                };
            if foreign {
                if !sig.has_mod("pure") {
                    for e in ["io", "mutate", "nondet"] {
                        s.add_uncertain(e, "trusted", "unknown", None);
                    }
                }
                if !sig.has_mod("noalloc") {
                    s.add_uncertain("alloc", "trusted", "unknown", None);
                }
            }
            return s;
        }
        self.requested.borrow_mut().insert(label.to_string());
        self.summaries.get(label).cloned().unwrap_or_else(|| Self::summary_from_modifiers(sig))
    }

    pub fn summary_from_modifiers(sig: &Sig<'a>) -> Summary {
        let mut s = Summary::default();
        if sig.has_mod("try") {
            s.add("fail", "proved", None);
        }
        if sig.has_mod("async") {
            s.add("suspend", "proved", None);
        }
        if sig.has_mod("recovers") {
            s.add("recovers", "proved", None);
        }
        // a function value's effects are those its type permits: possible, not shown to happen (11.3; X-132)
        if !sig.has_mod("pure") {
            for e in ["io", "mutate", "nondet"] {
                s.add_uncertain(e, "proved", "possible", None);
            }
        }
        if !sig.has_mod("noalloc") {
            s.add_uncertain("alloc", "proved", "possible", None);
        }
        s
    }

    pub fn expanded_mods(&self, sig: &Sig<'a>) -> BTreeSet<String> {
        let mut mods: BTreeSet<String> = sig.modifiers.iter().cloned().collect();
        for c in &sig.contracts {
            if let Some(d) = self.dc.table.get(c) {
                if d.kind == "contract" {
                    if let Some(m) = d.items.get("modifiers") {
                        mods.extend(m.iter().cloned());
                    }
                }
            }
        }
        mods
    }

    /// A `Shared[A]` handle carries A's authority (11.6, X-137).
    pub fn unshared<'t>(t: &'t Type) -> &'t Type {
        match strip(t) {
            Type::Named(n) if (n.package.is_empty() && n.name == "Shared") && n.args.len() == 1 => strip(&n.args[0]),
            s => s,
        }
    }

    /// An Authority type (11.6), `Shared[A]` included, recognised by its declaration: a program's own class named like a
    /// capability is not one (review of D-145); a type whose declaration is unknown keeps the name rule.
    pub fn is_authority(&self, t: &Type) -> bool {
        let s = Self::unshared(t);
        match s {
            Type::Named(n) => match self.decl_of(s) {
                Some(d) if d.kind != "unknown" => self.dc.implements_core_authority(&d),
                _ => AUTHORITIES.contains(&n.name.as_str()),
            },
            _ => false,
        }
    }

    pub fn authority_params(&self, sig: &Sig<'a>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for (_, pt, _) in &sig.params {
            if let Type::Named(n) = Self::unshared(pt) {
                if self.is_authority(pt) {
                    out.insert(n.name.clone());
                }
            }
        }
        out
    }

    /// The authorities a method's receiver of type `t` names (11.6; D-173): the receiver's own type when it is an
    /// authority, else each field of its class whose type is an authority or a `Shared` of one, as X-137's owners hold a
    /// `Shared[Console]`. A field of any other type names none: a `Capabilities` record names none, as a `Capabilities`
    /// parameter names none.
    pub fn receiver_authorities(&self, t: &Type) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let s = Self::unshared(t);
        let name = match s {
            Type::Named(n) => n.name.clone(),
            _ => return out,
        };
        if self.is_authority(s) {
            out.insert(name);
            return out;
        }
        if let Some(d) = self.decl_of(s) {
            if d.kind == "class" {
                for (_, (ft, _, _)) in d.fields.iter() {
                    if let (true, Type::Named(f)) = (self.is_authority(ft), Self::unshared(ft)) {
                        out.insert(f.name.clone());
                    }
                }
            }
        }
        out
    }

    /// The body being typed (labelled `label`) is the entry `main` (4.6): the function `main` of the entry module.
    pub fn is_entry_main(&self, label: &str) -> bool {
        label == "function main" && self.dc.entry_module.as_deref() == Some(self.module.as_str())
    }

    pub fn is_entry_main_decl(&self, d: &Decl<'a>) -> bool {
        d.kind == "func" && d.name == "main" && self.dc.entry_module.as_deref() == Some(d.module.as_str())
    }

    /// The entry `main` is never referenced from Fortis code: only the runtime calls it, once, so the capabilities it
    /// creates are created once (11.6, LANG-11-07).
    pub fn entry_main_ref(&mut self, d: &Decl<'a>, tok: &Token) {
        if self.is_entry_main_decl(d) {
            let w = if self.current_label.is_empty() { "a declaration".to_string() } else { self.current_label.clone() };
            self.eff_err(
                "E-EFF-010",
                "the entry `main` is referenced from Fortis code: only the runtime calls it, once, so the capabilities it creates are created once (11.6)".into(),
                tok,
                "no reference to the entry main",
                "io",
                &w,
                "LANG-11-07",
            );
        }
    }

    /// `Host` of `fortis.host`, not a program's own class of that name.
    pub fn is_host(d: &Decl<'a>) -> bool {
        d.name == "Host" && d.kind == "class" && d.module == "fortis.host"
    }

    /// `Host.capabilities` (11.6, LANG-11-07): only a direct call in the entry `main`'s own body, at most once, outside
    /// every loop (its condition included), lambda and aspect hook; never taken as a value.
    pub fn capabilities_use(&mut self, tok: &Token, called: bool) {
        let w = if self.current_label.is_empty() { "a declaration".to_string() } else { self.current_label.clone() };
        if !called {
            self.eff_err("E-EFF-010", "`Host.capabilities` is taken as a value: the capabilities are created only by a direct call in the entry `main` (11.6)".into(), tok, "a direct call in the entry main", "io", &w, "LANG-11-07");
            return;
        }
        if !self.is_entry_main(&w) {
            self.eff_err("E-EFF-010", "`Host.capabilities()` is called outside the entry `main` (11.6)".into(), tok, "the entry main only", "io", &w, "LANG-11-07");
            return;
        }
        let caps_params = self.dc.table.get("main").and_then(|d| d.sig.as_ref().map(|sig| {
            sig.params.iter().any(|(_, t, _)| matches!(t, Type::Named(n) if n.package.is_empty() && fortis_names::reading::CAPABILITY_TYPES.contains(&n.name.as_str())))
        }));
        if caps_params == Some(true) {
            self.eff_err(
                "E-EFF-010",
                "`Host.capabilities()` is called in a `main` that takes capability parameters: the runtime has already created the capabilities, once (4.6, 11.6)".into(),
                tok,
                "no Host.capabilities() beside capability parameters",
                "io",
                &w,
                "LANG-11-07",
            );
            return;
        }
        let has = |c: &str| self.ctx.iter().any(|x| x == c);
        let inside = if has("lambda") {
            Some("a lambda")
        } else if has("hook") {
            Some("an aspect's hook")
        } else if has("loop") || has("whilecond") {
            Some("a loop")
        } else {
            None
        };
        if let Some(inside) = inside {
            self.eff_err(
                "E-EFF-010",
                format!("`Host.capabilities()` is called inside {inside}: the capabilities are created once, by a direct call in the entry `main`'s own body (11.6)"),
                tok,
                "a direct call in the entry main",
                "io",
                &w,
                "LANG-11-07",
            );
            return;
        }
        self.caps_calls += 1;
    }

    pub fn check_contracts(&mut self, sig: &Sig<'a>, head_tok: &Token, label: &str) {
        let params: BTreeSet<String> = sig.params.iter().filter_map(|(_, pt, _)| base_name(Self::unshared(pt)).map(|s| s.to_string())).collect();
        for c in sig.contracts.clone() {
            let d = match self.dc.table.get(&c) {
                Some(d) if d.kind == "contract" => d.clone(),
                _ => {
                    let mut cands: Vec<String> = self.dc.table.decls.iter().filter(|(_, v)| v.kind == "contract").map(|(k, _)| k.clone()).collect();
                    cands.sort();
                    self.rec("E-NAM-001", format!("`{c}` is not a declared contract"), "LANG-11-06", head_tok, Self::nam(&c, cands));
                    continue;
                }
            };
            for f in d.items.get("forbids").cloned().unwrap_or_default() {
                if self.eff.has(&f) {
                    let site = self.eff.sites.get(&f).cloned().unwrap_or_else(|| head_tok.clone());
                    self.eff_err("E-EFF-001", format!("{label} carries contract {c}, which forbids `{f}`, but its body has that effect"), &site, &c, &f, label, "LANG-11-06");
                }
            }
            for a in d.items.get("requires").cloned().unwrap_or_default() {
                if !params.contains(&a) {
                    self.eff_err("E-EFF-001", format!("{label} carries contract {c}, which requires capability {a}, but takes no parameter of that type"), head_tok, &c, "io", label, "LANG-11-06");
                }
            }
            for a in d.items.get("excludes").cloned().unwrap_or_default() {
                if params.contains(&a) {
                    self.eff_err("E-EFF-001", format!("{label} carries contract {c}, which excludes capability {a}, but takes a parameter of that type"), head_tok, &c, "io", label, "LANG-11-06");
                } else if self.eff.authorities.contains(&a) {
                    self.eff_err("E-EFF-001", format!("{label} carries contract {c}, which excludes capability {a}, but calls a function that requires it"), head_tok, &c, "io", label, "LANG-11-06");
                }
            }
        }
    }

    /// A declaration's written modifiers and the contracts it carries must agree, two by two (11.5, E-EFF-050): `pure`
    /// beside a required capability, a modifier beside `forbids` of the effect it permits, an authority both required
    /// and excluded. A contract's own contradiction is reported at the contract.
    pub fn check_contradictions(&mut self, sig: &Sig<'a>, head_tok: &Token, label: &str) {
        let mut parts: Vec<(String, BTreeMap<String, BTreeSet<String>>)> = vec![(String::new(), [("modifiers".to_string(), sig.modifiers.iter().cloned().collect())].into_iter().collect())];
        for c in &sig.contracts {
            if let Some(cd) = self.dc.table.get(c).filter(|cd| cd.kind == "contract") {
                parts.push((c.clone(), cd.items.clone()));
            }
        }
        let side = |o: &str| if o.is_empty() { "its own modifiers".to_string() } else { format!("contract {o}") };
        for c in crate::effects::contract_contradictions(&parts, true) {
            let msg = match c.kind {
                "pure" => format!("{label} is `pure` (by {}) and requires capability {} (by {}): a pure function receives no authority, so the declaration contradicts itself (11.5)", side(&c.first), c.claim, side(&c.second)),
                "forbids" => format!("{label} is `{}` (by {}), which permits `{}`, and forbids `{}` (by {}): the declaration contradicts itself (11.5)", c.claim, side(&c.first), c.effect, c.effect, side(&c.second)),
                _ => format!("{label} requires capability {} (by {}) and excludes it (by {}): the declaration contradicts itself (11.5)", c.claim, side(&c.first), side(&c.second)),
            };
            self.eff_err("E-EFF-050", msg, head_tok, &c.second, &c.effect, label, "LANG-11-06");
        }
    }

    pub fn check_claims(&mut self, sig: &Sig<'a>, head_tok: &Token, label: &str) {
        let mods = self.expanded_mods(sig);
        self.check_contracts(sig, head_tok, label);
        self.check_contradictions(sig, head_tok, label);
        let site = |s: &Self, e: &str| s.eff.sites.get(e).cloned().unwrap_or_else(|| head_tok.clone());
        // the authority must be in this function's own signature (11.6): one a callee takes, reached through a value that
        // is not an Authority (`caps.clock` of a Capabilities), does not count; a lambda acts under its enclosing
        // function's (D-144)
        let mut own = if label == "lambda" { self.eff.authorities.clone() } else { self.authority_params(sig) };
        if label != "lambda" && sig.receiver.is_some() {
            if let Some(st) = self.self_type.clone() {
                own.extend(self.receiver_authorities(&st));
            }
        }
        if label != "lambda" {
            // a function that owns a value of a class holding an authority may drop it, and its `cleanup` acts under that
            // authority (11.6; D-173): an `owned` parameter carries what its class holds, as a receiver does (D-393, X-495)
            for (_, pt, pm) in &sig.params {
                if pm.trim_start_matches('.') == "owned" {
                    let mut core = strip(pt);
                    while let Type::Optional(i) = core {
                        core = i;
                    }
                    own.extend(self.receiver_authorities(core));
                }
            }
        }
        if self.eff.needs_auth && own.is_empty() && !self.is_entry_main(label) {
            let t = site(self, "io");
            self.eff_err(
                "E-EFF-010",
                format!("{label} performs I/O but takes no Authority value: a capability or per-activation authority must be a parameter (11.6)"),
                &t,
                "an Authority parameter",
                "io",
                label,
                "LANG-11-07",
            );
        }
        if self.is_entry_main(label) && self.caps_calls > 1 {
            self.eff_err("E-EFF-010", "`Host.capabilities()` is called more than once in main: the capabilities are created once (11.6)".into(), head_tok, "one Host.capabilities() call", "io", label, "LANG-11-07");
        }
        if mods.contains("pure") && label != "lambda" {
            // a `pure` function may not take a capability parameter at all (11.5): the claim rules out the I/O it is for (D-145)
            for a in self.authority_params(sig) {
                self.eff_err("E-EFF-001", format!("{label} is declared `pure` but takes the capability {a}: a pure function receives no authority (11.5)"), head_tok, "pure", "io", label, "LANG-11-06");
            }
        }
        // the record's status says how certain the effect is (PROG-DIA-02, X-132)
        let why = |status: &str| match status {
            "unproved" => ": a callback's type permits it, so it may happen (write the claim in the function type)",
            "unknown" => ": it comes through a foreign `trusted` function that does not claim its absence, so it is behind a trust boundary",
            _ => "",
        };
        for effect in crate::effects::PURE_FORBIDS {
            if mods.contains("pure") && self.eff.has(effect) {
                let t = site(self, effect);
                let status = self.eff.status(effect);
                let verb = if status == "violated" { "has" } else { "may have" };
                self.eff_err("E-EFF-001", format!("{label} is declared `pure` but its body {verb} the `{effect}` effect{}", why(status)), &t, "pure", effect, label, if *effect == "mutate" { "LANG-11-02" } else { "LANG-11-01" });
                self.set_last_status(status);
            }
        }
        if mods.contains("noalloc") && self.eff.has("alloc") {
            let t = site(self, "alloc");
            let status = self.eff.status("alloc");
            let verb = if status == "violated" { "allocates" } else { "may allocate" };
            let call = if status == "unproved" { format!(" at call `{}`", t.text) } else { String::new() };
            self.eff_err("E-EFF-001", format!("{label} is declared `noalloc` but its body {verb}{call}{}", why(status)), &t, "noalloc", "alloc", label, "LANG-11-03");
            self.set_last_status(status);
        }
        let ef = |req: &str, eff: &str| Json::obj([("required_contract", Json::str(req)), ("violating_effect", Json::str(eff)), ("witness_path", Json::str(label))]);
        if self.eff.has("suspend") && !mods.contains("async") {
            let t = site(self, "suspend");
            let c = if label != "lambda" { self.insert_modifier("declare the effect", "async", &["try", "recovers", "func"]) } else { None };
            self.rec_c("E-EFF-001", format!("{label} suspends (`await`) but is not declared `async`"), "LANG-11-05", &t, ef("async", "suspend"), c);
        }
        if self.eff.has("recovers") && !mods.contains("recovers") && label != "lambda" {
            let t = site(self, "recovers");
            let c = self.insert_modifier("declare the effect", "recovers", &["func"]);
            self.rec_c("E-EFF-001", format!("{label} discharges a failure without reporting it but is not declared `recovers`"), "LANG-10-05", &t, ef("recovers", "recovers"), c);
        }
        if self.eff.has("block") && mods.contains("async") {
            let t = site(self, "block");
            self.eff_err("E-EFF-020", format!("{label} is `async` but its body may block the executor's thread"), &t, "async", "block", label, "LANG-11-01");
        }
    }

    // ---- obligations (10) -----------------------------------------------------------------
    pub fn obl_err(&mut self, code: &str, msg: String, tok: &Token, origin: &str, path: &str, rule: &str) {
        self.rec(code, msg, rule, tok, Json::obj([("obligation_origin", Json::str(origin)), ("unconsumed_path", Json::str(path))]));
    }

    pub fn is_linear(&self, t: &Type) -> bool {
        if t.is_loan() {
            return false;
        }
        let s = strip(t);
        if let Type::TypeParam(n) = s {
            // the body's own `T: linear` (6.12, 10.7): that parameter is a linear type inside its body
            return self.linear_in_scope.contains(n);
        }
        self.dc.is_linear_with(s, &self.linear_in_scope)
    }

    /// Whether a type holds `tp` by value in the sense that needs `T: linear` (6.12, X-519): the parameter itself, or
    /// a structure whose linearity depends on `tp` (`T?`, `List[T]`). A lent `T` or an `(alias T from x)?` leaves the
    /// value with the caller, and a type that is linear whatever `tp` is (`Future[tp]`, `Task[tp]`, `fn once`) is one
    /// the obligation machinery holds at every instantiation, so neither needs the bound. The declaration's own
    /// linearity is judged with every type parameter non-linear (`dc.is_linear` never consults a body's bounds), so a
    /// caller's own `T: linear` never masks it. Results (including callback results) do not get the always-linear
    /// wrapper exemption: Outcome<List<T>> must not hide a call that drops T on a failure path (D-596).
    pub fn holds_by_value(&self, t: &Type, tp: &str, result: bool) -> bool {
        if t.is_loan() { return false; }
        match strip(t) {
            Type::Loan { .. } => false,
            Type::TypeParam(n) => n == tp,
            s if !result && known(s) && self.dc.is_linear(s) => false,
            Type::Optional(i) => self.holds_by_value(i, tp, result),
            s => Self::tparams_of(s).iter().any(|x| x == tp),
        }
    }

    pub fn obligate(&mut self, path: &str, tok: &Token) {
        let d = self.depth;
        self.own.open_obligation(path, tok.clone(), d);
    }

    pub fn optional_consume(&mut self, path: &str) {
        for (obligations, tok) in self.optional_scopes.clone() {
            if let Some((origin, depth)) = obligations.get(path) {
                if self.own.obligations.get(path).is_some_and(|(current, d)| current.line == origin.line && current.col == origin.col && d == depth) {
                    self.obl_err("E-OBL-001", "an argument of `?.` must not consume an existing obligation: test for null with `match` or `if let` and call in the present branch".into(), &tok, &format!("{}:{}", origin.line, origin.col), path, "LANG-10-01");
                }
            }
        }
    }

    pub fn consume(&mut self, path: Option<String>, line: usize, how: &str) {
        if let Some(p) = path {
            self.optional_consume(&p);
            self.own.close_obligation(&p, line, how);
        }
    }

    pub fn report_open(&mut self, min_depth: i64, where_: &str) {
        for (p, (tok, d)) in self.own.obligations.items.clone() {
            if self.catch_bound.contains(&p) && min_depth == 0 {
                self.discharges.push((tok.line, "catch left by return/fail/Never".into()));
                self.eff.add("recovers", "proved", Some(&tok));
                self.own.close_obligation(&p, tok.line, "discharged");
                continue;
            }
            if d >= min_depth {
                self.obl_err(
                    "E-OBL-001",
                    format!("the linear value in `{p}` is dropped {where_} without being consumed (or fail, or recover, catch, match, report, move)"),
                    &tok,
                    &format!("{}:{}", tok.line, tok.col),
                    &p,
                    "LANG-10-01",
                );
                self.own.forget(&p);
            }
        }
    }

    pub fn linear_loop_exit(&mut self, min_depth: i64, tok: &Token, what: &str, unwind: bool) {
        for (depth, item) in self.linear_loops.clone() {
            if depth >= min_depth {
                self.obl_err(
                    if unwind { "E-OBL-005" } else { "E-OBL-001" },
                    format!("`{what}` may leave unvisited linear elements in the consuming loop binding `{}`; finish the loop and consume every item (5.3)", item.text),
                    tok, &format!("{}:{}", item.line, item.col), &format!("remainder of {}", item.text),
                    if unwind { "LANG-06-09" } else { "LANG-10-01" },
                );
            }
        }
    }

    pub fn unwind_point(&mut self, tok: &Token, what: &str, env: &Env, keep: Option<&str>) {
        self.linear_loop_exit(0, tok, what, true);
        for (p, (tok0, _)) in self.own.obligations.items.clone() {
            if Some(p.as_str()) == keep || self.catch_bound.contains(&p) {
                continue;
            }
            let t = if p.contains('.') { None } else { env_get(env, &p) };
            if let Some(t) = &t {
                if known(strip(t)) && self.dc.implements(strip(t), "Abandonable") {
                    continue;
                }
            }
            let origin = format!("{}:{}", tok0.line, tok0.col);
            if what == "await" {
                self.obl_err("E-OBL-005", format!("the linear value in `{p}` is live across `await`, a cancellation point, and its type is not Abandonable"), tok, &origin, &p, "LANG-06-09");
            } else {
                self.obl_err("E-OBL-005", format!("the linear value in `{p}` is live across `{what}`, which may unwind the function, and its type is not Abandonable"), tok, &origin, &p, "LANG-06-09");
            }
        }
    }

    pub fn report_half(&mut self) {
        for (p, tok) in std::mem::take(&mut self.own.half) {
            self.obl_err("E-OBL-001", format!("the linear value in `{p}` is consumed on one path only; every path must consume it"), &tok, &format!("{}:{}", tok.line, tok.col), &p, "LANG-10-01");
        }
    }

    /// A conditional operand may not run: the right of `&&` / `||` / `??`, or an `or recover` / `catch`
    /// handler (LR 5.7, 10.2). Join its ownership with the path that skips it and report a consume that
    /// happened on only one of them (LR 7.4, 10.1). Reservations belong to the call being typed.
    /// Reports immediately: `Owners::join` drops an earlier half, and a later `if` or `match` must not
    /// be the only report or a second one.
    pub fn join_conditional(&mut self, skip: &Owners) {
        let reservations = self.own.reservations.clone();
        self.own = skip.join(&self.own);
        self.own.reservations = reservations;
        self.report_half();
    }

    pub fn holder_chain(&self, path: &str) -> BTreeSet<String> {
        let mut chain = BTreeSet::new();
        let mut cur = Some(root_of(path));
        while let Some(c) = cur {
            if c.is_empty() || chain.contains(&c) {
                break;
            }
            chain.insert(c.clone());
            cur = match self.own.holders.get(&c) {
                Some(h) => {
                    let h = h.borrow();
                    let r = h.parent.clone().unwrap_or_else(|| root_of(&h.owner));
                    if r != c {
                        Some(r)
                    } else {
                        None
                    }
                }
                None => None,
            };
        }
        chain
    }

    /// The active loans of `path`'s owner held by other bindings. A loan drawn through a read path (an item a cursor's
    /// `next()` lent, 7.6) is the owner's, not the path's: using the path itself leaves it alone (D-348).
    pub fn foreign_loans(&self, path: &str) -> Vec<Loan> {
        let chain = self.holder_chain(path);
        self.own
            .active(path)
            .into_iter()
            .filter(|l| {
                let l = l.borrow();
                l.holder.as_ref().map_or(false, |h| !chain.contains(h)) && !l.via.as_ref().map_or(false, |v| chain.contains(v))
            })
            .collect()
    }

    /// A use of a place: how in read | write | move | alias | editable | copy. Sets use_ok.
    pub fn use_place(&mut self, path: Option<&str>, tok: &Token, how: &str, env: &Env) {
        self.use_ok = true;
        let path = match path {
            Some(p) => p.to_string(),
            None => return,
        };
        if self.own.is_moved(&path) && !(how == "write" && !path.contains('.')) {
            self.own_err("E-OWN-020", format!("`{path}` was moved and is used again ({how})"), tok, &path, "use after move", "LANG-07-04", "");
            self.use_ok = false;
            return;
        }
        if how == "move" {
            self.optional_consume(&path);
        }
        let kind = self.binding_kind(&path, env);
        let kind_s = kind.clone().unwrap_or_default();
        if kind_s == "capture-probe" {
            self.capture_uses.push((path.clone(), how.into(), tok.clone()));
        }
        let lhold = |l: &Loan| -> (String, String, String) {
            let d = l.borrow();
            (d.holder.clone().unwrap_or_default(), d.owner.clone(), d.mode.clone())
        };
        if how == "read" || how == "copy" {
            if let Some(l) = self.foreign_loans(&path).into_iter().find(|x| x.borrow().mode == "editable") {
                let (h, o, _) = lhold(&l);
                self.own_err("E-OWN-011", format!("`{path}` is suspended while `{h}` holds an exclusive loan of `{o}`"), tok, &path, &format!("exclusive loan held by {h}"), "LANG-07-05", &format!("until the end of {h}'s block"));
                self.use_ok = false;
            }
            return;
        }
        if how == "alias" {
            if let Some(l) = self.foreign_loans(&path).into_iter().find(|x| x.borrow().mode == "editable") {
                let (h, o, _) = lhold(&l);
                self.own_err(
                    "E-OWN-010",
                    format!("read loan of `{path}` while `{h}` holds an exclusive loan of `{o}`, which freezes the owner"),
                    tok,
                    &path,
                    &format!("exclusive loan held by {h}"),
                    "LANG-07-05",
                    &format!("until the end of {h}'s block"),
                );
                self.use_ok = false;
            }
            return;
        }
        if how == "editable" {
            if !self.editable_root(&path, env) {
                self.own_err("E-OWN-001", format!("`editable {path}` needs a root that is a `let mut`, an `editable` parameter or an exclusive loan"), tok, &path, "editable on an immutable root", "LANG-07-01", "");
                self.use_ok = false;
                return;
            }
            if let Some(l) = self.foreign_loans(&path).into_iter().next() {
                let (h, o, m) = lhold(&l);
                self.own_err("E-OWN-010", format!("exclusive loan of `{path}` while `{h}` holds a {m} loan of `{o}`"), tok, &path, &format!("{m} loan held by {h}"), "LANG-07-05", &format!("until the end of {h}'s block"));
                self.use_ok = false;
            }
            return;
        }
        if how == "write" {
            if let Some(l) = self.foreign_loans(&path).into_iter().next() {
                let (h, o, m) = lhold(&l);
                self.own_err("E-OWN-011", format!("`{path}` is assigned while `{h}` holds a {m} loan of `{o}`"), tok, &path, &format!("{m} loan held by {h}"), "LANG-07-05", &format!("until the end of {h}'s block"));
                self.use_ok = false;
                return;
            }
            if self.own.holders.contains(&root_of(&path)) && !path.contains('.') && !["param", "editable", "owned"].contains(&kind_s.as_str()) {
                // a binding that holds a standard cursor owns the cursor value (7.7), so it may be reassigned: the
                // old cursor's stored loan ends on this path (7.9) and the assignment attaches the new value's loan
                // to the binding (D-401)
                if !env_get(env, &root_of(&path)).as_ref().map_or(false, Self::cursor_type) {
                    self.own_err("E-OWN-011", format!("`{path}` holds a loan and cannot be reassigned"), tok, &path, "loan-holding binding", "LANG-07-05", "");
                    self.use_ok = false;
                }
            }
            return;
        }
        if how == "move" {
            for (d_depth, roots, dtok) in self.deferred.clone() {
                if roots.contains(&root_of(&path)) && d_depth <= self.depth {
                    self.def_err(
                        "E-DEF-002",
                        format!("`{path}` is moved out after the defer at line {} names it; a deferred block's places must be initialised at every exit (12.1)", dtok.line),
                        tok,
                        &format!("move {path}"),
                        "LANG-12-01",
                    );
                }
            }
            if path.contains('.') && !self.own.holders.contains(&root_of(&path)) && kind.is_some() {
                self.own_err(
                    "E-OWN-040",
                    format!("partial move of `{path}` out of `{}`, which still owns the rest; move the whole binding or restructure", root_of(&path)),
                    tok,
                    &path,
                    "partial move",
                    "LANG-07-01",
                    "",
                );
                self.use_ok = false;
                return;
            }
            // a field of the loan too: the parameter owns nothing, so nothing in it can be moved out (D-33)
            if (kind_s == "param" || kind_s == "editable") && self.own.holders.contains(&root_of(&path)) {
                self.own_err("E-OWN-001", format!("`move {path}`: a plain or editable parameter holds a loan and owns nothing to move; take it `owned`"), tok, &path, "move of a loan-holding parameter", "LANG-07-06", "");
                self.use_ok = false;
                return;
            }
            if kind_s == "capture-move" && self.lambda_kind.last().map_or(false, |k| k != "fn once") {
                let lk = self.lambda_kind.last().unwrap().clone();
                let corr = self.lambda_toks.last().map(|(fnt, mutt)| {
                    let f = self.dc.file.clone();
                    let e = match mutt {
                        Some(m) => crate::decls::edit(&f, m.line, m.col, crate::decls::tok_end(m), "once"),
                        None => crate::decls::edit(&f, fnt.line, fnt.col, crate::decls::tok_end(fnt), "fn once"),
                    };
                    crate::decls::correction("strengthen the closure kind", vec![e])
                });
                self.rec_c(
                    "E-FUN-010",
                    format!("this `{lk}` lambda moves its capture `{}` out: declare it `fn once`", root_of(&path)),
                    "LANG-08-02",
                    tok,
                    Json::obj([("violation", Json::str("move out of a capture")), ("capture_span", Json::str(root_of(&path))), ("use_span", Json::str(&path))]),
                    corr,
                );
                self.use_ok = false;
                return;
            }
            if kind_s == "capture-alias" || kind_s == "capture-editable" {
                self.own_err("E-OWN-001", format!("`move {path}`: an `{}` capture is a loan and owns nothing to move", &kind_s[8..]), tok, &path, "move of a loan capture", "LANG-08-02", "");
                self.use_ok = false;
                return;
            }
            if (kind_s == "pattern-loan" || kind_s == "for") && self.own.holders.contains(&root_of(&path)) {
                self.own_err(
                    "E-OWN-001",
                    format!("`move {path}`: a binding of `match p` or `for x in p` is a read loan; use `match move p` / `for x in move p` to own it (9.8)"),
                    tok,
                    &path,
                    "move of a loaned binding",
                    "LANG-09-06",
                    "",
                );
                self.use_ok = false;
                return;
            }
            if let Some(l) = self.foreign_loans(&path).into_iter().next() {
                let (h, o, m) = lhold(&l);
                self.own_err("E-OWN-011", format!("`{path}` is moved while `{h}` holds a {m} loan of `{o}`"), tok, &path, &format!("{m} loan held by {h}"), "LANG-07-05", &format!("until the end of {h}'s block"));
                self.use_ok = false;
            }
        }
    }

    pub fn reserve(&mut self, path: &str, mode: &str, tok: &Token) {
        if mode == "move" || mode == "editable" {
            let mut all = self.own.reservations.clone();
            for level in &self.res_stack {
                all.extend(level.iter().cloned());
            }
            for (rp, rm) in all {
                if overlaps(&rp, path) || overlaps(path, &rp) {
                    self.own_err("E-OWN-012", format!("argument `{mode} {path}` breaks the reservation of `{rp}` ({rm}) taken by an earlier argument of this call"), tok, path, &format!("reservation of {rp}"), "LANG-07-04", "");
                    return;
                }
            }
        }
        self.own.reservations.push((path.to_string(), mode.to_string()));
    }

    /// 5.7 step 2: reservations become loans at once; conflicts among them or with active loans are reported. Step 4:
    /// the loans end when the call returns unless the result carries them. A result that holds a loan inside an
    /// optional, a tuple or a record (`(alias T from self)?`) keeps, for each reservation `keep` names (the argument it
    /// is `from`), a loan of the mode it declares, carried `nested` to the binding that receives it (D-348).
    pub fn activate(&mut self, tok: &Token, carries: bool, env: &Env, keep: &BTreeMap<usize, String>) {
        let res = std::mem::take(&mut self.own.reservations);
        let mut added = Vec::new();
        let mut kept: Vec<(String, String)> = Vec::new();
        for (i, (path, mode)) in res.iter().enumerate() {
            if mode == "move" {
                self.own.move_(path);
                continue;
            }
            let lmode = if mode == "editable" { "editable" } else { "alias" };
            for (j, (other, omode)) in res.iter().enumerate() {
                if j != i && (overlaps(other, path) || overlaps(path, other)) && (lmode == "editable" || omode == "editable" || omode == "move") {
                    self.own_err("E-OWN-010", format!("the call takes a {lmode} loan of `{path}` and a {omode} of `{other}` at once"), tok, path, &format!("{omode} of {other}"), "LANG-07-05", "");
                }
            }
            let d = self.depth;
            added.push(self.own.add_loan(path, lmode, None, d, tok.line));
            if let Some(km) = keep.get(&i) {
                kept.push((path.clone(), km.clone()));
            }
        }
        if carries {
            self.carried.extend(added);
            return;
        }
        for l in &added {
            self.own.remove_loan(l);
        }
        for (path, mode) in kept {
            // the kept loan has the mode the result declares (7.6): `editable cursor.next()` lends a read loan, drawn
            // from what the cursor lends, so advancing the cursor leaves it alone; so does any use of a read path
            let root = root_of(&path);
            let cursor = env_get(env, &root).map_or(false, |rt| {
                let rt = match &rt {
                    Type::Loan { inner, .. } => (**inner).clone(),
                    _ => rt.clone(),
                };
                matches!(strip(&rt), Type::Named(n) if (n.package.is_empty() && ["StringView", "BytesView", "ListView", "ListCursor"].contains(&n.name.as_str())))
            });
            let via = cursor || self.own.holders.get(&root).map_or(false, |h| h.borrow().mode == "alias");
            let d = self.depth;
            let l = self.own.add_loan(&path, &mode, None, d, tok.line);
            {
                let mut ld = l.borrow_mut();
                ld.nested = true;
                ld.via = if via { Some(root) } else { None };
            }
            self.carried.push(l);
        }
    }

    /// The origins of the loans a result holds inside an optional, a tuple or a record, with the mode each declares:
    /// `(alias T from self)?` gives self, alias (D-348). A result that is itself a loan carries its loan whole
    /// (`result_carries`), so it gives none.
    pub fn nested_origins(t: &Type, top: bool, out: &mut Vec<(String, String)>) {
        let put = |o: &str, m: &str, out: &mut Vec<(String, String)>| match out.iter_mut().find(|(k, _)| k == o) {
            Some(e) => e.1 = m.to_string(),
            None => out.push((o.to_string(), m.to_string())),
        };
        match t {
            Type::Fn { origin, capture_origins, .. } => {
                if !origin.is_empty() { put(origin, "alias", out); }
                for (o, m) in capture_origins { put(o, m, out); }
            }
            Type::Loan { mode, origin, .. } => {
                if !top {
                    put(origin, mode, out);
                }
            }
            Type::Optional(i) => Self::nested_origins(i, false, out),
            Type::Tuple(items) => {
                for x in items {
                    Self::nested_origins(x, false, out);
                }
            }
            Type::Record(fs) => {
                for (_, x) in fs {
                    Self::nested_origins(x, false, out);
                }
            }
            _ => {}
        }
    }

    /// Value provenance only: a signature's formal from regions are resolved by the call's loan state.
    pub fn captured_loans(t: &Type, out: &mut Vec<(String, String)>) {
        match t {
            Type::Fn { capture_origins, .. } => out.extend(capture_origins.iter().cloned()),
            Type::Loan { inner, .. } | Type::Optional(inner) => Self::captured_loans(inner, out),
            Type::Named(n) => for x in &n.args { Self::captured_loans(x, out); },
            Type::Tuple(xs) => for x in xs { Self::captured_loans(x, out); },
            Type::Record(fs) => for (_, x) in fs { Self::captured_loans(x, out); },
            _ => {}
        }
    }

    /// Whether the value holds an ownership-bearing loan, directly or in an aggregate (D-348).
    pub fn holds_loan(&self, t: &Type) -> bool {
        match t {
            Type::Loan { inner, .. } => self.is_bearing(inner) || self.holds_loan(inner),
            Type::Optional(i) => self.holds_loan(i),
            Type::Tuple(items) => items.iter().any(|x| self.holds_loan(x)),
            Type::Record(fs) => fs.iter().any(|(_, x)| self.holds_loan(x)),
            // a `try` call's result holds what its success value holds (D-394)
            Type::Named(n) if (n.package.is_empty() && n.name == "Outcome") && !n.args.is_empty() => self.holds_loan(&n.args[0]),
            _ => self.loan_carrying_or_cursor(t), // a view carries a loan despite its trivial representation (D-606)
        }
    }

    /// A binding receives a value of type `t` from expression `e` (D-348). When `t` holds a loan of an
    /// ownership-bearing value, the binding holds, until the end of its block, the loans the value carries inside it:
    /// those of the calls in `e` (`xs.get(0)`, `(xs.get(0), 1)`, `xs.get(0) ?? alias d`) and those of the bindings `e`
    /// reads (`move t`, `t ?? u`). Otherwise those loans end with the statement. Returns the loans it holds.
    pub fn keep_nested(&mut self, holder: &str, depth: i64, t: &Type, e: Option<N<'a>>) -> Vec<Loan> {
        let keep = self.holds_loan(t);
        let mut rest = Vec::new();
        let mut held = Vec::new();
        for l in std::mem::take(&mut self.carried) {
            if keep && (matches!(t, Type::Optional(_) | Type::Tuple(_) | Type::Record(_)) || matches!(t, Type::Named(n) if n.package.is_empty() && n.name == "Outcome")) {
                l.borrow_mut().nested = true; // optional injection keeps the call's loan (D-606)
            }
            if !l.borrow().nested {
                rest.push(l);
            } else if keep {
                {
                    let mut ld = l.borrow_mut();
                    crate::journal::kept(&ld.owner, &ld.mode, ld.line, holder, depth);
                    ld.holder = Some(holder.to_string());
                    ld.depth = depth;
                }
                held.push(l);
            } else {
                self.own.remove_loan(&l);
            }
        }
        self.carried = rest;
        if let (true, Some(e)) = (keep, e) {
            let names: BTreeSet<String> = e.tokens().iter().filter(|tk| tk.kind == Kind::Lower).map(|tk| tk.text.clone()).collect();
            for l in self.own.loans.clone() {
                let ld = l.borrow().clone();
                let Some(h) = ld.holder.clone() else { continue };
                // a parameter's loan of its own argument is the caller's and outlives the function: not passed on
                if names.contains(&h) && h != holder && root_of(&ld.owner) != h {
                    let via = ld.via.clone().or_else(|| if ld.mode == "alias" { Some(h.clone()) } else { None });
                    crate::journal::loan(&ld.owner, &ld.mode, Some(holder), depth, ld.line);
                    let c = Rc::new(RefCell::new(LoanData { owner: ld.owner, mode: ld.mode, holder: Some(holder.to_string()), depth, line: ld.line, nested: true, via, parent: ld.parent.clone() }));
                    self.own.loans.push(c.clone());
                    held.push(c);
                }
            }
            let line = e.tokens().first().map_or(0, |t| t.line);
            for h in names.intersection(&self.part_owners.clone()) {
                if h != holder {
                    // the binding read owns a part of what it holds (a `??` fallback, a tuple's own element): a loan
                    // drawn from it is a loan of that binding, and ends with it (7.9; D-369, D-388)
                    crate::journal::loan(h, "alias", Some(holder), depth, line);
                    let c = Rc::new(RefCell::new(LoanData { owner: h.clone(), mode: "alias".into(), holder: Some(holder.to_string()), depth, line, nested: true, via: None, parent: None }));
                    self.own.loans.push(c.clone());
                    held.push(c);
                }
            }
        }
        held
    }

    pub fn result_carries(sig: &Sig<'a>) -> bool {
        if sig.ret.is_loan() {
            return true;
        }
        if let Type::Named(n) = strip(&sig.ret) {
            if n.package.is_empty() && ["StringView", "BytesView", "ListView", "Future"].contains(&n.name.as_str()) {
                return true;
            }
        }
        sig.has_mod("async")
    }

    /// An argument that is a temporary of a type a read parameter lends (not a place, not a trivial value, not a
    /// literal string, which is static): a future keeps the loan (12.3; D-31).
    pub fn lends_temporary(&self, e: N<'a>, t: &Type) -> bool {
        // a grouped `copy` is still a copy
        let copied = e.find("markerExpr").is_some() && e.tokens().into_iter().find(|t| t.text != "(").map_or(false, |t| t.text == "copy");
        if self.path_of_expr(Some(e)).is_some() && !copied {
            return false;
        }
        self.is_bearing(t) || is_interp(strip(t))
    }

    /// The core of `e`, seen through the wrappers of one expression and parentheses (D-447): `(move d)` is marked
    /// exactly as `move d`, since parentheses group an expression and change nothing else (5.6).
    pub fn core_expr(&self, e: N<'a>) -> N<'a> {
        let mut e = e;
        loop {
            let kids = e.kids();
            let next = if e.name() == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") {
                kids[1].node()
            } else if kids.len() == 1 {
                kids[0].node()
            } else {
                None
            };
            match next {
                Some(k) => e = k,
                None => return e,
            }
        }
    }

    pub fn check_arg_marker(&mut self, e: N<'a>, p_name: &str, mode: &str, t: &Type, tok: &Token, env: &Env) {
        let core = self.core_expr(e);
        let lent_marked = core.name() == "markerExpr" && ["alias", "editable"].contains(&core.tokens()[0].text.as_str());
        if ["owned", "payload"].contains(&mode.trim_start_matches('.')) && !lent_marked {
            // a written `alias` is E-OWN-001 at its marker
            self.loan_into_owned(t, None, tok, &format!("`{p_name}`"));
        }
        let path = match self.path_of_expr(Some(e)) {
            Some(p) => p,
            None => {
                let lit = core.tokens();
                let static_literal = lit.len() == 1 && lit[0].kind == Kind::Str && !lit[0].text.contains("${");
                if mode == "from" && e.find("markerExpr").is_none() && !static_literal {
                    self.own_err("E-OWN-051", format!("the result borrows from `{p_name}`, so its argument must be a place, not a temporary (7.6)"), tok, p_name, "loan of a temporary", "LANG-07-06", "");
                }
                let m = mode.trim_start_matches('.');
                if (m.is_empty() || m == "loan" || m == "from") && !t.is_loan() {
                    self.charge_drop(t, tok); // a temporary lent for the call is destroyed with the statement (7.9, 11.3; D-393)
                }
                if m == "owned" || m == "payload" {
                    self.coalesced_marker(Some(e), t, &format!("`{p_name}:`")); // an owned argument or field of `p ?? v` (D-357)
                }
                return;
            }
        };
        let mode = mode.strip_prefix("...").unwrap_or(mode);
        if !self.is_bearing(t) {
            if mode == "editable" && e.find("markerExpr").is_none() {
                self.own_err("E-OWN-001", format!("argument `{p_name}` is an `editable` parameter: write `editable {path}`"), tok, &path, "editable marker required", "LANG-07-01", "");
            }
            return;
        }
        if core.name() == "markerExpr" && ["copy", "alias", "move", "editable"].contains(&core.tokens()[0].text.as_str()) {
            return;
        }
        let fields = |c: &str| Json::obj([("place", Json::str(&path)), ("conflict", Json::str(c)), ("spans", Json::str(&path)), ("loan_region", Json::str(""))]);
        match mode {
            "owned" => {
                // a plain or editable parameter holds a loan and owns nothing to move: copy it, when the type allows
                let kind = self.binding_kind(&path, env).unwrap_or_default();
                let loan_root = (kind == "param" || kind == "editable") && !path.contains('.');
                let c = if !loan_root {
                    Some(self.insert_before("add the marker", tok, "move "))
                } else if self.dc.is_copyable(strip(t)) {
                    Some(self.insert_before("add the marker", tok, "copy "))
                } else {
                    None
                };
                self.rec_c("E-OWN-001", format!("argument `{p_name}` is an `owned` parameter: write `move {path}` or `copy {path}`"), "LANG-07-01", tok, fields("move or copy required"), c)
            }
            "editable" => {
                let c = self.insert_before("add the marker", tok, "editable ");
                self.rec_c("E-OWN-001", format!("argument `{p_name}` is an `editable` parameter: write `editable {path}`"), "LANG-07-01", tok, fields("editable marker required"), Some(c))
            }
            "from" => {
                let c = self.insert_before("add the marker", tok, "alias ");
                self.rec_c("E-OWN-001", format!("the result borrows from `{p_name}`: write `alias {path}` (or `editable {path}`) so the kept loan shows at the call"), "LANG-07-06", tok, fields("alias marker required"), Some(c))
            }
            "loan" => {
                let c = self.insert_before("add the marker", tok, "alias ");
                self.rec_c("E-OWN-001", format!("`{p_name}` stores a loan: write `alias {path}` or `editable {path}`"), "LANG-07-06", tok, fields("alias marker required"), Some(c))
            }
            _ => self.reserve(&path, "alias", tok),
        }
    }

    // ---- generics ---------------------------------------------------------------------
    pub fn recv_subst(&self, recv: &Type, d: &Decl<'a>) -> BTreeMap<String, Type> {
        let mut recv = strip(recv).clone();
        if let Type::Refined { base, .. } = &recv {
            recv = Type::Named(base.clone());
        }
        let mut m = BTreeMap::new();
        m.insert("Self".to_string(), recv.clone());
        if let Type::Named(n) = &recv {
            for (p, a) in d.type_params.iter().zip(n.args.iter()) {
                m.insert(p.clone(), a.clone());
            }
        }
        m
    }

    pub fn unify(&self, pattern: &Type, actual: &Type, m: &mut BTreeMap<String, Type>) {
        self.unify_pin(pattern, actual, m, true);
    }

    /// `pin_empty` is false when a pending lambda can constrain this type parameter: a declared function
    /// (empty identity) must not pin it, so the lambda does (8.4). A lambda on a concrete `fn` parameter
    /// does not. Optional and loan wrappers peel at this binding only. Recursion keeps the flag, so
    /// `List[T]` against `List[fn]` refuses the same pin.
    pub fn unify_pin(&self, pattern: &Type, actual: &Type, m: &mut BTreeMap<String, Type>, pin_empty: bool) {
        if let Type::TypeParam(n) = pattern {
            if let Some(want) = m.get(n) { m.insert(n.clone(), self.with_closure(want, actual)); }
            // an argument of the body's own type parameter fixes the callee's: `label(x)` with `x: U` binds T to U, whose
            // bounds are then the body's (D-171)
            let in_scope = matches!(actual, Type::TypeParam(a) if self.tparams_in_scope.contains(a));
            if !m.contains_key(n) && (known(actual) || in_scope) {
                let mut a = if matches!(actual, Type::Loan { origin, .. } if origin == "static") { actual.clone() } else { crate::literals::concrete(strip(actual)) };
                if !matches!(&a, Type::Loan { origin, .. } if origin == "static") {
                    if let Some(lent) = Self::lent(&a) {
                        if self.dc.is_trivial(&lent) { a = lent; } // a trivial loan is its value (7.6), including optional wrappers
                    }
                }
                if let Type::Refined { base, kind, .. } = &a {
                    if kind == "exact" {
                        a = Type::Named(base.clone());
                    }
                }
                if a == never() {
                    return; // `null` (the absent case of any `T?`) fixes nothing (6.8, 6.10, X-513; D-52.3)
                }
                if !pin_empty && Self::empty_declared_fn(&a) {
                    return;
                }
                m.insert(n.clone(), a);
            }
            return;
        }
        if let Type::Named(p) = pattern {
            if m.contains_key(&p.name) && p.args.is_empty() {
                return;
            }
        }
        let actual = strip(actual);
        if let (Type::Named(_), Type::Refined { base, .. }) = (pattern, actual) {
            // A value's refinement leaves its base's type arguments intact (LR 6.3).
            return self.unify_pin(pattern, &base.as_type(), m, pin_empty);
        }
        match (pattern, actual) {
            // an owned value fixes the parameters of the view taken of it: `List[Int]` fixes `ListView[T]` (6.6, 6.7; D-52.8)
            (Type::Named(p), Type::Named(a)) if (p.name == a.name && p.package == a.package && p.origin == a.origin) || (a.package.is_empty() && p.package.is_empty() && crate::literals::view_of(&a.name, &p.name)) => {
                for (pa, aa) in p.args.iter().zip(a.args.iter()) {
                    self.unify_pin(pa, aa, m, pin_empty);
                }
            }
            (Type::Optional(p), Type::Optional(a)) => self.unify_pin(p, a, m, pin_empty),
            (Type::Loan { inner, .. }, _) => self.unify_pin(inner, actual, m, pin_empty),
            (Type::Fn { params: pp, ret: pr, .. }, Type::Fn { params: ap, ret: ar, .. }) => {
                for (x, y) in pp.iter().zip(ap.iter()) {
                    self.unify_pin(x, y, m, pin_empty);
                }
                self.unify_pin(pr, ar, m, pin_empty);
            }
            (Type::Tuple(pi), Type::Tuple(ai)) => {
                for (x, y) in pi.iter().zip(ai.iter()) {
                    self.unify_pin(x, y, m, pin_empty);
                }
            }
            (Type::Record(ps), Type::Record(xs)) => {
                for (key, p) in ps {
                    if let Some((_, x)) = xs.iter().find(|(k, _)| k == key) {
                        self.unify_pin(p, x, m, pin_empty);
                    }
                }
            }
            _ => {}
        }
    }

    fn empty_declared_fn(t: &Type) -> bool {
        let mut cur = strip(t);
        loop {
            match cur {
                Type::Optional(inner) | Type::Loan { inner, .. } => cur = strip(inner),
                Type::Fn { dynamic: false, identity, .. } if identity.is_empty() => return true,
                _ => return false,
            }
        }
    }

    pub fn tparams_of(t: &Type) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        match t {
            Type::TypeParam(n) => {
                out.insert(n.clone());
            }
            Type::Named(n) => n.args.iter().for_each(|a| out.extend(Self::tparams_of(a))),
            Type::Optional(i) => out.extend(Self::tparams_of(i)),
            Type::Loan { inner, .. } => out.extend(Self::tparams_of(inner)),
            Type::Refined { base, .. } => base.args.iter().for_each(|a| out.extend(Self::tparams_of(a))),
            Type::Tuple(xs) => xs.iter().for_each(|x| out.extend(Self::tparams_of(x))),
            Type::Fn { params, ret, .. } => {
                params.iter().for_each(|x| out.extend(Self::tparams_of(x)));
                out.extend(Self::tparams_of(ret));
            }
            Type::Record(fs) => fs.iter().for_each(|(_, x)| out.extend(Self::tparams_of(x))),
            _ => {}
        }
        out
    }

    /// Does the bound `implements have` grant `implements want`: the same interface with the same arguments, or one it
    /// requires (6.10).
    pub fn grants(&self, have: &NamedT, want: &NamedT, seen: &[String]) -> bool {
        if have.name == want.name && have.package == want.package && have.origin == want.origin {
            return want.args.is_empty() || have.args == want.args;
        }
        if seen.contains(&have.interface_key()) {
            return false;
        }
        let mut seen2 = seen.to_vec();
        seen2.push(have.interface_key());
        match self.dc.resolve_named(have, None) {
            Some(hd) if hd.kind == "interface" => {
                // `J[Int] requires I[T]` grants I[Int]
                let sub: BTreeMap<String, Type> = hd.type_params.iter().cloned().zip(have.args.iter().cloned()).collect();
                hd.requires.iter().any(|r| match self.subst(&Type::Named(r.clone()), &sub) {
                    Type::Named(rs) => self.grants(&rs, want, &seen2),
                    _ => false,
                })
            }
            _ => false,
        }
    }

    /// Does `a` satisfy `implements want` (6.10)? None while a type variable of the call is unsolved. A type parameter of
    /// the body being typed satisfies only what its `where` bounds grant; an applied interface needs a declared
    /// implementation with the same arguments, and its own bounds hold for them (D-171).
    pub fn bound_holds(&self, a: &Type, want: &NamedT) -> Option<bool> {
        let wt = Type::Named(want.clone());
        let mut free = Self::tparams_of(a);
        free.extend(Self::tparams_of(&wt));
        if matches!(a, Type::Unknown(_)) || free.iter().any(|p| !self.tparams_in_scope.contains(p)) {
            return None;
        }
        if want.package.is_empty() && (want.name == "Copyable" || want.name == "Shareable") {
            return Some(self.dc.implements(a, &want.interface_key()));
        }
        if let Type::TypeParam(n) = a {
            return Some(self.where_in_scope.iter().any(|(tp, b)| tp == n && self.grants(b, want, &[])));
        }
        if !self.dc.implements(a, &want.interface_key()) {
            return Some(false);
        }
        if !want.args.is_empty() {
            if let Some(idecl) = self.dc.resolve_named(want, None) {
                let sub: BTreeMap<String, Type> = idecl.type_params.iter().cloned().zip(want.args.iter().cloned()).collect();
                for (tp2, b2) in idecl.where_.iter() {
                    if let Some(x) = sub.get(tp2) {
                        if let Type::Named(b2s) = self.subst(&Type::Named(b2.clone()), &sub) {
                            if self.bound_holds(strip(x), &b2s) == Some(false) {
                                return Some(false);
                            }
                        }
                    }
                }
            }
            if let Type::Named(an) = a {
                let d = self.decl_of(a);
                let mut declared: Vec<NamedT> = d.as_ref().map(|d| d.implements.clone()).unwrap_or_default();
                declared.extend(self.dc.table.implement_blocks.iter().filter(|(_, tt, _, _)| tt.name == an.name && tt.package == an.package && tt.origin == an.origin).map(|(i, _, _, _)| i.clone()));
                let same: Vec<&NamedT> = declared.iter().filter(|i| i.name == want.name && i.package == want.package && i.origin == want.origin).collect();
                if !same.is_empty() {
                    let sub: BTreeMap<String, Type> = d.as_ref().map(|d| d.type_params.iter().cloned().zip(an.args.iter().cloned()).collect()).unwrap_or_default();
                    return Some(same.iter().any(|i| match self.subst(&Type::Named((*i).clone()), &sub) {
                        Type::Named(s) => s.args == want.args,
                        _ => false,
                    }));
                }
            }
        }
        Some(true)
    }

    pub fn subst(&self, t: &Type, m: &BTreeMap<String, Type>) -> Type {
        self.dc.subst(t, m)
    }

    pub fn result_of(&self, sig: &Sig<'a>, m: &BTreeMap<String, Type>) -> Type {
        self.result_of_call(sig, m, false)
    }

    /// The result type of a call. An uninferred type argument is one the call leaves unbound; a parameter bound to
    /// an enclosing generic's own parameter (a generic body) is not one, and neither is one an unknown argument
    /// left unbound.
    pub fn result_of_call(&self, sig: &Sig<'a>, m: &BTreeMap<String, Type>, unknown_args: bool) -> Type {
        let mut r = self.subst(&sig.ret, m);
        let mut in_result = BTreeSet::new();
        free_names(&r, &mut in_result);
        // a result that names only the body's own parameters is typed by them, so a loan of `T` stays a loan (6.12; D-352)
        if has_free(&r) && !in_result.iter().all(|n| self.tparams_in_scope.contains(n)) {
            let mut names = BTreeSet::new();
            free_names(&sig.ret, &mut names);
            let unbound = names.iter().any(|n| !m.contains_key(n));
            return unknown(if unbound && !unknown_args { "generic result" } else { "generic body" });
        }
        if sig.has_mod("try") {
            r = named_args("Outcome", vec![r]);
        }
        if sig.has_mod("async") {
            r = named_args("Future", vec![r]);
        }
        r
    }

    /// Whether a value of type `t` can carry a loan onward: a future, a task or a loan, also inside an optional, an
    /// outcome or a tuple (D-31).
    pub fn carries_onward(&self, t: &Type) -> bool {
        if self.loan_carrying(t) {
            return true;
        }
        match strip(t) {
            Type::Unknown(_) | Type::TypeParam(_) => true, // unresolved results cannot end a loan
            Type::Optional(i) => self.carries_onward(i),
            Type::Named(n) if (n.package.is_empty() && n.name == "Future") || (n.package.is_empty() && n.name == "Task") => true,
            Type::Named(n) => n.args.iter().any(|x| self.carries_onward(x)),
            Type::Tuple(ts) => ts.iter().any(|x| self.carries_onward(x)),
            Type::Record(fs) => fs.iter().any(|(_, x)| self.carries_onward(x)),
            _ => false,
        }
    }

    pub fn loan_carrying(&self, t: &Type) -> bool {
        if t.is_loan() || self.dc.view_newtype(t) {
            return true;
        }
        match strip(t) {
            Type::Fn { origin, capture_origins, .. } => !origin.is_empty() || !capture_origins.is_empty(),
            Type::Refined { base, .. } => self.loan_carrying(&base.as_type()),
            Type::Optional(i) => self.loan_carrying(i),
            Type::Named(n) => {
                if n.package.is_empty() && ["StringView", "BytesView", "ListView"].contains(&n.name.as_str()) {
                    return true;
                }
                self.decl_of(&Type::Named(n.clone())).map_or(false, |d| d.kind == "class" && d.lifetime.is_some())
            }
            _ => false,
        }
    }

    pub fn check_result_origin(&mut self, sig: &Sig<'a>, label: &str, tok: &Token) {
        if sig.has_mod("trusted") || sig.body.is_none() {
            return;
        }
        let mut r = &sig.ret;
        while let Type::Optional(i) = r {
            if matches!(**i, Type::Loan { .. } | Type::Optional(_)) {
                r = i;
            } else {
                break;
            }
        }
        if r.is_loan() || matches!(r, Type::Fn { origin, .. } if !origin.is_empty()) {
            return;
        }
        if self.loan_carrying(r) {
            let rs = r.show();
            self.own_err("E-OWN-053", format!("{label} returns the loan-carrying type {rs} without naming its origin: write `{rs} from <parameter>`"), tok, label, "result without an origin", "LANG-07-06", "");
        }
    }

    pub fn check_exported_sig(&mut self, sig: &Sig<'a>, label: &str, tok: &Token) {
        let core = self.dc.package == "fortis";
        for (pn, pt, _) in &sig.params {
            self.check_type_exported(pt, &format!("{label}({pn})"), tok, core);
        }
        self.check_type_exported(&sig.ret, &format!("{label} result"), tok, core);
    }

    pub fn check_type_exported(&mut self, t: &Type, label: &str, tok: &Token, allow_structural: bool) {
        match t {
            Type::Tuple(_) | Type::Record(_) if !allow_structural => {
                self.rec("E-TYP-030", format!("exported signature {label} uses a structural type {}", t.show()), "LANG-04-05", tok, DeclChecker::typ_fields("a nominal exported type", &t.show(), label));
            }
            Type::Named(n) => {
                if (n.package.is_empty() && n.name == "VariantMap") && !allow_structural {
                    self.rec("E-TYP-030", format!("exported signature {label} uses a structural type {}", t.show()), "LANG-04-05", tok, DeclChecker::typ_fields("a nominal exported type", &t.show(), label));
                    return;
                }
                if let Some(d) = self.dc.resolve_named(n, None) {
                    if d.module == self.module && !d.exported && ["class", "enum", "type", "interface", "error"].contains(&d.kind.as_str()) {
                        self.rec("E-TYP-030", format!("exported signature {label} names non-exported type {}", n.name), "LANG-04-05", tok, DeclChecker::typ_fields("an exported type", &n.name, label));
                    }
                }
                for a in &n.args {
                    self.check_type_exported(a, label, tok, allow_structural);
                }
            }
            Type::Optional(i) => self.check_type_exported(i, label, tok, allow_structural),
            Type::Loan { inner, .. } => self.check_type_exported(inner, label, tok, allow_structural),
            Type::Refined { base, .. } => self.check_type_exported(&base.as_type(), label, tok, allow_structural),
            Type::Fn { params, ret, .. } => {
                for p in params {
                    self.check_type_exported(p, label, tok, allow_structural);
                }
                self.check_type_exported(ret, label, tok, allow_structural);
            }
            Type::Dynamic(i) => self.check_type_exported(&i.as_type(), label, tok, allow_structural),
            _ => {}
        }
    }

    pub fn def_err(&mut self, code: &str, msg: String, tok: &Token, statement: &str, rule: &str) {
        self.rec(code, msg, rule, tok, Json::obj([("offending_statement", Json::str(statement)), ("rule_reference", Json::str("12.1"))]));
    }
}

/// The type parameters a type mentions.
pub fn free_names(t: &Type, out: &mut BTreeSet<String>) {
    match t {
        Type::TypeParam(n) => {
            out.insert(n.clone());
        }
        Type::Named(n) => n.args.iter().for_each(|a| free_names(a, out)),
        Type::Optional(i) => free_names(i, out),
        Type::Loan { inner, .. } => free_names(inner, out),
        Type::Tuple(items) => items.iter().for_each(|x| free_names(x, out)),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|x| free_names(x, out));
            free_names(ret, out);
        }
        Type::Record(fs) => fs.iter().for_each(|(_, x)| free_names(x, out)),
        _ => {}
    }
}

pub fn has_free(t: &Type) -> bool {
    match t {
        Type::TypeParam(_) => true,
        Type::Named(n) => n.args.iter().any(has_free),
        Type::Optional(i) => has_free(i),
        Type::Loan { inner, .. } => has_free(inner),
        Type::Tuple(items) => items.iter().any(has_free),
        Type::Fn { params, ret, .. } => params.iter().any(has_free) || has_free(ret),
        Type::Record(fs) => fs.iter().any(|(_, x)| has_free(x)),
        _ => false,
    }
}


