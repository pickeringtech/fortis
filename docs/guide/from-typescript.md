# Notes for readers coming from TypeScript

<!-- covers: LR 1 -->
*Human mode. Source of truth: the LR; each note cites the section that states the rule.*

TypeScript adds static types to a dynamic language and leaves the runtime alone. Fortis is statically checked
and compiled all the way down: the checker's view is the program's behaviour, there is no `any`, and a type can
never be wrong at run time (LR 1, LR 6.1). Much of the syntax will look familiar (braces, `let`, `${…}`
interpolation, `async`/`await`, `?.` and `??`); the meaning underneath is stricter. This page goes through the
differences you will meet first.

## Types are nominal

TypeScript compares object types by shape: any value with the right fields fits. Fortis compares by name. Two
classes with identical fields are different types, and `type` always declares a new nominal type rather than an
alias (LR 6.1, LR 6.9). That is what lets an id type protect itself:

```fortis
// reject: E-TYP-040
type UserId = Int<1..>
type OrderId = Int<1..>

pure func describe(id: UserId) String => "user ${id}"

export func main(args: List[String]) Void {
  let order = OrderId(7)
  discard describe(order)      // reject: E-TYP-040
}
```

There is no top type (`any`, `unknown`), no implicit conversion between distinct types and no runtime type
inspection (LR 6.1, LR 1). Anonymous records (`{ name: A, other: B }`) and tuples exist for local convenience
but may not appear in an exported signature (LR 6.9).

## `null` is one thing, and nothing is truthy

There is no `undefined`, and `null` exists only as the absent case of an optional type `T?` (LR 6.8). `??` and
`?.` work as you expect, and a null test narrows the type inside the branch, as TypeScript's control-flow
analysis does (LR 6.8).

```fortis
// accept
import fortis.host { Host }

class User {
  export name: String
  export nickname: String?
}

pure func greeting(user: User) String {
  if user.nickname != null {
    return "hi ${user.nickname}"
  }
  return "hello ${user.name}"
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print(greeting(User(name: "Ada", nickname: null)))
  caps.console.print(greeting(User(name: "Brian", nickname: "Bri")))
  let maybe: Int? = null
  caps.console.print("${maybe ?? 0}")
}
```

```output
hello Ada
hi Bri
0
```

What TypeScript allows and Fortis does not is using a value as a condition. A condition must be a `Bool`; an
`Int`, a string or an optional is `E-TYP-010` (LR 5.2). Write the comparison you mean.

```fortis
// reject: E-TYP-010
pure func label(count: Int) StringView from static {
  if count {                   // reject: E-TYP-010
    return "some"
  }
  return "none"
}
```

`T?`, `Bool` and a fallible outcome are three different types with three different operators (`??`, `||`, `or`),
and none converts into another (LR 1, LR 6.8).

## Exceptions become outcomes

Fortis has no exceptions and no `throw`. A function that may fail is declared `try`; calling it gives an
outcome, which the caller must handle, and the compiler rejects code that ignores one (LR 10.1, LR 10.2). The
handling forms are written at the call: `or fail` passes the failure on (like letting an exception propagate),
`or recover v` replaces it, and `catch e { … }` handles it in a block (LR 10.2, LR 10.4).

```fortis
// accept
import fortis.host { Host }

export error NotFound {
  code: "users.not_found"
  payload { id: Int }
  message: "no user ${id}"
}

pure try func lookup(id: Int) String {
  if id == 1 {
    return "ada"
  }
  fail NotFound(id: id)
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let first = lookup(1) catch e {
    report e
    recover "nobody"
  }
  let second = lookup(2) catch e {
    caps.console.print("lookup failed: ${e.code}")
    report e
    recover "nobody"
  }
  caps.console.print("${first}, ${second}")
}
```

```output
lookup failed: users.not_found
ada, nobody
```

The difference from `try { … } catch (e) { … }` is that the handler covers exactly one operation, its left
operand, so you can always see which call a handler is for (LR 10.4). Failures carry a stable code declared by an
`error` declaration, so callers match on `is NotFound`, not on message text (LR 10.3, LR 10.5). `report e` writes
the failure once to the diagnostic plane and consumes it (LR 10.6).

## Assigning an object does not share it

In TypeScript `const b = a` makes `b` refer to the same object as `a`, and a change through one is visible
through the other. In Fortis every value has one owner, and copying, borrowing or moving it is written down:
`copy a`, `alias a` or `move a` (LR 7.5). A bare `let b = a` on an object is rejected (`E-OWN-001`), so the
question never arises silently.

