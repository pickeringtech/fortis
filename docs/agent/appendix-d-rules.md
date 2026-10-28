# Rule index (LR D)

<!-- covers: LR D -->
*Agent mode. Human mode: [Rule index](../guide/appendix-d-rules.md). Source of truth: each LR section's rule
table; LR D for old IDs.*

Every `LANG-` rule, its section, a condensed statement and the agent page that carries its codes and correction
patterns. A diagnostic record's `rule` field names one of these IDs, or a `PROG-` or `PLAT-` ID (Programme 7.1).
The LR rule text is authoritative; the statements here are condensed (LR 0).

## LR 3-6

| Rule | LR | Statement | Page |
|---|---|---|---|
| LANG-03-01 | LR 3.2 | `;` outside strings, comments, raw template regions: parse error | [03](03-lexical.md) |
| LANG-03-02 | LR 3.2 | newline termination and continuation follow the 3.2 token lists exactly | [03](03-lexical.md) |
| LANG-03-03 | LR 3.3 | identifier category from its first character | [03](03-lexical.md) |
| LANG-03-04 | LR 3.4 | keywords reserved in declaration/binding positions; allowed as method/associated-function names and after `.` | [A](appendix-a-keywords.md) |
| LANG-03-05 | LR 3.5 | literal typing per 3.5; out-of-range literal rejected at compile time | [03](03-lexical.md) |
| LANG-03-06 | LR 3.6 | interpolations evaluated once, in order; never fallible or suspending | [03](03-lexical.md) |
| LANG-04-01 | LR 4.2 | module identity = file path; modules hold only declarations and constants | [04](04-modules.md) |
| LANG-04-02 | LR 4.2 | importing executes no code | [04](04-modules.md) |
| LANG-04-03 | LR 4.3 | only `exports:` modules importable across packages | [04](04-modules.md) |
| LANG-04-04 | LR 4.1 | distinct nominal types per package version and per module | [04](04-modules.md) |
| LANG-04-05 | LR 4.5 | shadowing a visible binding rejected | [04](04-modules.md) |
| LANG-04-06 | LR 4.6 | entry = exported `main` of the manifest's entry module, one of the four signatures | [04](04-modules.md) |
| LANG-05-01 | LR 5.1 | `let` declares; `=` assigns only a visible mutable target | [05](05-statements.md) |
| LANG-05-02 | LR 5.1 | block exit: end loans, run defers LIFO, drop locals in reverse initialisation order | [05](05-statements.md) |
| LANG-05-03 | LR 5.2 | `if`/`while` conditions are `Bool` | [05](05-statements.md) |
| LANG-05-04 | LR 5.3 | ranged `for` refines its index; index immutable | [05](05-statements.md) |
| LANG-05-05 | LR 5.4 | non-`Void` function ends every path in `return`, `fail` or `Never` | [05](05-statements.md) |
| LANG-05-06 | LR 5.6 | precedence per 5.6; mixed families without parentheses: parse error | [05](05-statements.md) |
| LANG-05-07 | LR 5.7 | evaluation order per 5.7, identical on both backends | [05](05-statements.md) |
| LANG-05-08 | LR 5.5, LR 12.3 | discarding a linear value rejected, except a never-polled future holding nothing linear and no exclusive loan; other non-`Void` values need `discard` | [05](05-statements.md) |
| LANG-05-09 | LR 5.8 | named after positional; each parameter once; names are exported signature | [05](05-statements.md) |
| LANG-06-01 | LR 6.1, LR 6.9 | nominal identity survives calls, storage, serialisation, FFI | [06](06-types.md) |
| LANG-06-02 | LR 6.3 | structural refinement subtyping; interval domain exactly as listed | [06](06-types.md), [proofs](06-proofs.md) |
| LANG-06-03 | LR 6.3 | untrusted values pass `validate` before holding a refined or nominal type | [06](06-types.md) |
| LANG-06-04 | LR 6.4, LR 6.5 | integer, `Decimal`, listed float operations bit-exact across VM and native | [06](06-types.md) |
| LANG-06-05 | LR 6.5 | float operators total, strict IEEE; float refinements rejected | [06](06-types.md) |
| LANG-06-06 | LR 6.7 | `Map`/`Set` iteration deterministic per build | [06](06-types.md) |
| LANG-06-07 | LR 6.11 | trivial and copyable sets exactly as in 6.11 | [06](06-types.md) |
| LANG-06-08 | LR 6.8 | optional, `Bool`, `Outcome` never convert into one another | [06](06-types.md) |
| LANG-06-09 | LR 6.12 | linear values consumed exactly once | [06](06-types.md) |
| LANG-06-10 | LR 6.10, LR 9.5 | coherence: implementation in the package of the interface or the type; no overlap | [06](06-types.md) |
| LANG-06-11 | LR 6.10 | type arguments unfixed by arguments and expected type: rejected | [06](06-types.md) |

