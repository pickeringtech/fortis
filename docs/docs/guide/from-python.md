# Notes for readers coming from Python

<!-- covers: LR 1 -->
*Human mode. Source of truth: the LR; each note cites the section that states the rule.*

> Preview: newtypes and mixins (LR 6.9, 9.6) arrive in v0.1 and property tests (LR 14) in v0.2; the examples under "Exceptions are outcomes", "Classes without inheritance" and "Tests are part of the language" that use them are checked and run today. The rest of these notes is in v0.0.2.

Python checks a program while it runs; Fortis checks it before anything runs. Every binding has a type the checker
knows, every function states the types it takes and returns, and a program the checker rejects never starts
(LR 1, LR 8.1). In exchange, the questions a Python reviewer has to answer by reading carefully (can this call
change my list, can it raise, does it do I/O) are answered on the page. This page goes through the differences you
will meet first.

## Braces, newlines and declared types

Blocks are delimited by braces, not indentation, and a newline ends a statement unless the line obviously
continues (it ends in an operator or a comma, or the next line starts with `.`, `or`, `catch`, `??` and a few
others); there are no semicolons (LR 3.2). Every parameter has a type and every function states its return type,
unless it returns nothing (LR 8.1). Local types are inferred.

```fortis
// accept
import fortis.host { Console }

pure func average(scores: ListView[Int<0..100>]) Int {
  let n = scores.count()
  if n <= 0 {
    return 0
  }
  let mut total = 0
  for s in scores {
    total = Int.saturatingAdd(total, s)
  }
  return total / n
}

func main(console: Console) {
  let scores: List[Int<0..100>] = List.of(72, 91, 64)
  console.print("average ${average(scores)}")
}
```

```output
average 75
```

`let` declares a name and `let mut` one that may be reassigned; plain `=` only reassigns, so a typo can never
create a new variable (LR 5.1).

The early `return` is not only for the empty case. Integer division must be proved safe, and after
`if n <= 0 { return 0 }` the checker knows that `n` is at least 1 for the rest of the function, so `total / n`
cannot divide by zero (LR 6.3, LR 6.4). Without the test, the division would be rejected and you would handle its
failure instead, `total / n or fail`, which fails with `numeric.divide_by_zero` (LR 6.4).

## Nothing is truthy

`if items:` in Python tests for a non-empty list. A Fortis condition must be a `Bool`, so a list, a number, a
string or an optional in a condition is `E-TYP-010` (LR 5.2). Say what you are testing.

```fortis
// reject: E-TYP-010
pure func describe(items: List[Int]) StringView from static {
  if items {                    // reject: E-TYP-010
    return "some"
  }
  return "none"
}
```

```fortis
// accept
pure func describe(items: List[Int]) StringView from static {
  if !items.isEmpty() {
    return "some"
  }
  return "none"
}

test "describe" {
  expect describe(List.of(1)) == "some"
  expect describe(List.default()) == "none"
}
```

## `None` is an optional type

A value that may be absent has type `T?`, and `null` is its absent case (LR 6.8). Python's
`d.get(key, default)` is `map.get(key) ?? default`; `x.y if x is not None else None` is `x?.y`. After
`if value != null`, the checker knows `value` is present inside the branch (LR 6.8).

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let mut stock: Map[String, Int] = Map.default()
  stock.set("tea", 4)
  let tea = stock.get("tea") ?? 0
  let coffee = stock.get("coffee") ?? 0
  console.print("tea ${tea}, coffee ${coffee}")
}
```

```output
tea 4, coffee 0
```

Absence, failure and `False` are three different things in Fortis, with three different types and operators,
and none of them converts into another (LR 1, LR 6.8).

## Exceptions are outcomes

There are no exceptions. A function that can fail is declared `try`, and its caller must deal with the outcome:
pass it on with `or fail`, replace it with `or recover v`, or handle it with `catch e { … }` (LR 10.1, LR 10.2).
Forgetting to is a compile error, not a crash in production (LR 10.1).

Converting untrusted input into a constrained type is the everyday case. Where Python would call `int(text)` and
check a range, Fortis declares the range in the type and converts with `validate`, a pure fallible conversion
(LR 6.3, LR 6.9) that, by its contract in `fortis.prelude`, fails with `refinement.out_of_range` when the value does
not fit:

```fortis
// accept
import fortis.host { Console }

type Percent = Int<0..100>

func main(console: Console) {
  for raw in List.of(42, 250) {
    let p = Percent.validate(raw) catch e {
      console.print("${raw} rejected: ${e.code}")
      report e
      recover Percent(0)
    }
    console.print("stored ${p}")
  }
}
```

```output
stored 42
250 rejected: refinement.out_of_range
stored 0
```

`report e` writes the failure once to standard error as a structured record and consumes it; a failure can be
reported or propagated, never both (LR 10.6). A `catch` covers only the call to its left, never a whole block
(LR 10.4).

## Assignment does not share a list

In Python, `b = a` makes `b` another name for the same list, and appending through one shows through the other.
In Fortis each value has one owner, and taking a second hold on it is written down: `copy a` for an independent
duplicate, `alias a` for a read-only loan, `move a` to hand it over (LR 7.5). A bare `let b = a` on a list is
rejected (`E-OWN-001`, LR 7.5):

```fortis
// reject: E-OWN-001
func main() {
  let a = List.of(1, 2, 3)
  let b = a                     // reject: E-OWN-001
}
```

Changing a value through a call is also visible. A function that may modify its argument declares the parameter
`editable`, and the caller writes `editable` too; a plain parameter can only be read (LR 7.6). Python's surprise
(a function that quietly appends to the list you passed) cannot happen without the word `editable` at the call:

```fortis
// accept
import fortis.host { Console }

