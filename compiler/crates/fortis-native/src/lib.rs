//! The runtime of natively lowered Fortis programs (Delivery Programme 5.3; DEL-36): the typed intrinsics of the
//! core library, the display form, failures and the record stream, built on the exact semantics of fortis-runtime
//! so that native code, the VM and the interpreter agree. Generated application crates carry
//! `#![forbid(unsafe_code)]`; so does this crate, except for the allocator probe of the `alloc-probe` feature
//! (SP-04-async), whose counting global allocator is the one `unsafe` impl and is never in a production build.
#![cfg_attr(not(feature = "alloc-probe"), forbid(unsafe_code))]
#![cfg_attr(feature = "alloc-probe", deny(unsafe_code))]

pub mod codec;
pub mod collections;
pub mod convert;
pub mod hash;
pub mod failure;
pub mod host;
pub mod numeric;
#[cfg(feature = "alloc-probe")]
pub mod probe;
pub mod pyrandom;
pub mod records;
pub mod shared;
pub mod show;
pub mod stack;
pub mod task;
pub mod trace;
pub mod text;

pub use collections::{OrderedMap, OrderedSet};
pub use failure::{outcome_err, Dyn, Failure, IntoDyn, Outcome, Report};
pub use fortis_runtime::{float, Decimal, Rounding};
pub use numeric::{Duration, Timestamp};
pub use shared::{AtomicBool, AtomicInt, Guard, IntoMaybeOwned, MaybeOwned, Mutex, Shared, WeakRef};
pub use show::Show;

/// The value of an intrinsic whose signature is infallible but whose operation can still fail on an input the
/// static layers proved impossible (an overflow): a defect if the proof was wrong.
#[inline]
pub fn value<T>(o: Outcome<T>) -> T {
    match o {
        Ok(v) => v,
        Err(f) => defect(f.code),
    }
}

/// A defect (10.8): the program stops at once with a `defect` record; no cleanup runs.
#[inline]
pub fn defect(class: &str) -> ! {
    records::defect(class, &task::running_task());
    host::release_dev_root();
    records::flush();
    std::process::exit(101)
}

/// `dynamic` typed as whatever its context needs (it never returns).
#[inline]
pub fn dynamic_t<T>(message: &str) -> T {
    dynamic(message)
}

/// A dynamic check the static layers exclude (the interpreter's `dynamic:` outcome): exit 2 with the message.
#[inline]
pub fn dynamic(message: &str) -> ! {
    host::release_dev_root();
    records::flush();
    eprintln!("dynamic:{message}");
    std::process::exit(2)
}

/// Run `main`: an unhandled failure is reported and the process exits 1 (10.6).
#[inline]
pub fn run_main(file: &str, main: impl FnOnce() -> Outcome<()>) -> i32 {
    stack::init();
    records::set_file(file);
    // the telemetry adapter opens its channel before any frame completes (SP-09 design 3.1.5)
    let _ = fortis_runtime::telemetry::mode();
    let code = match main() {
        Ok(()) => 0,
        Err(f) => {
            records::report(&f, "main");
            1
        }
    };
    #[cfg(feature = "alloc-probe")]
    probe::report();
    host::release_dev_root();
    records::flush();
    code
}

/// A false `expect` inside a closure of a test: unwinds to the test runner, which fails the case (test builds
/// are built with `panic = "unwind"`; the panic hook is not run).
pub struct ExpectFailed(pub String);

pub fn expect_failed(evidence: String) -> ! {
    std::panic::resume_unwind(Box::new(ExpectFailed(evidence)))
}

/// One case of a test or property: a failure or a false `expect` fails it (14).
pub enum CaseResult {
    Pass,
    Fail(String),
    /// a test the builder could not lower: the result record stays `unbuilt`, and the summary counts it as failed
    Unbuilt(String),
}

/// `expect cond`: a false condition fails the case with its evidence.
#[inline]
pub fn expect(cond: bool, evidence: impl FnOnce() -> String) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(evidence())
    }
}

/// Run every test case, emitting one test-result record each; prints `N passed, M failed`.
/// An unbuilt test counts as a failure: a test that was not built did not pass (D-55).
pub fn run_tests(file: &str, cases: Vec<(String, String, Box<dyn FnOnce() -> CaseResult>)>) -> i32 {
    stack::init();
    records::set_file(file);
    // the telemetry adapter opens its channel before any frame completes (SP-09 design 3.1.5)
    let _ = fortis_runtime::telemetry::mode();
    let (mut passed, mut failed, mut unbuilt) = (0, 0, 0);
    for (test, case, run) in cases {
        let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
            Ok(r) => r,
            Err(payload) => match payload.downcast::<ExpectFailed>() {
                Ok(e) => CaseResult::Fail(e.0),
                Err(other) => std::panic::resume_unwind(other),
            },
        };
        match result {
            CaseResult::Unbuilt(why) => {
                unbuilt += 1;
                records::test_result(&test, &case, "unbuilt", &why);
            }
            CaseResult::Pass => {
                passed += 1;
                records::test_result(&test, &case, "pass", "");
            }
            CaseResult::Fail(ev) => {
                failed += 1;
                records::test_result(&test, &case, "fail", &ev);
            }
        }
    }
    host::release_dev_root();
    records::flush();
    // an unbuilt test is a failure. The summary stays `N passed, M failed`: the docset's "every test passes"
    // check matches that shape, and M counts tests that did not pass (D-55).
    let reported_failed = failed + unbuilt;
    println!("{passed} passed, {reported_failed} failed");
    if reported_failed > 0 { 1 } else { 0 }
}
