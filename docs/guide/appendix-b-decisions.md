# Appendix B: Closed decisions

<!-- covers: LR B -->
*Human mode. Agent mode: [Closed decisions, by topic](../agent/appendix-b-decisions.md). Source of truth: LR B.*

Some questions come up whenever someone meets Fortis: why no semicolons, why `give` and not `yield`, why there is
no `?` operator. The LR keeps a table of these questions with their answers, the **closed decisions** (LR B).
They are closed in the sense that Language 1.0 will not reopen them; each is stated normatively in the section
cited beside it, and this appendix explains the reasoning, which comes back again and again to one principle:
a piece of notation exists only if it changes what a reader may conclude about ownership, mutation, failure,
effects or timing (LR 1.1).

## Notation and syntax

| Decision | Why | LR |
|---|---|---|
| `let` and `let mut` declare; `=` only assigns | A reader can tell a new name from a change to an existing one without looking anything up. | LR 5.1 |
| No semicolons: a `;` is a parse error | Newlines end statements, with listed continuation tokens; a second way to end a statement would carry no decision. | LR 3.2 |
| `.ft` files, `//` the only comment form, documentation is the `//` run directly above a declaration | One comment form, so there is never a question of which kind documents what. | LR 3.1, LR 3.7 |
| Unsuffixed integer literals take their type from context (default `Int`) and must fit it | An out-of-range literal is a compile-time error, not a runtime surprise. | LR 3.5 |
| A float literal needs `.` or an exponent (`f32` suffix for `Float32`); a `Decimal` literal ends in `d` | The literal's type is visible on the page: `10` is an integer, `10.0` a float, `10d` a decimal. | LR 3.5 |
| A string literal is a `StringView from static`, converted to `String` (allocating) only where a `String` is expected | The one implicit conversion in the language, and the checker counts its allocation. | LR 3.5, LR 6.6 |
| `give` ends a block that produces a value (`yield` was replaced) | A distinct word for "this block's value", only where a block needs one; single-expression match arms need none. | LR 5.4, LR 9.8 |
| The type of `fail` is `Never` | `fail` can stand where any value is expected, such as a match arm. | LR 5.4 |
| `catch e { … }` names its failure explicitly | No implicit binding enters scope. | LR 10.4 |
| Named arguments after positionals, in any order, each once; parameter names are public API | A call can say what each argument is, and a rename shows up as an API change. | LR 5.8 |
| Defaults are pure constant expressions, re-evaluated per call; no user variadics (pass a `ListView[T]`) | A default never hides an effect or shared state; `List.of(…)` is the one variadic form. | LR 5.8, LR 6.2 |
| Constructors are named `create`, `of` or `from`; there is no `new` | Construction is an ordinary associated function, named for what it does. | LR 1.1, LR 9.2 |
| No transparent type aliases: `type` is always nominal | A reader never has to ask whether two type names are the same type. | LR 6.9 |
| No operator overloading (`Money.add(a, b)`, not `a + b`) | An operator always means the same thing; behaviour is named. | LR 6.10 |
| Or-patterns, range patterns, generic lambdas, const generics over `N`, associated types and explicit type arguments at calls are not in v1 | Each was left out of Language 1.0; the LR states the workaround (annotate the binding; take a generic parameter). | LR 6.2, LR 6.10, LR 8.2, LR 9.5, LR 9.8 |
| The standard package is `fortis` (`fortis.prelude`, `fortis.text`, …) | One name for the standard library. | LR 4.3 |

The decision about semicolons is checked like any rule. A stray `;` is `E-SYN-001` (LR 3.2), and the checker's
record carries a correction that deletes it, as the syntax class allows (Programme 7.1):

```fortis
// reject: E-SYN-001
export func main(args: List[String]) Void {
  let a = 1;          // reject: E-SYN-001
}
```

## Ownership and loans

| Decision | Why | LR |
|---|---|---|
| `move p`, `copy p`, `editable p` at the call site for existing places; fresh values need no marker | The call shows what happens to each argument. | LR 7.5 |
| `editable p.m()` and `move p.m()` for `editable self` and `owned self` methods | The receiver is an argument like any other, so it is marked the same way. | LR 7.5, LR 7.6 |
| `alias p` or `editable p` for an argument the result borrows `from` | A loan that outlives the statement is visible where it is taken. | LR 7.6 |
| Loans are lexical; no non-lexical lifetimes | A loan ends at the end of its block, which a reader can see; to end one early, close a block. | LR 7.3 |
| No stored loans: no lifetime parameters on user types, `E-OWN-053` | A reader never follows a region through a value's type; adding stored loans later breaks nothing. | LR 7.7 |
| Stored and returned loans both say `from`; `within` is removed | One word for "this loan comes from there". | LR 7.6, LR 7.7 |
| Two-phase borrows: reserve at argument evaluation, activate at call entry | Ordinary calls such as `sum = Money.add(sum, x) or fail` are legal without special cases. | LR 5.7 |
| A loan from two origins is rejected | Every returned loan has one origin a reader can name. | LR 7.6 |
| No partial moves | A value is either owned or moved, never half of each. | LR 7.2 |

## Functions, closures and failures

