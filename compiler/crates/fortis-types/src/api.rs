//! The API-change report (LANG-11-05, 11.3): weakening an exported function's written modifiers is reported.
//!
//! Across a module boundary only written claims count, so an exported function's API includes the modifiers it
//! writes, with each named contract expanded. Removing `pure` or `noalloc`, or adding `async`, `try` or
//! `recovers`, is a weakening: callers that relied on the old signature may break. `check --api-baseline OLD`
//! compares a module (or a package, module by module) with its previous version and reports each weakening as an
//! `E-EFF-040` warning at the current declaration. Strengthening is not reported.

use std::collections::{BTreeMap, BTreeSet};

use fortis_diag::{Json, Record};
use fortis_syntax::Parsed;

/// An exported function or method: its qualified name, written modifiers (contracts expanded) and position.
#[derive(Clone, Debug)]
pub struct ApiFn {
    pub name: String,
    pub modifiers: BTreeSet<String>,
    pub line: usize,
    pub column: usize,
}

const CLAIMS: &[&str] = &["pure", "noalloc"];
const EFFECTS: &[&str] = &["async", "try", "recovers"];

/// The module's exported functions and the exported methods of its exported types.
pub fn exported(parsed: &Parsed, file: &str) -> BTreeMap<String, ApiFn> {
    let (_, out) = crate::check::check_parsed_with(parsed, file, |ty| {
        let mut sigs = Vec::new();
        for (name, d) in ty.dc.table.decls.iter() {
            if !d.exported || d.module != "main" {
                continue;
            }
            if let Some(sig) = d.sig.as_ref().filter(|_| d.kind == "func") {
                sigs.push((name.clone(), sig));
            }
            for (m, sig) in d.methods.iter() {
                if sig.exported {
                    sigs.push((format!("{name}.{m}"), sig));
                }
            }
        }
        let mut out = BTreeMap::new();
        for (name, sig) in sigs {
            let mods = ty.expanded_mods(sig);
            let tok = sig.node.and_then(|n| n.direct_tokens().into_iter().find(|t| t.text == "func"));
            out.insert(name.clone(), ApiFn { name, modifiers: mods, line: tok.map_or(1, |t| t.line), column: tok.map_or(1, |t| t.col) });
        }
        out
    });
    out
}

/// The weakenings from `old` to `new`, as records against `file`.
pub fn compare(old: &BTreeMap<String, ApiFn>, new: &BTreeMap<String, ApiFn>, file: &str) -> Vec<Record> {
    let show = |m: &BTreeSet<String>| if m.is_empty() { "no modifiers".to_string() } else { m.iter().cloned().collect::<Vec<_>>().join(" ") };
    let mut out = Vec::new();
    for (name, n) in new {
        let Some(o) = old.get(name) else { continue };
        let mut changes = Vec::new();
        for c in CLAIMS {
            if o.modifiers.contains(*c) && !n.modifiers.contains(*c) {
                changes.push((format!("removes `{c}`"), c.to_string()));
            }
        }
        for e in EFFECTS {
            if !o.modifiers.contains(*e) && n.modifiers.contains(*e) {
                changes.push((format!("adds `{e}`"), e.to_string()));
            }
        }
        for (what, word) in changes {
            let mut r = Record::new(
                "E-EFF-040",
                format!("API change: exported `{name}` {what}; callers that relied on its previous signature may break"),
                "LANG-11-05",
                file,
                n.line,
                n.column,
                Json::obj([
                    ("required_contract", Json::str(&show(&o.modifiers))),
                    ("violating_effect", Json::str(&word)),
                    ("witness_path", Json::str(&format!("{name}: {} -> {}", show(&o.modifiers), show(&n.modifiers)))),
                ]),
            );
            r.severity = "warning";
            out.push(r);
        }
    }
    out.sort_by_key(|r| (r.line, r.column));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(src: &str) -> BTreeMap<String, ApiFn> {
        exported(&fortis_syntax::parse_module(src).unwrap(), "t.ft")
    }

    #[test]
    fn weakening_is_reported_and_strengthening_is_not() {
        let old = api("export pure func a(x: Int) Int => x\nexport func b(x: Int) Int => x\n");
        let new = api("export func a(x: Int) Int => x\nexport pure func b(x: Int) Int => x\n");
        let r = compare(&old, &new, "t.ft");
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].code.as_str(), r[0].line, r[0].severity), ("E-EFF-040", 1, "warning"));
        assert!(compare(&new, &new, "t.ft").is_empty());
    }

    #[test]
    fn private_functions_are_not_api() {
        assert!(api("pure func hidden(x: Int) Int => x\n").is_empty());
    }
}
