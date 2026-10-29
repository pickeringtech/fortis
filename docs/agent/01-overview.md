# Overview: design invariants and notation (LR 1, LR 2)

<!-- covers: LR 1, LR 2 -->
*Agent mode. Human mode: [A tour of Fortis](../guide/tour.md). Source of truth: LR 1 and LR 2.*

LR 1 and LR 2 state no numbered rules; they fix the invariants every later rule serves. Write code so that each of
these is visible on the line.

## Invariants

| Invariant | Consequence when writing code | LR |
|---|---|---|
| Every local decision is visible: ownership operations, mutation permission, failure disposition, effect claims, observation points. | Never rely on inference for these; write the marker, modifier or handler. A marker the declaration already shows is left out: an `editable self` call on a `let mut` binding, `return local` (LR 7.5). | LR 1 |
| Imported signatures can never be weaker than their bodies. | Read the callee's signature (`fortis view --symbol`) instead of its body. | LR 1 |
| Library-first: program = packages, package = modules, module = one file; HTTP, data, web UI, telemetry, jobs are kits (libraries). | The core imports nothing from kits. | LR 1 |
| Explicit ownership: assigning or passing an existing owned value states `copy`, `alias` or `move`; mutable access is `editable`. No hidden clones or reference counts. | See [ownership](07-ownership.md). | LR 1 |
| Three distinct answers: `T?`, `Bool`, `Outcome[T]` with `??`, `\|\|`, `or`. Nothing is truthy. | Compare explicitly (`x != null`, `n != 0`). | LR 1 |
| Linear failures: an outcome is consumed exactly once (`or fail`, `or recover`, `catch`, `match`, `report`); dropping one is a compile error. | Every call to a `try` function is followed by a disposition; in a `test` or `property` body `or fail` is implied (LR 14). | LR 1 |
| Closed effect set: `pure`, `noalloc`, `async`, `try` are checked claims over eight effects; contracts are conjunctions of them. | See [effects](11-effects.md). | LR 1 |
| Lexical loans; synchronous `defer`; one cancellation point (`await`); structured `scope`. | End a loan early by closing a block. | LR 1 |
| Two backends, one semantics: development VM and safe-Rust native code run the same checked IR. | Never depend on backend behaviour. | LR 1 |
| Absent by design: scripting semantics, runtime reflection, inheritance, macros, exceptions, implicit conversions. | Do not emulate them. | LR 1 |

## Notation

Every non-alphabetic symbol and what it answers (LR 1.1):

| Notation | Meaning | Never |
|---|---|---|
| `{ }` | block, record literal, record type | |
| `( )` | call, parameter list, tuple, grouping | |
| `Name[ ]` | generic arguments after an uppercase name | |
| `place[ ]` | index after a lowercase place | slices |
| `fn[ ]` | a lambda's capture list, only when non-empty | `fn[]` |
| `+ - * / %` `<< >>` `& \| ^` | arithmetic and bitwise; proof-required on integers | overloading |
| `== !=` `< <= > >=` | `Equatable`, `Comparable`; IEEE comparison on floats | chained comparisons |
| `&& \|\|` `!` | Boolean logic | truthiness |
| `+=` and friends | `x = x op e` | |
| `< >` after a type | refinement (type position only) | |
| `:` | has type, named argument, record field, label | ternary, slicing |
| `=` | assignment, `let` initialiser | equality |
| `=>` | expression body of a function, lambda or match arm | map literals |
| `.` | member access, module paths | ranges |
| `..` `..<` | integer ranges in `for` headers and refinements | |
| `?` `?.` | optional type suffix, optional chaining | error propagation |
| `??` | absent-value fallback | |
| `@name(args)` | apply an aspect, on the lines above a function head | contracts |
| `${ }` | string interpolation | |
| `//` | comment; documentation directly above a declaration | `///`, `/* */` |
| `,` | list separator | |
| `...` | spread a place into a constructor; forward remaining aspect arguments | |

Keywords that carry a decision (LR 1.1): `let` (new name), `mut` (changes), `copy`/`alias`/`move`/`editable`
(how a place is used), `owned`/`editable` on a parameter, `try`/`pure`/`noalloc`/`async`/`recovers` and named
contracts in the modifier position, `or fail`/`or recover`/`catch`, `give`, `export`.

Silence (LR 1.1): an unmarked argument, or the receiver of a `self` method, is a read loan ending when the call
returns (or with the future of an `async` call); an unmarked `for` source is a read loan for the loop; an unmarked
assignment is a fresh or trivial value. The unmarked receiver of an `editable self` method, or a `fn mut` being
called, is the exclusive loan its root's declaration shows (`let mut`, `editable`), and `return local` moves the
local (LR 7.5).

