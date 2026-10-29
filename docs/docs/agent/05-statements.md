# Statements and expressions (LR 5)

<!-- covers: LR 5 -->
*Agent mode. Human mode: [Statements and expressions](../guide/05-statements.md). Source of truth: LR 5.*

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-05-01 | `let x = e` / `let mut x = e` declare (initialiser required); `let Pattern = move p` destructures a class, tuple or record, and a pattern that can fail to match (a variant, a literal, `null`, `present(…)`, at any depth) is `E-MAT-004`: use `match`; `x = e` assigns only to a visible `let mut` binding, or a field path rooted in a `let mut` binding, an `editable` parameter or an `editable` loan. Assignment is a statement and does not chain. | LR 5.1 |
| LANG-05-02 | Every block exit (fall-through, `return`, `fail`, `break`, `continue`, `give`) ends the block's loans, runs its `defer`s LIFO, then drops its owned locals in reverse initialisation order. | LR 5.1 |
| LANG-05-03 | `if` and `while` conditions are `Bool`; nothing else converts. | LR 5.2 |
| LANG-05-04 | `for i in a..<b` (or `a..b`) gives an immutable index refined to the range; the index is never assignable. | LR 5.3 |
| LANG-05-05 | A non-`Void` function ends every path in `return`, `fail` or a `Never` call; no implicit value. | LR 5.4 |
| LANG-05-06 | Precedence and associativity follow the LR 5.6 table; outcome operators (`or fail`, `or recover`, `catch`), `??` and `&&`/`\|\|` do not mix without parentheses. | LR 5.6 |
| LANG-05-07 | Operands and arguments evaluate left to right, once; receiver before arguments; calls run evaluate, activate, invoke, release. | LR 5.7 |
| LANG-05-08 | A non-`Void` expression statement needs `discard`, except a call of a function declared `@ignorable`; a linear value (`Outcome`, `Future`, `Task`, …) cannot be discarded, except a never-polled future whose environment holds no linear value and no exclusive loan (it emits a `not_started` record). | LR 5.5, 12.3 |
| LANG-05-09 | Positional arguments first, then named in any order; each parameter bound once; parameter names are exported API. A pun `wanted` is `wanted: wanted`: after a named argument, and anywhere in a class constructor; before the first named argument a bare name is positional; an owned ownership-bearing local is written `name: move name`, never punned. | LR 5.8, X-345 |

Related, stated in LR 4.5: shadowing a visible binding is `E-NAM-003`.

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-NAM-001 | assignment to an undeclared name | declare it with `let mut`, or fix the name | LR 5.1 |
| E-NAM-002 | assignment to an immutable binding or a parameter | declare it `let mut`; a parameter is never reassignable, so copy it into a `let mut` local | LR 5.1 |
| E-NAM-003 | `let` shadows a visible binding | choose a new name | LR 4.5 |
| E-TYP-010 | non-`Bool` condition (`if n`, `if maybe`) | compare explicitly: `n != 0`, `maybe != null` | LR 5.2 |
| E-TYP-110 | a path reaches the end of a non-`Void` function, or a block arm or `if`-expression branch does not end in `give` | add `return`/`fail` on that path, or `give` in the branch | LR 5.4 |
| E-TYP-120 | `give` outside a match arm or `if` expression, `recover` outside `catch`, `break`/`continue` outside a loop | move the statement into the construct it ends | LR 5.4 |
| E-ERR-001 | `fail` outside a `try` function | declare the function `try`, or handle the condition without failing | LR 5.4 |
| E-TYP-140 | unused non-`Void` expression statement (a call of an `@ignorable` function excepted) | `discard e`, or bind and use the value; declare the function `@ignorable` when most callers drop its result | LR 5.5 |
| E-OBL-001 | `discard` of a linear value (a never-polled future holding nothing linear and no exclusive loan excepted) | consume it: `or fail`, `or recover v`, `catch`, `match`, `await`, launch | LR 5.5, 12.3 |
| E-SYN-004 | outcome operators, `??` and `&&`/`\|\|` mixed without parentheses | parenthesise the intended grouping | LR 5.6 |
| E-SYN-000 | not a sentence of the grammar (for example `f() or fail ?? 0`) | split the expression; `let v = f() or fail` on its own line | LR 5.6 |
| E-OWN-012 | a later argument moves, assigns or exclusively loans a place an earlier argument reserved | evaluate the conflicting argument into a binding first | LR 5.7 |
| E-TYP-130 | positional after named, unknown parameter name, parameter bound twice, too many arguments | fix the argument list | LR 5.8 |
| E-TYP-050 | a parameter without a default is not bound | pass it | LR 5.8 |

## Correction patterns

### E-NAM-002: assignment to an immutable binding (LR 5.1)

```fortis
// reject: E-NAM-002
pure func next(n: Int<0..9>) Int {
  let x = n
  x = n + 1                 // reject: E-NAM-002
  return x
}

test "next" {
  expect next(1) == 2
}
```

```fortis
// accept
pure func next(n: Int<0..9>) Int {
  let mut x: Int<0..10> = n
  x = n + 1
  return x
}

test "next" {
  expect next(1) == 2
}
```

### E-TYP-010: non-`Bool` condition (LR 5.2)

```fortis
// reject: E-TYP-010
pure func describe(n: Int) StringView from static {
  if n {                    // reject: E-TYP-010
    return "non-zero"
  }
  return "zero"
}

test "describe" {
  expect describe(0) == "zero"
}
```

