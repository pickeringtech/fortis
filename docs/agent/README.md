# Fortis for agents

<!-- covers: LR 0 -->
*Agent mode. Human mode: [the Fortis guide](../guide/README.md). Source of truth: the
[Language Reference](../Fortis_Language_Reference.md) (LR).*

Fortis is a statically checked, compiled, library-first language built so that code written mostly by agents can
be reviewed and trusted by people: every ownership operation, mutation permission, failure disposition, effect
claim and observation point is written in the source (LR 1). This mode states the rules densely, by rule ID, with
the records the tools print and the corrections that fix them. It restates the LR; it never extends it.

## Precedence of sources

1. The Language Reference (`LANG-` rules) is normative for the core; where its prose and the grammar
   `reference/grammar/fortis.ebnf` disagree on syntax, the grammar wins; where a rule and an example disagree, the
   rule wins (LR 0).
2. The Platform Specification (`PLAT-` rules) defines the kits. A `PROG-` ID, in a record's `rule`, names a
   requirement on the toolchain itself: its tools, records and process.
3. This agent mode and the human guide are derived from them. A page that contradicts them is a defect in the page.

Decisions are logged as register rows `X-nnn`, and known toolchain defects as `D-nn`, in the maintainers'
planning records; pages cite them by ID. What v0.1.0 runs, and what it still refuses, is
[Known limitations](known-limitations.md); the release note is
[v0.1.0](../releases/v0.1.0.md).

## Tools

Run from the repository root after `cd compiler && cargo build --release`. `fortis` is `bin/fortis`.

| Command | Output (pipe, or `--format json`) | Use |
|---|---|---|
| `fortis view PATH` / `--decls` / `--symbol NAME` / `--query Q` / `--json` | JSON lines, `kind` one of `line`, `declaration`, `symbol`, `match`, `query` | read `.ft` source; never `cat`/`grep` |
| `fortis check FILE.ft\|DIR --format json` | one diagnostic record per line; none means clean | all static checks |
| `fortis run FILE.ft\|DIR [ARGS…]` | the program's standard output; runtime records on standard error | run `main` on the definitional interpreter with `ARGS` as its arguments; files from `FORTIS_DEV_ROOT` (relative to the working directory), else a fresh empty directory |
| `fortis test FILE.ft\|DIR --format json` | `test-result` records on standard error, then one `test-summary` on standard output | run tests and properties on the interpreter |
| `fortisc test FILE.ft\|DIR --format json` | the same records, on the same streams | run tests on the development VM |
| `fortisc vm FILE.ft\|DIR` | `observed: line\|line\|…` (the conformance form) | run `main` on the development VM |
| `fortis build FILE.ft\|DIR --out APP [--emit-only]` | one `build` record (`binary`, `manifest`); `--emit-only`: one `emit` record (`program`, `crate`) | native executable through safe Rust; `--emit-only` writes the generated crate where the build would compile it (`build/native-build/`) and builds nothing |
| `fortis fmt --check PATH` / `--write PATH` | files not in canonical layout / rewritten | canonical layout, after rewriting older forms into the forms these pages teach where the module checks and the meaning is unchanged (`--no-migrate`: layout only) |

Every finding and runtime report is a record first; text is rendered from it, and the stream chooses the form
(JSON lines on a pipe) unless `--format` says otherwise. Schemas and record kinds are in
[records](records.md).

A diagnostic record (`reference/schemas/diagnostic.schema.json`), here for a use after a move
(LR 7.4):

```fortis
// reject: E-OWN-020
pure func size(items: owned List[Int]) Int {
  let kept = move items
  return items.count()                // reject: E-OWN-020
}

test "size" {
  expect size(List.of(1, 2)) == 2
}
```

```records
{"class": "Ownership", "code": "E-OWN-020", "column": 10, "fields": {"conflict": "use after move", "loan_region": "", "place": "items", "spans": "items"}, "file": "example.ft", "line": 4, "message": "`items` was moved and is used again (read)", "rule": "LANG-07-04", "severity": "error", "status": "violated"}
```

- `code`: stable; look it up in [codes](codes.md). `rule`: the `LANG-`/`PLAT-`/`PROG-` ID it enforces.
- `status`: `violated` (a proved violation), `unproved` (the checker could not prove a refinement or claim; not
  proof of a bug), `unknown` (a trust boundary).
- `fields`: the class's mandatory fields. `correction`, when present, is an applicable edit:
  `{"kind": …, "edits": [{"file", "line", "column", "end_line", "end_column", "replacement"}]}`.

