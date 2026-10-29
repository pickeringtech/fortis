# Classes, interfaces, mixins and enums (LR 9)

<!-- covers: LR 9 -->
*Agent mode. Human mode: [Classes, interfaces, mixins and enums](../guide/09-classes.md). Source of truth: LR 9.*

> Preview: interfaces and mixins (LR 9.5, 9.6) arrive in v0.1. Classes, cleanup, enums and match (LR 9.1, 9.2, 9.3, 9.4, 9.7 and 9.8) are in v0.0.1.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-09-01 | No inheritance, no runtime member lookup, no reflection-based construction. | LR 9.1 |
| LANG-09-02 | The generated constructor `Name(field: v, …)` is callable only where every field is visible; private fields are settable only inside the module. | LR 9.2 |
| LANG-09-03 | An implementation may not strengthen a precondition (narrower parameter refinement) or weaken a guarantee (drop `pure`, add `try`/`async`, widen a return refinement); coherence: implement only in the package defining the interface or the type, at most one implementation per (interface, type). | LR 9.5 |
| LANG-09-04 | Mixins require methods only, add no fields, constructors or `cleanup`; a method name provided by two applied mixins and not declared by the class is rejected. | LR 9.6 |
| LANG-09-05 | `match` over a closed type (enum, optional) is exhaustive without `otherwise`; each missing variant is named. | LR 9.8 |
| LANG-09-06 | Pattern ownership: `match p` takes a read loan and binds read loans; `match move p` consumes and binds owned values; a match of a fresh value owns it; a linear payload needs the owning form and must be consumed. | LR 9.8 |

## Facts

- Class shape: `[export] [trivial] class Name[T] [derive …] [implements …] [with …] { fields, methods,
  associated functions, const, cleanup }`; fields and members private unless `export`; field defaults are constant
  expressions; one namespace for fields, methods, associated functions and constants (LR 9.1).
- Constructor arguments are owned positions: an existing ownership-bearing place needs `copy`/`move`; fields with
  defaults may be omitted. A field may be given by a pun, `Item(sku, wanted)` for `Item(sku: sku, wanted: wanted)`;
  a pun passes the place unmarked, so an owned ownership-bearing local is written `name: move name` (LR 9.2,
  LR 5.8).
- Spread: `Name(...copy p, f: v)` copies same-named fields of matching type (or with `From`), leaves `p`;
  `...move p` consumes `p`, drops untaken fields (never a linear one), and `p` may not have `cleanup`. Order: spread,
  explicit fields left to right, then defaults (LR 9.2).
- Associated function: no `self`, called `Name.f(…)`; the only path to values with private fields; constructors
  are named `create`, `of` or `from` (LR 9.2, LR 1.1).
- `derive Default` on a class gives `Name.default()`, no arguments: each field at its initialiser, a field without
  one at its type's default (`false`, `0`, empty, `null`, a class's own derived default, the `default()` of a type
  that implements `Default`, a newtype's representation's); a field with neither is `E-TYP-090` (a class without
  `Default`, a capability, `Int<1..10>`, an `Array`, `FixedList`, `FixedMap` or `OrderedMap`, also behind a
  newtype, a type parameter `T`). On an enum it is `E-TYP-090`: the enum `implements Default` and its `default()`
  returns the variant. Use it instead of a hand-written `create()` that sets every field empty (LR 6.10, LR 9.2,
  X-345).
- Receivers: `self` (read; call `v.m()`), `editable self` (exclusive; call `p.m()` on a `let mut` binding, an
  `editable` parameter or the method's own `editable self`, as `self.items.push(x)`; elsewhere `E-OWN-001`, LR 7.5), `owned self` (call
  `move p.m()`); fields as `self.field`; associated constants `const c = …` read as `Name.c` (LR 9.3). Older code
  writes `editable p.m()`, which stays accepted.
- A `pure` method may write through `editable self` (its footprint), nothing else (LR 9.3).
- `cleanup(editable self)` runs at destruction before fields drop; not `try`, not `async`, not callable; its
  effects join every function that may drop the type (LR 9.4).
- Interfaces: signatures with modifiers and receiver modes, associated functions, associated constants; no
  bodies, no associated types; `where` on type parameters; `requires` other interfaces (LR 9.5).
- Implementing: `implements I` with every member, `derive I`, or `implement I for T { … }` at module level
  (the form for generic implementations); same names alone never implement (LR 9.5).
