# Ownership (LR 7)

<!-- covers: LR 7 -->
*Agent mode. Human mode: [Ownership](../guide/07-ownership.md). Source of truth: LR 7.*

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-07-01 | Assigning, returning or capturing an existing ownership-bearing place carries `copy`, `alias` or `move`; `owned` arguments carry `move`/`copy`, `editable` arguments carry `editable`. | LR 7.5 |
| LANG-07-02 | Parameter bindings are never reassignable. | LR 7.6 |
| LANG-07-03 | The checker never inserts clones, reference counts or hidden sharing to repair a rejected program. | LR 7.10 |
| LANG-07-04 | Use after move, conflicting loans, owner use while loaned and escaping loans are rejected statically, including through closures, `defer`, `await` and `scope`. | LR 7.3, 7.4 |
| LANG-07-05 | Loans are lexical; the aliasing invariant (one exclusive loan, or any number of read loans, never both) holds at every point. | LR 7.3 |
| LANG-07-06 | A returned loan names one origin: a function needing loans from two parameters in its result is `E-OWN-050` (a loan type written with two origins is not a sentence of the grammar, `E-SYN-000`, LR C.2), and a loan of a local or temporary returned is `E-OWN-051`; lifetime parameters and loan-carrying fields (other than a loan `from static`) are `E-OWN-053` (X-137). | LR 7.6, 7.7, C.2 |
| LANG-07-07 | Destruction follows LR 7.9 on every backend. | LR 7.9 |
| LANG-07-08 | Shared ownership is explicit (`retain`); `copy` of `Shared` is rejected. | LR 7.8 |
| LANG-07-09 | A shape with no safe-Rust lowering is rejected with `E-OWN-090`, never lowered with `unsafe` or erasure. | LR 7.10 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-OWN-001 | an existing ownership-bearing place assigned, returned or initialised into a field without a marker | `copy p` (independent duplicate), `alias p` (read loan), or `move p` (transfer) | LR 7.5 |
| E-OWN-002 | `copy` of a non-copyable place (`Shared`, `Weak`, …) | `s.retain()` for `Shared`/`Weak`; otherwise `move` or a loan | LR 7.5 |
| E-OWN-010 | a loan conflicting with an active loan (exclusive while read loans live, two exclusive) | end the earlier loan: put its binding in an inner block, or loan once | LR 7.3 |
| E-OWN-011 | the owner (or a suspended parent loan) used while loaned | use the owner after the loan's block ends, or go through the loan | LR 7.3 |
| E-OWN-012 | a later argument breaks an earlier argument's reservation | evaluate the later argument into a binding first | LR 5.7 |
| E-OWN-020 | a place used after `move` | `copy` before the move, re-initialise a `let mut`, or restructure | LR 7.4 |
| E-OWN-040 | a partial move (`move x.field` while `x` still owns the rest) | move the whole value, or consume it with a class pattern `let C(field: f, other: _) = move x` | LR 7.2 |
| E-OWN-050 | a return keeps a loan of a parameter other than the declared `from` origin | return an owned value, or split the function | LR 7.6 |
| E-OWN-051 | a loan of a local or a temporary returned or kept beyond it | bind the owner first (`let u = make()` then `f(alias u)`), or return an owned value | LR 7.6 |
| E-OWN-052 | a deferred block uses a binding of its block that holds a loan | use the owner, or move the use out of the `defer` | LR 12.1 |
| E-OWN-053 | a lifetime parameter, or a field of a loan or loan-carrying type (other than `from static`) | own the value (`String`, not `StringView`), hold a `Shared`, or pass it per call | LR 7.7 |
| E-OWN-090 | an ownership shape with no lowering (a loan stored in `Shared`/`Mutex`, a `fn mut` escaping into `Shared`) | own the value; `Mutex` around state with an `fn` closure | LR 7.10, C.2 |

## Facts used when writing code

- Places: a binding, a field path from a binding or `self`, an `Array` element at a literal index. Call results,
  `a ?? b`, literals and `list[i]` with a variable index are values, not places (LR 7.2).