```fortis
// accept
pure func describe(n: Int) StringView from static {
  if n != 0 {
    return "non-zero"
  }
  return "zero"
}

test "describe" {
  expect describe(0) == "zero"
  expect describe(3) == "non-zero"
}
```

### E-TYP-110: a path without `return` (LR 5.4)

The record is reported at the function head.

```fortis
// reject: E-TYP-110
pure func sign(n: Int) Int {        // reject: E-TYP-110
  if n < 0 {
    return -1
  }
}

test "sign" {
  expect sign(-4) == -1
}
```

```fortis
// accept
pure func sign(n: Int) Int {
  if n < 0 {
    return -1
  }
  if n == 0 {
    return 0
  }
  return 1
}

test "sign" {
  expect sign(-4) == -1
  expect sign(0) == 0
  expect sign(9) == 1
}
```

### E-TYP-120: `give` outside an `if` expression or arm (LR 5.4)

```fortis
// reject: E-TYP-120
pure func pick(flag: Bool) Int {
  if flag {
    give 1                  // reject: E-TYP-120
  }
  return 0
}

test "pick" {
  expect pick(false) == 0
}
```

```fortis
// accept
pure func pick(flag: Bool) Int {
  return if flag { give 1 } else { give 0 }
}

test "pick" {
  expect pick(true) == 1
  expect pick(false) == 0
}
```

### E-TYP-140: unused value (LR 5.5)

```fortis
// reject: E-TYP-140
pure func double(n: Int<0..1000>) Int => n * 2

test "double" {
  double(4)                 // reject: E-TYP-140
  expect double(2) == 4
}
```

```fortis
// accept
pure func double(n: Int<0..1000>) Int => n * 2

test "double" {
  discard double(4)
  expect double(2) == 4
}
```

### E-SYN-004: mixed operator families (LR 5.6)

```fortis
// reject: E-SYN-004
pure func choose(a: Bool, b: Bool?, c: Bool) Bool {
  return a || b ?? c        // reject: E-SYN-004
}

test "choose" {
  expect choose(false, null, true)
}
```

```fortis
// accept
pure func choose(a: Bool, b: Bool?, c: Bool) Bool {
  return a || (b ?? c)
}

test "choose" {
  expect choose(false, null, true)
  expect !choose(false, false, true)
}
```

### E-OWN-012: a reservation broken by a later argument (LR 5.7)

```fortis
// reject: E-OWN-012
class Buffer {
  export bytes: Int
}

func update(target: editable Buffer, size: Int) {
  target.bytes = size
}

func consume(b: owned Buffer) Int => b.bytes

test "update" {
  let mut buffer = Buffer(bytes: 0)
  update(editable buffer, consume(move buffer))   // reject: E-OWN-012
  expect buffer.bytes == 0
}
```

```fortis
// accept
class Buffer {
  export bytes: Int
}

func update(target: editable Buffer, size: Int) {
  target.bytes = size
}

func size(b: Buffer) Int => 7

test "update" {
  let mut buffer = Buffer(bytes: 0)
  let n = size(buffer)
  update(editable buffer, n)
  expect buffer.bytes == 7
}
```

### E-TYP-130 and E-TYP-050: argument binding (LR 5.8)

```fortis
// reject: E-TYP-130
pure func area(width: Int<0..100>, height: Int<0..100>) Int => width * height

test "area" {
  expect area(width: 2, depth: 3) == 6    // reject: E-TYP-130
}
```

```fortis
// reject: E-TYP-050
pure func area(width: Int<0..100>, height: Int<0..100>) Int => width * height

test "area" {
  expect area(width: 2) == 6              // reject: E-TYP-050
}
```

```fortis
// accept
pure func area(width: Int<0..100>, height: Int<0..100>) Int => width * height

test "area" {
  expect area(2, height: 3) == 6
  expect area(height: 3, width: 2) == 6
}
```

### `@ignorable` instead of `discard` (LR 5.5)

A function whose result most callers do not need is declared `@ignorable` once, and its call statements drop the
result without `discard` (LR 5.5, X-345). A linear result can never be ignorable (`E-TYP-150`).

```fortis
// accept
@ignorable
func remember(seen: editable Set[Int], n: Int) Bool {
  return seen.add(n)
}

test "remember" {
  let mut seen: Set[Int] = Set.default()
  remember(editable seen, 4)
  expect seen.count() == 1
}
```

## Facts used when writing code

- Literal booleans, `if` expressions and `match` give values; `if` without `else` is a statement only (LR 5.2).
- Ranges `a..b` (inclusive) and `a..<b` (exclusive) exist only in `for` headers and refinement types (LR 5.6).
- `for x in p` takes a read loan of `p` for the loop and binds read loans of the elements; `for x in move p`
  consumes `p` and binds owned elements (LR 5.3).
- Labels: `outer: for …`, then `break outer` / `continue outer`; `break` carries no value (LR 5.3).
- `return`/`fail` inside a lambda refer to the lambda; `give`/`recover` never cross a function or lambda
  boundary (LR 5.4).
- `await f() or fail` is `(await f()) or fail`; `copy await f()` is a parse error: markers take places (LR 5.6).
- Defaults are pure, non-allocating constant expressions re-evaluated at each call (LR 5.8). No user variadics:
  take `ListView[T]` and pass `List.of(…)` (LR 5.8).
- Temporaries drop at the end of the full statement in reverse creation order (LR 5.7).
