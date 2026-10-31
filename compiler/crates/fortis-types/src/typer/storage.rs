//! Storage boundaries, not local loan wrappers (7.7, X-605).
use super::*;
use crate::decls::{tok_of, named_of_decl};

impl<'a> Typer<'a> {
    fn stored_loan(&self, t: &Type, stored: bool, seen: &[(String, String)]) -> bool {
        match t {
            Type::Loan { inner, origin, .. } => (stored && origin != "static") || self.stored_loan(inner, false, seen),
            Type::Optional(inner) => self.stored_loan(inner, stored, seen),
            Type::Refined { base, .. } => self.stored_loan(&Type::Named(base.clone()), stored, seen),
            Type::Tuple(items) => items.iter().any(|t| self.stored_loan(t, stored, seen)),
            Type::Record(fields) => fields.iter().any(|(_, t)| self.stored_loan(t, stored, seen)),
            Type::Fn { params, ret, origin, capture_origins, .. } => (stored && ((!origin.is_empty() && origin != "static") || capture_origins.iter().any(|(o, _)| o != "static")))
                || params.iter().chain(std::iter::once(ret.as_ref())).any(|t| self.stored_loan(t, false, seen)),
            Type::Named(n) => {
                let d = self.decl_of(t);
                let core = d.as_ref().map_or(false, |d| d.module == "fortis" || d.module.starts_with("fortis.") || d.module.starts_with("fortis@"));
                let view = core && (["StringView", "BytesView", "ListView", "ListCursor"].contains(&n.name.as_str()) || d.as_ref().unwrap().lifetime.is_some());
                if stored && view { return true; }
                let transient = core && (view || ["Outcome", "Future", "Task"].contains(&n.name.as_str()));
                if n.args.iter().any(|t| self.stored_loan(t, if transient { stored } else { true }, seen)) { return true; }
                if let Some(d) = d {
                    let key = (if d.type_identity().is_empty() { d.module.clone() } else { d.type_identity() }, d.name.clone());
                    if d.kind == "type" && !seen.contains(&key) {
                        if let Some(alias) = &d.alias {
                            let mut seen = seen.to_vec();
                            seen.push(key);
                            // A direct ListView newtype retains its local loan and region (6.9, X-581).
                            return self.stored_loan(&self.subst(alias, &self.recv_subst(t, &d)), if self.dc.view_newtype(t) { stored } else { true }, &seen);
                        }
                    }
                }
                false
            }
            _ => false,
        }
    }

    pub(super) fn check_stored_type(&mut self, t: &Type, tok: &Token, stored: bool) {
        let t = self.subst(t, &self.storage_substitution);
        if self.stored_loan(&t, stored, &[]) {
            self.storage_failed = true;
            self.own_err("E-OWN-053", format!("{} stores a loan: storage requires owned values or a loan from static (7.7, X-605)", t.show()), tok, &t.show(), "stored loan", "LANG-07-06", "");
        }
    }

