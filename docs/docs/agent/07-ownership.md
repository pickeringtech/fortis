# Ownership (LR 7)

<!-- covers: LR 7 -->
*Agent mode. Human mode: [Ownership](../guide/07-ownership.md). Source of truth: LR 7.*

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-07-01 | Assigning, returning or capturing an existing ownership-bearing place carries `copy`, `alias` or `move`, except that `return local` moves a local `let` binding; `owned` arguments carry `move`/`copy`, `editable` arguments carry `editable`; an unmarked `editable self` receiver or `fn mut` call is checked exactly as the marked one, and is rejected where the marker could not be written. | LR 7.5 |
| LANG-07-02 | Parameter bindings are never reassignable. | LR 7.6 |
| LANG-07-03 | The checker never inserts clones, reference counts or hidden sharing to repair a rejected program. | LR 7.10 |
| LANG-07-04 | Use after move, conflicting loans, owner use while loaned and escaping loans are rejected statically, including through closures, `defer`, `await` and `scope`. | LR 7.3, 7.4 |
| LANG-07-05 | Loans are lexical; the aliasing invariant (one exclusive loan, or any number of read loans, never both) holds at every point. | LR 7.3 |
| LANG-07-06 | A returned loan names one origin: a function needing loans from two parameters in its result is `E-OWN-050` (a loan type written with two origins is not a sentence of the grammar, `E-SYN-000`, LR C.2), and a loan of a local or temporary returned is `E-OWN-051`; lifetime parameters, loan-carrying fields and storage type arguments (other than a loan `from static`) are `E-OWN-053` (X-137). | LR 7.6, 7.7, C.2 |
| LANG-07-07 | Destruction follows LR 7.9 on every backend. | LR 7.9 |
| LANG-07-08 | Shared ownership is explicit (`retain`); `copy` of `Shared` is rejected. | LR 7.8 |
| LANG-07-09 | A shape with no safe-Rust lowering is rejected with `E-OWN-090`, never lowered with `unsafe` or erasure. | LR 7.10 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
 | E-OWN-001 | an existing ownership-bearing place assigned, returned (other than a local) or initialised into a field without a marker; a loan of a copyable value (`xs.get(0)`, a binding holding one) where an owned value is expected; an `editable self` call or `fn mut` call, marked or not, on a root that cannot change (a `let` binding, a read parameter, capture or loan, the `self` of a read method); a marker written on a call of a function, which is not a place (`alias first(alias xs)`; D-382) | `copy p` (independent duplicate), `alias p` (read loan), or `move p` (transfer); for a loan, `let v = xs.get(0)` then `copy v`; for a receiver, declare the binding `let mut` or the parameter `editable`; for a marker on a call, write the call plain and put the markers on its arguments | LR 7.5, LR 7.6 |
| E-OWN-002 | `copy` of a non-copyable place (`Shared`, `Weak`, …); a loan of a value that is not Copyable where an owned value is expected | `s.retain()` for `Shared`/`Weak`; otherwise `move` or a loan | LR 7.5, LR 7.6 |
| E-OWN-010 | a loan conflicting with an active loan (exclusive while read loans live, two exclusive) | end the earlier loan: put its binding in an inner block, or loan once | LR 7.3 |
| E-OWN-011 | the owner (or a suspended parent loan) used while loaned | use the owner after the loan's block ends, or go through the loan | LR 7.3 |
| E-OWN-012 | a later argument breaks an earlier argument's reservation | evaluate the later argument into a binding first | LR 5.7 |
| E-OWN-020 | a place used after `move` | `copy` before the move, re-initialise a `let mut`, or restructure | LR 7.4 |
| E-OWN-040 | a partial move (`move x.field` while `x` still owns the rest) | move the whole value, or consume it with a class pattern `let C(field: f, other: _) = move x` | LR 7.2 |
| E-OWN-050 | a return keeps a loan of a parameter other than the declared `from` origin | return an owned value, or split the function | LR 7.6 |
| E-OWN-051 | a loan of a local or a temporary returned or kept beyond it; a value of the function's own (a `move`, a copy, a fresh value, a tuple or record literal, a constructor, an arm or branch giving one) where the result is declared `from` a parameter | bind the owner first (`let u = make()` then `f(alias u)`), or return an owned value | LR 7.6 |
| E-OWN-052 | a deferred block uses a binding of its block that holds a loan | use the owner, or move the use out of the `defer` | LR 12.1 |
| E-OWN-053 | a lifetime parameter, a stored non-static loan, or a claim of `from static` without a static source | own the value (`String`, not `StringView`), hold a `Shared`, or pass it per call; a static origin must be proved | LR 7.7 |
| E-OWN-090 | an ownership shape with no lowering (a `fn mut` escaping into `Shared`) | own the value; `Mutex` around state with an `fn` closure | LR 7.10, C.2 |