func addDefaults(items: editable List[String]) {
  items.push("salt")
}

func count(items: List[String]) Int => items.count()

func main(console: Console) {
  let mut shopping: List[String] = List.of("bread")
  addDefaults(editable shopping)
  console.print("${count(shopping)} items")
}
```

```output
2 items
```

## Integers have a size

Python's integers grow without bound. `Int` is 64 bits, and arithmetic that might overflow is either proved safe
by the checker or says what happens: `a * b or fail` fails with `numeric.overflow`, `Int.wrappingMul` wraps,
`Int.saturatingMul` clamps (LR 6.4). `BigInt` is the unbounded
integer, and it allocates (LR 6.2).

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let big = 4611686018427387904
  let doubled = big * 2 catch e {
    console.print("overflow: ${e.code}")
    report e
    recover 0
  }
  console.print("saturating: ${Int.saturatingMul(big, 2)}")
}
```

```output
overflow: numeric.overflow
saturating: 9223372036854775807
```

## Classes without inheritance

A class has fields and methods, and no superclass (LR 9.1). What inheritance does in Python is split three ways:
an `interface` declares methods a type promises to have, a `mixin` contributes methods to the classes that apply
it with `with`, and a class implements an interface only by saying so (LR 9.5, LR 9.6). A mixin may require
methods but never adds fields, and its methods see only the class's exported members, so it never reads private
state (LR 9.6).

```fortis
// accept
mixin Greeting {
  requires pure func name(self) String
  export pure func greet(self) String => "hello, ${self.name()}"
}

class Person with Greeting {
  export first: String
  export pure func name(self) String => copy self.first
}

test "a mixin method uses a required method" {
  let p = Person(first: "Ada")
  expect p.greet() == "hello, Ada"
}
```

Fields and methods are private to their module unless marked `export` (LR 4.4). There is no `__init__`: every
class has a generated constructor taking each field by name, and a class that must validate provides an
associated function, conventionally `create`, `of` or `from` (LR 9.2, LR 1.1).

## Comprehensions are `map` and `filter`

`[x * 2 for x in xs if x > 0]` becomes calls to `filter` and `map` with lambdas (LR 6.7). Their callbacks must be
`pure`; a loop that does I/O is a `for` loop (LR 6.7). A lambda lists the variables it captures, with a marker
for each, and a trivial value such as an `Int` may be listed bare (LR 8.3).

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let readings: List[Int<-100..100>] = List.of(-5, 12, 0, 40)
  let floor = 0
  let positive = readings.filter(fn[floor](r: Int<-100..100>) Bool => r > floor)
  let doubled = positive.map(fn(r: Int<-100..100>) Int => r * 2)
  console.print("${doubled}")
}
```

```output
[24, 80]
```

## `print` needs a capability

There is no global `print` and no global `open`. Input and output go through capability values: `main` declares
the ones it needs as parameters, the runtime supplies them, and `main` passes them to the functions that need them
(LR 4.6, LR 11.6). A function that does I/O therefore names, in its signature, what it may touch. Diagnostics are
different: `log.info`, `log.warn` and `log.error`, the counterpart of Python's `logging`, write a log record to
standard error from any function, with no capability, and `report e` records a failure the same way (LR 13,
LR 10.6). Creating capabilities anywhere but `main`, as `Host.capabilities()` does in older code, is `E-EFF-010`,
and so is doing I/O in a function that takes no capability, so this attempt is reported twice (LR 11.6):

```fortis
// reject: E-EFF-010
import fortis.host { Host }

func say(message: StringView) {
  let caps = Host.capabilities()          // reject: E-EFF-010
  caps.console.print(message)             // reject: E-EFF-010
}

func main() {
  say("starting")
}
```

```fortis
// accept
import fortis.host { Console }

func say(console: Console, message: StringView) {
  console.print(message)
}

func main(console: Console) {
  say(console, "starting")
}
```

```output
starting
```

## Modules run nothing when imported

A Python module runs its top-level code on import. A Fortis module contains only declarations and constants, so
importing it executes nothing, and there is no `if __name__ == "__main__"`: a program's entry point is the
`main` of the entry module its manifest names (LR 4.2, LR 4.6).

## Tests are part of the language

`test` declarations live next to the code and need no framework; `cases { … } as row` is a table of named rows,
like `pytest.mark.parametrize`, and `property … forall` draws inputs from the parameter types (LR 14).
`expect condition` records the actual and expected values and fails the test when the condition is false (LR 14).

```fortis
// accept
pure func isAdult(age: Int<0..150>) Bool => age >= 18

test "adult boundary" cases {
  "below": { age: 17, expected: false }
  "equal": { age: 18, expected: true }
} as row {
  expect isAdult(row.age) == row.expected
}

property "adults are at least 18" forall (age: Int<0..150>) {
  expect !isAdult(age) || age >= 18
}
```

`fortis test file.ft` runs them (LR 14).