Removed, never write (LR 1.1): semicolons; `///`, `/* */`; `yield`; `or die`; `nostate`; `self: editable Self`
(write `editable self`); `alias editable` (write `editable`); `alias type` (a `type` is always nominal); `within`;
`give` in a single-expression arm; `fn[]`; `::`; operator overloading; implicit `it`; postfix `?` for errors; `new`
(name constructors `create`, `of`, `from`); `return` in an expression-bodied function.

Two deliberate costs (LR 1.1): `(f() or fail).amount` needs parentheses, so write `let v = f() or fail` on its own
line; an `async` call builds a `Future`, and `await` or `launch` is the visible point where it runs.

Formatted source shows every inferred fact (closure kind, loan modes, a contract's modifiers), except a lambda's
inferred `pure`/`noalloc`, which the function type it is passed to states (LR 1.1, LR 8.2). `fortis fmt --write`
also rewrites older forms into the forms these pages teach where the meaning is unchanged; run it before review.

## Correction patterns: notation from other languages

All of these are `E-SYN-000` (LR 3.2), a sentence outside the grammar of LR 1.1's notation.

### Postfix `?` for errors (LR 1.1)

```fortis
// reject: E-SYN-000
try func parse(n: Int) Int => n

try func twice(n: Int) Int {
  let v = parse(n)?                   // reject: E-SYN-000
  return v
}

test "twice" {
  expect twice(2) == 2
}
```

```fortis
// accept
try func parse(n: Int) Int => n

try func twice(n: Int) Int {
  let v = parse(n) or fail
  return v
}

test "twice" {
  expect twice(2) == 2
}
```

### Empty capture list (LR 1.1)

```fortis
// reject: E-SYN-000
pure func countPositive(items: List[Int]) Int {
  let positive = fn[](n: Int) Bool => n > 0    // reject: E-SYN-000
  return items.countWhere(positive)
}

test "countPositive" {
  expect countPositive(List.of(1, -2, 3)) == 2
}
```

```fortis
// accept
pure func countPositive(items: List[Int]) Int {
  let positive = pure noalloc fn(n: Int) Bool => n > 0
  return items.countWhere(positive)
}

test "countPositive" {
  expect countPositive(List.of(1, -2, 3)) == 2
}
```

### `return` in an expression body (LR 1.1)

```fortis
// reject: E-SYN-000
pure func double(n: Int<0..100>) Int => return n * 2    // reject: E-SYN-000

test "double" {
  expect double(2) == 4
}
```

```fortis
// accept
pure func double(n: Int<0..100>) Int => n * 2

test "double" {
  expect double(2) == 4
}
```

### `::` paths and `new` (LR 1.1)

```fortis
// reject: E-SYN-000
class Point {
  export x: Int
}

pure func origin() Point => Point::new(0)     // reject: E-SYN-000

test "origin" {
  expect origin().x == 0
}
```

```fortis
// accept
class Point {
  export x: Int
  export pure func of(x: Int) Point => Point(x)
}

pure func origin() Point => Point.of(0)

test "origin" {
  expect origin().x == 0
}
```

## The worked module (LR 2)

LR 2 is one module that uses every construct; [the tour](../guide/tour.md) has a self-contained counterpart that
runs. Its constructs, by section:

| Construct in LR 2 | Example | LR |
|---|---|---|
| imports: aliased module, braced members | `import fortis.text`, `import fortis.money { Money, Currency }` | LR 4.3 |
| nominal refined newtype | `export type CustomerId = Int<1..>` | LR 6.9, LR 6.3 |
| class with exported and private fields, generated constructor | `export class Line derive Equatable { … }` | LR 9.1, LR 9.2 |
| refined field type | `quantity: Int<1..10000>` | LR 6.3 |
| method modes | `total(self)`, `issue(editable self, …)` | LR 9.3, LR 7.6 |
| enum with payloads | `Issued(at: Timestamp)` | LR 9.7 |
| declared failure with payload and template | `export error CurrencyMismatch { … }` | LR 10.3 |
| interface, mixin with `requires` | `Renderable`, `Summarised` | LR 9.5, LR 9.6 |
| fallible associated constructor | `pure try func create(…) Invoice` with `fail` | LR 9.2, LR 10.1 |
| `or fail` propagation, return of a local (a move) | `let lineTotal = line.total() or fail`, `return sum` | LR 10.2, LR 7.5 |
| `editable self` call on a `let mut` binding, unmarked | `out.appendLine(…)` | LR 7.5 |
| exhaustive `match` with payload bindings | `Paid(at: paidAt, reference: ref) => …` | LR 9.8 |
| closure with explicit kind and capture | `pure fn[alias threshold](line: Line) Bool { … }` | LR 8.2, LR 8.3 |
| `defer`, `catch` with `report` and `recover`, `recovers` | `withAudit` | LR 12.1, LR 10.4 |
| tests, table test with `as row` | `test "quantity boundary" cases { … } as row { … }` | LR 14 |
| entry point in another package, capabilities as parameters | `try func main(console: Console)` | LR 4.6 |
