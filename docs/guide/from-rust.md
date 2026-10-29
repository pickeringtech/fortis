# Notes for readers coming from Rust

<!-- covers: LR 1 -->
*Human mode. Source of truth: the LR; each note cites the section that states the rule.*

> Preview: interfaces, mixins and generics (LR 6.10, 9.5, 9.6) arrive in v0.1, effects and claims as a whole (LR 11.1 to 11.4, 11.6) are completed in v0.2, and async with scopes (LR 12.3, 12.5, 12.7) arrives in v0.3; the examples under "Traits are interfaces" and "Async is structured" are checked and run today. The rest of these notes is in v0.0.2.

Fortis will feel familiar to a Rust programmer: values have one owner, moves are real, shared mutation is not
allowed, failures are values, and the native backend emits safe Rust (LR 1). The differences come from one
design choice: everything a reader needs to know about a line is written on the line, and nothing is inferred
from somewhere the reader cannot see (LR 1, LR 1.1). This page goes through what carries over and what does not.

## Borrows are written at the call

In Rust the borrow is written at the call (`&x`, `&mut x`) and a move is silent. In Fortis a read loan is the
silent case, and every other use of an ownership-bearing place carries a marker: `editable` for an exclusive
loan, `move` to transfer ownership, `copy` for an independent duplicate (LR 7.5, LR 7.6).

| Rust | Fortis parameter | Fortis call |
|---|---|---|
| `fn f(a: &Account)` | `func f(a: Account)` | `f(account)` |
| `fn f(a: &mut Account)` | `func f(a: editable Account)` | `f(editable account)` |
| `fn f(a: Account)` | `func f(a: owned Account)` | `f(move account)` or `f(copy account)` |
| `account.clone()` | | `copy account` |

```fortis
// accept
import fortis.host { Console }

class Account {
  export owner: String
  export balance: Int
}

// A plain parameter is a read loan for the call, like `&Account`.
func describe(a: Account) String => "${a.owner}: ${a.balance}"

// An `editable` parameter is an exclusive loan, like `&mut Account`.
func deposit(a: editable Account, amount: Int<0..1000>) {
  a.balance = Int.saturatingAdd(a.balance, amount)
}

// An `owned` parameter takes ownership, like passing `Account` by value.
func close(a: owned Account) Int => a.balance

func main(console: Console) {
  let mut account = Account(owner: "ada", balance: 10)
  console.print(describe(account))
  deposit(editable account, 5)
  let backup = copy account
  console.print("closed with ${close(move account)}")
  console.print(describe(backup))
}
```

```output
ada: 10
closed with 15
ada: 15
```