- Markers by position (LR 7.5): `let x = p`, `x = p`, `name: p` field initialiser and `return p` need a marker;
  plain parameter arguments and `self` receivers are implicit read loans; `editable self` receivers need
  `editable p.m()`, `owned self` receivers `move p.m()`; `editable` parameters `editable p`; `owned` parameters
  `move p`/`copy p`; an argument the result borrows `from` needs `alias p` (or `editable p` for an `editable`
  parameter); `fn mut` / `fn once` calls are `editable f()` / `move f()`; comparison and interpolation operands,
  `match p` and `for x in p` are implicit read loans; `match move p` / `for x in move p` consume. Fresh and
  trivial values never need a marker.
- Loan lifetimes (LR 7.3): binding-held loans last to the end of the declaring block; call-argument loans end at
  return unless the result carries them; comparison/interpolation/`capture` loans end with the statement;
  `for`-header loans last for the loop; `async`-call argument loans are held by the future; `from` results are
  held by the receiving binding; no non-lexical shortening.
- Child loans (LR 7.3): a read child of an exclusive parent freezes it; an exclusive child suspends it; a parent
  cannot be moved, reassigned or dropped while a child lives; a loan-holding binding is never reassignable; at a
  join a loan is live if live on any path.
- Receivers (LR 7.6): `self` read loan, `editable self` exclusive loan, `owned self` consumes. `from` names
  exactly one plain or `editable` parameter or `self`, never `owned`. The kept loan has the declared mode
  (`StringView from user` read; `editable Cell from c` exclusive, needs `c: editable`).
- Returns (LR 7.6): `return move local`; `return copy obj.field` or a `from` signature for a borrowed field; fresh
  expressions unmarked.
- Loan-carrying types (LR 7.7): `StringView`, `BytesView`, `ListView[T]`, standard cursors. Usable as a local's
  type, a parameter type, or a `from` return type; owned by their holder; may not cross `launchParallel` or
  `launchBlocking`.
- `Shared[T]` (LR 7.8): `Shared.create(move v)`, `s.retain()`, `s.downgrade()` → `Weak[T]`, `w.upgrade()` →
  `Shared[T]?` (`nondet`), `w.retain()`. Reads through `s.field`/`s.method()` are read loans; mutation needs
  `Mutex[T]` or an atomic; cycles leak; neither `Shared` nor `Weak` is linear. A `Shared[A]` of an authority
  carries `A` (LR 11.6).
- Destruction order (LR 7.9): block exit = loans end, `defer`s last-first, locals in reverse initialisation;
  assignment = evaluate new, drop old, store; temporaries at statement end in reverse creation; a class value =
  its `cleanup`, then fields in declaration order.

## Correction patterns

### E-OWN-001: missing marker (LR 7.5)

```fortis
// reject: E-OWN-001
class User {
  export name: String
}

pure func rename(user: User) User {
  let other = user                    // reject: E-OWN-001
  return move other
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
  return move other
}

test "rename" {
  expect rename(User(name: "a")).name == "a"
}
```

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
  export func issue(editable self) Void {
    self.customer = 0
  }
}

test "issue" {
  let mut invoice = Invoice(customer: 7, lines: List.of("tea"))
  let lines = alias invoice.lines
  editable invoice.issue()            // reject: E-OWN-010
  expect invoice.customer == 0
}
```

```fortis
// accept
class Invoice {
  export customer: Int
  export lines: List[String]
  export func issue(editable self) Void {
    self.customer = 0
  }
}

test "issue" {
  let mut invoice = Invoice(customer: 7, lines: List.of("tea"))
  {
    let lines = alias invoice.lines
    expect lines.count() == 1
  }
  editable invoice.issue()
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

### E-OWN-053: a loan in a field (LR 7.7)

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

### E-OWN-090: a loan stored in `Shared` (LR 7.10, LR C.2)

```fortis
// reject: E-OWN-090
class User {
  export name: String
}

pure func displayName(user: User) StringView from user => alias user.name

func run(user: User) Int {
  let seen = displayName(alias user)
  let shared = Shared.create(move seen)    // reject: E-OWN-090
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
