//! The record stream (stderr, one JSON object per line, sorted keys): runtime failures, observation frames and
//! events, test results and defects, as the definitional interpreter emits them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use fortis_runtime::telemetry;

use crate::failure::Failure;
use crate::show::Show;

/// The records a task's reporter buffers before overflow becomes visible (PROG-NFR-13).
pub const OBSERVATION_BOUND: usize = 1000;

/// The fixed storage of a held event (the record at the bound); a longer event is held as its truncation record.
const HELD_BYTES: usize = 16 * 1024;

thread_local! {
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static DROPPED: Cell<usize> = const { Cell::new(0) };
    /// the record at the bound, held back: a later record turns it into the buffer-overflow record
    static PENDING: RefCell<Option<String>> = const { RefCell::new(None) };
    static CLEANUP: Cell<usize> = const { Cell::new(0) };
    /// the held record when it is an event: written into fixed storage, so holding it back allocates nothing
    static HELD: RefCell<([u8; HELD_BYTES], usize)> = const { RefCell::new(([0; HELD_BYTES], 0)) };
    static FILE: RefCell<String> = const { RefCell::new(String::new()) };
    /// source name -> declaring module file; the first name wins, as the interpreter's `setdefault` (D-325)
    static DECL_FILES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

pub fn set_file(f: &str) {
    FILE.with(|x| *x.borrow_mut() = f.to_string());
}

/// The modules a package run loaded besides the entry. An empty list clears the map.
pub fn set_decl_files(pairs: &[(&str, &str)]) {
    DECL_FILES.with(|m| {
        let mut m = m.borrow_mut();
        m.clear();
        for (name, file) in pairs {
            m.entry((*name).to_string()).or_insert_with(|| (*file).to_string());
        }
    });
}

fn q(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Emit one record given as sorted (key, JSON value) pairs; past the bound, one buffer-overflow record counts.
pub fn emit(mut fields: Vec<(&str, String)>) {
    let n = COUNT.with(|c| c.get());
    fields.sort_by(|a, b| a.0.cmp(b.0));
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{}: {v}", q(k))).collect();
    let line = format!("{{{}}}", body.join(", "));
    if n >= OBSERVATION_BOUND {
        // past the bound the held record is replaced by one overflow record whose count grows (interpreter rule)
        PENDING.with(|p| p.borrow_mut().take());
        HELD.with(|h| h.borrow_mut().1 = 0);
        DROPPED.with(|c| c.set(if c.get() == 0 { 2 } else { c.get() + 1 }));
        return;
    }
    COUNT.with(|c| c.set(n + 1));
    if n + 1 == OBSERVATION_BOUND {
        PENDING.with(|p| *p.borrow_mut() = Some(line));
        return;
    }
    eprintln!("{line}");
}

/// The end of a run: the held record, or the overflow record with its final count.
pub fn flush() {
    if let Some(l) = PENDING.with(|p| p.borrow_mut().take()) {
        eprintln!("{l}");
    }
    HELD.with(|h| {
        let mut h = h.borrow_mut();
        let n = std::mem::take(&mut h.1);
        if n > 0 {
            use std::io::Write;
            let _ = std::io::stderr().lock().write_all(&h.0[..n]);
        }
    });
    crate::trace::flush();
    let d = DROPPED.with(|c| c.get());
    if d > 0 {
        eprintln!("{{\"bound\": {OBSERVATION_BOUND}, \"dropped\": {d}, \"kind\": \"buffer-overflow\", \"rule\": \"PROG-NFR-13\"}}");
    }
}

pub fn enter_cleanup() {
    CLEANUP.with(|c| c.set(c.get() + 1));
}
pub fn leave_cleanup() {
    CLEANUP.with(|c| c.set(c.get().saturating_sub(1)));
}

/// `report f` and an unhandled failure of `main` (10.6).
pub fn report(f: &Failure, where_: &str) {
    let mut chain = Vec::new();
    let mut c = f.cause.as_deref();
    while let Some(x) = c {
        chain.push(q(x.code));
        c = x.cause.as_deref();
    }
    let payload: Vec<String> = f.payload.iter().map(|(k, v)| format!("{}: {}", q(k), q(&v.show()))).collect();
    let origin = if f.origin.is_empty() { where_.to_string() } else { f.origin.to_string() };
    let file = DECL_FILES.with(|m| m.borrow().get(&origin).cloned()).unwrap_or_else(|| FILE.with(|x| x.borrow().clone()));
    let by = if CLEANUP.with(|c| c.get()) > 0 { "cleanup".to_string() } else { where_.to_string() };
    emit(vec![
        ("kind", q("runtime-failure")),
        ("code", q(f.code)),
        ("message", q(&f.message)),
        ("payload", format!("{{{}}}", payload.join(", "))),
        ("origin", format!("{{\"file\": {}, \"function\": {}}}", q(&file), q(&origin))),
        ("cause", format!("[{}]", chain.join(", "))),
        ("annotations", format!("[{}]", f.annotations.iter().map(|a| q(a)).collect::<Vec<_>>().join(", "))),
        ("reported_by", q(&by)),
        ("rule", q("LANG-10-06")),
    ]);
}

/// A field value of a `record`, written as the JSON string the interpreter writes (its display), straight to the
/// stream: an integer, a Boolean or text needs no heap (11.4; Appendix C.3, a record of a bounded frame).
pub trait Value {
    fn write_to(&self, w: &mut dyn std::io::Write) -> std::io::Result<()>;
    /// the display text, for the telemetry adapter (which allocates the canonical record anyway)
    fn text(&self) -> String;
}

impl Value for i64 {
    fn write_to(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        write!(w, "\"{self}\"")
    }
    fn text(&self) -> String {
        self.to_string()
    }
}

impl Value for bool {
    fn write_to(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        w.write_all(if *self { b"\"true\"" } else { b"\"false\"" })
    }
    fn text(&self) -> String {
        self.to_string()
    }
}

impl Value for &str {
    fn write_to(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        write_json(w, self)
    }
    fn text(&self) -> String {
        self.to_string()
    }
}

impl Value for String {
    fn write_to(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        write_json(w, self)
    }
    fn text(&self) -> String {
        self.clone()
    }
}

/// Any other value, by its display (this one allocates; the lowering uses it only for types without a direct form).
pub struct Shown<'a, T: Show>(pub &'a T);

impl<T: Show> Value for Shown<'_, T> {
    fn write_to(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        write_json(w, &self.0.show())
    }
    fn text(&self) -> String {
        self.0.show()
    }
}

