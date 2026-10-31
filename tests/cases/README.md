# Shared unit-test cases

Each `tests/cases/<component>/<name>.json` file is a non-empty JSON array. Every row has exactly three fields:

```json
[
  {"name": "add_positive", "input": {"op": "+", "a": [1, 3], "b": [4, 8]}, "expect": [5, 11]}
]
```

- `name`: unique within the file, matching `[a-z][a-z0-9_]*`. It is the test's stable name in both languages.
- `input`: the data supplied to the component's function, in the format below or documented by a new table.
- `expect`: the expected result, written from the language rule, not copied from a checker's output.

Values may be null, booleans, signed 64-bit integers, Unicode scalar strings, arrays and objects. Use unique
object keys. No floating-point JSON numbers, comments, trailing commas or extra row metadata. Larger numbers and
floating-point inputs can be represented as strings when the component's adapter defines that representation.
Readers are `tests/python/support.py` and `compiler/crates/fortis-testkit`; Rust reuses `fortis-diag::Json`.
Both reject empty tables, invalid row shapes, invalid names and duplicate names. Python also checks duplicate
object keys. Each test adapter converts data to the production types and compares the function's result.

| Table | Input | Expected result |
|---|---|---|
| `intervals/arithmetic.json` | `op`, `a`, `b`; closed intervals `[lo, hi]` | `+`, `-`, `*`: closed result interval; `meet`: intersection or null for disjoint intervals |
| Same table, `subtype` | `a` is the source Int refinement, `b` the target; null denotes the unrefined Int base | The actual assignability verdict: true when proved, null when unproved; false is available for a known violation |
| `lexer/tokens.json` | Source text as a string | Ordered `[kind, spelling]` pairs, including the final logical newline (`NL`); only the EOF sentinel is omitted |
| `text/intrinsics.json` | `op`, `text`, `args` (an array of strings) | Text for trim, replace and asciiUpper; array for split; scalar offset or null for find |

Containment cases use the interval intersection function; subtype cases exercise the actual assignability
function. Lexer spelling includes literal delimiters and escapes as written in source. Text cases use the
interpreter's registered intrinsic and the corresponding `fortis-runtime::text` function. Case names are also
listed in the Rust `case_tests!` invocation so Cargo exposes each row separately; the registration test catches
any missing name. Python discovers each row automatically.

`identity/nominal.json` calls nominal assignability, interface-key comparison and core-type recognition.
Its input has `op` (`assignable`, `interface` or `core`), type `a` and, for comparisons, type `b`.
A named type has `name`, optional `args`, `package` and `origin`; wrappers are `optional`, `tuple`,
`record` (ordered name/type pairs), `fn` (parameter array and result), `loan`, `dynamic` and `refined`.
The latter fixes the interval at 0..10. Interface comparisons expect a boolean; core recognition expects the core name or null.
Assignability expects an object with `clash` (boolean) and `assignable` (boolean or null when this
low-level helper leaves a structural check to its caller), checked in both directions.
The U3 runtime tables cite their Language Reference sections in each row name. `runtime/integers.json` supplies
`op`, `a`, `b` for checked Int operations (LR 6.4); `runtime/modular.json` covers wrapping and saturating operations.
Results are integers or `{code, payload}` failure objects. Python calls its intrinsic registry; Rust calls the VM
registry and, for checked arithmetic, the native runtime. Native wrapping and saturating arithmetic are emitted
as Rust integer methods and belong to the lowering text tests.

`runtime/conversions.json` supplies `kind`, `value`, `target` and, for bounded integers, `low` and `high` (LR 6.13).
Integer inputs and results, including numeric payload fields, use decimal strings so UInt64 boundaries fit the
shared schema. `kind` is `int`, `text`, `decimal` or `bytes`; bytes are integer arrays. Success is canonical text
(the scalar itself for Scalar); failure is `{code, payload}`. One table exercises the interpreter, VM and native
conversion functions. `runtime/text_edges.json` supplies `op`, `text` for LR 6.6, with text or an array of lines as
result. `runtime/messages.json` supplies `template`, `payload` for LR 10.3, with the rendered message as result.

