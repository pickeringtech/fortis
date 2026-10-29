# Closed decisions (LR B)

<!-- covers: LR B -->
*Agent mode. Human mode: [Closed decisions](../guide/appendix-b-decisions.md). Source of truth: LR B.*

Decisions Language 1.0 does not reopen. Never write the rejected form; the normative statement is in the section
cited (LR B).

## Syntax and literals

| Topic | Decision | LR |
|---|---|---|
| Declaration vs assignment | `let` / `let mut` declare; `=` assigns only | LR 5.1 |
| Semicolons | `;` is `E-SYN-001` | LR 3.2 |
| File extension, comments | `.ft`; `//` only; documentation = the `//` run directly above a declaration; no `/* */`, no `///` | LR 3.1, LR 3.7 |
| Integer literal | type from context, default `Int`; must fit the target refinement | LR 3.5 |
| Float literal | needs `.` or exponent; `f32` suffix for `Float32` | LR 3.5 |
| Decimal literal | `d` suffix (`19.99d`) | LR 3.5 |
| String literal | `StringView from static`; converts to `String` only where a `String` is expected (allocates) | LR 3.5, LR 6.6 |
| Block value | `give` (not `yield`); single-expression match arms need none; `catch` ends in `recover` | LR 5.4, LR 9.8 |
| Type of `fail` | `Never` | LR 5.4 |
| Catch binding | explicit: `catch e { … }` | LR 10.4 |
| Named arguments | after positionals, any order, each once; names are public API | LR 5.8 |
| Default arguments | pure constant expressions, re-evaluated per call | LR 5.8 |
| Variadics | none in user code; take `ListView[T]`, pass `List.of(…)` | LR 5.8, LR 6.2 |
| Constructor naming | `create`, `of`, `from`; no `new` | LR 1.1, LR 9.2 |
| Transparent type aliases | none; `type` is always nominal | LR 6.9 |
| Operator overloading | none; `Type.add(a, b)` | LR 6.10 |
| Or-patterns | narrow: `A \| B => v`, whole patterns of an arm, no bindings, no guard | LR 9.8 |
| Not in v1 | range patterns, generic lambdas, generic functions over `N`, associated types, explicit type arguments at calls (annotate the binding) | LR 6.2, LR 6.10, LR 8.2, LR 9.5, LR 9.8 |
| Standard package | `fortis`; no `std` | LR 4.3 |
| Negative example marker | `// reject: E-XXX-NNN` | LR 0 |
| Test rows | `cases { … } as row { … }` | LR 14 |

## Ownership

