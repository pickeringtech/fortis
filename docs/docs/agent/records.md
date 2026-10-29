# Records

<!-- covers: LR 0 -->
*Agent mode. Source of truth: the schema `reference/schemas/diagnostic.schema.json`, and LR 10.6, LR 10.8, LR 13
and LR 14 for the runtime records.*

Every finding, runtime failure report, test result and observation the toolchain produces is a record first: a
JSON object of a known kind, from which the text a person sees is rendered. Read the
record, not the text.

## Streams

| Situation | What is written |
|---|---|
| standard output is a pipe | one JSON object per line |
| standard output is a terminal | text rendered from the same record |
| `--format json` / `--format text` | forces that rendering |
| a running program's records (failure reports, defects, logs, observation frames and events) | standard error, same rule; the program's own output is standard output (LR 4.6) |

Where each command writes, as the toolchain does today:

| Command | Standard output | Standard error |
|---|---|---|
| `fortis check`, `fortisc check` | diagnostic records | — |
| `fortis run`, a native executable | the program's output | runtime records |
| `fortis test` (interpreter) | the `test-summary` record | `test-result` and other runtime records |
| `fortisc test` (VM) | the `test-summary` record | `test-result` and other runtime records, as `fortis test` writes them |
| `fortis build`, `fortisc build` | the `build` record | diagnostic records if the check fails |
| `fortisc vm` | the conformance format: `=== FILE`, `observed: …` (output lines joined by `\|`), `trace: …`, `record: …` | — |

A finding has one identity: `fortis check`, the daemon and the agent protocol produce byte-equal records for the
same snapshot. `fortis check` (the reference checker) and `fortisc check` (the
production checker) print records of this one schema, with the same codes at the same lines.

## Diagnostic records

Schema: `reference/schemas/diagnostic.schema.json`.

| Field | Type | Meaning |
|---|---|---|
| `code` | string | stable diagnostic code, `E-XXX-NNN` |
| `class` | string | the diagnostic class (table below) |
| `rule` | string | the normative rule the record enforces (`LANG-`, `PROG-` or `PLAT-` ID) |
| `file`, `line`, `column` | string, integer, integer | primary location, the user's nearest responsible operation (1-based) |
| `severity` | `error` or `warning` | a warning does not fail the check |
| `status` | `violated`, `unproved` or `unknown` | a proved violation, an inability to prove, an unknown trust boundary |
| `message` | string | one-line explanation |
| `fields` | object | the class's mandatory fields |
| `correction` | object, optional | `{kind, edits: [{file, line, column, end_line, end_column, replacement}]}`, an applicable edit |

`unproved` means the checker could not prove a bound, not that the program is wrong: supply the proof or use the
checked alternative the message names (LR 6.3). A `correction`'s `kind` is a short
description of the edit (for example `add the marker`). A fix preserves behaviour, changes it or weakens a
contract, but today's records do not carry that class, so judge it from the edit itself; the compiler never
removes `noalloc` or a security requirement to make code compile, and an agent never applies an edit that weakens
a claim.

### Classes and mandatory fields

| Class | Code prefix | Mandatory `fields` | Correction |
|---|---|---|---|
| Type | `E-TYP`, `E-MIX-001` (LR 6.10) | `expected_type`, `actual_type`, `primary_span` | behaviour-preserving or behaviour-changing fix |
| Ownership | `E-OWN` | `place`, `conflict`, `spans`, `loan_region` | marker, block restructuring, `copy` |
| Obligation | `E-OBL`, `E-ERR-001` (LR 5.4) | `obligation_origin`, `unconsumed_path` | `or fail`, `or recover`, `report`, `match` |
| Effect | `E-EFF` | `required_contract`, `violating_effect`, `witness_path` | contract-weakening or behaviour-changing fix |
| Refinement unproved | `E-REF` | `required_range`, `proved_interval`, `widening_point` | a checked operator (`a + b or fail`), `validate` |
| Unsupported shape | `E-OWN-090` (LR 7.10) | `lowering_row` | none (restructure) |
| Resource limit | `E-LIM` | `budget`, `operation` | none |
| Internal defect | `E-INT` | `reproduction` | none |
| Syntax | `E-SYN` | `expected_tokens`, `found_token` | where the grammar allows one |
| Names and modules | `E-NAM` | `name`, `candidates` | import, rename |
| Patterns | `E-MAT` | `missing_variants`, `arm_set` | add arms |
| Closures | `E-FUN` | `violation`, `capture_span`, `use_span` | capture entry, stronger kind |
| Defer and async | `E-DEF`, `E-ASY` | `offending_statement`, `rule_reference` | move the statement out of `defer`, add an `await` site |
| Observation | `E-OBS` | `frame_field`, `exit_path` | add or remove `capture` |
| Dependency injection (kit, Platform 3) | `E-DI` | `consumer`, `slot`, `candidates` | target, `replaces`, `lifetime:` |
| Telemetry (kit, Platform 6.4) | `E-TEL` | `plan_line`, `projection`, `field` | `bind_at`, a new metric name, a new schema version |

