//! Stack exhaustion as a defect (Language Reference 10.8: unbounded recursion aborts natively).
//!
//! A function in a cycle of the call graph enters through [`enter`]. The guard compares the distance from the
//! program's stack base with a budget below the platform's default main-thread stack, and raises the
//! `stack_exhaustion` defect before the real stack overflows. The guard's drop runs after the recursive call, so
//! the call is never in tail position and the optimiser cannot turn unbounded recursion into a loop that would
//! never abort. Native tasks share the main thread (the controlled scheduler), so one base serves the program.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The stack a program may use before recursion is a defect: below the 8 MiB default main-thread stack.
pub const STACK_BUDGET: usize = 7 << 20;

static BASE: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn here() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

/// Record the program's stack base (at its entry).
#[inline(never)]
pub fn init() {
    BASE.store(here(), Ordering::Relaxed);
}

/// The guard of one activation of a recursive function.
pub struct Depth(());

impl Drop for Depth {
    #[inline(always)]
    fn drop(&mut self) {
        std::hint::black_box(&self.0);
    }
}

/// The stack check of a recursive function whose recursive calls are not in tail position (the recursion
/// consumes stack, so the check fires on unbounded recursion).
#[inline(always)]
pub fn check() {
    let base = BASE.load(Ordering::Relaxed);
    let h = here();
    if base.wrapping_sub(h) > STACK_BUDGET && base > h {
        exhausted();
    }
}

/// The stack check and guard of a function with a recursive call in tail position: the guard's drop runs after
/// that call, so it is never turned into a loop.
#[inline(always)]
pub fn enter() -> Depth {
    check();
    Depth(())
}

#[cold]
#[inline(never)]
fn exhausted() -> ! {
    crate::defect("stack_exhaustion")
}