/// `s` as a JSON string, escaped as `q` escapes it, written without building it.
fn write_json(w: &mut dyn std::io::Write, s: &str) -> std::io::Result<()> {
    w.write_all(b"\"")?;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        let esc: Option<&[u8]> = match c {
            '"' => Some(b"\\\""),
            '\\' => Some(b"\\\\"),
            '\n' => Some(b"\\n"),
            '\t' => Some(b"\\t"),
            _ => None,
        };
        if esc.is_some() || (c as u32) < 0x20 {
            w.write_all(&s.as_bytes()[start..i])?;
            match esc {
                Some(e) => w.write_all(e)?,
                None => write!(w, "\\u{:04x}", c as u32)?,
            }
            start = i + c.len_utf8();
        }
    }
    w.write_all(&s.as_bytes()[start..])?;
    w.write_all(b"\"")
}

fn write_event(w: &mut dyn std::io::Write, schema: &str, fields: &[(&str, &dyn Value)]) -> std::io::Result<()> {
    w.write_all(b"{\"fields\": {")?;
    for (i, (k, v)) in fields.iter().enumerate() {
        if i > 0 {
            w.write_all(b", ")?;
        }
        write_json(w, k)?;
        w.write_all(b": ")?;
        v.write_to(w)?;
    }
    w.write_all(b"}, \"kind\": \"observation-event\", \"rule\": \"LANG-13-01\", \"schema\": ")?;
    write_json(w, schema)?;
    w.write_all(b"}\n")
}

