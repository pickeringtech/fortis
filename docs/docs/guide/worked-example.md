# Model a domain: a coffee shop

<!-- covers: LR 6, LR 8, LR 9, LR 10, LR 11.5, LR 14 -->
*Human mode. Agent mode: [Coffee shop, by rule](../agent/worked-example.md). Source of truth: LR 6, LR 8,
LR 9, LR 10, LR 11.5 and LR 14.*

This v0.1 program reads orders such as `large, 2`, rejects bad lines, prices the accepted drinks and prints a
summary. Start with the types, add a conversion at the input boundary, and then choose loops or callbacks for
working with the checked values. The [complete program](#the-complete-program) is also pinned as
[`examples/showcase/v0.1.0-coffee.ft`](../../examples/showcase/v0.1.0-coffee.ft).

## 1. Give the domain its own types

`Pence` is a new nominal type over `Int<0..>` (LR 6.9). A price cannot be confused with a quantity or a plain
integer. `Pence(700)` constructs it because 700 is proved non-negative. For a value the checker cannot prove,
`raw as Pence or fail` checks the range at run time (LR 6.13). Inside this module, `price as Int` unwraps it
while retaining the non-negative refinement. That unwrap is private to the declaring module.

`Size` is a closed enum. Its derivations supply equality, hashing, ordering and display. `Small`, `Medium`,
`Large` compare in declaration order, so sorting has the domain's intended order (LR 6.10). `Hashable`
requires `Equatable`, which makes `Size` a valid set element and map key (LR 6.7).

`Drink.quantity` is an `Int<1..maxQuantity>`, with the `const maxQuantity = 20` folded into the bound
(LR 6.3). `price()` matches all sizes to a per-cup price in `150..350`; multiplying by a quantity in `1..20`
proves the result fits `Pence`, so `Pence(each * self.quantity)` needs no failure branch (LR 6.3, LR 6.4).

`Priced` states what a price provider must do. `Drink` implements it directly; `Pastry` adds the implementation
in an `implement` block (LR 9.5). `Labelled` shares a method body and requires the two method signatures it
calls, never an interface. Each applying class is checked against those requirements (LR 9.6). `Pricing`
collects `pure` and `forbids recovers` into a named contract applied in the modifier position, making the price
calculation's effect promises visible (LR 11.5).

## 2. Convert text at the boundary

The enum's `derive convert from String` recognises variant names ignoring ASCII case. It also accepts a
`StringView`; it does not trim the text itself (LR 6.13). `Drink` declares a fallible `convert from String(line)`.
The body splits at the comma, requires exactly two pieces, trims each view and converts the pieces to `Size`
and `Int`. The final `n as Int<1..maxQuantity> or fail` checks the quantity (LR 6.6, LR 6.13).

Each `or fail` forwards the first failure. The wrong number of fields produces the domain's `BadOrder`, an
unknown size or malformed integer fails conversion, and an out-of-range quantity fails its refinement. A
caller receives one outcome and must handle it (LR 10.1, LR 10.2). `parts` owns its strings; `trim()` borrows
them for the conversion. Keeping that view past the owner would be rejected (LR 7.6, LR 7.7).

`Ticket derive convert from Drink` is a different conversion: a total field-by-field copy of the two matching
fields, explicitly requested by the declaration (LR 6.13). No search through intermediate conversions occurs.

## 3. Read orders with a loop

`Basket` is a newtype over `List[Drink]`. Its reading methods are public wherever the newtype is visible;
editing and consuming methods stay inside this module (LR 6.9). `orders.lines()` returns owned strings, so the
loop can lend each line to the `Drink` conversion without storing a view of the input (LR 6.6).

The `match line as Drink` consumes the conversion's outcome. A success moves the drink into the basket; a
failure prints the skipped line and `report f` consumes the failure (LR 9.8, LR 10.2, LR 10.6). Thus bad input
does not leave an unhandled obligation or prevent the remaining orders from being read.

Later, another `for` builds `seen` and `cupsBySize`. Set insertion returns whether the member was new;
`discard` makes it explicit that this program does not need that non-linear Boolean (LR 5.5). Map updates
read the previous quantity with `get(...) ?? 0` and use saturating addition. Map and set traversal keeps
first-insertion order, so the printed summary is deterministic (LR 6.7).

## 4. Summarise with callbacks

`each` prints labels, while `fold` adds prices, `any` and `all` ask predicates, `countWhere` counts matches,
and `indexWhere` finds a position (LR 6.7). `sortedBy` copies and stably sorts drinks by price; `groupBy`
collects copies under each size in the order the sizes first appear. `map` and `enumerate` build the numbered
size list. The full [types chapter](06-types.md#collections-in-both-styles) also covers `filter`, `find`,
`contains`, `zip`, `take`, `skip`, `flatMap` and `chunked`.

A lambda written directly into a non-`owned` callback argument infers captures and kind (LR 8.2, LR 8.3).
Printing captures a loan of `console`; updating `cups` captures it editable and infers `fn mut`. A stored or
returned lambda would list those captures. Callback parameter modes must still match the function type
exactly (LR 8.4). `each` allows effects; `map` and `fold` require pure callbacks. Their permitted callback
effects count in the caller even when a particular callback does less (LR 11.3).

The three `try` traversals stop at the first failure: `tryMap` parses all counts, `tryFold` parses and adds,
and `tryEach` checks and prints lines. Each result has `or fail` on the call (LR 6.7, LR 10.2). An optional's
`map` transforms only a present value; an outcome's `map` transforms only success and transfers a failure
unchanged (LR 6.8, LR 10.2). `cheapest.first()?.label()` calls `label` only when the optional is present;
`?? "none"` supplies the absent case (LR 6.8).

## 5. Reuse the model

`costOf` accepts a `List[dynamic Priced]`, so drinks and pastries share one price calculation. `Priced` is
dynamic-eligible: its method takes only its receiver and returns a concrete type (LR 9.5). The expected list
type fixes the common element type when `List.of` boxes the two implementations (LR 6.10).

`larger[T]` uses `Comparable` to compare and `Copyable` to return a copy of the chosen read parameter.
Without the copy bound, a generic loan could not become an owned result (LR 6.10). `Pending[T] where T: linear`
can hold an outcome; `take(owned self)` destructures and transfers the obligation, and its caller handles it
with `or fail` (LR 6.12, LR 10.7). A declared conversion from `String` also accepts a literal or a `StringView`; its read parameter borrows
the text (LR 6.13).

`band` unwraps `Pence` in its own module and uses range patterns (LR 9.8). `Deque` serves the front of a queue;
`SortedSet` keeps unique sizes in their declared order; `Array.get` returns an optional and `asView` lends the
array's contents (LR 6.7). `String.join`, ASCII case mapping, search, replacement and repetition build the
receipt (LR 6.6). `spare.clear()` is allowed because its integers are non-linear; a list of outcomes could not
be cleared (LR 10.7).

## 6. Test the rules

The example test checks the large-drink price. The `cases` table sends three bad lines through the conversion
and checks the small drink supplied by recovery. The type of `Drink.quantity` separately enforces the
quantity range at construction. The property draws quantities from `Int<1..20>` and checks their prices are positive (LR 14).
`expect` belongs in these test, property and cases bodies, never in the production conversion or price method.
Property tests already run in today's tools; their full release scope, including shrinking, remains v0.2.

## The complete program

<!-- no-citation: pinned source and commands; the preceding steps cite each language rule -->

The gate checks the program with both checkers, runs its `main` and tests on the interpreter, VM and native
backend, and compares standard output with the pinned output below. Each skipped line also reports a failure
on standard error. From the repository root:

```sh
bin/fortis check examples/showcase/v0.1.0-coffee.ft
bin/fortis run examples/showcase/v0.1.0-coffee.ft
bin/fortis test examples/showcase/v0.1.0-coffee.ft
compiler/target/release/fortisc vm --stdout examples/showcase/v0.1.0-coffee.ft
bin/fortis build examples/showcase/v0.1.0-coffee.ft --out build/coffee
build/coffee
```

`bin/fortis build --emit-only examples/showcase/v0.1.0-coffee.ft` writes the generated Rust crate without
compiling it, for inspection before a native build.

```fortis
// accept
// Fortis v0.1.0: a coffee shop's orders, read from text, checked, priced and summarised.
import fortis.host { Console }
import fortis.collections { Deque, SortedSet }

const maxQuantity: Int = 20

// ---- Types that carry their rules -------------------------------------------------------------------

// A newtype: its own type over Int<0..>, never mixed up with a plain Int.
export type Pence = Int<0..>

// An enum that converts from text by name, ignoring ASCII case.
export enum Size derive Equatable, Hashable, Comparable, Displayable, convert from String {
  Small
  Medium
  Large
}

export error BadOrder {
  code: "shop.bad_order"
  payload { reason: String }
}

export interface Priced {
  pure func price(self) Pence
}

// A mixin lends methods; it requires methods, never an interface.
export mixin Labelled {
  requires pure func name(self) String
  requires pure func price(self) Pence

  export pure func label(self) String => "${self.name()}: ${self.price()}p"
}

// A contract names a set of effect rules once and is used like a modifier.
export contract Pricing {
  pure
  forbids recovers
}

export class Drink implements Priced with Labelled {
  export size: Size
  export quantity: Int<1..maxQuantity>      // a refinement with a const bound

  export pure func name(self) String => "${self.quantity} x ${self.size} coffee"

  export Pricing pure func price(self) Pence {
    let each: Int<150..350> = match self.size {
      Small => 150
      Medium => 250
      Large => 350
    }
    return Pence(each * self.quantity)      // proved in range, so no check is needed
  }

  // A conversion the type declares: "large, 2" as Drink.
  export try convert from String(line) {
    let parts = line.split(",")
    if parts.count() != 2 {
      fail BadOrder(reason: "expected size, quantity")
    }
    let size = (parts.get(0) ?? "").trim() as Size or fail
    let n = (parts.get(1) ?? "").trim() as Int or fail
    return Drink(size, quantity: n as Int<1..maxQuantity> or fail)
  }
}

// A class that implements an interface through an `implement` block.
export class Pastry {
  export name: String
  export pence: Pence
}

implement Priced for Pastry {
  pure func price(self) Pence => self.pence
}

// A field-by-field conversion, derived.
export class Ticket derive convert from Drink {
  export size: Size
  export quantity: Int<1..maxQuantity>
}

// A newtype over a collection: reading methods everywhere, editing only in this module.
export type Basket = List[Drink]

// A generic class that may hold a linear value, such as an outcome, and must hand it on.
export class Pending[T] where T: linear {
  item: T

  export func take(owned self) T {
    let Pending(item: value) = move self
    return move value
  }
}

// Range patterns.
export pure func band(total: Pence) String {
  return match total as Int {               // a newtype unwraps with `as` in its own module
    0..<500 => "small"
    500..<2000 => "medium"
    otherwise => "large"
  }
}

// Generics with bounds.
export pure func larger[T](a: T, b: T) T where T implements Comparable, T implements Copyable {
  if a > b {
    return copy a
  }
  return copy b
}

try func addCount(sum: Int, text: StringView) Int {
  let n = text.trim() as Int or fail
  return Int.saturatingAdd(sum, n)
}

try func checkLine(console: Console, line: String) {
  let d = line as Drink or fail
  console.print("checked ${d.label()}")
}

pure func costOf(items: List[dynamic Priced]) Int {
  return items.fold(0, fn(sum, item) => Int.saturatingAdd(sum, item.price() as Int))
}

// ---- The program ------------------------------------------------------------------------------------

try recovers func main(console: Console) {
  let orders = "large, 2\nsmall, 6\nmedium, 1\nhuge, 3\nlarge, many\nsmall, 40\n" as String

  // Traditional: a for loop, a match on the outcome, a report for each bad line.
  let mut basket = Basket(List.default())
  for line in orders.lines() {
    match line as Drink {
      success(drink) => basket.push(move drink)
      failure(f) => {
        console.print("skipped '${line}'")
        report f
      }
    }
  }

  // Functional: lambdas written into a call work out what they capture.
  basket.each(fn(drink) => console.print(drink.label()))
  let total = basket.fold(0, fn(sum, drink) => Int.saturatingAdd(sum, drink.price() as Int))
  console.print("total ${total}p, any large? ${basket.any(fn(d) => d.size == Large)}")
  console.print("all under 20? ${basket.all(fn(d) => d.quantity < 20)}, small orders ${basket.countWhere(fn(d) => d.size == Small)}")
  let cheapest = basket.sortedBy(fn(d) => d.price())
  console.print("cheapest first: ${cheapest.first()?.label() ?? "none"}")
  console.print("first large at ${basket.indexWhere(fn(d) => d.size == Large) ?? -1}")
  basket.groupBy(fn(d) => d.size).each(fn(size, drinks) => console.print("${size}: ${drinks.count()} order(s)"))
  basket.map(fn(d) => d.size).enumerate().each(fn(pair) => console.print("#${pair.0} ${pair.1}"))

  // A lambda that changes a variable outside it.
  let mut cups = 0
  basket.each(fn(d) {
    cups = Int.saturatingAdd(cups, d.quantity)
  })
  console.print("cups ${cups}, larger size ${larger(Size.Small, Medium)}")

  // Failing callbacks: tryMap, tryEach and tryFold stop at the first failure.
  let counts = "3, 4, 5".split(",").tryMap(fn(t) => t.trim() as Int) or fail
  let sum = "3, 4, 5".split(",").tryFold(0, fn(acc, t) => addCount(acc, t)) or fail
  "small, 1\nlarge, 2".lines().tryEach(fn(line) => checkLine(console, line)) or fail
  console.print("parsed ${counts.count()} counts, sum ${sum}, both lines valid")

  // Helpers on T? and Outcome[T].
  let firstPrice = basket.first().map(fn(d) => d.price() as Int) ?? 0
  let doubled = ("21" as Int).map(fn(n) => Int.saturatingAdd(n, n)) or recover 0
  console.print("first price ${firstPrice}, doubled ${doubled}")

  // Conversions: derived, unwrapped, checked.
  let ticket = basket.first()?.size ?? Small
  let firstTicket = Drink(size: Large, quantity: 2) as Ticket
  let totalPence = total as Pence or recover Pence(0)
  console.print("ticket ${firstTicket.size} x ${firstTicket.quantity}, size ${ticket}, band ${band(totalPence)}")

  // Interfaces through `dynamic`, with a class that implements one by an `implement` block.
  let till: List[dynamic Priced] = List.of(Drink(size: Small, quantity: 1), Pastry(name: "croissant", pence: Pence(180)))
  console.print("till ${costOf(till)}p")

  // A linear value inside a generic class still has to be handled.
  let pending = Pending(item: "medium, 1" as Drink)
  let late = move pending.take() or fail
  console.print("late order ${late.label()}")

  // More collection types.
  let mut queue: Deque[String] = Deque.default()
  queue.pushBack("Ann" as String)
  queue.pushBack("Bo" as String)
  queue.pushFront("Cy" as String)
  let served = queue.popFront() ?? ("nobody" as String)
  console.print("served ${served}, next ${queue.peekFront() ?? "nobody"}, waiting ${queue.count()}")
  let sizes = SortedSet.fromList(basket.map(fn(d) => d.size))
  console.print("sizes from ${sizes.first() ?? Small} to ${sizes.last() ?? Large}")
  let mut shelf = Array[Int, 3].filled(0)
  console.print("shelf slot 1 holds ${shelf.get(1) ?? -1}, view of ${shelf.asView().count()}")

  // Maps and sets, the traditional way.
  let mut seen: Set[Size] = Set.default()
  let mut cupsBySize: Map[Size, Int] = Map.default()
  for d in basket {
    discard seen.add(d.size)
    cupsBySize.set(d.size, Int.saturatingAdd(cupsBySize.get(d.size) ?? 0, d.quantity))
  }
  let mut wanted: Set[Size] = Set.default()
  discard wanted.add(Large)
  discard wanted.add(Medium)
  console.print("${seen.count()} sizes, large cups ${cupsBySize.get(Large) ?? 0}, both ${seen.intersection(wanted).count()}")
  console.print("keys ${cupsBySize.keys().count()}, cups ${cupsBySize.values().fold(0, fn(a, n) => Int.saturatingAdd(a, n))}")

  // Text.
  let receipt = String.join(basket.map(fn(d) => d.label()), " | ")
  console.print(receipt.asciiUpper())
  console.print("has small? ${receipt.contains("Small")}, starts ${receipt.startsWith("2 x")}, at ${receipt.find("|") ?? 0}")
  console.print(receipt.replace(" coffee", "").repeat(1))
  console.print("-".repeat(20))

  let mut spare = List.of(1, 2, 3)
  spare.clear()
  console.print("spare ${spare.count()}")
}

// ---- Tests ------------------------------------------------------------------------------------------

test "a large coffee costs 350p a cup" {
  expect Drink(size: Large, quantity: 2).price() == Pence(700)
}

test "lines that are not orders fail" cases {
  "unknown size": { line: "huge, 1" }
  "not a number": { line: "large, many" }
  "too many": { line: "small, 40" }
} as row {
  let ok = row.line as Drink catch e {
    report e
    recover Drink(size: Small, quantity: 1)
  }
  expect ok.size == Size.Small
}

property "every quantity in range prices above zero" forall (n: Int<1..20>) {
  expect Drink(size: Small, quantity: n).price() as Int > 0
}
```

```output
skipped 'huge, 3'
skipped 'large, many'
skipped 'small, 40'
2 x Large coffee: 700p
6 x Small coffee: 900p
1 x Medium coffee: 250p
total 1850p, any large? true
all under 20? true, small orders 1
cheapest first: 1 x Medium coffee: 250p
first large at 0
Large: 1 order(s)
Small: 1 order(s)
Medium: 1 order(s)
#0 Large
#1 Small
#2 Medium
cups 9, larger size Medium
checked 1 x Small coffee: 150p
checked 2 x Large coffee: 700p
parsed 3 counts, sum 12, both lines valid
first price 700, doubled 42
ticket Large x 2, size Large, band medium
till 330p
late order 1 x Medium coffee: 250p
served Cy, next Ann, waiting 2
sizes from Small to Large
shelf slot 1 holds 0, view of 3
3 sizes, large cups 2, both 2
keys 3, cups 9
2 X LARGE COFFEE: 700P | 6 X SMALL COFFEE: 900P | 1 X MEDIUM COFFEE: 250P
has small? true, starts true, at 23
2 x Large: 700p | 6 x Small: 900p | 1 x Medium: 250p
--------------------
spare 0
```
