# Fortis performance against Rust

`python3 -I benchmarks/run.py [--runs N] [--vm] [--json]`

Each benchmark is a Fortis program in `fortis/` and a Rust program in `rust/` that performs the same operations
in the same order, reviewed independently for equal work before the DEL-40 measurement
(the DEL-40 kernel reviews). The Fortis program is checked, lowered to safe Rust by the production
lowering (`fortisc native`, `#![forbid(unsafe_code)]`, overflow checks on, `panic = "abort"`) and built in release
mode. The Rust program is built with the same code-generation settings: `rustc -C opt-level=3 -C codegen-units=1
-C overflow-checks=yes -C panic=abort`. The runner alternates the two programs in a seeded random order per round,
checks every run's exit status and output, and reports each side's median, minimum and maximum, and the ratio of
medians with a 95% bootstrap interval over whole rounds. A baseline pair (`baseline/`) shows the fixed cost of a run.

| benchmark | what it exercises |
|---|---|
| fib | recursive calls and integer arithmetic |
| loops | a tight integer loop |
| lists | list building, library `map` and `filter` with closures, iteration |
| maps | a separate lookup and insertion per key in an insertion-ordered map |
| objects | small value classes and method calls, a result that depends on every iteration |
| sort | a user-written merge sort (saturating index arithmetic, optional element reads) |
| strings | string interpolation and scalar counting |

What each one measures:

- **fib, loops, objects, sort** measure the lowering: the generated code against hand-written Rust.
- **lists** measures the lowering together with the native bodies of `List.map` and `List.filter`. Those are
  hand-written in the runtime with iterators, as a Rust programmer would write them.
- **maps** measures the lowering and the native `Map`. The Rust program builds the same structure itself: an
  insertion-ordered map (Fortis maps iterate in first-insertion order, 6.7), using the same folded-multiply hasher
  (X-119).
- **strings** measures the lowering and native interpolation. The Rust program writes digits directly into a sized
  string, as the runtime does, rather than using `format!`.
