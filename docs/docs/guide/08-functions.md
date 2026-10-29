# Functions and closures

<!-- covers: LR 8 -->
*Human mode. Agent mode: [Functions and closures, by rule](../agent/08-functions.md). Source of truth: LR 8.*

Functions are where most Fortis code lives, and their heads carry more information than in most languages: what
the function may do (its modifiers), what it does with each argument (the parameter modes of
[Ownership](07-ownership.md)) and, for a lambda, exactly which outside values it can see and how. This chapter
covers named functions, lambdas and their three kinds, capture lists and function types.

## Named functions

A function head lists, in this order: the aspects applied to it (one per line above the head), `export`, any
named contracts, then the modifiers `pure`, `noalloc`, `async`, `try` and `recovers`, then `func`, the name, type
parameters, parameters, the success type and an optional `from` or `where` clause (LR 8.1):

```fortis
// illustrative
[@aspect(args) …]
[export] [Contract …] [pure] [noalloc] [async] [try] [recovers] func name[TypeParams](params) ReturnType [from …] [where …] { body }
```

The modifiers always appear in that order (LR 8.1). Every parameter has a type, and the success type is written
unless it is `Void`: `func greet(console: Console) { … }` returns nothing (LR 8.1). Older code writes `Void` there
too; it is still accepted, and `fortis fmt` drops it. A function type keeps its result, `fn(Int) Void`, and so does
a lambda with a block body (LR 8.2, LR 8.4). What each modifier means:

- `try`: the function may fail; a caller receives an `Outcome` it must deal with (LR 8.1, LR 10.1).
- `async`: the function may suspend; a caller receives a `Future` (LR 8.1, LR 12.3).
- `recovers`: the function discharges a failure without reporting it (LR 8.1, LR 10.4).
- `pure` and `noalloc`: claims the checker verifies (LR 8.1, LR 11.2).

`try`, `async` and `recovers` change what a caller gets, so a function must write them whenever its body has the
effect. `pure` and `noalloc` are promises a function may choose to make, and callers can rely on them only when
they are written (LR 11.3). [Effects and contracts](11-effects.md) covers all five in depth.

A body is a block, or `=> expression` for a function whose body is one expression (LR 8.1):

```fortis
// accept
import fortis.host { Console }

pure func square(n: Int<-1000..1000>) Int => n * n

pure func describe(n: Int<-1000..1000>) String {
  let s = square(n)
  return "${n} squared is ${s}"
}

func main(console: Console) {
  console.print(describe(12))
}
```

```output
12 squared is 144
```

A named function is not a value you can store, but you can pass its name wherever a callable is expected: the
compiler makes a non-capturing `fn` of the matching type for you (LR 8.1).

```fortis
// accept
import fortis.host { Console }

pure func increment(n: Int) Int => Int.saturatingAdd(n, 1)

pure func applyTwice(f: pure fn(Int) Int, x: Int) Int => f(f(x))

func main(console: Console) {
  console.print("${applyTwice(increment, 5)}")
}
```

```output
7
```

## Lambdas

A lambda is written `fn`, an optional capture list in brackets, the parameters, the return type and a body
(LR 8.2). It takes the same modifiers as a named function, in the same order, and must write `async`, `try` and
`recovers` when its body has those effects; `pure` and `noalloc` may be written and are otherwise inferred; the
formatter prints no inferred one back, and keeps one you wrote, because the function type the lambda is passed to
already says what it must be (LR 8.2). A block body writes its return type; an expression body may leave it out, `fn(t) => t.isOpen()`, and it
is then the result type of the function type the lambda is passed to, or the body's own type where that is a type
parameter (LR 8.2). Parameter types may be left out only when the lambda is passed directly to a parameter whose
function type supplies them (LR 8.2). Lambdas are not generic.

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let numbers = List.of(3, 8, 1, 6)
  let evens = numbers.filter(fn(n: Int) Bool => n % 2 == 0)
  let doubled = numbers.map(fn(n) Int => Int.saturatingMul(n, 2))
  console.print("${evens.count()} even, first doubled ${doubled.first() ?? 0}")
}
```

```output
2 even, first doubled 6
```

The second lambda leaves out its parameter's type, because `map`'s parameter already says what it receives.

### Three kinds of closure

Every lambda is one of three kinds, and the kind is always written, because it tells the reader how often the
closure may run and whether it changes anything (LR 8.2):

| Kind | It may | Called | A call looks like |
|---|---|---|---|
| `fn` | read its captures | any number of times | `f(x)` |
| `fn mut` | also change what it captured | any number of times; the binding must be `let mut` | `f(x)` |
| `fn once` | also move a capture out | exactly once; the call consumes it | `move f(x)` |

Calling a `fn mut` takes an exclusive loan of the closure, and its `let mut` binding already says that it changes,
so the call is written `bump()`; calling a `fn once` consumes it, so it is written `move finish()` (LR 7.5,
LR 8.2). Older code writes `editable bump()`; it is still accepted, and `fortis fmt` rewrites it.

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let mut total = 0
  {
    let mut add = fn mut[editable total](n: Int) Void {
      total = Int.saturatingAdd(total, n)
    }
    add(4)
    add(6)
  }
  console.print("total ${total}")
  let name = String.from("ada")
  let takeName = fn once[move name]() String {
    return move name
  }
  let taken = move takeName()
  console.print("took ${taken}")
}
```