A runtime failure is written to standard error once, by `report`, by a cleanup, by a scope abandoning a task's
result, or by a failing `try main` (LR 4.6, LR 10.6, LR 12.1, LR 12.5), as
a record with `kind: "runtime-failure"`, the failure's `code` and `payload`, its `origin` (`file`, `function`),
`cause` and `annotations` (the `annotate` trail), `reported_by` (`report`, `main`, `cleanup` or `abandoned`) and
`rule`. Tests write
`test-result` records (`test`, `case`, `status`, `evidence`) and one `test-summary` (`passed`, `failed`).

## The loop

1. Inspect: `fortis view --decls DIR`, `fortis view --symbol NAME DIR`, `fortis view --query Q DIR` (queries:
   `async`, `try`, `async-try`, `pure`, `noalloc`, `trusted`, `recovers`, `editable-params`, `owned-params`,
   `or-fail`, `or-recover`, `catch`, `fail`, `return`, `defer`, `scope`, `capture`, `expect`, `move`, `lambda`,
   `tests`, `errors`, `classes`, `aspect:NAME`).
2. Edit.
3. Check: `fortis check DIR --format json`. For each record: read `code`, `rule`, `fields`; apply `correction`
   when present; otherwise use the correction pattern of the code's page.
4. Test: `fortis test DIR --format json`; a failing test is evidence, not an obstacle.
5. Format: `fortis fmt --write DIR`, so inferred facts are printed back and old forms take their reading forms where
   they mean the same: unmarked `editable self` receivers and `fn mut` calls, `return local`, heads without `Void`,
   `main`'s capability parameters, implied `or fail` in tests, `Map.get` keys without `copy`, puns, or-pattern arms,
   checked operators, bare variants (LR 1.1, X-345).
6. Repeat until `check` prints nothing and every test passes.

## Prohibitions

- Do not read `.ft` files with `cat`, `head`, `tail`, `sed -n` or `less`, or search them with `grep` or `rg`; use
  `fortis view`.
- Do not weaken a contract, remove an `expect`, or change an expected output to make a check or test pass.
 
- Do not lower a threshold or delete an assertion without a visible, approved policy change.
- Do not hand-format Fortis code; run `fortis fmt --write`.
- Do not treat `unproved` as a bug or as safe: add the explicit proof or checked alternative the record names.

## Example notation on these pages

Every `fortis` block is checked by `python3 -I conformance/tools/docset_check.py` on the real toolchain.
First line: `// accept` (both checkers report nothing; it runs: `main` on the interpreter, the VM
and native code with exactly the `output` block shown, or its tests pass on all three; `(exit status N)` when
`main` fails on purpose), `// reject: CODE` (exactly that code on each line carrying the same marker, nothing
else), `// illustrative` (not a program). A `records` block is the exact JSON the checker prints for the block
above it, with the file named `example.ft` (LR 0).

## Pages

| Page | LR |
|---|---|
| [Overview: invariants and notation](01-overview.md) | 1, 2 |
| [Lexical structure](03-lexical.md) | 3 |
| [Modules and packages](04-modules.md) | 4 |
| [Statements and expressions](05-statements.md) | 5 |
| [Types](06-types.md) | 6 |
| [Proving arithmetic and indexing](06-proofs.md) | 6.3, 6.4, 6.7 |
| [Ownership](07-ownership.md) | 7 |
| [Functions and closures](08-functions.md) | 8 |
| [Classes, interfaces, mixins and enums](09-classes.md) | 9 |
| [Outcomes and failures](10-outcomes.md) | 10 |
| [Effects and contracts](11-effects.md) | 11 |
| [Cleanup, async and concurrency](12-async.md) | 12 |
| [Observation](13-observation.md) | 13 |
| [Tests](14-tests.md) | 14 |
| [Core runtime](15-runtime.md) | 15 |
| [Kits and declaration heads](16-kits.md) | 16 |
| [Embedded languages](embedded-languages.md) (the raw template region; blocks are a later edition, X-145) | 3, 16 |
| [Keywords](appendix-a-keywords.md), [closed decisions](appendix-b-decisions.md), [lowering](appendix-c-lowering.md), [rule index](appendix-d-rules.md) | A, B, C, D |
| [Records](records.md): diagnostic, runtime, test, view and build records | 0 |
| [Codes](codes.md): every diagnostic code with its class, rule, section and correction | all |
| [Glossary](glossary.md): kit, package, toolchain component, head, binding, verifier | E |
| [Coffee shop worked example](worked-example.md): construction order, rules and backend checks | LR 6, LR 8, LR 9, LR 10, LR 11.5, LR 14 |
| [Core library](library.md): every exported operation of the seven core modules with its effects and contract (generated; human mode: [The core library](../guide/library.md)) | LR 15 |

The skill `skills/fortis-code` (installed with `fortis skill install`) points agents here.