    fn bad_static_source(&self, have: &Type, want: &Type, tok: &Token, signatures_only: bool) -> bool {
        if *have == never() { return false; }
        if let Type::Loan { inner, origin, .. } = want {
            if origin == "static" && !signatures_only {
                if let Type::Loan { inner: a, origin, .. } = have {
                    if origin == "static" { return self.bad_static_source(a, inner, tok, false); }
                }
                if self.dc.is_trivial(strip(have)) { return false; }
                let literal = matches!(base_type(inner), Type::Named(n) if n.package.is_empty() && ((n.name == "StringView" && (is_strlit(have) || (is_interp(have) && tok.kind == Kind::Str && super::expr::hole_sources(tok).is_empty()))) || (n.name == "BytesView" && *have == named("BytesView") && tok.kind == Kind::Bytes)));
                return !literal;
            }
            return self.bad_static_source(strip(have), inner, tok, signatures_only);
        }
        if let Type::Optional(b) = want {
            return self.bad_static_source(if let Type::Optional(a) = have { a } else { have }, b, tok, signatures_only);
        }
        let have = strip(have);
        match (have, want) {
            (Type::Tuple(a), Type::Tuple(b)) => a.iter().zip(b).any(|(a, b)| self.bad_static_source(a, b, tok, signatures_only)),
            (Type::Record(a), Type::Record(b)) => a.iter().any(|(k, a)| b.iter().find(|(j, _)| j == k).map_or(false, |(_, b)| self.bad_static_source(a, b, tok, signatures_only))),
            (Type::Named(a), Type::Named(b)) if a.name == b.name && a.package == b.package && a.origin == b.origin => a.args.iter().zip(&b.args).any(|(a, b)| self.bad_static_source(a, b, tok, signatures_only)),
            (Type::Fn { params: ap, ret: ar, origin: ao, identity: ai, .. }, Type::Fn { params: bp, ret: br, origin: bo, .. }) =>
                (!signatures_only && bo == "static" && ao != "static" && (!ai.is_empty() || !ao.is_empty())) || self.bad_static_source(ar, br, tok, false) || ap.iter().zip(bp).any(|(a, b)| self.bad_static_source(b, a, tok, false)),
            _ => false,
        }
    }

    pub(super) fn check_static_signatures(&mut self, have: &Type, want: &Type, tok: &Token, what: &str) {
        if self.bad_static_source(&self.subst(have, &self.storage_substitution), &self.subst(want, &self.storage_substitution), tok, true) {
            self.storage_failed = true;
            self.own_err("E-OWN-053", format!("{what} cannot claim from static: its source is not proved static (7.7, D-584)"), tok, what, "unproved static origin", "LANG-07-06", "");
        }
    }

    pub(super) fn check_static_source(&mut self, have: &Type, want: &Type, tok: &Token, what: &str) {
        if self.bad_static_source(&self.subst(have, &self.storage_substitution), &self.subst(want, &self.storage_substitution), tok, false) {
            self.storage_failed = true;
            self.own_err("E-OWN-053", format!("{what} cannot claim from static: its source is not proved static (7.7, D-584)"), tok, what, "unproved static origin", "LANG-07-06", "");
        }
    }

    /// Keep the loans ordinary inference erases, only for owned arguments and callback results.
    pub(super) fn storage_bind(&self, pattern: &Type, actual: &Type, bound: &mut BTreeMap<String, Type>) {
        match (pattern, actual) {
            (Type::TypeParam(n), a) => {
                if Self::lent(a).map_or(false, |t| self.dc.is_trivial(&t)) { return; } // trivial loans copy their value (7.6)
                if self.stored_loan(a, true, &[]) { bound.insert(n.clone(), a.clone()); }
            }
            (Type::Loan { inner, .. }, a) => self.storage_bind(inner, strip(a), bound),
            (Type::Optional(p), Type::Optional(a)) => self.storage_bind(p, a, bound),
            (Type::Named(p), Type::Named(a)) if p.name == a.name && p.package == a.package && p.origin == a.origin => {
                for (p, a) in p.args.iter().zip(&a.args) { self.storage_bind(p, a, bound); }
            }
            (Type::Tuple(p), Type::Tuple(a)) => { for (p, a) in p.iter().zip(a) { self.storage_bind(p, a, bound); } }
            (Type::Record(p), Type::Record(a)) => { for ((_, p), (_, a)) in p.iter().zip(a) { self.storage_bind(p, a, bound); } }
            (Type::Fn { ret: p, .. }, Type::Fn { ret: a, .. }) => self.storage_bind(p, a, bound),
            _ => {}
        }
    }