```output
total 10
took ada
```

The inner block around `add` matters: `add` holds an exclusive loan of `total` for as long as `add` exists
(LR 8.3), so `total` can be read again only after the block that declares `add` has ended.

A lambda whose body needs a stronger kind than it declares is rejected with `E-FUN-010`, which names the capture
and the use (LR 8.2). Here a `fn` writes to its capture:

```fortis
// reject: E-FUN-010
func main() {
  let mut count = 0
  let bump = fn[editable count]() Void {
    count = Int.saturatingAdd(count, 1)     // reject: E-FUN-010
  }
}
```

Declaring `fn mut` fixes it. A `move` capture alone does not make a closure `fn once`; what matters is whether the
body moves the captured value out (LR 8.2). Declaring a stronger kind than the body needs is allowed (LR 8.2).

The binding that holds a `fn mut` must be `let mut`, because each call changes it. Through a `let mut` binding the
call needs no `editable`, since the declaration already says it (LR 7.5); through a `let` binding it is an
ownership error:

```fortis
// reject: E-OWN-001
func main() {
  let mut total = 0
  let add = fn mut[editable total](n: Int) Void {
    total = Int.saturatingAdd(total, n)
  }
  add(4)                                    // reject: E-OWN-001
}
```

And a `fn once` really runs once: after `move finish()` the closure is gone, and a second call is a use after move
(LR 7.4, LR 8.2):

```fortis
// reject: E-OWN-020
func main() {
  let name = String.from("ada")
  let finish = fn once[move name]() String {
    return move name
  }
  let first = move finish()
  let second = move finish()                // reject: E-OWN-020
}
```

## Capture lists

A lambda sees only the outside names it lists, each with a marker that says how: `copy x`, `alias x`,
`editable x` or `move x`. A trivial value such as an `Int` may be listed bare (`[threshold]`), which copies it
(LR 8.3). The list is written only when it is not empty: a lambda with no captures has no brackets (LR 8.2).

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let threshold = 4
  let prefix = String.from("item")
  let numbers = List.of(1, 5, 2, 8)
  let above = numbers.countWhere(fn[threshold](n: Int) Bool => n > threshold)
  let labels = numbers.map(fn[alias prefix](n: Int) String => "${prefix} ${n}")
  console.print("${above} above ${threshold}; ${labels.first() ?? "none"}")
}
```

```output
2 above 4; item 1
```

A name that is not in the list is not visible in the body: `E-FUN-011` (LR 8.3). Module-level constants,
functions and types are never captures and need no entry.

```fortis
// reject: E-FUN-011
func main() {
  let limit = 3
  let numbers = List.of(1, 5, 2, 8)
  let over = numbers.countWhere(fn(n: Int) Bool => n > limit)    // reject: E-FUN-011
}
```

Captures are evaluated, left to right, when the lambda expression is evaluated, not when it is called (LR 8.3).
Capturing a field path such as `alias self.lines` captures that path and takes the matching loan of its owner for
as long as the closure lives (LR 8.3). The loan rules of [Ownership](07-ownership.md) apply to captures as they do
everywhere: two captures of one owner where one of them is `editable` conflict (`E-OWN-010`, LR 8.3).

```fortis
// reject: E-OWN-010
func main() {
  let mut count = 0
  let mut both = fn mut[editable count, alias count]() Void {   // reject: E-OWN-010
    count = 1
  }
}
```

### Closures that hold loans

A closure that captures by `alias` or `editable` holds loans, and it cannot outlive them (LR 8.3). Passing such a
closure to an ordinary callback parameter, as above, needs nothing special: the callee only borrows the closure
for the call. Returning one, or passing it to an `owned fn` parameter, is different: the type must name the region
the closure lives in, with `from` (LR 8.3, LR 8.4).

```fortis
// accept
import fortis.host { Console }