- Bundle = requirements, no members: satisfied automatically (`Codec`). Marker = no members, no requirements:
  explicit only (`Authority`). `Copyable`, `Shareable` are structural (LR 9.5).
- `dynamic I`: owned and boxed (allocates on creation); `alias dynamic I` (loan); `Shared[dynamic I]`.
  Dynamic-eligible: no associated types, no generic methods, no `Self` by value. An `async` method through
  `dynamic` allocates its future (LR 9.5).
- Mixin: `requires` method signatures; body checked at each application against the class's exported members
  only; a class method beats a mixin method of the same name (LR 9.6).
- Enum variants UpperCamel, optional named payloads; `E.V`, `E.V(f: x)`; prefix optional in match patterns and
  wherever the expected type is that enum (return, argument, field initialiser, annotated `let`, the right operand
  of `==`/`!=`/`<`… whose left operand is that enum); a variant name where a function type is expected is its
  constructor (LR 9.7). A bare name that is also a class, error or type in scope, or one nothing expects the enum of
  (`let x = Error`, an unannotated `List.of(Error)`), is `E-TYP-040`: write `E.V`, or annotate the binding
  (LR 9.7, X-345).
- Patterns: `name`, `_`, literal, `Variant(…)`, `present(x)`/`null`, `success(x)`/`failure(f)`,
  `is Error(…) [as n]`, `Name(field: p, …)` (consuming only, every field named or `_` for non-linear, no
  `cleanup`), `(p1, p2)`, `{ f: p }`, `otherwise` (open types only: integers, strings, `Failure`). Guards
  `when b`; guarded arms do not count for exhaustiveness. No range patterns, no `x @ p` (LR 9.8).
- Or-patterns: `A | B | C => v` matches when any alternative does, left to right. Alternatives bind nothing
  (`Circle(radius: _)`, literals, `null`, `present(0)`), are never `otherwise`, and the arm takes no `when`
  guard (`E-MAT-003`); each alternative counts for exhaustiveness (`E-MAT-001` for a variant none names), and a
  `_` alternative is a catch-all (`E-MAT-002` over a closed enum). Use them for tables of equal results
  (LR 9.8, X-345).

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-TYP-070 | a name declared twice in one class (fields, methods, associated functions and constants share one namespace) | rename one | LR 9.1 |
| E-NAM-004 | generated constructor called where a field is not visible | call an exported associated function (`create`) | LR 4.4, LR 9.2 |
| E-TYP-050 | constructor call missing a field with no default and no spread source | pass the field | LR 9.2 |
| E-OBL-007 | `...move p` would drop a linear field the target lacks | consume the field first (destructure), or give the target a field for it | LR 9.2 |
| E-TYP-040 | `cleanup` called directly | let destruction run it; expose an explicit method for fallible finalisation | LR 9.4 |
| E-TYP-080 | missing member, incompatible signature (a dropped `pure`, an added `try`), unmet `requires`, unmet mixin requirement | provide the member with the interface's exact contract | LR 9.5 |
| E-TYP-060 | interface requirement cycle | break the cycle | LR 9.5 |
| E-MIX-001 | derive plus a hand-written method the derivation generates; or two mixins provide one method the class does not declare | drop one; declare the method in the class | LR 9.5, LR 9.6 |
| E-TYP-100 | implementation outside the packages defining the interface or the type; two implementations for one type | implement in the owning package; keep one | LR 9.5 |
| E-TYP-090 | `derive` whose rule is not met (a field does not implement the interface) | implement it for the field type, or write the method by hand | LR 6.10 |
| E-MAT-001 | match over a closed type misses variants (one record, each missing variant named) | add an arm per variant | LR 9.8 |
| E-MAT-002 | catch-all (`_`, a bare binding, `otherwise`) as the last arm over a closed enum | name every variant | LR 9.8 |
| E-MAT-003 | an or-pattern alternative binds a name or is `otherwise`, or the arm has a `when` guard | match the binding or guarded cases in arms of their own; `_` for an unused payload | LR 9.8 |

## Correction patterns

### E-TYP-070: field and method share a name (LR 9.1)

```fortis
// reject: E-TYP-070
class Account {
  balance: Int
  export pure func balance(self) Int => self.balance        // reject: E-TYP-070
}

test "balance" {
  expect true
}
```

```fortis
// accept
class Account {
  balance: Int
  export pure func current(self) Int => self.balance
}

test "balance" {
  expect Account(balance: 5).current() == 5
}
```

