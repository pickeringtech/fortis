//! The allocator probe (SP-04-async, PROG-CMP-08), built only with the `alloc-probe` feature: a counting global
//! allocator, and the phase accounting of the futures of `noalloc` async functions.
//!
//! The probe lowering (`FORTIS_ALLOC_PROBE=1`) wraps each `noalloc` async function in [`construct`], which
//! counts the heap allocations of four phases of its future. **construct** is the call that creates the future.
//! **poll** is each poll returning `Pending`, and **complete** is the poll returning `Ready`. **drop** is dropping
//! the future, separately after completion and before it (a cancelled or never-polled future). The probe's own
//! bookkeeping is excluded, so a probed callee inside a probed caller adds only its own allocations. This is the
//! only module of the runtime with `unsafe` code, and a production build does not compile it.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::task::{Context, Poll};

static COUNT: AtomicUsize = AtomicUsize::new(0);
static PAUSED: AtomicBool = AtomicBool::new(false);

/// The system allocator, counting every allocation and reallocation (deallocation is not an allocation).
pub struct Counting;

// SAFETY: each method forwards its arguments unchanged to the system allocator, which upholds GlobalAlloc's
// contract; the only addition is a relaxed atomic increment, which neither allocates nor touches the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        // SAFETY: the caller's layout is passed on as received (GlobalAlloc::alloc's contract).
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        // SAFETY: `ptr` was allocated by this allocator (that is, by System) with `layout`, per the caller.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by this allocator (System) with `layout`, per the caller.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn bump() {
    if !PAUSED.load(Relaxed) {
        COUNT.fetch_add(1, Relaxed);
    }
}

fn count() -> usize {
    COUNT.load(Relaxed)
}

/// Run probe bookkeeping without counting its allocations.
fn paused<R>(f: impl FnOnce() -> R) -> R {
    let was = PAUSED.swap(true, Relaxed);
    let r = f();
    PAUSED.store(was, Relaxed);
    r
}

/// One function's totals over every future the run created.
#[derive(Clone, Copy, Default)]
struct Row {
    futures: usize,
    construct: usize,
    polls: usize,
    poll: usize,
    complete: usize,
    dropped_after: usize,
    drop_after: usize,
    dropped_before: usize,
    drop_before: usize,
}

static ROWS: Mutex<Vec<(&'static str, Row)>> = Mutex::new(Vec::new());

fn record(name: &'static str, f: impl FnOnce(&mut Row)) {
    paused(|| {
        let mut rows = ROWS.lock().unwrap_or_else(|p| p.into_inner());
        let i = match rows.iter().position(|(n, _)| *n == name) {
            Some(i) => i,
            None => {
                rows.push((name, Row::default()));
                rows.len() - 1
            }
        };
        f(&mut rows[i].1);
    });
}

/// The probed future of a `noalloc` async function.
pub struct Phased<F: Future> {
    name: &'static str,
    fut: Option<Pin<Box<F>>>,
    done: bool,
}

/// Call a `noalloc` async function (`make` creates its future), counting the construct phase.
pub fn construct<F: Future>(name: &'static str, make: impl FnOnce() -> F) -> Phased<F> {
    let before = count();
    let fut = make();
    let n = count() - before;
    record(name, |r| {
        r.futures += 1;
        r.construct += n;
    });
    // the probe's own box (so the wrapper is Unpin without unsafe code) is not the function's allocation
    let fut = paused(|| Box::pin(fut));
    Phased { name, fut: Some(fut), done: false }
}

impl<F: Future> Future for Phased<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let fut = this.fut.as_mut().expect("a probed future polled after it completed");
        let before = count();
        let r = fut.as_mut().poll(cx);
        let n = count() - before;
        match r {
            Poll::Ready(_) => {
                this.done = true;
                record(this.name, |row| row.complete += n);
            }
            Poll::Pending => record(this.name, |row| {
                row.polls += 1;
                row.poll += n;
            }),
        }
        r
    }
}