Every code, its class, its rule and its correction pattern: [codes.md](codes.md).

### Example: a violation

The record for an assignment to an immutable binding (LR 5.1):

```fortis
// reject: E-NAM-002
pure func next(n: Int<0..9>) Int {
  let x = n
  x = 1                     // reject: E-NAM-002
  return x
}

test "next" {
  expect next(1) == 1
}
```

```records
{"class": "Names and modules", "code": "E-NAM-002", "column": 3, "fields": {"candidates": ["let mut"], "name": "x"}, "file": "example.ft", "line": 4, "message": "`x` is immutable", "rule": "LANG-05-01", "severity": "error", "status": "violated"}
```

### Example: an unproved bound

An integer `+` the checker cannot prove in range is `E-REF-002` with status `unproved`; `proved_interval` is what
the checker established and `required_range` what the type needs (LR 6.4):

```fortis
// reject: E-REF-002
pure func sum(a: Int, b: Int) Int {
  return a + b              // reject: E-REF-002
}

test "sum" {
  expect sum(1, 2) == 3
}
```

```records
{"class": "Refinement unproved", "code": "E-REF-002", "column": 12, "fields": {"proved_interval": "[-18446744073709551616, 18446744073709551614]", "required_range": "[-9223372036854775808, 9223372036854775807]", "widening_point": ""}, "file": "example.ft", "line": 3, "message": "`+` may overflow Int: operands [-9223372036854775808, 9223372036854775807] and [-9223372036854775808, 9223372036854775807] give [-18446744073709551616, 18446744073709551614]; use Int.add, or write the operator as the checked operation: `a + b or fail` (or `or recover`, `catch`)", "rule": "LANG-06-02", "severity": "error", "status": "unproved"}
```

### Example: a record with a correction

The text rendering lists the fields, then the correction's `kind` and each edit as
`file:line:column-end_line:end_column -> "replacement"`. A missing ownership marker (LR 7.5):

```fortis
// reject: E-OWN-001
class Note {
  export text: String
}

test "note" {
  let a = Note(text: "x")
  let b = a                 // reject: E-OWN-001
  expect b.text == "x"
}
```

```output
example.ft:8:11: error E-OWN-001 [LANG-07-01] (Ownership, violated): `a` is an existing ownership-bearing place: write `copy a`, `alias a` or `move a`
    conflict: "marker required"
    loan_region: ""
    place: "a"
    spans: "a"
    correction: add the marker
      example.ft:8:11-8:11 -> "copy "
```

The edit inserts `copy ` (a behaviour-preserving choice); `move a` or `alias a` are the other repairs the message
names, chosen by what the code does with `a` afterwards (LR 7.5).

## Runtime records

Written while a program or its tests run (see Streams above for where each command writes them). The schemas are
`interpreter/records.py`; field values below are real records of the reference toolchain.