## Facts used when writing code

- Places: a binding, a field path from a binding or `self`, an `Array` element at a literal index. Call results,
  `a ?? b`, literals and `list[i]` with a variable index are values, not places (LR 7.2).
- Markers by position (LR 7.5): `let x = p`, `x = p`, `name: p` field initialiser and `return p` need a marker,
  except `return local`, which moves a local `let`/`let mut` binding; plain parameter arguments and `self`
  receivers are implicit read loans; `owned self` receivers `move p.m()`; `editable` parameters `editable p`;
  `owned` parameters `move p`/`copy p`; an argument the result borrows `from` needs `alias p` (or `editable p` for
  an `editable` parameter); `fn once` calls `move f()`; comparison and interpolation operands, `match p` and
  `for x in p` are implicit read loans; `match move p` / `for x in move p` consume. Fresh and trivial values never
  need a marker.
- What the declaration shows (LR 7.5): an `editable self` receiver and a `fn mut` call take an exclusive loan with
  no marker, `readings.push(40)`, `bump()`, when the root is a `let mut` binding, an `editable` parameter, an
  `editable` or `move` capture, a binding holding an exclusive loan, or the method's own `editable self`
  (`self.order.push(w)`). The unmarked call is checked as the marked one (the same loans, `E-OWN-010`,
  `E-OWN-020`, and `E-REF-003` on a length-refined collection, LR 6.3), on a trivial receiver too. On any other root it is `E-OWN-001`, marked or not.
  Arguments keep their markers: `normalise(editable readings)`. Older code writes `editable readings.push(40)`,
  `editable bump()` and `return move out`; they stay accepted and `fortis fmt` removes the marker.