impl<F: Future> Drop for Phased<F> {
    fn drop(&mut self) {
        let fut = self.fut.take();
        let before = count();
        drop(fut);
        let n = count() - before;
        let done = self.done;
        record(self.name, |r| {
            if done {
                r.dropped_after += 1;
                r.drop_after += n;
            } else {
                r.dropped_before += 1;
                r.drop_before += n;
            }
        });
    }
}

/// The probe's report: one JSON record on standard error with each probed function's allocations per phase.
pub fn report() {
    paused(|| {
        let rows = ROWS.lock().unwrap_or_else(|p| p.into_inner());
        let items: Vec<String> = rows
            .iter()
            .map(|(n, r)| {
                format!(
                    "{{\"complete\": {}, \"construct\": {}, \"drop_after\": {}, \"drop_before\": {}, \"dropped_after\": {}, \"dropped_before\": {}, \"function\": \"{n}\", \"futures\": {}, \"poll\": {}, \"polls\": {}}}",
                    r.complete, r.construct, r.drop_after, r.drop_before, r.dropped_after, r.dropped_before, r.futures, r.poll, r.polls
                )
            })
            .collect();
        eprintln!("{{\"functions\": [{}], \"kind\": \"alloc-probe\", \"rule\": \"PROG-CMP-08\"}}", items.join(", "));
    });
}

/// The probe's calibration (SP-08, Codex review finding 14): a future that allocates in every phase is counted in
/// every phase, and a future that allocates in none is counted in none. A zero the probe reports for a program is
/// therefore a measurement, not a probe that failed to count.
#[cfg(test)]
mod calibration {
    use super::*;
    use std::task::Waker;

    /// Allocates once when created, once on its pending poll, once on its ready poll and once when dropped.
    struct Allocating {
        polled: bool,
        _held: Vec<u8>,
    }

    impl Future for Allocating {
        type Output = usize;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
            let this = self.get_mut();
            let v: Vec<u8> = Vec::with_capacity(8);
            std::hint::black_box(&v);
            if this.polled {
                Poll::Ready(v.capacity())
            } else {
                this.polled = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    impl Drop for Allocating {
        fn drop(&mut self) {
            std::hint::black_box(Vec::<u8>::with_capacity(8));
        }
    }

    struct Quiet {
        polled: bool,
    }

    impl Future for Quiet {
        type Output = usize;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
            let this = self.get_mut();
            if this.polled {
                Poll::Ready(1)
            } else {
                this.polled = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    fn row(name: &str) -> (usize, usize, usize, usize, usize, usize) {
        let rows = ROWS.lock().unwrap_or_else(|p| p.into_inner());
        let r = &rows.iter().find(|(n, _)| *n == name).unwrap().1;
        (r.construct, r.poll, r.complete, r.drop_after, r.drop_before, r.polls)
    }

    fn drive<F: Future>(mut f: Pin<&mut F>) {
        let mut cx = Context::from_waker(Waker::noop());
        while f.as_mut().poll(&mut cx).is_pending() {}
    }

    #[test]
    fn every_phase_is_counted() {
        let mut done = construct("calibration_alloc", || Allocating { polled: false, _held: Vec::with_capacity(8) });
        drive(Pin::new(&mut done));
        drop(done);
        let never = construct("calibration_alloc", || Allocating { polled: false, _held: Vec::with_capacity(8) });
        drop(never);
        let (construct_n, poll_n, complete_n, drop_after_n, drop_before_n, polls) = row("calibration_alloc");
        assert!(construct_n >= 2 && poll_n >= 1 && complete_n >= 1 && drop_after_n >= 1 && drop_before_n >= 1 && polls >= 1, "{construct_n} {poll_n} {complete_n} {drop_after_n} {drop_before_n} {polls}");

        let mut quiet = construct("calibration_quiet", || Quiet { polled: false });
        drive(Pin::new(&mut quiet));
        drop(quiet);
        drop(construct("calibration_quiet", || Quiet { polled: false }));
        let (c, p, done_n, da, db, polls) = row("calibration_quiet");
        assert_eq!((c, p, done_n, da, db), (0, 0, 0, 0, 0));
        assert!(polls >= 1);
    }
}