| `kind` | Written when | Fields | Source |
|---|---|---|---|
| `runtime-failure` | `report f`, a failure reported during cleanup, a failure in a task result the scope abandoned, or a failing `try main` (exactly one report, exit status 1) | `code`, `message`, `payload`, `origin` (`file`, `function`), `cause`, `annotations`, `reported_by` (`report`, `main`, `cleanup`, `abandoned`), `rule` | LR 4.6, LR 10.6, LR 12.1, LR 12.5 |
| `defect` | a defect terminates a task | `class` (`stack_exhaustion`, …), `task`, `rule` | LR 10.8 |
| `observation-frame` | an observed activation completes | `schema`, `function`, `outcome`, `fields`, `truncated`, `rule` | LR 13 |
| `observation-event` | a `record` statement runs | `schema`, `fields`, `rule` | LR 13 |
| `log` | a `log.info`, `log.warn` or `log.error` statement runs; text `warn: a fallback was taken` | `level` (`info`, `warn`, `error`), `message`, `function`, `rule` (`LANG-13-04`) | LR 13 |
| `test-result` | each test, table row or property case | `test`, `case`, `status` (`pass`, `fail`, `error`), `evidence` (typed actual and expected), `rule` | LR 14 |
| `property-failure` | a property or fuzz target fails | `test`, `mode`, `seed`, `case`, `original`, `minimal` (the shrunk input), `shrinks`, `evidence`, `replay` (the command that replays it), `rule` | LR 14 |
| `test-summary` | end of a test run | `passed`, `failed`, `rule` | LR 14 |
| `task-lifecycle` | a never-polled future is discarded, or a task is cancelled before it starts (`state` `not_started`) | `function`, `state`, `rule` | LR 12.3, LR 13 |
| `task-completion` | a detached task completes, as its supervisor observes it | `task`, `outcome`, `rule` | LR 12.6 |
| `buffer-overflow` | the task reporter's buffer overflowed: one record, its `dropped` count growing | `bound`, `dropped`, `rule` | LR 15 |
| `executor-trace` | a native program run with `FORTIS_TRACE=1` | `events` (`launch`, `start`, `suspend`, `resume`, `complete`, `join`, `loans-end`) | LR 12.5 |
| `build` | `fortis build` / `fortisc build` finishes | `binary`, `manifest` | LR 1 |

The backends do not yet agree on every field (D-57): the interpreter counts each property case as a test, native
code writes `evidence` as a string, and a failure's `origin` has no line on any backend; `fortis test` and
`fortisc test` write the same `evidence`, its `line` included. Match on `kind`, `code`, `test` and `status`.

Captured field values leave the process only through the Telemetry kit's pipeline; with no telemetry plan the
task reporter writes a frame's schema, outcome class and duration but never a captured value (`fields` is empty),
and `FORTIS_TELEMETRY=permit-all` is the explicit development plan that writes them unchanged (LR 15).

A failure's `origin.file` is never an absolute path, on any backend, so an executable does not depend on where it
was built: a module of a package is named relative to the package's directory (`src/pricing.ft`), and a standalone
module relative to the current directory when it lies below it, else by its file name.

A `report` of a declared error, as written by the interpreter and the native executable (LR 10.6):

```json
{"annotations": [], "cause": [], "code": "demo.not_even", "kind": "runtime-failure", "message": "", "origin": {"file": "examples/02_outcomes.ft", "function": "half"}, "payload": {"value": "9"}, "reported_by": "report", "rule": "LANG-10-06"}
```

A frame and an event under `FORTIS_TELEMETRY=permit-all` (LR 13, LR 15):

```json
{"fields": {"value": "5"}, "kind": "observation-event", "rule": "LANG-13-01", "schema": "Line"}
{"fields": {"items": "3", "total": "23"}, "function": "process", "kind": "observation-frame", "outcome": "success", "rule": "LANG-13-02", "schema": "OrderProcessing", "truncated": []}
```

A failed `expect` (LR 14):

```json
{"case": "", "evidence": {"actual": "4", "expected": "== 5", "line": 4}, "kind": "test-result", "rule": "LANG-14-02", "status": "fail", "test": "deliberately wrong"}
```

## `fortis view --json` records

<!-- no-citation: the toolchain's source viewer, not a language rule -->

`fortis view` reads Fortis source as the compiler does and never writes; `--json` is its primary form.
One object per line, `kind` one of:

| `kind` | From | Fields |
|---|---|---|
| `line` | `fortis view FILE` | `path`, `line`, `text`, `spans` (`[column, length, class]`, class `keyword`, `name`, `type`, `punctuation`, `comment`, …) |
| `declaration` | `--decls` | `path`, `line`, `column`, `name`, `declaration`, `head`, `modifiers`, `contracts`, `aspects`, `doc`, `exported`, `member_of` |
| `symbol` | `--symbol NAME` | `path`, `name`, `line`, `end_line`, `text` |
| `match` | `--query Q` | `path`, `line`, `column`, `query`, `text` |
| `query` | `--queries` | `name`, `description` |

```json
{"aspects": [], "column": 10, "contracts": [], "declaration": "func", "doc": "", "exported": false, "head": "try func half(n: Int) Int", "kind": "declaration", "line": 11, "member_of": null, "modifiers": ["try"], "name": "half", "path": "examples/02_outcomes.ft"}
{"column": 21, "kind": "match", "line": 45, "path": "examples/02_outcomes.ft", "query": "or-fail", "text": "or"}
```