## LR 7-9

| Rule | LR | Statement | Page |
|---|---|---|---|
| LANG-07-01 | LR 7.5 | existing ownership-bearing place assigned/returned/captured carries `copy`/`alias`/`move`; `owned`/`editable` arguments carry their marker | [07](07-ownership.md) |
| LANG-07-02 | LR 7.6 | parameters never reassignable | [07](07-ownership.md) |
| LANG-07-03 | LR 7.10 | no inserted clones, reference counts or hidden sharing | [07](07-ownership.md), [C](appendix-c-lowering.md) |
| LANG-07-04 | LR 7.3, LR 7.4 | use after move, conflicting loans, loaned-owner use, escaping loans rejected, incl. closures, `defer`, `await`, `scope` | [07](07-ownership.md) |
| LANG-07-05 | LR 7.3 | loans lexical; aliasing invariant at every point | [07](07-ownership.md) |
| LANG-07-06 | LR 7.6, LR 7.7 | returned loans name one origin; lifetime parameter or loan-carrying field (not `from static`) rejected | [07](07-ownership.md) |
| LANG-07-07 | LR 7.9 | destruction order per 7.9 on both backends | [07](07-ownership.md) |
| LANG-07-08 | LR 7.8 | sharing explicit (`retain`); `copy` of `Shared` rejected | [07](07-ownership.md) |
| LANG-07-09 | LR 7.10 | unsupported shapes rejected, never lowered with `unsafe` or erasure | [C](appendix-c-lowering.md) |
| LANG-08-01 | LR 8.1 | parameter and success types stated; modifiers in 8.1 order | [08](08-functions.md) |
| LANG-08-02 | LR 8.2, LR 8.3 | lambdas list captures (no brackets when none) and declare kind; stronger-kind body rejected | [08](08-functions.md) |
| LANG-08-03 | LR 8.3 | names outside the capture list inaccessible | [08](08-functions.md) |
| LANG-08-04 | LR 8.4 | callable assignability per 8.4; no boxing without `dynamic fn` | [08](08-functions.md) |
| LANG-09-01 | LR 9.1 | no inheritance, runtime member lookup, reflection-based construction | [09](09-classes.md) |
| LANG-09-02 | LR 9.2 | generated constructor only where every field is visible | [09](09-classes.md) |
| LANG-09-03 | LR 9.5 | implementations never strengthen preconditions or weaken guarantees; coherence | [09](09-classes.md) |
| LANG-09-04 | LR 9.6 | mixins require methods only, add no state; conflicts rejected | [09](09-classes.md) |
| LANG-09-05 | LR 9.8 | closed-type `match` exhaustive without `otherwise`; each missing variant named | [09](09-classes.md) |
| LANG-09-06 | LR 9.8 | pattern binding ownership per 9.8 | [09](09-classes.md) |

## LR 10-14