- Loan lifetimes (LR 7.3): binding-held loans last to the end of the declaring block; call-argument loans end at
  return unless the result carries them; comparison/interpolation/`capture` loans end with the statement;
  `for`-header loans last for the loop; `async`-call argument loans are held by the future; `from` results are
  held by the receiving binding, a loan inside an optional, tuple or record result too (`let t = xs.get(0)`: until
  `t`'s block ends, `xs.pop()` is `E-OWN-010` and `move xs` is `E-OWN-011`; a `List[Int]` element holds nothing);
  no non-lexical shortening.
- Child loans (LR 7.3): a read child of an exclusive parent freezes it; an exclusive child suspends it; a parent
  cannot be moved, reassigned or dropped while a child lives; a loan-holding binding (a parameter, `alias p`,
  `editable p`, a `from` result that is a loan) is never reassignable, but a `let mut` holding a loan inside an
  optional, tuple or record is (`t = xs.get(1)`, `t = null`: the new value's loans replace the old), and so is a
  `let mut` holding a standard cursor (`c = alias ys.iterate()`: the binding owns the cursor, LR 7.7; the old
  cursor is destroyed, its stored loan ends, and the binding holds the new cursor's loan to the end of its own
  block; D-401); at a join a loan is live if live on any path.
- Receivers (LR 7.6): `self` read loan, `editable self` exclusive loan, `owned self` consumes. `from` names
  exactly one plain or `editable` parameter or `self`, never `owned`. The kept loan has the declared mode
  (`StringView from user` read; `editable Cell from c` exclusive, needs `c: editable`).
- Returns (LR 7.6): `return local` (a move); `return move param` for an `owned` parameter; `return copy obj.field`
  or a `from` signature for a borrowed field; fresh expressions unmarked.
- Loan-carrying types (LR 7.7): `StringView`, `BytesView`, `ListView[T]`, standard cursors. Usable as a local's
  type, a parameter type, or a `from` return type; owned by their holder; may not cross `launchParallel` or
  `launchBlocking`.
- `Shared[T]` (LR 7.8): `Shared.create(move v)`, `s.retain()`, `s.downgrade()` → `Weak[T]`, `w.upgrade()` →
  `Shared[T]?` (`nondet`), `w.retain()`. Reads through `s.field`/`s.method()` are read loans; mutation needs
  `Mutex[T]` or an atomic; cycles leak; neither `Shared` nor `Weak` is linear. A `Shared[A]` of an authority
  carries `A` (LR 11.6).
- Destruction order (LR 7.9): block exit = loans end, `defer`s last-first, locals in reverse initialisation;
  assignment = evaluate new, drop old, store; temporaries at statement end in reverse creation; a class value =
  its `cleanup`, then fields in declaration order. Only an owner destroys: a loan's end (`xs.get(0)`, `alias p`, a
  `from` result, a parameter that is not `owned`, a borrowed pattern or loop binding) runs no `cleanup`, which
  runs once per value, at its owner's destruction.

## Correction patterns

### E-OWN-001: missing marker (LR 7.5)

```fortis
// reject: E-OWN-001
class User {
  export name: String
}

pure func rename(user: User) User {
  let other = user                    // reject: E-OWN-001
  return other
}

test "rename" {
  expect rename(User(name: "a")).name == "a"
}
```

```fortis
// accept
class User {
  export name: String
}

pure func rename(user: User) User {
  let other = copy user
  return other
}

test "rename" {
  expect rename(User(name: "a")).name == "a"
}
```

### E-OWN-001: a marker written on a call of a function (LR 5.6)

A marker takes a place, optionally followed by a call on it; a function's name is not a place, so a marker on a
call of a function marks nothing (D-382). A fresh result needs no marker (7.5), and a `from` result keeps the loan
through the marker on its argument (7.6).

```fortis
// reject: E-OWN-001
class User {
  export name: String
}

pure func fresh(name: StringView) User {
  return User(name: name as String)
}

pure func renamed(user: User) User {
  let other = move fresh("b")         // reject: E-OWN-001
  return other
}

test "renamed" {
  expect renamed(User(name: "a")).name == "b"
}
```

```fortis
// accept
class User {
  export name: String
}

pure func fresh(name: StringView) User {
  return User(name: name as String)
}

pure func renamed(user: User) User {
  let other = fresh("b")
  return other
}

test "renamed" {
  expect renamed(User(name: "a")).name == "b"
}
```

### E-OWN-001: an `editable self` call on a binding that cannot change (LR 7.5)

The receiver of an `editable self` method needs no marker on a `let mut` binding; on a `let` binding the call is
rejected, marked or not.

```fortis
// reject: E-OWN-001
func readings() List[Int] {
  let out: List[Int] = List.default()
  out.push(40)                        // reject: E-OWN-001
  return out
}

test "readings" {
  expect readings().count() == 1
}
```

```fortis
// accept
func readings() List[Int] {
  let mut out: List[Int] = List.default()
  out.push(40)
  return out
}

test "readings" {
  expect readings().count() == 1
}
```

### E-OWN-001: a loan where an owned value is expected (LR 7.6)

`xs.get(0)` lends the list's element. A loan stays a loan, so returning it as an owned `T?`, binding it to an owned
type, or passing it `owned` needs `copy`, which copies the value the loan lends.

```fortis
// reject: E-OWN-001
func first(names: List[String]) String? {
  return names.get(0)                 // reject: E-OWN-001
}

test "first" {
  let got = first(List.of("a")) ?? "none"
  expect got == "a"
}
```

```fortis
// accept
func first(names: List[String]) String? {
  let name = names.get(0)
  return copy name
}

test "first" {
  let got = first(List.of("a")) ?? "none"
  expect got == "a"
}
```

A value that is not Copyable (a `Shared`, a class holding a capability) has no owned copy: the same return is
`E-OWN-002`. Keep the loan, or take the value out of its owner.

### E-OWN-020: use after move (LR 7.4)

```fortis
// reject: E-OWN-020
func consume(items: owned List[Int]) Int => items.count()

test "consume" {
  let items = List.of(1, 2, 3)
  expect consume(move items) == 3
  expect items.count() == 3           // reject: E-OWN-020
}
```

```fortis
// accept
func consume(items: owned List[Int]) Int => items.count()

test "consume" {
  let items = List.of(1, 2, 3)
  expect consume(copy items) == 3
  expect items.count() == 3
}
```

### E-OWN-010: conflicting loans (LR 7.3)

```fortis
// reject: E-OWN-010
class Invoice {
  export customer: Int
  export lines: List[String]
  export func issue(editable self) {
    self.customer = 0
  }
}

test "issue" {
  let mut invoice = Invoice(customer: 7, lines: List.of("tea"))
  let lines = alias invoice.lines
  invoice.issue()                     // reject: E-OWN-010
  expect invoice.customer == 0
}
```

```fortis
// accept
class Invoice {
  export customer: Int
  export lines: List[String]
  export func issue(editable self) {
    self.customer = 0
  }
}

test "issue" {
  let mut invoice = Invoice(customer: 7, lines: List.of("tea"))
  {
    let lines = alias invoice.lines
    expect lines.count() == 1
  }
  invoice.issue()
  expect invoice.customer == 0
}
```

### E-OWN-011: owner used while loaned (LR 7.3)

```fortis
// reject: E-OWN-011
class Cell {
  export value: Int
}

func set(c: editable Cell) Int {
  let r = editable c
  r.value = 1
  return c.value                      // reject: E-OWN-011
}

test "set" {
  let mut cell = Cell(value: 0)
  expect set(editable cell) == 1
}
```

```fortis
// accept
class Cell {
  export value: Int
}

func set(c: editable Cell) Int {
  {
    let r = editable c
    r.value = 1
  }
  return c.value
}

test "set" {
  let mut cell = Cell(value: 0)
  expect set(editable cell) == 1
}
```

### E-OWN-040: partial move (LR 7.2)

```fortis
// reject: E-OWN-040
class Order {
  export id: Int
  export lines: List[String]
}

func keep(lines: owned List[String]) Int => lines.count()

test "keep" {
  let order = Order(id: 1, lines: List.of("tea"))
  expect keep(move order.lines) == 1   // reject: E-OWN-040
}
```

```fortis
// accept
class Order {
  export id: Int
  export lines: List[String]
}

func keep(lines: owned List[String]) Int => lines.count()

test "keep" {
  let order = Order(id: 1, lines: List.of("tea"))
  let Order(id: _, lines: lines) = move order
  expect keep(move lines) == 1
}
```

### E-OWN-051: a loan of a temporary (LR 7.6)

```fortis
// reject: E-OWN-051
class User {
  export name: String
  export pure func guest() User => User(name: "guest")
}

pure func displayName(user: User) StringView from user => alias user.name

test "guest" {
  let name = displayName(User.guest())     // reject: E-OWN-051
  expect name == "guest"
}
```

```fortis
// accept
class User {
  export name: String
  export pure func guest() User => User(name: "guest")
}

pure func displayName(user: User) StringView from user => alias user.name

test "guest" {
  let user = User.guest()
  let name = displayName(alias user)
  expect name == "guest"
}
```

The same correction applies when a receiver may own a temporary on only one branch (D-562, LR 7.9).
`let part = (xs.get(0) ?? (" temp " as String)).trim()` is rejected: the fallback dies with the statement.
Bind `let fallback = " temp " as String` first, then use `(xs.get(0) ?? alias fallback).trim()`. A static
fallback such as `""` is permitted, and a temporary view may be read within the statement that creates it.
The check also follows `if` and `match` arms, recovery, conversions, calls and interpolation.

A value of the function's own is the same case where the result is declared `from` a parameter: `return
xs.get(0) ?? move d`, `return move d` or a fresh value would leave the caller holding a loan of what ended with
the call (D-373). A `from` result is a loan, so what produces it must lend: a place read through a loan, `alias p`,
a call whose result lends, a `??` both of whose sides lend, an `if` or `match` every one of whose results lends,
or `present` of one. A tuple or record literal (`return (xs.get(0) ?? move d, 1)`), a constructor, a lambda, a
call whose result is owned, and an arm or branch that gives any of these are the function's own too (D-377,
D-381). Bind the loan and return an owned value from a function whose result is owned, or lend on every path.

```fortis
// reject: E-OWN-051
class Token {
  export label: String
}

func pick(xs: List[Token], d: owned Token) alias Token from xs {
  return xs.get(0) ?? move d                // reject: E-OWN-051
}
```

```fortis
// accept
class Token {
  export label: String
}

func pick(xs: List[Token], d: owned Token) Token {
  let v = xs.get(0)
  return copy v ?? move d
}

test "pick" {
  let xs: List[Token] = List.default()
  let t = pick(xs, Token(label: "fallback"))
  expect t.label == "fallback"
}
```

### E-OWN-050: a second origin (LR 7.6)

```fortis
// reject: E-OWN-050
class User {
  export name: String
}

pure func longer(a: User, b: User) StringView from a {
  if a.name.scalarCount() > b.name.scalarCount() {
    return alias a.name
  }
  return alias b.name                      // reject: E-OWN-050
}

test "longer" {
  let a = User(name: "ada")
  let b = User(name: "brian")
  expect longer(alias a, b) == "brian"
}
```

```fortis
// accept
class User {
  export name: String
}

pure func longer(a: User, b: User) String {
  if a.name.scalarCount() > b.name.scalarCount() {
    return copy a.name
  }
  return copy b.name
}

test "longer" {
  let a = User(name: "ada")
  let b = User(name: "brian")
  expect longer(a, b) == "brian"
}
```

### E-OWN-053: a stored loan (LR 7.7)

A collection stores its elements, even when it is local. `List[StringView]`, `Shared[StringView]` and a
collection of optional views are rejected too. Use `List[String]`, converting a view with `as String` before
inserting it. A local optional view or a local tuple holding a loan remains allowed; storing that wrapper does
not hide its loan. Only an explicit `from static` loan type is exempt.

```fortis
// reject: E-OWN-053
class Line {                          // reject: E-OWN-053
  export number: Int
  export text: StringView
}

test "line" {
  expect true
}
```

```fortis
// accept
class Line {
  export number: Int
  export text: String
}

test "line" {
  let line = Line(number: 1, text: "first")
  expect line.text == "first"
}
```

### E-OWN-002: copy of a `Shared` (LR 7.5, LR 7.8)

```fortis
// reject: E-OWN-002
class Settings {
  export name: String
}

test "share" {
  let first = Shared.create(Settings(name: "prod"))
  let second = copy first             // reject: E-OWN-002
  expect second.name == "prod"
}
```

```fortis
// accept
class Settings {
  export name: String
}

test "share" {
  let first = Shared.create(Settings(name: "prod"))
  let second = first.retain()
  expect second.name == "prod"
}
```

### E-OWN-053: a loan stored in `Shared` (LR 7.10, LR C.2)

```fortis
// reject: E-OWN-053
class User {
  export name: String
}

pure func displayName(user: User) StringView from user => alias user.name

func run(user: User) Int {
  let seen = displayName(alias user)
  let shared = Shared.create(move seen)    // reject: E-OWN-053
  return shared.scalarCount()
}

test "run" {
  expect run(User(name: "ada")) == 3
}
```

```fortis
// accept
class User {
  export name: String
}

func run(user: User) Int {
  let shared = Shared.create(copy user.name)
  return shared.scalarCount()
}

test "run" {
  expect run(User(name: "ada")) == 3
}
```

## Collections store values (LR 7.7)

An owned collection cannot store a non-static loan, even when the collection itself is local
(`E-OWN-053`). The rule covers elements, keys and values, optional and aggregate contents, outcomes,
newtype representations and storing generic arguments. `List[T]` is valid generic code only when every
instantiation meets that rule. An explicit `StringView from static` is exempt, but the source must prove
that origin; an annotation cannot turn a local view into a static one. A plain `StringView` is not the
exception even if its initial value is a literal (LR 7.7).

```fortis
// reject: E-OWN-053
func main() {
  let text: String = " tea "
  let pieces = List.of(text.trim())          // reject: E-OWN-053
}
```

```fortis
// accept
func size() Int {
  let text: String = " tea "
  let pieces = List.of(text.trim() as String)
  return pieces.count()
}
test "store owned text" {
  expect size() == 1
}
```

A view of an owned temporary cannot survive its statement (`E-OWN-051`, LR 7.6, LR 7.9).
This includes a receiver that owns a value on only one branch of `??`, `if`, `match` or recovery.
Bind the owner first, then take the view; a view used only within the statement is allowed.

```fortis
// reject: E-OWN-051
func main() {
  let trimmed = (" tea " as String).trim()   // reject: E-OWN-051
}
```

```fortis
// accept
func size() Int {
  let text: String = " tea "
  let trimmed = text.trim()
  return trimmed.scalarCount()
}
test "the owner outlives its view" {
  expect size() == 3
}
```

A scalar result carries no loan from its operands (LR 7.6; X-714). Returning `a.trim().scalarCount()` gives an
`Int` without a loan of `a`, and comparing trimmed views gives a `Bool` without one. Returning `a.trim()` still
lends from `a` and needs the declared origin. Other operands of an enclosing expression keep their own loans.
