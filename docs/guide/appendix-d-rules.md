# Appendix D: Rule index

<!-- covers: LR D -->
*Human mode. Agent mode: [Rule index](../agent/appendix-d-rules.md). Source of truth: the rule tables at the end of
each LR section, and LR D.*

Every normative rule of the core has an ID of the form `LANG-<section>-<nn>`, and each LR section ends with a
table of its rules and the evidence that tests them (LR 0). This appendix lists every rule in one place, says in
a line what it means, and points to the chapter that explains it. The one-line summaries are paraphrases; the
rule text in the LR is the rule.

## Lexical structure and modules

| Rule | In short | LR | Chapter |
|---|---|---|---|
| LANG-03-01 | A `;` outside strings, comments and raw template regions is a parse error. | LR 3.2 | [Lexical structure](03-lexical.md) |
| LANG-03-02 | A newline ends a statement except after the listed trailing tokens, before the listed leading tokens, or inside brackets. | LR 3.2 | [Lexical structure](03-lexical.md) |
| LANG-03-03 | An identifier's first character decides its category: uppercase for types, lowercase for values. | LR 3.3 | [Lexical structure](03-lexical.md) |
| LANG-03-04 | Keywords are reserved in every declaration and binding position, but may name a method or follow a `.`. | LR 3.4 | [Keywords](appendix-a-keywords.md) |
| LANG-03-05 | A literal's type follows the literal table, and an out-of-range literal is a compile-time error. | LR 3.5 | [Lexical structure](03-lexical.md) |
| LANG-03-06 | An interpolated expression is evaluated once, in order, and cannot fail or suspend. | LR 3.6 | [Lexical structure](03-lexical.md) |
| LANG-04-01 | A module's identity is its file path, and a module holds only declarations and constants. | LR 4.2 | [Modules and packages](04-modules.md) |
| LANG-04-02 | Importing a module runs no code. | LR 4.2 | [Modules and packages](04-modules.md) |
| LANG-04-03 | Only modules a package lists in `exports:` can be imported from other packages. | LR 4.3 | [Modules and packages](04-modules.md) |
| LANG-04-04 | Two versions of a package, and two modules, never share a nominal type. | LR 4.1 | [Modules and packages](04-modules.md) |
| LANG-04-05 | A `let` may not shadow a visible binding. | LR 4.5 | [Modules and packages](04-modules.md) |
| LANG-04-06 | The program starts at the `main` of the manifest's entry module, exported or not, in one of four forms; the runtime supplies each of its parameters, `List[String]` or a capability type, by type. | LR 4.6 | [Modules and packages](04-modules.md) |

## Statements and types

| Rule | In short | LR | Chapter |
|---|---|---|---|
| LANG-05-01 | `let` declares, and its pattern matches every value; `=` only assigns to a visible mutable target. | LR 5.1 | [Statements and expressions](05-statements.md) |
| LANG-05-02 | Leaving a block ends its loans, runs its `defer`s last-first, then drops its locals in reverse order. | LR 5.1 | [Statements and expressions](05-statements.md) |
| LANG-05-03 | `if` and `while` conditions are `Bool`. | LR 5.2 | [Statements and expressions](05-statements.md) |
| LANG-05-04 | A ranged `for` gives an immutable index refined to the range. | LR 5.3 | [Statements and expressions](05-statements.md) |
| LANG-05-05 | A non-`Void` function ends every path in `return`, `fail` or a call that never returns. | LR 5.4 | [Statements and expressions](05-statements.md) |
| LANG-05-06 | Operators follow the precedence table, and the outcome, `??` and Boolean families do not mix without parentheses. | LR 5.6 | [Statements and expressions](05-statements.md) |
| LANG-05-07 | Evaluation is left to right, once, in the four steps of a call, identically on every backend. | LR 5.7 | [Statements and expressions](05-statements.md) |
| LANG-05-08 | A linear value cannot be discarded (except a never-polled future holding nothing linear and no exclusive loan); any other unused non-`Void` value needs `discard`. | LR 5.5, LR 12.3 | [Statements and expressions](05-statements.md) |
| LANG-05-09 | Named arguments follow positional ones, bind each parameter once, and are part of the exported signature; a pun `name` binds the parameter of its name. | LR 5.8 | [Statements and expressions](05-statements.md) |
| LANG-06-01 | A nominal type keeps its identity through calls, storage, serialisation and FFI. | LR 6.1, LR 6.9 | [Types](06-types.md) |
| LANG-06-02 | A refinement is a subtype of any wider refinement, and the interval domain is exactly as listed, with no extra inference. | LR 6.3 | [Types](06-types.md), [Proving arithmetic and indexing](06-proofs.md) |
| LANG-06-03 | An untrusted value passes `validate` before it can hold a refined or nominal type. | LR 6.3 | [Types](06-types.md) |
| LANG-06-04 | Integer, `Decimal` and the listed float operations give bit-identical results on the VM and native code; a checked operator fails as its checked function does, at the same operation. | LR 6.4, LR 6.5 | [Types](06-types.md) |
| LANG-06-05 | Float operators are total, strict IEEE 754; float refinements are rejected. | LR 6.5 | [Types](06-types.md) |
| LANG-06-06 | `Map` and `Set` iterate in an order fixed per build. | LR 6.7 | [Types](06-types.md) |
| LANG-06-07 | The trivial and copyable sets are exactly the listed ones. | LR 6.11 | [Types](06-types.md) |
| LANG-06-08 | An optional, a `Bool` and an `Outcome` never convert into one another. | LR 6.8 | [Types](06-types.md) |
| LANG-06-09 | A linear value is consumed exactly once. | LR 6.12 | [Types](06-types.md) |
| LANG-06-10 | An interface is implemented for a type only in the package that defines one of them, and never twice. | LR 6.10, LR 9.5 | [Types](06-types.md) |
| LANG-06-11 | A call whose type arguments nothing fixes is rejected. | LR 6.10 | [Types](06-types.md) |