`runtime/numeric_text.json` extends the conversion schema with full-input numeric parsing edges. All fixed-width
integer targets and Decimal use the same cases in Python, fortis-runtime, the VM and native. A range-failure
`value` beyond signed 128 bits is exact canonical text. `types/refined_reads.json` supplies a complete in-memory
`source` fragment and expects the sorted unique diagnostic codes from both checkers, including an empty list for
accepted reads. It covers length-refined List/ListView operations, bounds, mutation and the absent text conversions.

## Front-end type tables (U1)

The `types/` tables cite LR 6.3, 6.10, 6.13 and 9.8 in their case names. A type is a name string,
`["ref", base, low, high]`, `["opt", type]`, `["tuple", ...types]`, `["param", name]`, or
`["named", name, ...arguments]`. These are data constructors, not a second type parser.

- `conversions.json`: `[source, target]` to `"total"`, `"fallible"` or null (no table entry).
  Identity is checked by the caller, so identity pairs are not sent to the table resolver.
- `assignability.json`: `{have, want}` to true (proved), false (violated) or null (unproved).
- `narrowing.json`: `{op, p, c}` to the closed interval after the comparison, or null if empty.
- `unification.json`: `{pattern, actual}` to the inferred type-parameter map.
- `expressions.json`: `{op, source, env, head?, expected?}` to a type, branch intervals, or diagnostic
  code set. `infer` calls the expression typer; `narrow` calls the condition narrower; `codes` checks
  pattern diagnostics; `statement` checks one loop. An optional `head` supplies only in-memory
  declarations to the declaration loader. No files, standard library loader or program checker run.
- `defects.json` and `expression_defects.json`: the same inputs and LR-derived expectations for
  D-635 to D-638, now passing regressions after F1. D-636 expects optional widening under X-685. The
  complete reproductions live under `conformance/programs/f1_*.ft`.

The narrowing property enumerates every comparison over each generated finite interval and checks
its result against the satisfying concrete values. The conversion property checks range containment.
## Names, declarations and records (U1)

`names/modules.json` provides small in-memory module signatures, including a minimal prelude. No
adapter reads a library file. `names/resolution.json` maps a declaration fragment to its diagnostic
code set (LR 3.6, 4.3-4.5). Scope resolution handles these; `names/member_calls.json` instead invokes
the expression typer, which owns qualified-call visibility, with `{head, source}` fragments.

`names/declarations.json` maps declaration groups to diagnostic code sets (LR 4.4, 6.13, 9.1, 9.5,
9.6). It loads signatures, checks declarations, and invokes the exported-signature rule separately;
no function body is checked or run. This covers interface requirements, mixin requirements and
conflicts, implementation coherence, declared and derived conversions, and enum text conversion.
`names/defects.json` keeps the five constant-collision expectations for D-639, now passing
in both languages.

`diagnostics/records.json` maps a record to its validity under the published diagnostic schema and
Programme 7.1. It covers common fields, each diagnostic class's mandatory fields, the three proof
statuses, severity and source positions. `diagnostics/malformed.json` carries six records the Python
validator rejects and the Rust reader/validator accepts (D-640); only the Rust tests are ignored.
`diagnostics/wire.json` maps JSON values to the canonical wire spelling (PROG-DIA-05), including
Unicode, control characters and signed 64-bit boundaries. Seeded properties exercise nested name
scopes and Unicode round trips. These tests check records of all classes, not the other streams'
ownership, effect or runtime rules that produce them.

F1 extends both adapters with `record` (name/type pairs) and an optional refinement measure after its bounds.
`types/structural.json` covers tuples, records, optional widening and nested container invariance;
`types/checker_fixes.json` covers narrowing, branch hulls, bindings, calls and patterns;
`names/member_namespace.json` checks all pairs of field, method and constant declarations, with distinct-name controls.

Ownership tables (`ownership/`, LR 7):

| Table | Input | Expected result |
|---|---|---|
| `state.json` | Two place paths in the checker's dotted component form | Overlap, use after moving the first path, and whether that move remains after reinitialising the second |
| `use.json` | An owner with no loan, a read loan, an editable loan or a prior move; a path and use mode | Exact diagnostic-code set and agreement with the use verdict |
| `storage.json` | A type tree and whether the enclosing position is a field | Exact diagnostic-code set for the storage boundary, including callable capture origins |
| `temporaries.json` | An expression tree with already inferred leaf types | Whether any supplying branch can own a statement temporary |