The marker is also required where Rust would move silently: `let other = account` on an ownership-bearing value
is `E-OWN-001`, because a reader could not tell a move from a copy (LR 7.5). A method with `owned self` is called
as `move account.close()`. A method with `editable self` is called as `account.deposit(5)`, like Rust's `&mut self`
auto-borrow, and only on a place that may change: a `let mut` binding (Rust's `let mut`), an `editable` parameter
or an exclusive loan (LR 7.5, LR 7.6).

```fortis
// reject: E-OWN-001
class Account {
  export owner: String
}

func main() {
  let account = Account(owner: "ada")
  let other = account          // reject: E-OWN-001
}
```

`copy` is Rust's `Clone` without the escape hatches: it is compiler-generated, field by field, cannot fail and
never shares mutable state, and it is rejected for types that are not copyable, such as `Shared` and `Mutex`
(LR 6.11, LR 7.5). The small set of trivial types (integers, `Bool`, `Decimal`, `Duration`, enums of trivial
payloads, `trivial class`es) are copied implicitly, like Rust's `Copy` types (LR 6.11).

## Loans are lexical

Rust's non-lexical lifetimes end a borrow at its last use. Fortis loans end at the end of the block that holds
them, and there is no shortening (LR 7.3). Code that Rust accepts can therefore be rejected:

```fortis
// reject: E-OWN-011
class Counter {
  export value: Int
}

test "a loan lasts until its block ends" {
  let mut counter = Counter(value: 0)
  let r = editable counter
  r.value = 1
  expect counter.value == 1    // reject: E-OWN-011
}
```

The fix is the one Fortis always uses to end a loan early: close a block (LR 7.3).

```fortis
// accept
class Counter {
  export value: Int
}

test "an inner block ends the loan" {
  let mut counter = Counter(value: 0)
  {
    let r = editable counter
    r.value = 1
  }
  expect counter.value == 1
}
```

The cost is an occasional extra pair of braces; the benefit is that a loan's extent is visible on the page rather
than computed by the checker. Calls get the same treatment Rust's two-phase borrows give them: arguments reserve
their loans first and activate them together (LR 5.7).

## No lifetime parameters, and no stored loans

A function that returns a loan names the parameter it borrows from with `from`, and the call site keeps the loan
visibly with `alias` (LR 7.6). That replaces Rust's lifetime parameters in signatures; a function borrowing from
two parameters is not expressible and is rejected, so it returns an owned value instead (LR 7.6).

```fortis
// accept
class User {
  export name: String
}

pure func nameOf(user: User) StringView from user {
  return alias user.name
}

test "a returned loan names its origin" {
  let user = User(name: "ada")
  let name = nameOf(alias user)
  expect name == "ada"
}
```

There is no Fortis equivalent of `struct Line<'a> { text: &'a str }`. In Language 1.0 a field never holds a loan
of an owner, and a class cannot declare a lifetime parameter (LR 7.7); the one loan a field may hold is a loan
`from static`, such as a string literal, which borrows from no owner (LR 7.7):

```fortis
// reject: E-OWN-053
export class Line {             // reject: E-OWN-053
  export number: Int
  export text: StringView
}
```

A value that must live as long as something else is owned, copied, held through an explicit `Shared`, or passed
to each call that needs it; a parser that would keep a view of its input keeps offsets into it instead (LR 7.7).
The standard views and cursors (`StringView`, `ListView`, `ListCursor`) are compiler-known types that carry a
loan, usable in locals, parameters and `from` returns (LR 7.7).

## `Result` is `Outcome`, and it cannot be ignored

A `try func f() T` returns an `Outcome[T]` (LR 10.1). Where Rust warns about an unused `Result`, Fortis rejects
it: an outcome is a linear value and must be consumed exactly once (`E-OBL-001`, LR 10.1, LR 6.12). The consuming
operations replace Rust's operators and combinators (LR 10.2):

| Rust | Fortis |
|---|---|
| `f()?` | `f() or fail` |
| `f().unwrap_or(v)` | `f() or recover v` |
| `match f() { Ok(x) => …, Err(e) => … }` | `match f() { success(x) => …, failure(e) => … }` or `f() catch e { … }` |
| `f().unwrap()` | none: there is no `unwrap` |

```fortis
// accept
export error Odd {
  code: "demo.odd"
  payload { value: Int }
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail Odd(value: n)
  }
  return n / 2
}

pure try func quarter(n: Int) Int {
  let h = half(n) or fail
  return half(h) or fail
}

test "outcomes" {
  expect quarter(12) == 3
  let fallback = quarter(6) catch e {
    expect e.code == "demo.odd"
    recover 0
  }
  expect fallback == 0
}
```

```fortis
// reject: E-OBL-001
export error Odd {
  code: "demo.odd"
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail Odd()
  }
  return n / 2
}

func main() {
  half(3)                      // reject: E-OBL-001
}
```

There are no panics in ordinary code. What Rust handles with a panic (an index out of bounds, `unwrap` on
`None`) is either proved impossible by the checker or written as an outcome; the only process-ending events are
the closed set of defects, such as stack exhaustion, which no `catch` can intercept (LR 10.8). A function that
silently discards a failure must say so with the `recovers` modifier, so a signature shows who swallowed one
(LR 8.1, LR 10.4).

## Integer arithmetic is proved, not wrapped or panicked

In Rust, `a + b` on `i64` panics on overflow in debug builds and wraps in release builds. In Fortis it is
accepted only when the checker can show it cannot overflow; otherwise it is rejected with `E-REF-002`, and you
say what happens on overflow: handle it, `a + b or fail` (Rust's `checked_add(b)?`), or use a named wrapping or
saturating operation (LR 6.4). Integer behaviour never depends on the build mode (LR 6.4).

```fortis
// reject: E-REF-002
pure func sum(a: Int, b: Int) Int => a + b      // reject: E-REF-002
```

```fortis
// accept
pure func sum(a: Int<0..1000>, b: Int<0..1000>) Int<0..2000> => a + b

pure try func checkedSum(a: Int, b: Int) Int => a + b or fail

test "arithmetic" {
  expect sum(1000, 1000) == 2000
  expect checkedSum(1, 2) == 3
  expect Int.wrappingAdd(9223372036854775807, 1) == -9223372036854775807 - 1
}
```

The refinement `Int<0..1000>` is part of the type; the checker tracks intervals through each function (LR 6.3).
The [proofs chapter](06-proofs.md) describes what it can establish on its own.

## Traits are interfaces

An `interface` is a trait without default method bodies and without associated types; a type implements one only
by saying so (`implements`, `derive` or an `implement … for` block), never by having methods of the right names
(LR 9.5). Default behaviour on top of an interface is a mixin (LR 9.6). Generic functions take `[T]` and bound
it with `where`, and are monomorphised, as in Rust (LR 6.10).

```fortis
// accept
interface Shape {
  pure func area(self) Int
}

class Square implements Shape {
  export side: Int<0..1000>
  export pure func area(self) Int => self.side * self.side
}

pure func largest[T](shapes: ListView[T]) Int where T implements Shape {
  let mut best = 0
  for s in shapes {
    if s.area() > best {
      best = s.area()
    }
  }
  return best
}

test "largest area" {
  expect largest(List.of(Square(side: 3), Square(side: 5))) == 25
}
```

There is no operator overloading: `==` and the comparisons dispatch to the built-in `Equatable` and `Comparable`
interfaces, and a sum of two `Money` values is `Money.add(a, b)` (LR 6.10). Coherence works like Rust's orphan
rule: an implementation lives in the package that defines the interface or the type (LR 9.5).

## Closures declare their kind and their captures

Rust infers `Fn`, `FnMut` and `FnOnce` and infers what a closure captures. Fortis writes both: the kind is `fn`,
`fn mut` or `fn once`, and the capture list names every captured variable with its marker (LR 8.2, LR 8.3). A
`move` capture is Rust's `move` closure for one variable; a closure that moves a captured value out is
`fn once` and is called as `move f()` (LR 8.2).

```fortis
// accept
test "closure kinds" {
  let limit = 3
  let small = fn[limit](n: Int) Bool => n < limit
  let mut seen = 0
  {
    let mut count = fn mut[editable seen](n: Int) Void {
      seen = Int.saturatingAdd(seen, n)
    }
    count(4)
    count(5)
  }
  let message = String.from("done")
  let finish = fn once[move message]() String => move message
  expect small(2)
  expect seen == 9
  expect move finish() == "done"
}
```

A closure is never boxed behind your back: each lambda has its own concrete type, and a heterogeneous collection
of closures needs the explicit `dynamic fn` type, which allocates (LR 8.4).

## Shared ownership is explicit

`Rc`/`Arc` is `Shared[T]` and `Weak` is `Weak[T]`. The only way to get a second handle is `s.retain()`, so every
reference-count increment is visible; `copy` of a `Shared` is rejected (`E-OWN-002`, LR 7.5, LR 7.8). A `Shared[T]` gives
read access only: shared mutation goes through `Mutex[T]` or an atomic (LR 7.8, LR 12.7).

```fortis
// reject: E-OWN-002
class Config {
  export name: String
}

func main() {
  let config = Shared.create(Config(name: "prod"))
  let second = copy config     // reject: E-OWN-002
}
```

```fortis
// accept
class Config {
  export name: String
}

test "retain is the visible clone of a handle" {
  let config = Shared.create(Config(name: "prod"))
  let second = config.retain()
  expect second.name == "prod"
}
```

## Effects are part of the signature

Rust tracks some effects through the type system (`async`, `Result`) and none of the others. Fortis has a closed
set of eight effects, computed for every function from its body and its callees, and four modifiers that state
facts about them: `pure` (no I/O, no outside mutation, no nondeterminism, no suspension, no blocking), `noalloc`,
`async` and `try` (LR 11.1, LR 11.2). `async`, `try` and `recovers` must be written whenever the function has the
effect; `pure` and `noalloc` are promises the checker verifies (LR 11.3). There is no ambient I/O either: there is
no `println!` reaching standard output from anywhere, only methods on capability values that `main` obtains once
and passes down (LR 11.6). A `pure` function that prints is rejected with `E-EFF-001`, reported at the statement
that introduces the effect (LR 11.1):

```fortis
// reject: E-EFF-001
import fortis.host { Console }

pure func greet(console: Console) {      // reject: E-EFF-001 (a pure function takes no capability)
  console.print("hello")                   // reject: E-EFF-001
}

func main() {
}
```

## Async is structured

Like Rust's, a Fortis future does nothing until it is awaited (LR 12.3). Unlike Rust, there is no detached
`spawn` by default: tasks are launched on a `scope`, which joins or cancels every one before its block ends
(LR 12.5). Because the scope always joins first, a cooperative task may borrow locals declared before the scope,
with no `'static` bound and no `Arc` (LR 12.5). Pool launches (`launchParallel`, `launchBlocking`) require
`Shareable` data and no loans, the equivalent of `Send + 'static` (LR 12.5, LR 12.7).

```fortis
// accept
import fortis.host { Console }

async func total(numbers: List[Int]) Int {
  let mut sum = 0
  for n in numbers {
    sum = Int.saturatingAdd(sum, n)
  }
  return sum
}

async func main(console: Console) {
  let odds = List.of(1, 3, 5)
  let evens = List.of(2, 4)
  scope tasks {
    let a = tasks.launch(total(odds))
    let b = tasks.launch(total(evens))
    let x = await a
    let y = await b
    console.print("${x} and ${y}")
  }
}
```

```output
9 and 6
```

A cancelled task unwinds at its `await`, running deferred blocks; cancellation is not a failure and cannot be
caught (LR 12.4).

## What the native backend produces

The production backend lowers checked Fortis to a Rust crate under `#![forbid(unsafe_code)]`, and `rustc`
re-checks the loans and types it can express (LR 7.10, LR C.1). Ownership situations with no safe Rust shape are
rejected before lowering rather than lowered with `unsafe` (LR 7.10, LR C.2). The [lowering appendix](appendix-c-lowering.md)
lists each construct's Rust shape.
