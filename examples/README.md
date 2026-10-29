# Fortis examples

Runnable programs for the reference toolchain. Every example is checked statically and then run on the
definitional interpreter; nothing here compiles to native code yet (the first native slice is delivery chunk DEL-25).

```
bin/fortis run  examples/01_hello.ft
bin/fortis run  examples/02_outcomes.ft      # exits 1: main propagates a failure (one record on stderr)
bin/fortis run  examples/03_ownership.ft
bin/fortis run  examples/04_collections.ft
bin/fortis run  examples/05_types.ft
bin/fortis run  examples/06_numeric.ft
bin/fortis run  examples/07_aspects.ft
bin/fortis run  examples/08_observation.ft   # frame and event records on stderr
bin/fortis test examples/09_tests.ft         # exits 1: one test fails on purpose
bin/fortis check examples/10_rejected.ft     # the checker's findings, one record per line
```

| Example | What it shows |
|---|---|
| `01_hello` | `main`, capabilities from `Host.capabilities()`, console output, interpolation, ranges |
| `02_outcomes` | `try` functions, declared errors with payloads, `or fail`, `or recover`, `catch`, `match` over outcomes, `report`, the `recovers` modifier |
| `03_ownership` | `editable` and `alias` loans at the call, `move`, `owned` parameters, `cleanup` in reverse declaration order |
| `04_collections` | `List` operations, `fn` callbacks, `fn mut` with an editable capture, `??` on an optional |
| `05_types` | refinement newtypes (`Age.validate`, proved literals), enums with payloads and methods, exhaustive `match` |
| `06_numeric` | checked `Int` operations as outcomes, wrapping and saturating forms, exact `Decimal`, `Duration`, floats |
| `07_aspects` | user aspects: `needs`, `before`, `after`, `on failure`; hook order around a function |
| `08_observation` | `@observe` schemas, `capture` snapshots into the activation frame, `record` events |
| `09_tests` | table tests bound with `as`, plain tests as `try` bodies, properties drawn from parameter types |
| `10_rejected` | a program the checker refuses: a dropped outcome, a use after move, an impure claim |

Records (runtime failures, observation frames and events, test results) go to standard error as text on a
terminal and as JSON lines on a pipe; `--format json|text` overrides. Exit codes: 0 success, 1 a failing
`try main` or any checker finding, 2 a dynamic check outside the kernel, 3 a defect such as stack exhaustion.