| Rule | LR | Statement | Page |
|---|---|---|---|
| LANG-10-01 | LR 10.1, LR 10.2 | outcomes wrapped until consumed by a 10.2 operation | [10](10-outcomes.md) |
| LANG-10-02 | LR 10.7 | obligations survive collections, fields, closures; dropping rejected | [10](10-outcomes.md) |
| LANG-10-03 | LR 10.6 | reported failure unreachable afterwards; `annotate` keeps one obligation | [10](10-outcomes.md) |
| LANG-10-04 | LR 10.3 | `noalloc try` uses the bounded failure representation | [10](10-outcomes.md) |
| LANG-10-05 | LR 10.4 | `recover` without a prior `report` adds `recovers` | [10](10-outcomes.md) |
| LANG-10-06 | LR 10.8 | defects follow the 10.8 taxonomy; never catchable | [10](10-outcomes.md) |
| LANG-11-01 | LR 11.1, LR 11.3 | exactly eight effects; transitive fixed-point summaries | [11](11-effects.md) |
| LANG-11-02 | LR 11.1, LR 11.3 | fresh-storage and own-`editable`-parameter writes are not `mutate`; captures, guards, atomics are | [11](11-effects.md) |
| LANG-11-03 | LR 11.4 | `noalloc` against the whitelist for the target and telemetry plan | [11](11-effects.md), [C](appendix-c-lowering.md) |
| LANG-11-04 | LR 11.3 | trusted summaries labelled `trusted`, never `proved` | [11](11-effects.md) |
| LANG-11-05 | LR 11.3 | `async`/`try`/`recovers` written; only written `pure`/`noalloc` cross modules; weakening exported modifiers reported | [11](11-effects.md) |
| LANG-11-06 | LR 11.5 | contracts are conjunctions of 11.5 elements; configuration adds none | [11](11-effects.md) |
| LANG-11-07 | LR 11.6 | `io` (except `report`) needs an `Authority` parameter; `Host.capabilities()` only in `main`, once | [11](11-effects.md) |
| LANG-11-08 | LR 11.5, LR 11.8 | contracts in modifier position, no arguments; `@` for aspects only | [11](11-effects.md) |
| LANG-11-09 | LR 11.8 | aspects have only the 11.8 parts, elaborate before checking, pass every check | [11](11-effects.md) |
| LANG-12-01 | LR 12.1 | `defer` checked at every exit; cannot alter the outcome; named places initialised at every exit | [12](12-async.md) |
| LANG-12-02 | LR 12.1 | failures in `defer` consumed there; never replace the primary outcome | [12](12-async.md) |
| LANG-12-03 | LR 12.3, LR 12.4 | `Future`/`Task` linear; pending future never destroyed; cancellation only at cancellation points | [12](12-async.md) |
| LANG-12-04 | LR 12.5 | a scope joins or cancels every task before its block exits | [12](12-async.md) |
| LANG-12-05 | LR 12.5, LR 12.7 | only `Shareable` environments cross pools; loans cross only cooperative `launch` | [12](12-async.md) |
| LANG-12-06 | LR 12.3, LR 12.7 | no `MutexGuard` live across `await` | [12](12-async.md) |
| LANG-13-01 | LR 13 | `capture` snapshots a pure, infallible expression | [13](13-observation.md) |
| LANG-13-02 | LR 13 | required fields assigned exactly once per applicable exit; loops and doubles rejected | [13](13-observation.md) |
| LANG-13-03 | LR 13 | frames bounded by the schema; truncation or omission recorded | [13](13-observation.md) |
| LANG-14-01 | LR 14 | table rows bound with `as` | [14](14-tests.md) |
| LANG-14-02 | LR 14 | `expect` takes `Bool`, records typed evidence | [14](14-tests.md) |

## Old IDs (FSPEC-0.1)

| Old | Now | LR |
|---|---|---|
| SYN-001 / SYN-002 / SYN-004 | LANG-05-05 + LANG-08-01 / LANG-03-03 / LANG-05-07 | LR D, LR 3.3, LR 5.4, LR 5.7, LR 8.1, LR 3, LR 5, LR 8 |
| SYN-003, SYN-005, SYN-006, TYP-005 | Programme families (formatter, diagnostics, library metadata) | LR D |
| TYP-001..004, TYP-006 | LANG-06-01, LANG-06-03, LANG-06-04, LANG-06-02, LANG-06-08 | LR D, LR 6.3, LR 6 |
| OWN-001..006 | LANG-07-01, -02, -03, -04, -06, -08 | LR D, LR 7.5, LR 7 |
| FUN-001, FUN-002 / OO-001 / GEN-001 | LANG-08-02 / LANG-09-01 + LANG-09-04 / LANG-06-10 + Programme (bounded solving) | LR D, LR 6.10, LR 8.2, LR 9.1, LR 6, LR 8, LR 9 |
| MOD-001 / DOC-001 | LANG-04-02 / PROG-TST-08 | LR D, LR 4.2, Programme 4.2, LR 4 |
| ERR-001..006 / ERR-007 | LANG-10-01, -02, -03 (ERR-003, -004), -04, LANG-09-05 / Telemetry and Runtime host kits | LR D, LR 9.8, LR 10.2, LR 9, LR 10 |
| EFF-001, -002, -004, -005, -006 / EFF-003 | LANG-11-01, -02, -03, -04, -06 / removed with `nostate` | LR D, LR 11.1, LR 11 |
| RES-001 / RES-002 | LANG-05-02 + LANG-07-07 / LANG-12-02 | LR D, LR 5.1, LR 7.9, LR 12.1, LR 5, LR 7, LR 12 |
| ASY-001, ASY-003 / ASY-002 / ASY-004 / ASY-005 | LANG-12-03 / LANG-12-05 / LANG-12-04 / Runtime host kit | LR D, LR 12.3, LR 12 |
| OBS-001, -002, -006 / OBS-003..005 | LANG-13-01, -02, -03 / Telemetry kit | LR D, LR 13 |