Type trees use a string for a named type, or `[constructor, arguments...]`: `loan` takes mode, origin and
inner type; `optional`, `tuple` and `record` wrap types; `closure` takes a capture origin. Other constructors
are named generic types. The adapters construct declaration heads in memory. Expression trees contain
`rule` and `kids`, or `token: [kind, text]`; `type` supplies a leaf's prior inference result. The temporary
adapter records the production `temporary_value` result at that inference boundary, then asks the same
analysis about the enclosing expression. No parser, module loader or whole-program check runs in these tests.

F3 fixes D-645: all storage rows are ordinary passing tests. `provenance.json` checks preservation of multiple
capture origins through substitution, fixed generic inference, annotations, coalescing and branch joins, under
optional, tuple, record, outcome and collection wrappers. `captures` constructs value provenance and `parameter`
constructs a type parameter. The origin oracle is independent of type spelling.

Obligation tables (`obligations/`, LR 6.12, 7.7, 8.4 and 10.7):

| Table | Input | Expected result |
|---|---|---|
| `matrix.json` | Seven value sources, six holders, nine exits; the held type at the analysis boundary | Exact diagnostic-code set for stored loans and unconsumed values |
| `types.json` | A type tree and a linearity, generic-instantiation or consuming operation | Linearity boolean or exact diagnostic-code set |
| `callbacks.json` | Parameter type and source/target parameter modes | Callable assignability; LR 8.4 requires exactly equal modes (D-541) |
| `loops.json` | Exit kind and the loop depth it crosses | Exact remaining-cursor paths and diagnostic codes, including nested continue and cancellation |

The matrix exhausts **378 cells**. Python runs seven seeded properties, each shuffling and checking all 54 cells
for one source; Rust gives every cell its own named test, with a separate seeded replay and completeness check.
The source types are a local `Must`, a loan of a parameter's `Must`, an owned class field `Must`, a temporary
`Must`, an `Int` literal, an `Outcome[Int]` call result and an optional outcome supplied by `??`. Holders are a
binding, collection element, class field, closure capture, pending returned value and outcome payload. A closure
that moves a linear capture is `fn once`; a loan or literal capture is a repeatable `fn`. The literal and parameter
loan are controls with no local obligation; an enclosing outcome still has its own obligation.

The nine exits are block end, return, break, continue, fail, or fail, recover, catch and a consuming loop's early
exit. The state is poised at the relevant exit boundary: normal exits report live obligations at the departing
block depth, unwinding checks E-OBL-005, a return transfers its result, and early termination of a linear consuming
loop still owes the hidden remainder after moving the collection. This tests the analysis of those states; the
corpus continues to test that parsed statements reach the correct boundary. Types and declaration heads are built
in memory. No subprocess, program build, parser or library file loading runs in the unit suite.

F3 fixes D-646: the ordinary passing test retains LR 6.12's verdict that a returned loan of a type parameter
is not held by value and therefore does not require a linear bound.

Loan origins and copying (`loan_origins/`, LR 6.11, 7.3, 7.6, 8.3):

| Table | Input | Expected result |
|---|---|---|
| `returns.json` | Parameter, local or receiver source; declared origin; direct or optional result | Exact E-OWN-050/051/053 code set for the returned loan |
| `children.json` | Read or exclusive parent and child, target path, and operation | Exact loan-conflict code set; D-647 marks child access rejected because the ancestor relation is lost |
| `copies.json` | Leaf type under plain, optional, collection, tuple, record or generic-class wrappers | Structural copyability; generic classes use their declaration under LR 6.7, and D-649 retains the rejection of copying Must |
| `captures.json` | Shared small AST, binding type and direct-callback status | Capture diagnostic codes and the inferred head's tokens for reads, copies, moves, loans and writes |

The capture adapters run the production private probe. Python stubs only construction of the already selected
capture head and a simple place, so it loads no grammar files; Rust uses its generated fragment parser in memory.
Both assert that the probe leaves the enclosing ownership and effect state untouched. A seeded property checks
that ending a child's lexical block retains its parent's loan. F3 fixes D-647 and D-649; all rows are ordinary passing tests in
Python and Rust. The case names and expected results cite the LR, including its
explicit exclusion of linear elements from Copyable in 10.7.

## Front-end syntax tables (U1)

- `lexer/continuation.json`: source text to exact `[kind, spelling]` tokens, omitting EOF.
  Covers LR 3.1-3.7, every continuation family, brace classification and refinement delimiters.