| Decision | Why | LR |
|---|---|---|
| Non-`Void` functions end every path in `return` or `fail`; `Void` functions may fall off the end | There is no implicit last value to miss. | LR 5.4 |
| Closure kinds `fn`, `fn mut`, `fn once` are always written | How a closure may be called is part of its type and visible at its definition. | LR 8.2 |
| Lambdas take the same modifiers as functions; capture brackets only when non-empty | One vocabulary for both; an empty `fn[]` would carry no information. | LR 8.2 |
| `recovers` is a modifier, written like `try` | A function that swallows a failure says so in its signature. | LR 8.1, LR 10.4 |
| `or die` is removed; use `try main` | A failing `main` prints one failure report and exits with status 1. | LR 4.6 |
| `report` is a statement; `report e as receipt` binds the receipt | Reporting consumes the failure, so it cannot also be propagated. | LR 10.6 |
| `nostate` is removed; teams that want to forbid `let mut` can turn on the opt-in lint `no-local-mutation`, which is not part of the language | Writing to storage that is fresh in the activation is not the `mutate` effect, so a `nostate` claim would add nothing a caller could rely on. | LR 11.1, LR 11.2 |
| `pure`, `noalloc`, `async`, `try`, `recovers` are the signature vocabulary; a full `effects(…)` clause was rejected as noise | Tools show the inferred `alloc`, `block` and `nondet`; the signature keeps the claims a caller relies on. | LR 11.2, LR B |

`or die` is gone because `try main` already does what it did. When `main` fails, the runtime prints exactly one
report of the failure to standard error and the process exits with status 1 (LR 4.6):

```fortis
// accept (exit status 1)
import fortis.host { Host }

export error Empty {
  code: "demo.empty"
  message: "nothing to do"
}

try func first(items: ListView[Int]) Int {
  if items.count() == 0 {
    fail Empty()
  }
  return items.get(0) ?? 0
}

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let items: List[Int] = List.default()
  caps.console.print("starting")
  let n = first(items) or fail
  caps.console.print("first is ${n}")
}
```

```output
starting
```

The second line never prints: `first` fails, `or fail` hands the failure to `main`'s caller, and the report goes
to standard error rather than standard output.

## Contracts, aspects, interfaces and serialisation

| Decision | Why | LR |
|---|---|---|
| Contracts are applied by name in the modifier position (`export FastCalculation func …`), never with `@`, and take no arguments | A contract only subtracts, so it sits with the other claims. | LR 11.5 |
| `@name(args)` applies an aspect, and is the only use of `@`; aspects have `needs`, `before`, `after`, `on failure`, `metadata`; `on failure` may re-fail; specialisation with `...rest` | An aspect adds behaviour around a function and can never change a line inside it. | LR 11.8 |
| No `around` hook and no re-running the body; failure handlers run innermost first | A retry is a library function taking a `fn once`, not an aspect. | LR 11.8 |
| `@observe(Schema)` attaches an observation schema; it is a core aspect | Observation uses the same mechanism as every other aspect. | LR 11.8, LR 13 |
| Interfaces are nominal; `requires` for sub-interfaces; bundles are satisfied automatically; no method bodies | Having methods with the right names is not an implementation; default behaviour is a mixin. | LR 9.5 |
| `derive Codec` (= `Encodable` + `Decodable`); canonical `toBytes`/`toText`; formats are packages with `…Serialised` mixins; hand-written `encode` and `derive` cannot coexist | One description of a value's shape serves every format. | LR 6.10 |
| Projection by constructor spread `...copy p` / `...move p`; `derive From[S]`; `to()` on every type | Narrowing one type into another is written once, with its ownership visible. | LR 6.10, LR 9.2 |
| Constants include record and variant-map literals, declaration names and the `Type.of(…)` constructors | Tables and route lists can be constants. | LR 5.1 |
| `VariantMap[E, V]` is written `{ Variant: value }`; an optional callable is written `(fn(…) R)?` | One entry per variant, checked for completeness; the parentheses say which part is optional. | LR 6.9 |

## Numbers, collections and concurrency

| Decision | Why | LR |
|---|---|---|
| `Float` and `Float32` have IEEE `==` but no `Equatable`, `Hashable` or `Comparable` | NaN breaks those interfaces' laws. | LR 6.5 |
| `Map` and `Set` iterate in a deterministic order per build, so they may be used in `pure` code | Iteration order never varies between runs or backends. | LR 6.7 |
| Length-refined collections are immutable; mutate an unrefined one and `validate` it | A length refinement is a property of a value. | LR 6.3 |
| One cooperative thread per scope tree; the parallel and blocking pools need `Shareable` | Cooperative tasks can borrow; only data that may cross threads goes to a pool. | LR 12.5 |
| Blocking in async code is forbidden; `launchBlocking` takes a synchronous `fn once` | The cooperative executor has one thread, so a blocked call would stall every task. | LR 11.1, LR 12.5 |
| Atomics are `AtomicInt` and `AtomicBool`, with `seqcst` and `acquire`/`release` only | A small set of orderings a reader can reason about. | LR 12.7 |
| `defer` has only a block form, and `defer await` is not in v1 | A deferred block lowers to cleanup code at each exit and cannot suspend. | LR 12.1 |
| Entry point: `main` in the manifest's entry module | The program's start is named in one place. | LR 4.6 |
| Negative examples are marked `// reject: E-XXX-NNN` | Every example says what the checker must do with it. | LR 0 |
| Test rows are bound with `cases { … } as row { … }` | The only name the table introduces is the one written after `as`. | LR 14 |
