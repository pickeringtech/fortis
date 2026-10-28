# Experimental lowering (DEL-25)

A thin lowering of the synchronous core of Fortis to safe Rust, used to close the G1 spikes with native evidence.
It is not the production backend (DEL-36 onward); it exists to show that every accepted program of the
ownership, closure, obligation, noalloc and purity corpora has a lowering `rustc` accepts, with no `unsafe`
and no repair copy.

| File | Role |
|---|---|
| `lower.py` | checks a module with the reference checker (a rejected program is never lowered) and writes a Cargo crate |
| `runtime/` | the audited runtime crate `fortis_rt`: failures with the bounded payload and the aborting `Drop` backstop, lists, text, `Rc` sharing, capabilities, the counting allocator probe |
| `native.py` | lowers, builds and runs a program or the whole DEL-25 corpus into `build/native/` |
| `audit.py` | copy audit (every clone or reference count attributed) and purity audit (no interior mutability) |
| `probe_selftest.py` | shows the allocator probe aborts an allocating `noalloc` body |

`bin/fortis build` no longer runs this lowering: it runs the production compiler's `fortisc build` (D-56). Run this
one directly:

```
python3 -I compiler/experimental-lowering/native.py conformance/programs/exec_hello.ft
python3 -I compiler/experimental-lowering/native.py --corpus
python3 -I conformance/tools/differential.py --left reference --right native
```

Outside the slice: `defer`, aspects, observation statements, async and the pools, Decimal and Duration, anonymous
records, kit modules. Decisions are logged as X-105.
