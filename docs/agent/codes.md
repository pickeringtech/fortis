# Diagnostic codes

<!-- covers: LR 0 -->
*Agent mode. Source of truth: the LR section cited in each row; classes and mandatory fields: Programme 7.1 and
[records.md](records.md).*

Every diagnostic code the Language Reference names, grouped by class. **Rule** is the rule ID the toolchain's
record carries for the code (most often; where several rules share one code, the record names the one the
finding breaks; where the record carries a rule that is not the one the LR states for the code, the row says so,
D-57). **Pairs** is the agent page holding a reject/accept correction pair for the code, or the rules
for it. Look a record up by its `code`, read the cited section, apply the correction, re-check (Programme 7.1).

## Syntax (E-SYN)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-SYN-000 | LANG-03-02 | LR 3.2, LR 5.6, LR C.2 | not a sentence of the grammar (also `f() or fail ?? 0`; a loan type naming two origins) | fix the syntax; split the expression; `let v = f() or fail` on its own line | [03](03-lexical.md), [05](05-statements.md) |
| E-SYN-001 | LANG-03-01 | LR 3.2 | a `;` outside strings, comments, raw template regions | delete it (the record carries the edit) | [03](03-lexical.md), [B](appendix-b-decisions.md) |
| E-SYN-002 | LANG-03-04 | LR 3.4 | a keyword in a declaration or binding position | rename; a method may keep a keyword name | [A](appendix-a-keywords.md) |
| E-SYN-004 | LANG-05-06 | LR 5.6 | outcome operators, `??` and `&&`/`\|\|` mixed without parentheses | parenthesise | [05](05-statements.md) |

## Names and modules (E-NAM)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-NAM-001 | LANG-04-05 (as recorded; for an assignment the rule is LANG-05-01, D-57) | LR 4.5, LR 5.1 | an undeclared name (assignment to it, a use of it); also used for other unresolved names and module errors | declare it, import it, or fix the name | [05](05-statements.md), [04](04-modules.md) |
| E-NAM-002 | LANG-05-01 | LR 5.1 | assignment to an immutable binding or a parameter | `let mut`; copy a parameter into a `let mut` local | [05](05-statements.md) |
| E-NAM-003 | LANG-04-05 | LR 4.5 | a `let` shadows a visible binding | a new name | [05](05-statements.md), [04](04-modules.md) |
| E-NAM-004 | none stated (records carry the shadowing rule in error, D-57) | LR 4.4 | a declaration or member not visible here (not exported, or a constructor with a non-visible field) | `export` it, or use an associated function | [04](04-modules.md) |
| E-NAM-010 | LANG-04-03 | LR 4.3 | dependency names that make the package/module split ambiguous | rename a package or module | [04](04-modules.md) |
| E-NAM-020 | PROG-ARC-04 | LR 4.1 | lockfile does not pin a dependency, pins one no manifest reaches, or is unreadable | `fortis lock` | [04](04-modules.md) |
| E-NAM-021 | PROG-ARC-04 | LR 4.1 | a package, compiler, standard library or Rust toolchain digest differs from the lockfile | restore the pinned input, or re-lock deliberately | [04](04-modules.md) |
| E-NAM-030 | PLAT-01-03 | LR 4.1 | an import of a standard module the build target's feature set lacks (`check --target`, `build --target`) | remove the import, or choose a target with the feature set | [04](04-modules.md) |
| E-NAM-031 | PLAT-01-01 | LR 4.1 | a standard-package dependency outside the published compatibility set | depend on the set's version | [04](04-modules.md) |
| E-NAM-032 | PROG-CMP-06 | LR 4.1 | a native build of a package that ships a Cargo build script | remove the build script | [04](04-modules.md) |

