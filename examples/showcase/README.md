# Release showcases

Each release keeps a pinned program and its expected standard output. The gate's `showcase` step checks it
with both checkers, runs it and its tests on the interpreter, VM and native backend, and compares the output.
Failures reported by the coffee program go to standard error, separately from the expected standard output.

| Release | Program | What it shows |
|---|---|---|
| v0.1.0 | [Coffee shop](v0.1.0-coffee.ft) | Refinements, newtypes, conversions, interfaces, mixins, contracts, generics, linear values, text, collections, inferred captures and tests. |

Read the [step-by-step walkthrough](../../docs/guide/worked-example.md), or run from the repository root:

```sh
bin/fortis check examples/showcase/v0.1.0-coffee.ft
bin/fortis run examples/showcase/v0.1.0-coffee.ft
bin/fortis test examples/showcase/v0.1.0-coffee.ft
compiler/target/release/fortisc vm --stdout examples/showcase/v0.1.0-coffee.ft
bin/fortis build examples/showcase/v0.1.0-coffee.ft --out build/coffee
build/coffee
```