| Topic | Decision | LR |
|---|---|---|
| Call-site markers | `move p`, `copy p`, `editable p` for existing places; fresh values unmarked | LR 7.5 |
| Receiver markers | `move p.m()` for `owned self`; an `editable self` call is `p.m()` on a root that shows it may change (`let mut`, `editable` parameter or capture, exclusive loan, the method's own `editable self`) and `E-OWN-001` on any other root, marked or not; `editable p.m()` stays accepted | LR 7.5, LR 7.6 |
| Kept loans at call sites | `alias p` / `editable p` for an argument the result borrows `from` | LR 7.6 |
| Returning a local | `return local` moves it; `return move local` stays accepted; parameters, fields and captures keep their marker | LR 7.5, LR 7.6 |
| Partial moves | not in v1 (`E-OWN-040`) | LR 7.2 |
| Non-lexical lifetimes | not in v1; loans end at their block | LR 7.3 |
| Stored loans | not in v1 (`E-OWN-053`): loans live in locals, parameters and `from` returns | LR 7.7 |
| Stored and returned loans | both spell `from`; `within` removed | LR 7.6, LR 7.7 |
| Two-phase borrows | reserve at argument evaluation, activate at call entry | LR 5.7 |
| Two-origin loans | rejected | LR 7.6 |

## Functions, failures, effects

| Topic | Decision | LR |
|---|---|---|
| `Void` functions | may fall off the end; non-`Void` need `return`/`fail` on every path; a function or method head leaves out `Void`, a function type keeps it | LR 5.4, LR 8.1 |
| Closure kinds | `fn`, `fn mut`, `fn once`, always written; a `fn mut` held by a `let mut` is called `f()` | LR 8.2 |
| Lambda modifiers | same as named functions; capture brackets only when non-empty | LR 8.2 |
| `recovers` | a modifier, written like `try` | LR 8.1, LR 10.4 |
| `or die` | removed; `try main` (one report, exit status 1) | LR 4.6 |
| `report` | statement; `report e as receipt` binds the receipt | LR 10.6 |
| `nostate` | removed; opt-in lint `no-local-mutation` only | LR 11.2 |
| Effect clauses | the vocabulary is `pure`/`noalloc`/`async`/`try`/`recovers`; no `effects(…)` clause; tools show inferred `alloc`, `block`, `nondet` | LR 11.2, LR B |
| Contracts | by name in modifier position (`export FastCalculation func …`); never `@`; no arguments | LR 11.5 |
| Aspects | `@name(args)`, lowercase, the only use of `@`; parts `needs`, `before`, `after`, `on failure`, `metadata`; `...rest` specialisation; `on failure` may re-fail; user packages may declare them | LR 11.8 |
| Aspect hooks | failure handlers innermost first; no `around`; no body re-execution | LR 11.8 |
| Observation attachment | `@observe(Schema)`, a core aspect | LR 11.8, LR 13 |
| Interfaces | nominal; `requires` for sub-interfaces; bundles satisfied automatically; no method bodies | LR 9.5 |
| Serialisation | `derive Codec` = `Encodable` + `Decodable`; canonical `toBytes`/`toText`; formats as packages with `…Serialised` mixins; hand-written `encode` and `derive` never coexist | LR 6.10 |
| Projection | `...copy p` copies taken fields; `...move p` consumes the whole source; `derive From[S]`; `to()` on every type | LR 6.10, LR 9.2 |
| Constants | include record and variant-map literals, declaration names, `Type.of(…)` intrinsics | LR 5.1 |
| Variant maps | `VariantMap[E, V]` written `{ Variant: value }`; optional callable `(fn(…) R)?` | LR 6.9 |
| Entry point | `main` in the manifest's entry module, `export` implied; its parameters are the capabilities and arguments it needs, supplied by type | LR 4.6 |

## Numbers, collections, concurrency

| Topic | Decision | LR |
|---|---|---|
| Float equality | IEEE `==` allowed; no `Equatable`/`Hashable`/`Comparable` | LR 6.5 |
| Hash iteration | deterministic per build; allowed in `pure` | LR 6.7 |
| Length-refined collections | immutable; mutate an unrefined collection and `validate` | LR 6.3 |
| Executor | cooperative single thread per scope tree; parallel/blocking pools need `Shareable` | LR 12.5 |
| Blocking in async code | forbidden; `launchBlocking` takes a synchronous `fn once` | LR 11.1, LR 12.5 |
| Atomics | `AtomicInt`, `AtomicBool`; `seqcst`, `acquire`/`release` | LR 12.7 |
| `defer` | block form only; `defer await` is `E-DEF-001` | LR 12.1 |

## Correction patterns

### `or die` → `try main` (LR 4.6)

```fortis
// accept
import fortis.host { Console }

export error Empty {
  code: "demo.empty"
}

try func first(items: ListView[Int]) Int {
  if items.count() == 0 {
    fail Empty()
  }
  return items.get(0) ?? 0
}

try func main(console: Console) {
  let n = first(List.of(4, 5)) or fail
  console.print("${n}")
}
```

```output
4
```

### `;` (LR 3.2)

```fortis
// reject: E-SYN-001
pure func one() Int {
  return 1;                 // reject: E-SYN-001
}

test "one" {
  expect one() == 1
}
```

```fortis
// accept
pure func one() Int {
  return 1
}

test "one" {
  expect one() == 1
}
```
