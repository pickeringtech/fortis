//! `fortisd DIR`: the incremental daemon over a package (Programme 5.1; SP-12), JSON lines on standard input and
//! output (fortis-query-db `daemon`). It runs on the size-class allocator (fortis-query-db `alloc`), which also
//! reports the live bytes requested, the live allocations, the bytes their blocks take and its footprint: the daemon's
//! retention, counted at the allocation boundary (X-141). `fortisc daemon DIR` runs it.

use std::path::Path;
use std::process::ExitCode;

#[global_allocator]
static ALLOC: fortis_query_db::alloc::Classes = fortis_query_db::alloc::Classes;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir] = args.as_slice() else {
        eprintln!("usage: fortisd DIR");
        return ExitCode::FAILURE;
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match fortis_query_db::daemon::serve(Path::new(dir), stdin.lock(), std::io::BufWriter::new(stdout.lock())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("daemon: {e}");
            ExitCode::FAILURE
        }
    }
}
