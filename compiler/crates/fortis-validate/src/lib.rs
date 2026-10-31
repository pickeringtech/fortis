//! The shared IR validator (Delivery Programme 5.2-5.3; DEL-45). `validate` re-establishes, from the typed IR alone,
//! every frontend-checked property of Appendix C (one check per C.1 row and C.3 pattern) plus the IR's own
//! invariants. Backends refuse a program it rejects; `rustc` acceptance is never evidence for these properties.

pub mod checks;
pub mod limits;
pub mod corrupt;
pub mod validate;

use fortis_mir::model::Program;

pub use validate::Violation;

/// Every violation in the program, sorted (deterministic for a given program).
pub fn validate(prog: &Program) -> Vec<Violation> {
    let mut ctx = validate::Ctx::new(prog);
    validate::run(&mut ctx);
    ctx.out
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use fortis_mir::{build::LibIndex, build_source};
    use fortis_types::types::Type;

    #[test]
    fn refuses_a_nested_nominal_identity_changed_after_checking() {
        let source = "class Bool { n: Int }\nfunc same(xs: owned List[Bool]) List[Bool] => move xs\n";
        let mut p = build_source(source, "identity.ft", &LibIndex::default()).unwrap();
        assert!(p.types.iter().any(|d| d.name == "Bool@main"));
        assert!(validate(&p).is_empty());
        let f = p.functions.iter_mut().find(|f| f.label == "function same").unwrap();
        let Type::Named(list) = &mut f.ret else { panic!("List result") };
        let Type::Named(key) = &mut list.args[0] else { panic!("Bool element") };
        key.name = "Bool".into();
        key.package.clear();
        assert!(validate(&p).iter().any(|v| v.check == "wf.calls" && v.message.contains("type identity")));
    }
}
