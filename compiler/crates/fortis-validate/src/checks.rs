//! The check registry: one entry per Appendix C.1 row and C.3 pattern, with the frontend-checked property it
//! validates and the subset that first needs it. `wf.*` checks are the IR's own invariants.

pub struct Check {
    pub id: &'static str,
    /// the Appendix C row or pattern, as the Language Reference writes it
    pub row: &'static str,
    /// the frontend-checked property the check validates
    pub property: &'static str,
    /// `G1` when the G1 subset needs it, else the chunk that extends coverage to it
    pub subset: &'static str,
}

pub const CHECKS: &[Check] = &[
    // ---- C.1 lowering shapes
    Check { id: "C1.newtype", row: "`type` newtype", property: "refinement invariant", subset: "G1" },
    Check { id: "C1.class", row: "class", property: "generated-constructor visibility", subset: "G1" },
    Check { id: "C1.trivial", row: "trivial type", property: "the trivial table", subset: "G1" },
    Check { id: "C1.copy", row: "`copy p`", property: "copyable set; allocation", subset: "G1" },
    Check { id: "C1.read-loan", row: "read loan", property: "lexical loan end (frontend is stricter)", subset: "G1" },
    Check { id: "C1.exclusive-loan", row: "exclusive loan", property: "field-freezing rule", subset: "G1" },
    Check { id: "C1.move", row: "`move p`", property: "use after move (Rust-checked; validated as well)", subset: "G1" },
    Check { id: "C1.from-return", row: "`from` return", property: "single-origin rule", subset: "G1" },
    Check { id: "C1.standard-loans", row: "standard view or cursor", property: "standard types only: no user type stores a loan (X-137)", subset: "G1" },
    Check { id: "C1.shared", row: "`Shared[T]` / `Weak[T]`", property: "`retain` visibility", subset: "G1" },
    Check { id: "C1.mutex", row: "`Mutex[T]`", property: "guard-across-await rule", subset: "G3" },
    Check { id: "C1.interface", row: "interface", property: "effect/refinement substitutability, coherence", subset: "G1" },
    Check { id: "C1.dynamic", row: "`dynamic I`", property: "dynamic-eligibility, allocation", subset: "G1" },
    Check { id: "C1.mixin", row: "mixin", property: "conflicts, required methods", subset: "G1" },
    Check { id: "C1.lambda", row: "lambda", property: "declared kind is at least the kind the body needs; capture list", subset: "G1" },
    Check { id: "C1.try", row: "`try func`", property: "linearity (no drop of a live Failure)", subset: "G1" },
    Check { id: "C1.catch", row: "`catch`", property: "recover/return semantics", subset: "G1" },
    Check { id: "C1.optional", row: "`T?`", property: "no implicit absence/failure equivalence", subset: "G1" },
    Check { id: "C1.match", row: "`match`", property: "ownership of bindings", subset: "G1" },
    Check { id: "C1.async", row: "`async func`", property: "linear futures, cancellation at `await`", subset: "G3" },
    Check { id: "C1.scope", row: "`scope`", property: "cancellation and join on exit", subset: "G3" },
    Check { id: "C1.launch-parallel", row: "`launchParallel`", property: "`Shareable`", subset: "G3" },
    Check { id: "C1.defer", row: "`defer`", property: "places initialised at every exit", subset: "G3" },
    Check { id: "C1.cleanup", row: "`cleanup`", property: "effects contributed", subset: "G1" },
    Check { id: "C1.noalloc", row: "`noalloc`", property: "pattern whitelist (C.3)", subset: "G1" },
    Check { id: "C1.effects", row: "`pure`, effects", property: "summaries", subset: "G1" },
    Check { id: "C1.observation", row: "`capture`/`record`", property: "definite assignment", subset: "G1" },
    Check { id: "C1.capability", row: "capability", property: "`io` requires capability", subset: "G1" },
    // ---- C.3 allocation patterns (in a `noalloc` function)
    Check { id: "C3.arithmetic", row: "arithmetic and comparison", property: "whitelisted", subset: "G1" },
    Check { id: "C3.access", row: "field and array access", property: "whitelisted", subset: "G1" },
    Check { id: "C3.inline-construct", row: "construction of trivial or inline types", property: "whitelisted only for trivial or inline types", subset: "G1" },
    Check { id: "C3.loans", row: "read and exclusive loans", property: "whitelisted", subset: "G1" },
    Check { id: "C3.call", row: "calls to functions whose summary lacks `alloc`", property: "whitelisted only for such callees", subset: "G1" },
    Check { id: "C3.match", row: "`match`", property: "whitelisted", subset: "G1" },
    Check { id: "C3.defer", row: "`defer` lowering", property: "whitelisted", subset: "G3" },
    Check { id: "C3.launch", row: "cooperative `launch` beyond the host's inline task storage (v1: none, X-125)", property: "alloc", subset: "G3" },
    Check { id: "C3.capture", row: "`capture` into a frame of statically bounded size", property: "whitelisted", subset: "G1" },
    Check { id: "C3.growth", row: "`List`/`String`/`Map` growth", property: "alloc", subset: "G1" },
    Check { id: "C3.shared-create", row: "`Shared.create`", property: "alloc", subset: "G1" },
    Check { id: "C3.dynamic-create", row: "`dynamic` creation", property: "alloc", subset: "G1" },
    Check { id: "C3.copy-heap", row: "`copy` of heap-owning types", property: "alloc", subset: "G1" },
    Check { id: "C3.boxed-future", row: "boxed futures from `dynamic` async calls", property: "alloc", subset: "G3" },
    Check { id: "C3.failure-payload", row: "extended failure payloads", property: "alloc", subset: "G1" },
    Check { id: "C3.parallel", row: "`launchParallel`/`launchBlocking`", property: "alloc", subset: "G3" },
    // ---- the IR's own invariants
    Check { id: "wf.locals", row: "IR invariant", property: "every local is declared and every block local is bound once", subset: "G1" },
    Check { id: "wf.calls", row: "IR invariant", property: "every callee and type the IR names exists; nominal identities agree across calls, bindings and returns", subset: "G1" },
    Check { id: "wf.control", row: "IR invariant", property: "break/continue in a loop with that label; give in a block expression; recover in a catch", subset: "G1" },
];

pub fn check(id: &str) -> Option<&'static Check> {
    CHECKS.iter().find(|c| c.id == id)
}