- `lexer/errors.json`: source text to the lexical error code (LR 3.1, 3.5, 3.6).
- `parser/fragments.json`: `{start, source}` to an acceptance Boolean. LR sections in the case
  names identify the oracle. Statement and pattern fragments receive only their enclosing block or
  match-arm delimiters, because generated follow sets are specialised to those contexts.
- `parser/precedence.json`: expression source to an operator tree (LR 5.6). A binary node is
  `[operator, left, right]`, a unary node `[operator, operand]`, and a leaf its spelling. The adapter
  removes grammar-only wrappers; it preserves the parser's grouping. Every pair of arithmetic,
  comparison and bitwise precedence levels is tested in both orders.

The parser tests use the generated parser directly, without reading the grammar or checking whole
programs. Rejected fragments also check a fresh parser succeeds afterwards. This tests failure
isolation and local backtracking; Fortis does not promise a parser that resumes after a bad statement.

`collections/intrinsics.json` describes LR 6.7 operation sequences: `type`, `initial`, and `ops` (an operation name
followed by its arguments). Expected `results` include each operation's value or `{code, payload}` failure, and
`items` records the final collection in iteration order. Cursor results include two reads after exhaustion. Map
entries are `[key, value]` pairs; a Deque input holds its reversed front and forward back lists. Python calls the
interpreter registry, Rust the VM registry and typed native collection helpers.

`evaluation/operators.json` gives `type`, `op`, `a` and `b` with the expected scalar result (LR 5.6, 6.4 and 6.5).
Float operands use strings for NaN, infinities and signed zero. The adapters call the interpreter's and VM's binary
operator functions without invoking a checker or executing a whole program.

Effect tables (`effects/`, LR 11.1 to 11.6):

| Table | Input | Expected result |
|---|---|---|
| `callables.json` | All 32 subsets of the five effect modifiers | Effect-to-diagnostic-status map; the four pure/noalloc combinations also exercise callback summaries |
| `imports.json` | Each effect with certain, possible or unknown evidence | Imported effect, certainty and trusted provenance |
| `claims.json` | A modifier and a body effect | Exact claim diagnostic-code set; failure disposition is checked when an outcome is consumed, rather than by this summary method |
| `contracts.json` | Named sets of modifiers, forbids, requires and excludes | Exact contradictions and their origins, also checked through E-EFF-050 reporting |
| `authorities.json` | A capability name with its core declaration or a local shadow | Authority verdict, checked for direct and Shared parameters and I/O admission |
| `footprints.json` | Binding kind and direct or loan-mediated argument path | Caller effects and instantiated write footprint |
| `capabilities.json` | Host capability creation context, entry status, direct call and parameters | Exact E-EFF-010 verdict for main, helpers, loops, callbacks, hooks, repeated calls and references |

Seeded properties cover summary union, evidence ordering, every one of the 256 effect subsets at synchronous and
asynchronous call boundaries, same-contract contradiction handling, transitive contract forbids, and required versus
excluded authority in parameters and callees. These adapters use in-memory signatures and declarations. Python's
trusted effect metadata is cached on its first library operation rather than read when importing the analysis;
unknown-module and data-only summaries need no effect-data files. Two Python tests pin lazy loading and caching.

`types/function_values.json` tests LR 8.1 and 9.7 through both checkers: each in-memory source has the exact set
of expected diagnostic codes, with optional message text. It covers import forms, generic owners, methods,
constructors, parameter modes and local shadowing. `runtime/function_values.json` supplies complete sources and
expected console lines to the interpreter, MIR/VM and native emission unit tests. The corresponding `exec_r4_`
corpus fixtures also compile and execute on native Rust.

`types/rc3_r6.json` checks LR 8.1's single call-result wrapper and retained effects for named function values,
with direct calls and lambdas as controls. `runtime/rc3_r6.json` checks those calls, resolved array sizes in
postfix chains and interpolation holes, and the native workaround for a generic associated method. Optional
`wrapper_returns` inputs pin generic adapters' result types and async status in MIR before native emission. The corpus
adds native builds and exact failure-record agreement, including the original named failure source.

`types/async_assignment.json` checks LR 8.4 async status and the unchanged try rules directly in both
assignability functions, including optional, tuple, record, generic and function-result nesting.
`types/rc4_r7.json` checks the D-749 conversion sites and the corrective diagnostic text through both checkers.