/// `record Schema(field: value, ...)` (13), its fields in key order: written straight to the record stream, so a
/// record of integers, Booleans and text allocates nothing. The record at the stream's bound is held back in fixed
/// storage, as `emit` holds it. The stream carries the values only under `permit-all` (Language Reference 15); under
/// a telemetry policy the canonical permitted record goes to the policy's channel (Platform 6.3).
pub fn event_with(schema: &str, version: u32, fields: &[(&str, &dyn Value)]) {
    if let telemetry::Mode::Policy(_) = telemetry::mode() {
        let texts: Vec<(&str, String)> = fields.iter().map(|(k, v)| (*k, v.text())).collect();
        let fs: Vec<(&str, &str)> = texts.iter().map(|(k, v)| (*k, v.as_str())).collect();
        telemetry::observe(&telemetry::Observation { schema, version, function: None, outcome: None, fields: &fs, truncated: &[] });
    }
    let fields = if telemetry::diagnostic_values() { fields } else { &[] };
    let n = COUNT.with(|c| c.get());
    if n >= OBSERVATION_BOUND {
        PENDING.with(|p| p.borrow_mut().take());
        HELD.with(|h| h.borrow_mut().1 = 0);
        DROPPED.with(|c| c.set(if c.get() == 0 { 2 } else { c.get() + 1 }));
        return;
    }
    COUNT.with(|c| c.set(n + 1));
    if n + 1 == OBSERVATION_BOUND {
        // held back in fixed storage: the record at the bound allocates nothing either
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            let (buf, len) = &mut *h;
            let mut cur = std::io::Cursor::new(&mut buf[..]);
            *len = if write_event(&mut cur, schema, fields).is_ok() {
                cur.position() as usize
            } else {
                // longer than the storage: held as the event's truncation record instead
                let mut cur = std::io::Cursor::new(&mut buf[..]);
                let _ = (|| -> std::io::Result<()> {
                    use std::io::Write;
                    cur.write_all(b"{\"fields\": {}, \"kind\": \"observation-event\", \"rule\": \"LANG-13-01\", \"schema\": ")?;
                    write_json(&mut cur, schema)?;
                    cur.write_all(b", \"truncated\": [\"held record\"]}\n")
                })();
                cur.position() as usize
            };
        });
        return;
    }
    let stderr = std::io::stderr();
    let mut lock = stderr.lock();
    let _ = write_event(&mut lock, schema, fields);
}

/// An `@observe` function's frame, emitted at its exit (13), its values as `event_with` writes them.
pub fn frame(schema: &str, version: u32, function: &str, fields: &[(&str, String)], outcome: &str, bounds: &[(&str, usize)]) {
    let mut fields: Vec<(&str, String)> = fields.to_vec();
    let truncated = telemetry::bound_fields(&mut fields, bounds);
    let cut: Vec<&str> = truncated.iter().map(|s| s.as_str()).collect();
    let fs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
    telemetry::observe(&telemetry::Observation { schema, version, function: Some(function), outcome: Some(outcome), fields: &fs, truncated: &cut });
    let fields = if telemetry::diagnostic_values() { &fields[..] } else { &[] };
    let fs: Vec<String> = fields.iter().map(|(k, v)| format!("{}: {}", q(k), q(v))).collect();
    emit(vec![
        ("kind", q("observation-frame")),
        ("schema", q(schema)),
        ("function", q(function)),
        ("fields", format!("{{{}}}", fs.join(", "))),
        ("outcome", q(outcome)),
        // a cut is about the value too: withheld with it
        ("truncated", format!("[{}]", if telemetry::diagnostic_values() { truncated.iter().map(|t| q(t)).collect::<Vec<_>>().join(", ") } else { String::new() })),
        ("rule", q("LANG-13-02")),
    ]);
}

pub fn test_result(test: &str, case: &str, status: &str, evidence: &str) {
    emit(vec![("kind", q("test-result")), ("test", q(test)), ("case", q(case)), ("status", q(status)), ("evidence", q(evidence)), ("rule", q("LANG-14-02"))]);
}

pub fn defect(class: &str, task: &str) {
    emit(vec![("kind", q("defect")), ("class", q(class)), ("task", q(task)), ("rule", q("LANG-10-06"))]);
}

/// A detached task's completion, observed by its supervisor (12.6).
pub fn task_completion(task: &str, outcome: &str) {
    emit(vec![("kind", q("task-completion")), ("task", q(task)), ("outcome", q(outcome)), ("rule", q("LANG-12-04"))]);
}

/// A task cancelled before it was first polled (12.3).
pub fn task_lifecycle(function: &str, state: &str) {
    emit(vec![("kind", q("task-lifecycle")), ("function", q(function)), ("state", q(state)), ("rule", q("LANG-12-03"))]);
}

/// The program's observed `async` functions: (function, schema, schema version), registered by `main` (D-27).
static OBSERVED: std::sync::OnceLock<&'static [(&'static str, &'static str, u32)]> = std::sync::OnceLock::new();

/// Register the program's observed `async` functions, whose tasks may be cancelled before they start.
pub fn observed(table: &'static [(&'static str, &'static str, u32)]) {
    let _ = OBSERVED.set(table);
}

/// A task cancelled before it started: its `not_started` lifecycle record, then, for an observed function, a frame
/// marked `not_started` without any captured fields (12.3, 13; D-27).
pub fn not_started(function: &str) {
    task_lifecycle(function, "not_started");
    if let Some((_, schema, version)) = OBSERVED.get().and_then(|t| t.iter().find(|(f, _, _)| *f == function)) {
        frame(schema, *version, function, &[], "not_started", &[]);
    }
}
