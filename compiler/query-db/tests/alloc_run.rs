//! The allocator test (`alloc.rs`, part of SP-12b's preregistered apparatus, unchanged), run so that nothing else in
//! its process allocates while it counts. It runs on its own, with one harness thread:
//!
//!     cargo test --release -p fortis-query-db --test alloc -- --test-threads=1
//!
//! (`cargo test` alone leaves it out: its target is not tested by default.) The test compares the global allocator's
//! counters before and after each check, so they are exact only while no other thread allocates. The harness's main
//! thread allocates while the first test runs: with more than one harness thread it enters the test in its table of
//! running tests and its queue of deadlines, and the first time it waits for a result it allocates its waiting
//! context and the channel's list of waiters, which it keeps for later waits. Those four allocations failed the test
//! in about one CI run in three. One harness thread removes the first two; the other two still failed 5 of 150 runs
//! on two busy CPUs. With one harness thread the tests run one at a time in name order, so `harness_is_waiting` runs
//! first and returns only once the main thread is asleep in that first wait. The allocator test, `sp12b::allocator`,
//! then runs beside a main thread that allocates nothing until it has finished (600 of 600 runs on the same CPUs).

#[path = "alloc.rs"]
mod sp12b;

use std::time::{Duration, Instant};

/// The scheduler state of a thread of this process (`R` running, `S` asleep, ...): the field after the name in
/// /proc/self/task/TID/stat. The name is in parentheses and may hold any character, so the last `)` ends it.
fn state(tid: u32) -> char {
    let stat = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat")).expect("/proc/self/task/TID/stat (Linux)");
    stat[stat.rfind(')').expect("a stat line names its thread in parentheses") + 1..].trim_start().chars().next().unwrap_or('?')
}

/// One test at a time: `--test-threads=1` (or `--test-threads 1`), or RUST_TEST_THREADS=1 when no option sets it.
fn one_harness_thread() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let option = args.iter().enumerate().find_map(|(i, a)| match a.strip_prefix("--test-threads") {
        Some("") => args.get(i + 1).cloned(),
        Some(v) => v.strip_prefix('=').map(str::to_string),
        None => None,
    });
    option.or_else(|| std::env::var("RUST_TEST_THREADS").ok()).as_deref() == Some("1")
}

#[test]
fn harness_is_waiting() {
    assert!(one_harness_thread(), "the allocator test needs one harness thread: cargo test -p fortis-query-db --test alloc -- --test-threads=1");
    // the main thread spawned this test and then waits for its result; its first sleep is that wait
    let main = std::process::id();
    let deadline = Instant::now() + Duration::from_secs(60);
    while state(main) != 'S' {
        assert!(Instant::now() < deadline, "the harness's main thread did not wait for this test within 60 s");
        std::thread::sleep(Duration::from_millis(1));
    }
}