## Ownership, functions and classes

| Rule | In short | LR | Chapter |
|---|---|---|---|
| LANG-07-01 | Assigning, returning or capturing an existing ownership-bearing place says `copy`, `alias` or `move`; `owned` and `editable` arguments say so too. | LR 7.5 | [Ownership](07-ownership.md) |
| LANG-07-02 | Parameters are never reassignable. | LR 7.6 | [Ownership](07-ownership.md) |
| LANG-07-03 | The checker never inserts a clone, a reference count or hidden sharing to repair a program. | LR 7.10 | [Ownership](07-ownership.md), [Lowering to Rust](appendix-c-lowering.md) |
| LANG-07-04 | Use after move, conflicting loans, use of a loaned owner and escaping loans are rejected, through closures, `defer`, `await` and `scope` too. | LR 7.3, LR 7.4 | [Ownership](07-ownership.md) |
| LANG-07-05 | Loans are lexical, and an owner never has an exclusive loan alongside any other loan. | LR 7.3 | [Ownership](07-ownership.md) |
| LANG-07-06 | A returned loan names one origin; a lifetime parameter, or a field of a loan or loan-carrying type other than a loan `from static`, is rejected. | LR 7.6, LR 7.7 | [Ownership](07-ownership.md) |
| LANG-07-07 | Values are destroyed in the order of the destruction table, on every backend. | LR 7.9 | [Ownership](07-ownership.md) |
| LANG-07-08 | Sharing is explicit (`retain`), and `copy` of a `Shared` is rejected. | LR 7.8 | [Ownership](07-ownership.md) |
| LANG-07-09 | An ownership shape with no safe lowering is rejected, never lowered unsafely. | LR 7.10 | [Lowering to Rust](appendix-c-lowering.md) |
| LANG-08-01 | Every function states its parameter types and success type (a lambda's may be inferred as LR 8.2 allows), with modifiers in the fixed order. | LR 8.1 | [Functions and closures](08-functions.md) |
| LANG-08-02 | A lambda lists its captures and declares its kind; a body needing a stronger kind is rejected. | LR 8.2, LR 8.3 | [Functions and closures](08-functions.md) |
| LANG-08-03 | A lambda body cannot use a name that is not in its capture list. | LR 8.3 | [Functions and closures](08-functions.md) |
| LANG-08-04 | Callables are assignable as LR 8.4 says, and closures are boxed only when `dynamic fn` is written. | LR 8.4 | [Functions and closures](08-functions.md) |
| LANG-09-01 | There is no inheritance, runtime member lookup or reflection-based construction. | LR 9.1 | [Classes, interfaces, mixins and enums](09-classes.md) |
| LANG-09-02 | The generated constructor is callable only where every field is visible. | LR 9.2 | [Classes, interfaces, mixins and enums](09-classes.md) |
| LANG-09-03 | An implementation never strengthens a precondition or weakens a guarantee, and coherence holds. | LR 9.5 | [Classes, interfaces, mixins and enums](09-classes.md) |
| LANG-09-04 | A mixin requires methods only, adds no state, and conflicting mixins are rejected. | LR 9.6 | [Classes, interfaces, mixins and enums](09-classes.md) |
| LANG-09-05 | A `match` on a closed type is exhaustive without `otherwise`, naming each missing variant; or-pattern alternatives each count, bind nothing, and take no guard. | LR 9.8 | [Classes, interfaces, mixins and enums](09-classes.md) |
| LANG-09-06 | Pattern bindings own or borrow as LR 9.8 says. | LR 9.8 | [Classes, interfaces, mixins and enums](09-classes.md) |

## Failures, effects, concurrency, observation and tests

| Rule | In short | LR | Chapter |
|---|---|---|---|
| LANG-10-01 | An outcome stays wrapped until one of the consuming operations takes it. | LR 10.1, LR 10.2 | [Outcomes and failures](10-outcomes.md) |
| LANG-10-02 | Obligations survive storage in collections, fields and closures, and dropping one is rejected. | LR 10.7 | [Outcomes and failures](10-outcomes.md) |
| LANG-10-03 | A reported failure is gone, and `annotate` keeps exactly one obligation. | LR 10.6 | [Outcomes and failures](10-outcomes.md) |
| LANG-10-04 | `noalloc try` functions use the bounded failure representation. | LR 10.3 | [Outcomes and failures](10-outcomes.md) |
| LANG-10-05 | Recovering a failure that was not reported adds the `recovers` effect. | LR 10.4 | [Outcomes and failures](10-outcomes.md) |
| LANG-10-06 | Defects follow the closed taxonomy and can never be caught. | LR 10.8 | [Outcomes and failures](10-outcomes.md) |
| LANG-11-01 | There are exactly eight effects, computed transitively to a fixed point. | LR 11.1, LR 11.3 | [Effects and contracts](11-effects.md) |
| LANG-11-02 | Writes to fresh storage and through a function's own `editable` parameters are not `mutate`; writes through captures, guards and atomics are. | LR 11.1, LR 11.3 | [Effects and contracts](11-effects.md) |
| LANG-11-03 | `noalloc` is checked against the allocation whitelist for the actual target and telemetry plan. | LR 11.4 | [Effects and contracts](11-effects.md), [Lowering to Rust](appendix-c-lowering.md) |
| LANG-11-04 | A trusted summary is labelled `trusted`, never `proved`. | LR 11.3 | [Effects and contracts](11-effects.md) |
| LANG-11-05 | `async`, `try` and `recovers` are always written; across modules only written `pure`/`noalloc` count; weakening an exported function's modifiers is a reported API change. | LR 11.3 | [Effects and contracts](11-effects.md) |
| LANG-11-06 | A contract is a conjunction of the LR 11.5 elements, and configuration adds no predicate. | LR 11.5 | [Effects and contracts](11-effects.md) |
| LANG-11-07 | An `io` effect other than `report` and a log statement needs an `Authority` value in the signature; only the entry `main` receives the capabilities, as parameters (or creates them once with `Host.capabilities()`). | LR 11.6 | [Effects and contracts](11-effects.md) |
| LANG-11-08 | Contracts sit in the modifier position without arguments; `@` applies aspects only. | LR 11.5, LR 11.8 | [Effects and contracts](11-effects.md) |
| LANG-11-09 | Aspects have only the listed parts and elaborate before checking into a function that passes every ordinary check. | LR 11.8 | [Effects and contracts](11-effects.md) |
| LANG-12-01 | A `defer` body is checked at every exit, cannot change the outcome, and its places are initialised at every exit. | LR 12.1 | [Cleanup, async and concurrency](12-async.md) |
| LANG-12-02 | Failures inside `defer` are consumed there and never replace the primary outcome. | LR 12.1 | [Cleanup, async and concurrency](12-async.md) |
| LANG-12-03 | `Future` and `Task` are linear, a pending future is never destroyed, and cancellation is observed only at cancellation points. | LR 12.3, LR 12.4 | [Cleanup, async and concurrency](12-async.md) |
| LANG-12-04 | A scope joins or cancels every task it launched before its block exits. | LR 12.5 | [Cleanup, async and concurrency](12-async.md) |
| LANG-12-05 | Only `Shareable` environments cross the parallel and blocking pools, and loans cross only a cooperative `launch`. | LR 12.5, LR 12.7 | [Cleanup, async and concurrency](12-async.md) |
| LANG-12-06 | A `MutexGuard` is never live across `await`. | LR 12.3, LR 12.7 | [Cleanup, async and concurrency](12-async.md) |
| LANG-13-01 | `capture` snapshots a pure, infallible expression at the statement. | LR 13 | [Observation](13-observation.md) |
| LANG-13-02 | Required frame fields are assigned exactly once on every applicable exit; captures in loops and second captures are rejected. | LR 13 | [Observation](13-observation.md) |
| LANG-13-03 | A frame is bounded by its schema, and truncation or omission is recorded. | LR 13 | [Observation](13-observation.md) |
| LANG-13-04 | `log.info`, `log.warn` and `log.error` write one `log` record to the diagnostic plane, never standard output, need no authority and have the `io` effect. | LR 13 | [Observation](13-observation.md) |
| LANG-14-01 | Table rows are bound explicitly with `as`. | LR 14 | [Tests](14-tests.md) |
| LANG-14-02 | `expect` takes a `Bool` and records typed evidence. | LR 14 | [Tests](14-tests.md) |
| LANG-14-03 | In a test or property body a failing call that nothing else consumes, and unproved integer arithmetic, have an implied `or fail`. | LR 14, LR 6.4 | [Tests](14-tests.md) |

## Old requirement IDs

Before FSPEC-0.2 the requirements had IDs such as `OWN-001`. LR D maps each to its rule today; where a requirement
moved to the toolchain's own requirements or to the Platform Specification it names a family there rather than a rule
(LR D).

| Old IDs | Now | LR |
|---|---|---|
| SYN-001 | LANG-05-05, LANG-08-01 | LR D, LR 5.4, LR 8.1, LR 5, LR 8 |
| SYN-002 | LANG-03-03 (the case rule replaced symbol-category disambiguation) | LR D, LR 3.3, LR 3 |
| SYN-003; SYN-005, SYN-006 | the toolchain's requirements (formatter; diagnostics) | LR D |
| SYN-004 | LANG-05-07 | LR D, LR 5.7, LR 5 |
| TYP-001, TYP-002, TYP-003, TYP-004, TYP-006 | LANG-06-01, LANG-06-03, LANG-06-04, LANG-06-02, LANG-06-08 | LR D, LR 6.1, LR 6.3, LR 6.4, LR 6.8, LR 6 |
| TYP-005 | the toolchain's requirements (library metadata) | LR D |
| OWN-001 … OWN-006 | LANG-07-01, LANG-07-02, LANG-07-03, LANG-07-04, LANG-07-06, LANG-07-08 | LR D, LR 7.5, LR 7 |
| FUN-001, FUN-002 | LANG-08-02 | LR D, LR 8.2, LR 8 |
| OO-001 | LANG-09-01, LANG-09-04 | LR D, LR 9.1, LR 9 |
| GEN-001 | LANG-06-10, and the toolchain's requirements (bounded solving) | LR D, LR 6.10, LR 6 |
| MOD-001 | LANG-04-02 | LR D, LR 4.2, LR 4 |
| DOC-001 | the toolchain's requirements: every documentation example runs as a test | LR D |
| ERR-001 … ERR-006 | LANG-10-01, LANG-10-02, LANG-10-03 (ERR-003 and ERR-004), LANG-10-04, LANG-09-05 | LR D, LR 10.2, LR 9.8, LR 9, LR 10 |
| ERR-007 | the Telemetry and Runtime host kits (disposition policy) | LR D |
| EFF-001, EFF-002, EFF-004, EFF-005, EFF-006 | LANG-11-01, LANG-11-02, LANG-11-03, LANG-11-04, LANG-11-06 | LR D, LR 11.1, LR 11 |
| EFF-003 | removed with `nostate` | LR D, LR 11.2 |
| RES-001, RES-002 | LANG-05-02 and LANG-07-07; LANG-12-02 | LR D, LR 5.1, LR 7.9, LR 12.1, LR 5, LR 7, LR 12 |
| ASY-001, ASY-003; ASY-002; ASY-004 | LANG-12-03; LANG-12-05; LANG-12-04 | LR D, LR 12.3, LR 12 |
| ASY-005 | the Runtime host kit | LR D |
| OBS-001, OBS-002, OBS-006 | LANG-13-01, LANG-13-02, LANG-13-03 | LR D, LR 13 |
| OBS-003, OBS-004, OBS-005 | the Telemetry kit | LR D |
