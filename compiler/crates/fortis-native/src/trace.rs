//! The run's trace (scheduler and block-exit events), when `FORTIS_TRACE` is set; the conformance harness compares it
//! with a fixture's `execute-trace` (12.3). A diagnostic facility, separate from the executor: a synchronous program
//! that records block-exit events links no executor state, and an untraced run formats and allocates nothing.

use std::cell::{Cell, RefCell};

thread_local! {
    static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// 0 until `FORTIS_TRACE` is read; then 1 (off) or 2 (on)
    static TRACING: Cell<u8> = const { Cell::new(0) };
}

/// Whether the run is traced.
pub fn tracing() -> bool {
    TRACING.with(|t| match t.get() {
        0 => {
            let on = std::env::var_os("FORTIS_TRACE").is_some();
            t.set(if on { 2 } else { 1 });
            on
        }
        v => v == 2,
    })
}

/// Turn the trace on or off (tests; programs read `FORTIS_TRACE`).
pub fn set_tracing(on: bool) {
    TRACING.with(|t| t.set(if on { 2 } else { 1 }));
}

/// Record an event; its text is formatted only when tracing.
pub fn record(event: impl FnOnce() -> String) {
    if tracing() {
        let text = event();
        EVENTS.with(|e| e.borrow_mut().push(text));
    }
}

/// A block-exit event the lowered code records (`loans-end`, `defer`, `abandon T`, `drop T`).
#[inline]
pub fn event(text: &str) {
    record(|| text.to_string());
}

/// The events so far (the trace is emptied).
pub fn take() -> Vec<String> {
    EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

/// The run's trace as one JSON record on standard error (`executor-trace`), when tracing and not empty.
pub fn flush() {
    if !tracing() {
        return;
    }
    let events = take();
    if events.is_empty() {
        return;
    }
    let quoted: Vec<String> = events.iter().map(|e| format!("\"{}\"", e.replace('\\', "\\\\").replace('"', "\\\""))).collect();
    eprintln!("{{\"events\": [{}], \"kind\": \"executor-trace\"}}", quoted.join(", "));
}
