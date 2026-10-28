# Definitional interpreter — architecture (DEL-08 skeleton, DEL-21 synchronous language)

The interpreter is the oracle the Programme's `proved` claim rests on (6.3). It evaluates the parse
tree from `conformance/tools/parser.py` directly; no IR, no bytecode. Clarity over speed.

## Pipeline

1. **Lex and parse** (`conformance/tools/lexer.py`, `parser.py`): the same tools as the syntax corpus; the
   derivation is greedy (longest span per sequence item), which is how `fail X()` and lambda bodies bind.
2. **Declarations** (`Interpreter.load_declarations`): classes (fields with default `constExpr`, methods),
   enums (variants with payload field names, methods), errors (code, payload, message), functions, consts,
   imports (member → package path, used only to name a kit at run time).
3. **Evaluation** (`Interpreter.eval`, `run_statement`, `call`): environments are chains of cells
   `(value, mutable, moved)`; control flow is signals (`Return`, `Give`, `Break`, `Continue`, `Recover`,
   `FailureSignal`). A `try` function returns a `Failure` value instead of raising; `or fail` re-raises;
   `catch e { … recover v }` runs the block with `e` bound.
4. **Records** (`interpreter/records.py`, PROG-DIA-07): `report e` and a failing `try main` write one
   `runtime-failure` record each; `@observe` frames and `record` statements write `observation-frame` /
   `observation-event` records; test cases write `test-result` records. Every record is validated against
   its schema before it is emitted, as JSON lines when standard error is not a terminal and as text otherwise.
   Console output goes through the `Console` capability from `Host.capabilities()`.
5. **Aspects** (`call_with_aspects`, 11.8): a function with `@` applications runs its before hooks in
   application order, the body, the after hooks in reverse, and on a failure the handlers innermost first;
   `needs` names bind to the target's parameter of that type; specialisations forward `...rest`.
6. **Tests** (`run_tests`, 14): table rows bind to the `as` name; properties draw 25 cases per run from the
   parameter types with a seed derived from the title; `expect` evidence names the row.

The `fortis` command (`bin/fortis check|run|test`) runs the static checker first and never evaluates a
program it rejects; `conformance/tools/execute_check.py` enforces the same order on the corpus.

## Where each later judgement plugs in

Every dynamic check that stands in for a static rule is marked `# plug-in:` in `eval.py`. The slot is the
function that raises `FortisError` today; the static layer replaces the dynamic failure with a rejection before
execution, and the harness (DEL-09) compares the two.

| Judgement | Dynamic stand-in today | Chunk that makes it static |
|---|---|---|
| Names | `Env.get` raises on an undeclared name (resolver already rejects statically) | DEL-06/07 (done) |
| Types | `expect_bool`, `binary` type checks, `Instance` field existence, `validate` range check | DEL-10–DEL-13 |
| Refinements and bounds | `index` out-of-range, `check_int` overflow on plain operators, `validate` | DEL-13 |
| Loans and moves | `Cell.moved` use-after-move, `move` of a field path (partial move) | DEL-14–DEL-16 |
| Obligations | a `catch` block that falls through; a `Failure` value never consumed is *not yet* detected | DEL-17–DEL-18 |
| Effects | `fail` inside a function without `try`; `await` refused | DEL-19–DEL-20 |
| Cleanup, async, pools | `defer`, `scope`, `await`, `capture` refused | DEL-22–DEL-24 |
| Kits | an imported kit name used at run time produces `kit not available` | kit groups |

## Fixture classes

`// requires: runtime` marks a fixture the kernel runs; its first line `// execute: <stdout joined by |>`
or `// execute: fail:<code>` is the expectation. A `// reject:` fixture may not carry `requires: runtime`
(the harness refuses it): the kernel makes no static claims. `conformance/tools/execute_check.py` is the
runner; results go to `conformance/execution/results.json` with backend `interpreter`.