    pub(super) fn check_storage_function(&mut self, sig: &Sig<'a>, bound: &BTreeMap<String, Type>, owner: &DeclRef<'a>, tok: &Token) {
        let parameters: BTreeSet<_> = owner.type_params.iter().chain(&sig.type_params).collect();
        if parameters.is_empty() { return; }
        let mut actual = bound.clone();
        if let Some(Type::Fn { params, ret, .. }) = self.storage_expected.as_deref().map(strip) {
            for ((_, pt, _), arg) in sig.params.iter().zip(params) { self.unify(pt, arg, &mut actual); }
            self.unify(&sig.ret, ret, &mut actual);
            for ((_, pt, mode), arg) in sig.params.iter().zip(params) {
                if matches!(mode.trim_start_matches('.'), "owned" | "payload") {
                    self.storage_bind(pt, arg, &mut actual);
                } else if let (Type::Fn { ret: p, .. }, Type::Fn { ret: a, .. }) = (pt, arg) {
                    self.storage_bind(p, a, &mut actual);
                }
            }
        }
        // An erased generic function value cannot hide an unfixed storage requirement. Borrow-only generic
        // functions remain legal: the non-storable probe finds no storage in their bodies.
        for tp in parameters { actual.entry(tp.clone()).or_insert_with(|| named("StringView")); }
        self.check_storage_call(sig, &actual, Some(owner), &format!("function value {}", sig.name), tok);
    }

    pub(super) fn check_storage_call(&mut self, sig: &Sig<'a>, m: &BTreeMap<String, Type>, owner: Option<&DeclRef<'a>>, label: &str, tok: &Token) {
        // A generic body can store T without exposing it in its result. Type it in its lexical context, with
        // substitution only for the storage judgement. Unrelated diagnostics belong to the declaration.
        let actual: BTreeMap<String, Type> = m.iter().map(|(k, v)| (k.clone(), self.subst(v, &self.storage_substitution))).collect();
        if sig.body.is_none() || !actual.values().any(|t| self.stored_loan(t, true, &[])) { return; }
        let inferred_result = self.subst(&sig.ret, &actual);
        if self.holds_loan(&inferred_result) && !self.holds_loan(&sig.ret) && self.stored_loan(&inferred_result, true, &[]) {
            self.storage_failed = true;
            self.own_err("E-OWN-053", format!("{label} infers a loan result whose origin its generic signature does not name (7.7, X-605)"), tok, label, "result without an origin", "LANG-07-06", "");
            return;
        }
        let shape: Vec<_> = actual.iter().map(|(k, t)| (k, self.stored_loan(t, true, &[]))).collect();
        let key = format!("{:?}:{shape:?}", sig.node.map(|n| n.id()));
        if self.storage_calls.contains(&key) { return; }
        let mut dc = owner.map(|d| self.dc.body_context(d)).unwrap_or_else(|| self.dc.clone());
        dc.records.clear();
        let mut probe = Typer::new(dc);
        probe.aliases = if probe.module == self.module { self.aliases.clone() } else { Self::body_aliases(&probe.dc) };
        probe.storage_substitution = actual;
        probe.storage_calls = self.storage_calls.clone();
        probe.storage_calls.insert(key);
        let self_t = owner.filter(|d| d.kind != "func").map(|d| named_of_decl(d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect()));
        probe.run_body(sig, self_t, sig.receiver.as_deref().unwrap_or("self"), tok, label);
        if probe.storage_failed {
            self.storage_failed = true;
            self.own_err("E-OWN-053", format!("{label} stores a loan in this instantiation: storage requires owned values or a loan from static (7.7, X-605)"), tok, label, "stored loan", "LANG-07-06", "");
        }
    }

    pub(super) fn check_storage_decl(&mut self, d: &DeclRef<'a>) {
        let tok = tok_of(d);
        for (t, _, _) in d.fields.values() { self.check_stored_type(t, &tok, true); }
        for payload in d.variants.values() {
            for (_, t) in payload { self.check_stored_type(t, &tok, true); }
        }
        if let Some(t) = &d.alias { self.check_stored_type(t, &tok, d.kind == "type" && !self.dc.view_newtype(&named_of_decl(d, vec![]))); }
        for sig in d.sig.iter().chain(d.methods.values()) {
            let at = sig.node.map(ftok).unwrap_or(&tok);
            for (_, t, _) in &sig.params { self.check_stored_type(t, at, false); }
            self.check_stored_type(&sig.ret, at, false);
        }
    }
}
