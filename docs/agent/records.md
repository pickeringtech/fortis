# Records

<!-- covers: LR 0 -->
*Agent mode. Source of truth: Programme 7 (diagnostics) and 7.4 (one record, two renderings), the schema
`reference/schemas/diagnostic.schema.json`, LR 10.6, LR 10.8, LR 13 and LR 14 for the runtime records.*

Every finding, runtime failure report, test result and observation the toolchain produces is a record first: a
JSON object of a known kind, from which the text a person sees is rendered (Programme 7.4, PROG-DIA-07). Read the
record, not the text.

## Streams

| Situation | What is written | Source |
|---|---|---|
| standard output is a pipe | one JSON object per line | Programme 7.4 |
| standard output is a terminal | text rendered from the same record | Programme 7.4 |
| `--format json` / `--format text` | forces that rendering | Programme 7.4 |
| a running program's records (failure reports, defects, observation frames and events) | standard error, same rule; the program's own output is standard output | Programme 7.4, LR 4.6 |

Where each command writes, as the toolchain does today (Programme 7.4 asks for the record; the stream is the
command's):

| Command | Standard output | Standard error |
|---|---|---|
| `fortis check`, `fortisc check` | diagnostic records | — |
| `fortis run`, a native executable | the program's output | runtime records |
| `fortis test` (interpreter) | the `test-summary` record | `test-result` and other runtime records |
| `fortisc test` (VM) | `test-result` records and the `test-summary` | — |
| `fortis build`, `fortisc build` | the `build` record | diagnostic records if the check fails |
| `fortisc vm` | the conformance format: `=== FILE`, `observed: …` (output lines joined by `\|`), `trace: …`, `record: …` | — |

A finding has one identity: `fortis check`, the daemon and the agent protocol produce byte-equal records for the
same snapshot (PROG-DIA-07, Programme 7.4). `fortis check` (the reference checker) and `fortisc check` (the
production checker) print records of this one schema, with the same codes at the same lines.

## Diagnostic records

Schema: `reference/schemas/diagnostic.schema.json` (Programme 7.3).

| Field | Type | Meaning | Source |
|---|---|---|---|
| `code` | string | stable diagnostic code, `E-XXX-NNN` | Programme 7.1 |
| `class` | string | the diagnostic class (table below) | Programme 7.1 |
| `rule` | string | the normative rule the record enforces (`LANG-`, `PROG-` or `PLAT-` ID) | Programme 7.1 |
| `file`, `line`, `column` | string, integer, integer | primary location, the user's nearest responsible operation (1-based) | PROG-DIA-08, Programme 7.3 |
| `severity` | `error` or `warning` | a warning does not fail the check | Programme 7.3 |
| `status` | `violated`, `unproved` or `unknown` | a proved violation, an inability to prove, an unknown trust boundary | PROG-DIA-02, Programme 7.1 |
| `message` | string | one-line explanation | Programme 7.1 |
| `fields` | object | the class's mandatory fields | PROG-DIA-01, Programme 7.1 |
| `correction` | object, optional | `{kind, edits: [{file, line, column, end_line, end_column, replacement}]}`, an applicable edit | PROG-DIA-08 |

`unproved` means the checker could not prove a bound, not that the program is wrong: supply the proof or use the
checked alternative the message names (Programme 7.1, LR 6.3). A `correction`'s `kind` is a short
description of the edit (for example `add the marker`). Programme 7.1 classifies fixes as behaviour-preserving,
behaviour-changing or contract-weakening, but today's records do not carry that class, so judge it from the edit
itself; the compiler never removes `noalloc` or a security requirement to make code compile (Programme 7.1), and
an agent never applies an edit that weakens a claim (PROG-AGT-04).

### Classes and mandatory fields

| Class | Code prefix | Mandatory `fields` | Correction | Source |
|---|---|---|---|---|
| Type | `E-TYP`, `E-MIX-001` | `expected_type`, `actual_type`, `primary_span` | behaviour-preserving or behaviour-changing fix | Programme 7.1 |
| Ownership | `E-OWN` | `place`, `conflict`, `spans`, `loan_region` | marker, block restructuring, `copy` | Programme 7.1 |
| Obligation | `E-OBL`, `E-ERR-001` | `obligation_origin`, `unconsumed_path` | `or fail`, `or recover`, `report`, `match` | Programme 7.1 |
| Effect | `E-EFF` | `required_contract`, `violating_effect`, `witness_path` | contract-weakening or behaviour-changing fix | Programme 7.1 |
| Refinement unproved | `E-REF` | `required_range`, `proved_interval`, `widening_point` | checked alternative (`Int.add`, `validate`) | Programme 7.1 |
| Unsupported shape | `E-OWN-090` | `lowering_row` | none (restructure) | Programme 7.1 |
| Resource limit | `E-LIM` | `budget`, `operation` | none | Programme 7.1 |
| Internal defect | `E-INT` | `reproduction` | none | Programme 7.1 |
| Syntax | `E-SYN` | `expected_tokens`, `found_token` | where the grammar allows one | Programme 7.1 |
| Names and modules | `E-NAM` | `name`, `candidates` | import, rename | Programme 7.1 |
| Patterns | `E-MAT` | `missing_variants`, `arm_set` | add arms | Programme 7.1 |
| Closures | `E-FUN` | `violation`, `capture_span`, `use_span` | capture entry, stronger kind | Programme 7.1 |
| Defer and async | `E-DEF`, `E-ASY` | `offending_statement`, `rule_reference` | move the statement out of `defer`, add an `await` site | Programme 7.1 |
| Observation | `E-OBS` | `frame_field`, `exit_path` | add or remove `capture` | Programme 7.1 |
| Dependency injection (kit) | `E-DI` | `consumer`, `slot`, `candidates` | target, `replaces`, `lifetime:` | Programme 7.1, Platform 3 |
| Telemetry (kit) | `E-TEL` | `plan_line`, `projection`, `field` | `bind_at`, a new metric name, a new schema version | Programme 7.1, Platform 6.4 |

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
the checker established and `required_range` what the type needs (LR 6.4, Programme 7.1):

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
{"class": "Refinement unproved", "code": "E-REF-002", "column": 12, "fields": {"proved_interval": "[-18446744073709551616, 18446744073709551614]", "required_range": "[-9223372036854775808, 9223372036854775807]", "widening_point": ""}, "file": "example.ft", "line": 3, "message": "`+` may overflow Int: operands [-9223372036854775808, 9223372036854775807] and [-9223372036854775808, 9223372036854775807] give [-18446744073709551616, 18446744073709551614]; use Int.add", "rule": "LANG-06-02", "severity": "error", "status": "unproved"}
```

### Example: a record with a correction

The text rendering lists the fields, then the correction's `kind` and each edit as
`file:line:column-end_line:end_column -> "replacement"` (Programme 7.4). A missing ownership marker (LR 7.5):

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
| `test-result` | each test, table row or property case | `test`, `case`, `status` (`pass`, `fail`, `error`), `evidence` (typed actual and expected), `rule` | LR 14 |
| `property-failure` | a property or fuzz target fails | `test`, `mode`, `seed`, `case`, `original`, `minimal` (the shrunk input), `shrinks`, `evidence`, `replay` (the command that replays it), `rule` | LR 14 |
| `test-summary` | end of a test run | `passed`, `failed`, `rule` | LR 14, Programme 8.1 |
| `task-lifecycle` | a never-polled future is discarded, or a task is cancelled before it starts (`state` `not_started`) | `function`, `state`, `rule` | LR 12.3, LR 13 |
| `task-completion` | a detached task completes, as its supervisor observes it | `task`, `outcome`, `rule` | LR 12.6 |
| `buffer-overflow` | the task reporter's buffer overflowed: one record, its `dropped` count growing | `bound`, `dropped`, `rule` | LR 15 |
| `executor-trace` | a native program run with `FORTIS_TRACE=1` | `events` (`launch`, `start`, `suspend`, `resume`, `complete`, `join`, `loans-end`) | LR 12.5 |
| `build` | `fortis build` / `fortisc build` finishes | `binary`, `manifest` | Programme 5.4 |

The backends do not yet agree on every field (D-57): the interpreter counts each property case as a test, the VM's
`evidence` has no `line`, native code writes `evidence` as a string, and a failure's `origin` has no line on any
backend. Match on `kind`, `code`, `test` and `status`.

Captured field values leave the process only through the Telemetry kit's pipeline; with no telemetry plan the
task reporter writes a frame's schema, outcome class and duration but never a captured value (`fields` is empty),
and `FORTIS_TELEMETRY=permit-all` is the explicit development plan that writes them unchanged (LR 15).

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
{"case": "", "evidence": {"actual": "4", "expected": "== 5"}, "kind": "test-result", "rule": "LANG-14-02", "status": "fail", "test": "deliberately wrong"}
```

## `fortis view --json` records

`fortis view` reads Fortis source as the compiler does and never writes; `--json` is its primary form
(Programme 9.2). One object per line, `kind` one of:

| `kind` | From | Fields | Source |
|---|---|---|---|
| `line` | `fortis view FILE` | `path`, `line`, `text`, `spans` (`[column, length, class]`, class `keyword`, `name`, `type`, `punctuation`, `comment`, …) | Programme 9.2 |
| `declaration` | `--decls` | `path`, `line`, `column`, `name`, `declaration`, `head`, `modifiers`, `contracts`, `aspects`, `doc`, `exported`, `member_of` | Programme 9.2 |
| `symbol` | `--symbol NAME` | `path`, `name`, `line`, `end_line`, `text` | Programme 9.2 |
| `match` | `--query Q` | `path`, `line`, `column`, `query`, `text` | Programme 9.2 |
| `query` | `--queries` | `name`, `description` | Programme 9.2 |

```json
{"aspects": [], "column": 10, "contracts": [], "declaration": "func", "doc": "", "exported": false, "head": "try func half(n: Int) Int", "kind": "declaration", "line": 11, "member_of": null, "modifiers": ["try"], "name": "half", "path": "examples/02_outcomes.ft"}
{"column": 21, "kind": "match", "line": 46, "path": "examples/02_outcomes.ft", "query": "or-fail", "text": "or"}
```