class Limit {
  export value: Int
}

func above(limit: Limit) fn(Int) Bool from limit {
  return fn[alias limit](n: Int) Bool => n > limit.value
}

func countWhere(items: List[Int], keep: fn(Int) Bool) Int {
  let mut n = 0
  for item in items {
    if keep(item) {
      n = Int.saturatingAdd(n, 1)
    }
  }
  return n
}

func main(console: Console) {
  let limit = Limit(value: 3)
  let check = above(alias limit)
  console.print("${countWhere(List.of(1, 5, 2, 8), check)} above ${limit.value}")
}
```

```output
2 above 3
```

The caller writes `alias limit`, because the result keeps a loan of `limit` past the call (LR 7.6). Without
`from limit`, the closure's loan would have nowhere to live, and the checker rejects the function (LR 8.3).

A closure is never stored in a field together with a loan, because fields never store loans (LR 7.7, LR 8.3).

### Closures and linear values

A closure that captures a linear value by `move` (an `Outcome`, a `Future`, a `Task`) owns an obligation, so it
must run exactly once: it must be `fn once`, and a `fn` or `fn mut` that captures one is `E-OBL-006` (LR 8.3).
Here the closure is also never called, so the obligation it owns is dropped as well, which is `E-OBL-001` on the
same line (LR 6.12):

```fortis
// reject: E-OBL-006, E-OBL-001
export error Oops {
  code: "demo.oops"
}

pure try func risky() Int {
  fail Oops()
}

recovers func run() Int {
  let pending = risky()
  let mut later = fn mut[move pending]() Int {             // reject: E-OBL-006, E-OBL-001
    return 0
  }
  return 0
}

func main() {
}
```

The repair is `fn once`, called exactly once with `move later()`, with a body that consumes what it captured
(LR 8.3).

## Function types

A function type names the kind, the parameter types with their modes, the return type and the modifiers
(LR 8.4):

```fortis
// illustrative
fn(Int) Bool
pure fn(StringView) Int
fn mut(editable Buffer) Void
try fn once(owned Job) Void
async fn(Request) Response
```

A callable fits a function type when its kind is no more demanding (`fn` fits where `fn mut` or `fn once` is
asked for), its parameter modes match exactly, and its modifiers are at least as strong: a `pure` callable fits a
plain `fn`, and a callable that cannot fail fits a `try fn` (LR 8.4). The reverse is never allowed: function types
are nominal in their modifiers, so an ordinary callback cannot be passed where a `pure fn` is required (LR 8.4).

```fortis
// reject: E-TYP-040
import fortis.host { Console }

pure func applyPure(f: pure fn(Int) Int, x: Int) Int => f(x)

func noisy(console: Console, n: Int) Int {
  console.print("called")
  return n
}

func main(console: Console) {
  discard applyPure(fn[alias console](n: Int) Int => noisy(console, n), 1)   // reject: E-TYP-040
}
```

A lambda literal whose body has no effect that `pure` excludes is `pure` without saying so, which is why the
lambdas passed to `filter` and `map` above needed no modifier (LR 8.2, LR 11.3).

### Closures in collections: `dynamic fn`

Each lambda has its own concrete type, which is what lets the backend compile it without boxing (LR 8.4). A list
of different closures therefore needs one common type: `dynamic fn(…) …`, a boxed, owned closure that allocates
when it is created (LR 8.4). Nothing is ever boxed unless you write `dynamic` (LR 8.4).

```fortis
// accept
import fortis.host { Console }

func applyAll(steps: List[dynamic fn(Int) Int], start: Int) Int {
  let mut value = start
  for step in steps {
    value = step(value)
  }
  return value
}

func main(console: Console) {
  let bonus = 10
  let double: dynamic fn(Int) Int = fn(n: Int) Int => Int.saturatingMul(n, 2)
  let addBonus: dynamic fn(Int) Int = fn[bonus](n: Int) Int => Int.saturatingAdd(n, bonus)
  let steps = List.of(move double, move addBonus)
  console.print("${applyAll(steps, 5)}")
}
```

```output
20
```