### E-TYP-080: implementation weakens the interface (LR 9.5)

```fortis
// reject: E-TYP-080
interface Priced {
  pure func price(self) Int
}

class Pear implements Priced {                  // reject: E-TYP-080
  export pure try func price(self) Int => 4
}

test "price" {
  expect true
}
```

```fortis
// accept
interface Priced {
  pure func price(self) Int
}

class Pear implements Priced {
  export pure func price(self) Int => 4
}

test "price" {
  expect Pear().price() == 4
}
```

### E-MIX-001: two mixins provide one method (LR 9.6)

```fortis
// reject: E-MIX-001
mixin English {
  export pure func hello(self) StringView from static => "hello"
}

mixin French {
  export pure func hello(self) StringView from static => "bonjour"
}

class Greeter with English, French {            // reject: E-MIX-001
  export id: Int
}

test "hello" {
  expect true
}
```

```fortis
// accept
mixin English {
  export pure func hello(self) StringView from static => "hello"
}

mixin French {
  export pure func hello(self) StringView from static => "bonjour"
}

class Greeter with English, French {
  export id: Int
  export pure func hello(self) StringView from static => "hi"
}

test "hello" {
  expect Greeter(id: 1).hello() == "hi"
}
```

### E-MAT-001 and E-MAT-002: closed matches (LR 9.8)

`E-MAT-001` is reported at the `match`; `E-MAT-002` at the catch-all arm.

```fortis
// reject: E-MAT-001
enum Light {
  Red
  Amber
  Green
}

pure func code(l: Light) Int {
  return match l {                       // reject: E-MAT-001
    Red => 1
    Amber => 2
  }
}

test "code" {
  expect code(Light.Red) == 1
}
```

```fortis
// reject: E-MAT-002
enum Light {
  Red
  Amber
  Green
}

pure func code(l: Light) Int {
  return match l {
    Red => 1
    otherwise => 2                       // reject: E-MAT-002
  }
}

test "code" {
  expect code(Light.Red) == 1
}
```

```fortis
// accept
enum Light {
  Red
  Amber
  Green
}

pure func code(l: Light) Int {
  return match l {
    Red => 1
    Amber => 2
    Green => 3
  }
}

test "code" {
  expect code(Red) == 1
  expect code(Green) == 3
}
```

### Class pattern only with `move` (LR 9.8)

A class pattern destructures, so it is permitted only in `match move` or `let … = move`, every field named (or `_`
for a non-linear one), and never for a class with `cleanup`. Which code reports a class pattern without `move` is
not yet settled (X-152).

```fortis
// accept
class Parcel {
  export label: String
  export grams: Int
}

pure func heavy(p: owned Parcel) Bool {
  return match move p {
    Parcel(label: _, grams: g) => g > 1000
  }
}

test "heavy" {
  expect heavy(Parcel(label: "books", grams: 2500))
  expect !heavy(Parcel(label: "feather", grams: 1))
}
```

### E-TYP-040: calling `cleanup` (LR 9.4)

```fortis
// reject: E-TYP-040
class Cell {
  export value: Int
  cleanup(editable self) {
  }
}

test "cell" {
  let mut c = Cell(value: 1)
  c.cleanup()                            // reject: E-TYP-040
  expect c.value == 1
}
```

```fortis
// accept
class Cell {
  export value: Int
  cleanup(editable self) {
  }
}

test "cell" {
  {
    let c = Cell(value: 1)
    expect c.value == 1
  }
}
```

### E-OBL-007: `...move` drops a linear field (LR 9.2)

```fortis
// reject: E-OBL-007
class Pending {
  export label: String
  export result: Outcome[Int]
}

class Label {
  export label: String
}

pure try func compute() Int => 1

pure recovers func relabel() String {
  let p = Pending(label: "p", result: compute())
  let l = Label(...move p)                 // reject: E-OBL-007
  return copy l.label
}

test "relabel" {
  expect relabel() == "p"
}
```

```fortis
// accept
class Pending {
  export label: String
  export result: Outcome[Int]
}

pure try func compute() Int => 1

pure recovers func relabel() String {
  let p = Pending(label: "p", result: compute())
  let Pending(label: label, result: r) = move p
  let value = move r or recover 0
  return "${label}${value}"
}

test "relabel" {
  expect relabel() == "p1"
}
```