## Type (E-TYP, E-MIX-001; Programme 7.1)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-TYP-010 | LANG-05-03 | LR 5.2, LR 6.8 | a non-`Bool` condition (`if n`, `if maybe`) | compare explicitly (`!= 0`, `!= null`) | [05](05-statements.md) |
| E-TYP-020 | LANG-06-05 | LR 6.3 | a float refinement | refine an integer, or validate by hand | [06](06-types.md) |
| E-TYP-030 | none stated (records carry the shadowing rule in error, D-57) | LR 4.4, LR 6.9 | an exported signature names a non-exported type, or a tuple, record or variant map (aspect parameters and the `fortis` package's own signatures excepted) | export the type; use a nominal class in exported signatures | [04](04-modules.md), [06](06-types.md) |
| E-TYP-040 | LANG-06-01 | LR 3.5, LR 6.1, LR 9.4, LR 10.1 | a type mismatch (an `Outcome[T]` where `T` is expected; an interpolated `String` bound to `StringView`; a non-`Displayable` interpolation; a direct `cleanup` call) | consume the outcome first; fix the type; never call `cleanup` | [10](10-outcomes.md), [06](06-types.md) |
| E-TYP-050 | LANG-05-09 | LR 5.8, LR 9.2 | a parameter without a default, or a constructor field, left unbound | pass it | [05](05-statements.md), [09](09-classes.md) |
| E-TYP-060 | LANG-09-03 | LR 9.5 | an interface requirement cycle | break the cycle | [09](09-classes.md) |
| E-TYP-070 | LANG-09-01 | LR 9.1 | a name declared twice in a class (fields, methods, constants share one namespace) | rename one | [09](09-classes.md) |
| E-TYP-080 | LANG-09-03 | LR 9.5, LR 9.6 | incomplete implementation: missing member or required interface, incompatible signature, unmet mixin requirement | add the member, implementation or method | [09](09-classes.md) |
| E-TYP-090 | none stated (records carry the trivial-set rule in error, D-57) | LR 6.10, LR 13 | a derivation whose rule is not met (a field without the interface; `Encodable` with a private field; a canonical codec call on a non-codec type; a non-`Encodable` schema field) | implement the interface on the field type, export the fields, or write `encode`/`decode` by hand | [06](06-types.md), [13](13-observation.md) |
| E-TYP-100 | LANG-06-10 | LR 6.10, LR 9.5 | coherence: an implementation outside the packages defining the interface or the type, or an overlap | implement in the defining package; remove the overlap | [09](09-classes.md) |
| E-TYP-110 | LANG-05-05 | LR 5.4 | a path reaches the end of a non-`Void` function; a block arm or `if`-expression branch without `give` (reported at the function) | `return`/`fail` on that path; `give` in the branch | [05](05-statements.md) |
| E-TYP-120 | LANG-05-05 | LR 5.4 | `give` outside a match arm or `if` expression, `recover` outside `catch`, `break`/`continue` outside a loop | move it into the construct it ends | [05](05-statements.md) |
| E-TYP-130 | LANG-05-09 | LR 5.8 | positional after named, unknown parameter name, parameter bound twice, too many arguments | fix the argument list | [05](05-statements.md) |
| E-TYP-140 | LANG-05-08 | LR 5.5 | an unused non-`Void` expression statement | `discard e`, or use the value | [05](05-statements.md) |
| E-TYP-150 | LANG-11-09 | LR 11.8 | an aspect application that does not match: unknown aspect, wrong arguments, unmet or ambiguous `needs`, `...rest` outside a parameter list | fix the application or the target's parameters | [11](11-effects.md) |
| E-TYP-160 | LANG-06-11 | LR 6.10 | a type argument fixed neither by the arguments nor by the expected type (`let names = List.default()`) | annotate the receiving binding | [06](06-types.md) |
| E-MIX-001 | LANG-09-04 | LR 6.10, LR 9.5, LR 9.6 | two mixins provide one method; a class derives an interface it also implements by hand | declare the method in the class; drop the `derive` or the hand-written method | [09](09-classes.md) |

## Ownership (E-OWN)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OWN-001 | LANG-07-01 | LR 7.5 | an existing ownership-bearing place assigned, returned, initialised into a field or passed `owned` without a marker | `copy p`, `alias p` or `move p` (the record carries an edit) | [07](07-ownership.md), [records](records.md) |
| E-OWN-002 | LANG-07-08 | LR 7.5, LR 7.8  | `copy` of a non-copyable place (`Shared`, `Weak`, a loan, a resource) | `retain()` for `Shared`/`Weak`; `move` or `alias` | [07](07-ownership.md) |
| E-OWN-010 | LANG-07-05 | LR 7.3, LR 8.3 | a conflicting loan: an exclusive loan while other loans are active; two `editable` captures of one owner | end the earlier loan by closing a block | [07](07-ownership.md), [08](08-functions.md) |
| E-OWN-011 | LANG-07-05 | LR 7.3, LR 8.3 | the owner used while loaned (suspended by an exclusive child, frozen by a read child) | use it after the loan's block ends | [07](07-ownership.md), [08](08-functions.md) |
| E-OWN-012 | LANG-07-04 | LR 5.7, LR 7.3 | a later argument moves, assigns or exclusively loans a place an earlier argument reserved | evaluate the conflicting argument into a binding first | [05](05-statements.md) |
| E-OWN-020 | LANG-07-04 | LR 7.4, LR 7.5 | use after move | `copy` before the move, or re-initialise a `let mut` | [07](07-ownership.md) |
| E-OWN-030 | LANG-06-07 | LR 6.11 | a `trivial class` with a non-trivial field | drop `trivial`, or make the field trivial | [06](06-types.md) |
| E-OWN-040 | LANG-07-01 | LR 7.2, LR C.2, LR 7.5  | a partial move | move the whole value; destructure it with `let Name(…) = move p` | [07](07-ownership.md), [C](appendix-c-lowering.md) |
| E-OWN-050 | LANG-07-06 | LR 7.6 | a result borrowing from two parameters | return an owned value, or split the function | [07](07-ownership.md) |
| E-OWN-051 | LANG-07-06 | LR 7.6 | returning a loan of a local or a temporary | return an owned value (`move`) | [07](07-ownership.md) |
| E-OWN-052 | LANG-07-04 | LR 7.3, LR 12.1 | a `defer` body using a binding of its block that holds a loan | use the owner, or move the deferred work | [12](12-async.md) |
| E-OWN-053 | LANG-07-06 | LR 7.7, LR C.2 | a lifetime parameter, or a field of a loan or loan-carrying type (not `from static`) | own the value, copy it, hold a `Shared`, or pass it per call | [07](07-ownership.md), [C](appendix-c-lowering.md) |

## Unsupported shape (E-OWN-090; Programme 7.1)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OWN-090 | LANG-07-09 | LR 7.10, LR C.2 | a shape with no lowering: a loan stored in `Shared`/`Mutex`, a `fn mut` escaping into `Shared` | own the value; `Mutex` or atomic state with a `fn` closure | [C](appendix-c-lowering.md) |

## Obligation (E-OBL, E-ERR-001; Programme 7.1)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OBL-001 | LANG-10-01 | LR 5.5, LR 6.12, LR 10.1, LR 11.8, LR 12.3, LR 12.4 | a linear value dropped, overwritten or discarded (an `Outcome`, `Failure`, `Future`, `Task`, `Must`; a never-polled future holding nothing linear and no exclusive loan may be discarded, LR 12.3); an unconsumed linear `owned` parameter on a failing `before` path | `or fail`, `or recover v`, `catch`, `match`, `report`, `await`, launch, or pass it on | [10](10-outcomes.md), [05](05-statements.md) |
| E-OBL-002 | LANG-10-01 | LR 10.4, LR 10.1  | a `catch` block path that does not end in `recover`, `return`, `fail` or a `Never` call | end every path | [10](10-outcomes.md) |
| E-OBL-005 | LANG-06-09 | LR 6.12, LR 12.4 | a linear value live across a cancellation point (`await`), or at an `or fail`, that does not implement `Abandonable` | consume it before the `await`, or pass it into the awaited call | [12](12-async.md) |
| E-OBL-006 | LANG-10-02 | LR 8.3, LR 10.7 | a `fn` or `fn mut` closure capturing a linear value by `move` | declare it `fn once` | [08](08-functions.md) |
| E-OBL-007 | LANG-10-02 | LR 9.2, LR 10.7 | `...move p` drops a linear field the target lacks | give the target the field, or consume it first | [09](09-classes.md) |
| E-OBL-008 | LANG-11-09 | LR 11.8 | an `after` hook that may fail when `R` is linear | do not fail in `after`, or return a non-linear type | [11](11-effects.md) |
| E-ERR-001 | LANG-10-01 | LR 5.4, LR 10.2 | `fail` outside a `try` function | declare the function `try` | [05](05-statements.md), [10](10-outcomes.md) |

## Effect (E-EFF)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-EFF-001 | LANG-11-01 | LR 11.1, LR 11.2 | the summary exceeds the claims: an effect in `pure`, an allocation in `noalloc`, `suspend` without `async`, `fail` without `try`, `recovers` without `recovers` (the record names the claim's rule) | remove the effect, or write the modifier the summary needs | [11](11-effects.md), [C](appendix-c-lowering.md) |
| E-EFF-010 | LANG-11-07 | LR 11.6 | `io` through a capability with no `Authority` parameter; `Host.capabilities()` outside `main` or twice | take the capability as a parameter; call `Host.capabilities()` once in `main` | [11](11-effects.md) |
| E-EFF-020 | LANG-11-01 | LR 11.1 | the `block` effect in an `async` function or anything it calls | move the work to `launchBlocking` | [12](12-async.md), [11](11-effects.md) |
| E-EFF-030 | LANG-11-03 | LR 11.4 | the telemetry plan attaches an allocating record to a `noalloc` function (at the policy site) | change the plan, not the function | [11](11-effects.md) |
| E-EFF-040 | LANG-11-05 | LR 11.3 | warning from `fortis check --api-baseline OLD`: an exported function lost a written `pure`/`noalloc` or gained `async`/`try`/`recovers` | restore the claim, or make the API change deliberately | [11](11-effects.md) |

## Refinement unproved (E-REF)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-REF-001 | LANG-06-02 | LR 3.5, LR 6.3 | a value (or literal) whose interval lies outside the target's refinement | `T.validate(x) or fail`, or a wider declared type | [06](06-types.md), [proofs](06-proofs.md) |
| E-REF-002 | LANG-06-02 | LR 6.4, LR 6.3  | an integer operator the interval rules cannot prove (overflow, division by zero, shift range, `MIN / -1`) | the checked operation (`Int.add(a, b) or fail`), or a refined type that proves it | [06](06-types.md), [proofs](06-proofs.md) |
| E-REF-003 | LANG-06-02 | LR 6.3 | an `editable` loan or mutation of a length-refined collection | mutate an unrefined collection, then `validate` | [06](06-types.md) |

## Patterns (E-MAT)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-MAT-001 | LANG-09-05 | LR 9.8 | a `match` over a closed enum missing variants (each named) | add the arms | [09](09-classes.md) |
| E-MAT-002 | LANG-09-05 | LR 9.8 | `otherwise`, or a last `_` or binding arm, over a closed enum or optional | list the variants | [09](09-classes.md) |

## Closures (E-FUN)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-FUN-010 | LANG-08-02 | LR 8.2 | a lambda body needing a stronger kind than declared (`fn` that mutates a capture) | declare `fn mut` or `fn once` | [08](08-functions.md) |
| E-FUN-011 | LANG-08-03 | LR 8.3 | a name used in a lambda body but not in its capture list | add it to the capture list with a marker | [08](08-functions.md) |

## Defer and async (E-DEF, E-ASY)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-DEF-001 | LANG-12-01 | LR 12.1 | `return`, `fail`, `give`, `break`, `continue` or `await` inside `defer` | move it out; consume failures with `or recover`/`catch`/`report` | [12](12-async.md) |
| E-DEF-002 | LANG-12-01 | LR 12.1 | a place the `defer` names is moved out before an exit | keep it owned until every exit (lend instead of moving), or do the deferred work before the move | [12](12-async.md) |
| E-ASY-001 | LANG-12-04 | LR 12.3, LR 12.5 | a `Task` leaves its scope block (`return`, `give`, assignment outside) | `await` it inside the scope, or `detach` it to a `Supervisor` | [12](12-async.md) |
| E-ASY-002 | LANG-12-06 | LR 12.3, LR 12.7 | a `MutexGuard` held across `await` | end the guard's block before the `await` | [12](12-async.md) |
| E-ASY-003 | LANG-12-05 | LR 12.7, LR C.2 | a pool launch whose future or closure holds a loan or a non-`Shareable` value | pass owned `Shareable` data | [12](12-async.md), [C](appendix-c-lowering.md) |

## Observation (E-OBS)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OBS-001 | LANG-13-02 | LR 13 | a required frame field unassigned on an applicable exit | `capture` it before that exit; changing the schema's `required:` is a change to the schema, never a correction | [13](13-observation.md) |
| E-OBS-010 | LANG-13-02 | LR 13 | a second `capture` of one field on one path | capture once | [13](13-observation.md) |
| E-OBS-011 | LANG-13-02 | LR 13 | a `capture` inside a loop | use `record` for repeated measurements | [13](13-observation.md) |
| E-OBS-012 | LANG-13-01 | LR 13 | a `capture`/`record` expression with an effect, or naming no field of the attached schema | a pure, infallible expression; a declared field | [13](13-observation.md) |

## Resource limits and internal defects (E-LIM, E-INT)

These have no automatic correction (Programme 7.1).

| Code | Rule | LR | Trigger | What to do | Pairs |
|---|---|---|---|---|---|
| E-LIM-001 | PROG-SEC-01 | LR 6.10 | a native build's instantiation budget (32) exhausted: a generic function instantiating itself at ever larger types | recurse through a non-generic helper, or a `dynamic` value | [06](06-types.md) |
| E-LIM-002 | PROG-SEC-01 | LR 3.1 | a source file over 16 MiB or 2,000,000 tokens, or nested more than 256 deep | split the file; flatten the nesting | [03](03-lexical.md) |
| E-LIM-003 | PROG-SEC-01 | LR 4.1 | a manifest over 1 MiB or 10,000 lines | shrink the manifest | [04](04-modules.md) |
| E-LIM-004 | PROG-SEC-01 | LR 4.1 | a lockfile over 8 MiB or 100,000 lines | shrink the dependency set | [04](04-modules.md) |
| E-LIM-005 | PROG-SEC-01 | LR 4.1 | JSON the toolchain reads (caches, saved fuzz inputs) over 256 MiB or nested more than 128 deep | remove the input | [04](04-modules.md) |
| E-INT-001 | PROG-CMP-05 | LR 4.1 | a checker defect; `reproduction` names a minimised input and the command | report it with the reproduction; never work around it silently | [04](04-modules.md) |
| E-INT-002 | PROG-CMP-05 | LR 4.1 | a native build whose generated Rust does not compile, at the Fortis function it came from | report it with the function it names | [04](04-modules.md) |
| E-INT-003 | PROG-CMP-05 | LR 4.1 | a native build cannot run a tool it needs (cargo, rustc, the linker driver) | install the named tool | [04](04-modules.md) |

## Kit codes

The kits name their own codes in the Platform Specification; the records have the same shape (Programme 7.1).

| Prefix | Class | Codes | Source |
|---|---|---|---|
| E-DI | Dependency injection (mandatory `consumer`, `slot`, `candidates`) | E-DI-001 … E-DI-006, E-DI-008 … E-DI-011 | Platform 3 |
| E-TEL | Telemetry (mandatory `plan_line`, `projection`, `field`) | E-TEL-001 … E-TEL-007 | Platform 6.4 |
