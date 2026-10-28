//! The daemon's protocol (SP-12, spikes/SP-12/design.md 2): one JSON request per line on standard input, one reply
//! line per request on standard output.
//!
//! | request | reply |
//! |---|---|
//! | `{"op":"open"}` | the package checked from scratch |
//! | `{"op":"edit","file":F,"text":T}` | the package checked after module `F` (relative to the package) has text `T` |
//! | `{"op":"verify"}` | the deep comparison with a fresh database (design 9) |
//! | `{"op":"stats"}` | live memos by kind, the resident set and the heap in use |
//! | `{"op":"exit"}` | none |
//!
//! A check's reply carries `records` (each record's line exactly as `fortisc check --format json` prints it),
//! `digest` (the SHA-256 of those lines joined by newlines), `ran` (queries by kind, every pass that ran, the
//! checker contexts built), `memos` and `us` (the daemon's own time for the request, in microseconds).

use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Instant;

use fortis_diag::Json;
use fortis_names::sha256::sha256;

use crate::package::{heap, rss, Pkg, Ran};

fn check_reply(db: &Pkg, recs: &[fortis_diag::Record], ran: &Ran, us: u128) -> Json {
    let mut lines: Vec<String> = recs.iter().map(|r| r.to_json_line()).collect();
    let digest = sha256(lines.join("\n").as_bytes());
    // fault `wire-corrupt`: the reply loses its last record but keeps the digest of them all
    if crate::package::fault("wire-corrupt") {
        lines.pop();
    }
    let passes = Json::Arr(ran.passes.iter().map(|(f, u, p)| Json::Arr(vec![Json::str(&db.module_file(*f)), Json::str(u), Json::Int(*p as i64)])).collect());
    Json::obj([
        ("ok", Json::Bool(true)),
        ("records", Json::strs(lines)),
        ("digest", Json::str(&digest)),
        (
            "ran",
            Json::obj([
                ("by_kind", Json::Obj(ran.by_kind.iter().map(|(k, v)| (k.clone(), Json::Int(*v as i64))).collect())),
                ("passes", passes),
                ("prepared", Json::Int(ran.prepared as i64)),
            ]),
        ),
        ("memos", Json::Int(db.memo_count() as i64)),
        ("us", Json::Int(us as i64)),
    ])
}

fn error(msg: &str) -> Json {
    Json::obj([("ok", Json::Bool(false)), ("error", Json::str(msg))])
}

/// Serve requests for the package at `dir` until `exit` or the end of the input.
pub fn serve(dir: &Path, input: impl BufRead, mut out: impl Write) -> std::io::Result<()> {
    // fault `free-leak`: the allocator stops reusing some returned blocks (a fault build only)
    if crate::package::fault("free-leak") {
        crate::alloc::leak_frees();
    }
    // fault `system-mix`: the allocator charges some requests by the system allocator's history (a fault build only)
    if crate::package::fault("system-mix") {
        crate::alloc::mix_system();
    }
    let mut db: Option<Pkg> = None;
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req = match Json::parse(&line) {
            Ok(j) => j,
            Err(e) => {
                writeln!(out, "{}", error(&format!("unreadable request: {e}")).to_line())?;
                out.flush()?;
                continue;
            }
        };
        let op = match req.get("op") {
            Some(Json::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let t = Instant::now();
        let reply = match (op.as_str(), db.as_mut()) {
            ("open", _) => match Pkg::open(dir) {
                Ok(p) => {
                    let (recs, ran) = p.check();
                    let r = check_reply(&p, &recs, &ran, t.elapsed().as_micros());
                    db = Some(p);
                    r
                }
                Err(e) => error(&e),
            },
            // an edit names its file and its whole new text; a malformed one changes nothing (SP-12 closing review 3)
            ("edit", Some(p)) => match (req.get("file"), req.get("text")) {
                (Some(Json::Str(file)), Some(Json::Str(text))) => match p.module_of(file) {
                    None => error(&format!("{file} is not a module of the package")),
                    Some(f) => match p.set_source(f, text) {
                        Err(e) => error(&e),
                        Ok(()) => {
                            let (recs, ran) = p.check();
                            check_reply(p, &recs, &ran, t.elapsed().as_micros())
                        }
                    },
                },
                _ => error("an edit needs `file` and `text`, both strings"),
            },
            // fault `verify-broken`: the deep comparison answers with an error
            ("verify", Some(_)) if crate::package::fault("verify-broken") => error("the deep comparison is unavailable"),
            ("verify", Some(p)) => {
                let diffs = p.verify();
                Json::obj([("ok", Json::Bool(true)), ("differences", Json::strs(diffs)), ("us", Json::Int(t.elapsed().as_micros() as i64))])
            }
            ("stats", Some(p)) => {
                let (rss_kib, hwm_kib) = rss();
                let a = crate::alloc::stats();
                Json::obj([
                    ("ok", Json::Bool(true)),
                    ("memos", Json::Int(p.memo_count() as i64)),
                    ("by_kind", Json::Obj(p.memo_counts().into_iter().map(|(k, v)| (k.to_string(), Json::Int(v as i64))).collect())),
                    ("rss_kib", Json::Int(rss_kib as i64)),
                    ("hwm_kib", Json::Int(hwm_kib as i64)),
                    ("heap_bytes", Json::Int(heap() as i64)),
                    // the daemon's allocator (X-141): live bytes requested, live allocations, the bytes their blocks
                    // take, its footprint (each class's peak) and what it mapped; zero under another allocator
                    ("live_bytes", Json::Int(a.live_bytes)),
                    ("live_allocs", Json::Int(a.live_allocs)),
                    ("in_use_bytes", Json::Int(a.in_use)),
                    ("footprint_bytes", Json::Int(a.footprint)),
                    ("mapped_bytes", Json::Int(a.mapped)),
                ])
            }
            ("exit", _) => break,
            (_, None) => error("open the package first"),
            (other, _) => error(&format!("unknown request {other}")),
        };
        writeln!(out, "{}", reply.to_line())?;
        out.flush()?;
    }
    Ok(())
}
