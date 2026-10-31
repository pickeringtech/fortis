# Testing Fortis

Unit tests form the base of the testing pyramid. Every change adds unit tests for the functions it changes,
in both languages where both implement them. Run these first: they should take seconds. Test a checker rule
through a shared case table so Python and Rust judge the same inputs and expected results. Use seeded property
tests where the logic is algebraic. Corpus programs belong above these tests, for end-to-end behaviour and
interactions between components.

The owner chose to build this foundation alongside the v0.1 release candidate on 30 October 2026. The follow-on
streams U1 (front end), U2 (ownership and obligations) and U3 (back ends and runtime) target about **1,500 Python
and 2,500 Rust unit tests**, with each language's unit suite taking **under a minute on a laptop**, after building.
These are targets, not current counts. Coverage reports using coverage.py and cargo-llvm-cov come later; neither
is installed or required now. Python uses the standard library and the Rust workspace has no external dependencies.

## Layout and commands

- Python: `tests/python/<area>/test_*.py`, using `unittest`. Give each area an `__init__.py` for discovery.
  Import `support` first; it derives the repository root from its own file and makes the reference modules
  importable even under `python3 -I`. Use area names distinct from production modules, such as `lexing`.
- Rust: `#[cfg(test)] mod tests` beside the function, or `compiler/crates/<crate>/tests/` for public functions.
  Add `fortis-testkit = { path = "../fortis-testkit" }` under `[dev-dependencies]` when using the helpers.
- Shared data: `tests/cases/<component>/<name>.json`. See [the case schema](cases/README.md).

Run from the repository root:

```bash
python3 -I conformance/tools/unit_check.py
python3 -I conformance/tools/unit_check.py --json
cargo test --manifest-path compiler/Cargo.toml
cargo test --manifest-path compiler/Cargo.toml -p fortis-types --test intervals
```

The Python runner discovers every `test_*.py` under `tests/python` and runs it in-process. It prints the test
count and elapsed time, and fails for a failing test, import error or empty discovery. `--json` writes one summary
object to stdout; unittest's progress and failure details go to stderr. Cargo remains the Rust runner, including
its normal test filters. The gate's `unit` step has a one-minute deadline and runs before builds or corpus checks;
a failure stops that gate run. Every CI shard runs it first, with exactly one owning shard and the others marking
it as a prerequisite. CI also checks it before its build and Rust test jobs compile. The quick tier includes
`cargo-test`, run by CI's Rust test job.

For whole programs, run `python3 -I conformance/tools/gate.py --tier quick`. Checker agreement is
`python3 -I conformance/tools/frontend_check.py --binary compiler/target/release/fortisc` after a release build.
The full gate belongs to the release candidate, not an individual PR. See [CONTRIBUTING.md](../CONTRIBUTING.md).

## Copying the first tests

The interval, lexer and text suites call the existing functions directly, without building a Fortis program.
`table_tests(TestClass, path, check)` installs one Python test method per named row. Rust's
`case_tests!(path, check; name_one, name_two)` creates normal named Cargo tests; its extra registration test fails
if a row is missing from that list. Add the expected result once, in JSON, and register its name in Rust.

U0 supplies 66 shared rows: 22 interval arithmetic, containment and refinement-subtyping cases, 24 lexer cases,
and 20 text cases. Both languages also run three interval properties. Python checks both text receiver types in
each row. Foundation tests check discovery, failure exits, gate order, shard coverage, replay and table reading.

Python's `@forall(generator, cases=200, seed=None)` decorates a check taking a generated value, including a
`unittest` method taking `self, value`. The generator receives a private `random.Random`. Rust's
`forall(generator, cases, seed, check)` supplies a small SplitMix64 `Rng`, with `next_u64()` and inclusive
`between(lo, hi)`. Both default to seed 0; `FORTIS_TEST_SEED` overrides an explicit seed. Seeds are unsigned
64-bit decimal integers. Failures report the seed, case index and failing value, with the original assertion.

```bash
FORTIS_TEST_SEED=123 python3 -I conformance/tools/unit_check.py
FORTIS_TEST_SEED=123 cargo test --manifest-path compiler/Cargo.toml -p fortis-types --test intervals
```

Replay is within each language: Python and Rust use different generators. Shared tables provide identical
cross-language cases. The interval properties generate 200 pairs of small intervals per operation and exhaust
every concrete operand pair within each interval, checking that its result lies within the computed interval.
These helpers deliberately provide no shrinking framework or external dependencies.