```fortis
// accept
import fortis.host { Host }

class Cart {
  export items: List[String]
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut cart = Cart(items: List.of("tea"))
  let snapshot = copy cart
  editable cart.items.push("milk")
  caps.console.print("cart has ${cart.items.count()}, snapshot has ${snapshot.items.count()}")
}
```

```output
cart has 2, snapshot has 1
```

The `editable` in front of `cart.items.push(…)` is the other half of the rule: a call that may change a value
says so at the call (LR 7.5, LR 7.6). A function that receives a plain parameter can read it but not change it
(LR 7.6).

## Numbers are not all one type

TypeScript's `number` is a 64-bit float. Fortis has 64-bit integers (`Int`), sized integers, `Float` (IEEE
binary64), exact `Decimal` for money, and `BigInt` (LR 6.2). Integer arithmetic must be proved not to overflow,
or use the checked operations, which fail instead (LR 6.4). `Decimal` has no operators in Language 1.0; its
arithmetic is the checked `Decimal.add`, `Decimal.mul` and so on (LR 6.5).

```fortis
// accept
import fortis.host { Host }

export recovers func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${0.1 + 0.2}")
  let price = Decimal.add(0.1d, 0.2d) or recover 0d
  caps.console.print("${price}")
  caps.console.print("${7 / 2}")
}
```

```output
0.30000000000000004
0.3
3
```

The first line is ordinary float arithmetic, exactly as in TypeScript; `Decimal` gives the answer a ledger
needs, and integer division truncates toward zero (LR 6.4, LR 6.5). There is no `+` on strings: operators are
arithmetic only, and text is built with interpolation or a `text.Builder` (LR 1.1, LR 6.6).

`main` is declared `recovers` because `or recover 0d` discards a possible failure without reporting it. The
checker requires the modifier on any function that does that, so a reader of the signature knows the function
can swallow a failure (LR 10.2, LR 11.2).

## Unions are enums, and `match` must be exhaustive

A discriminated union (`{ kind: "circle"; radius: number } | …`) is a Fortis `enum` with payloads, and a
`switch` with a `never` check is a `match`, which must cover every variant without a default (LR 9.7, LR 9.8).

```fortis
// accept
import fortis.host { Host }

enum Shape {
  Circle(radius: Int<0..1000>)
  Rect(width: Int<0..1000>, height: Int<0..1000>)
}

pure func area(s: Shape) Int => match s {
  Circle(radius: r) => 3 * r * r
  Rect(width: w, height: h) => w * h
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${area(Shape.Circle(radius: 2))} ${area(Shape.Rect(width: 3, height: 4))}")
}
```

```output
12 12
```

Leaving a variant out is an error naming it (`E-MAT-001`), and a catch-all is not allowed as the last arm of a
match over an enum, so adding a variant finds every place that must handle it (LR 9.8).

## Arrow functions list their captures

A TypeScript arrow function captures whatever it mentions. A Fortis lambda lists what it captures and how, and
names its kind: `fn` reads, `fn mut` may change what it captured, `fn once` runs once (LR 8.2, LR 8.3). A lambda
with no captures has no brackets.

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let prices: List[Int<0..1000>] = List.of(120, 80, 45, 300)
  let limit = 100
  let cheap = prices.filter(fn[limit](p: Int<0..1000>) Bool => p < limit)
  let doubled = prices.map(fn(p: Int<0..1000>) Int => p * 2)
  caps.console.print("${cheap} ${doubled}")
}
```

```output
[80, 45] [240, 160, 90, 600]
```

`map`, `filter` and `countWhere` take `pure` callbacks; a lambda whose body has no effects qualifies without
saying so, and anything that does I/O belongs in a `for` loop (LR 6.7, LR 11.3).

## Promises are futures, and concurrency is scoped

`async` and `await` read the same, with two differences. A Fortis future does not start when it is created: the
body runs when it is awaited or launched (LR 12.3). And there is no fire-and-forget: concurrent work is launched
on a `scope`, which waits for or cancels every task before it ends, so `Promise.all` becomes a scope or `join`
(LR 12.5).

```fortis
// accept
import fortis.host { Host }

async func square(n: Int<0..1000>) Int => n * n

export async func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let both = await join(square(3), square(4))
  caps.console.print("${both.0} ${both.1}")
}
```

```output
9 16
```

A future must be awaited or launched, or deliberately discarded if it never started and holds nothing linear and
no exclusive loan; dropping one by accident is an error, like any
unconsumed outcome (LR 12.3, LR 6.12).

## Modules run nothing when imported

A TypeScript module can run code when it is first imported. A Fortis module contains only declarations and
constants, so importing one never executes anything (LR 4.2). Side effects begin in `main`, and I/O needs a
capability value that `main` obtains from `Host.capabilities()` and passes to whatever needs it: there is no
global `console` (LR 11.6).
