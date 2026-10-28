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
