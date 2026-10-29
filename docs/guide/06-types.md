# Types

<!-- covers: LR 6 -->
*Human mode. Agent mode: [Types, by rule](../agent/06-types.md). Source of truth: LR 6.*

> Preview: refinements (LR 6.3), newtypes (LR 6.9) and generics (LR 6.10) arrive in v0.1. Normalisation, case mapping and the collections beyond `List`, `Map` and `Set` (LR 6.6, LR 6.7) arrive in v0.4. `derive Default` (LR 6.10) is in v0.0.2.

Fortis is nominally typed: two types are the same only when they are the same declaration, whatever their
representation (LR 6.1). There is no top type, no `Any`, no implicit conversion between distinct types and no
way to change a type at run time. This chapter walks through the types the core gives you, how you narrow them
with refinements, and the few interfaces that the operators and string interpolation rely on.

## Nominal types

Every type is one of: a built-in type, a `type` newtype, a class, an enum, an interface used as a `dynamic`
type, a function type, a tuple or anonymous record, a refinement of a built-in type, a generic instantiation of
any of these, or a loan or view type (LR 6.1). "Nominal" means the name is what counts. Two newtypes over the
same representation never mix:

```fortis
// reject: E-TYP-040
type CustomerId = Int<1..>
type OrderId = Int<1..>

pure func describe(customer: CustomerId) Int => 1

pure func describeOrder(order: OrderId) Int {
  return describe(order)           // reject: E-TYP-040
}

func main() {
}
```

`OrderId` and `CustomerId` are both positive integers underneath, but a function that takes a customer cannot
be handed an order (LR 6.9). The checker reports the mismatch as `E-TYP-040`, the code the LR uses for a value
of the wrong type (LR 10.1), naming the expected and the actual type.

## The built-in types

The core's types, all in the implicitly imported prelude (LR 6.2, LR 4.3):

| Group | Types |
|---|---|
| Unit | `Void` (one value), `Never` (no value) |
| Boolean | `Bool` |
| Integers | `Int8` `Int16` `Int32` `Int64` `UInt8` `UInt16` `UInt32` `UInt64`; `Int` is `Int64`, `UInt` is `UInt64`, `Byte` is `UInt8` |
| Floating point | `Float32`, `Float` (64-bit) |
| Exact | `Decimal`, `BigInt` |
| Text | `String`, `StringView`, `Bytes`, `BytesView` |
| Time | `Duration`, `Instant`, `Timestamp` |
| Collections | `List[T]`, `Array[T, N]`, `ListView[T]`, `Map[K, V]`, `OrderedMap[K, V]`, `Set[T]`, `FixedList[T, N]`, `FixedMap[K, V, N]` |
| Compound | tuples `(A, B)`, anonymous records `{ name: A, other: B }`, variant maps `VariantMap[E, V]` |
| Optional | `T?` |
| Outcome | `Outcome[T]`, `Failure`, `Report` |
| Concurrency | `Future[T]`, `Task[T]`, `TaskCompletion[T]`, `Shared[T]`, `Weak[T]`, `Mutex[T]`, `AtomicInt`, `AtomicBool` |
| Callable | `fn(Params) Ret`, `fn mut(Params) Ret`, `fn once(Params) Ret` |

The `N` of `Array[T, N]` and `FixedList[T, N]` is an integer literal; functions generic over `N` are not in
Language 1.0 (LR 6.2). `List.of(a, b, c)` takes any number of arguments, a form reserved for the standard
library: your own functions take a `ListView[T]` instead (LR 6.2, LR 5.8).

## Refinements

A refinement narrows a built-in integer, text, byte or collection type with a predicate in `<…>` after the type
name, and it is allowed only where a type is written (LR 6.3):

| Form | Meaning |
|---|---|
| `Int<0..100>` | 0 to 100 inclusive |
| `Int<0..<100>` | 0 up to but not including 100 |
| `Int<1..>`, `Int<..0>` | one-sided |
| `String<scalars: 1..120>` | between 1 and 120 Unicode scalars |
| `Bytes<length: 0..65536>`, `List[T]<length: 0..100>` | element count |

A refinement is a subtype of its base and of every refinement whose range contains it, with no conversion at
all: an `Int<0..50>` can be used where an `Int<0..100>` or a plain `Int` is expected. The other direction never
happens on its own (LR 6.3).

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let small: Int<0..50> = 20
  let wide: Int<0..100> = small
  let any: Int = wide
  console.print("${small} ${wide} ${any}")
}
```

```output
20 20 20
```

To go from a wider type to a narrower one, ask: `Int<0..100>.validate(x)`, or `Percent.validate(x)` for a named
type, is a pure function that returns an outcome, a success when `x` is in range and a failure otherwise
(LR 6.3). That outcome must be handled like any other ([Outcomes and failures](10-outcomes.md)).

```fortis
// accept
import fortis.host { Console }

type Percent = Int<0..100>

try func main(console: Console) {
  let fine = Percent.validate(75) or fail
  console.print("accepted ${fine}")
  let tooBig = Percent.validate(140) catch e {
    console.print("rejected with ${e.code}")
    report e
    recover Percent(0)
  }
  console.print("fell back to ${tooBig}")
}
```

```output
accepted 75
rejected with refinement.out_of_range
fell back to 0
```

A literal is checked against the type it is given when the program is compiled: `140` does not fit
`Int<0..100>`, so the line is rejected with `E-REF-001` (LR 3.5, LR 6.3).

```fortis
// reject: E-REF-001
func main() {
  let score: Int<0..100> = 140      // reject: E-REF-001
}
```

Values that are not literals are proved too. The checker follows the possible range of every integer through a
function and accepts an assignment, an operator or an index only when it can show the result stays in range;
the rules it uses are the subject of [Proving arithmetic and indexing](06-proofs.md), and when they cannot show
a bound, `validate` and the checked operations below are the explicit fallback (LR 6.3).

Two refinements are not available. Floats cannot be refined, because NaN and rounding make ranges unreliable
(`E-TYP-020`, LR 6.3). And a collection refined by its length is a property of a value, so it may not be mutated:
change an unrefined collection and `validate` the result (`E-REF-003`, LR 6.3).

```fortis
// reject: E-TYP-020
type Ratio = Float<0..1>              // reject: E-TYP-020

func main() {
}
```

## Integer arithmetic

The integer operators `+ - * / % << >>` are proof-required: each is accepted only when the checker can show it
cannot overflow, divide by zero or shift out of range (LR 6.4). When you cannot or do not want to give that
proof, handle the failure instead: on `Int`, an operator the checker cannot prove, written as the left operand of
`or fail`, `or recover` or `catch`, is the checked operation, as in `big + 1 or recover -1` below (LR 6.4). `Int`
also has three families of named operations (LR 6.4); the sized integer types, `Int32`, `UInt8` and the rest, have
none yet and get theirs in v0.4:

| Family | Example | Result |
|---|---|---|
| Checked | `Int.add(a, b)`, `Int.sub`, `Int.mul`, `Int.div`, `Int.rem`, `Int.shiftLeft`; on `Int`, the operator under a handler | an outcome that fails with `numeric.overflow`, `numeric.divide_by_zero` or `numeric.shift_range` |
| Wrapping | `Int.wrappingAdd(a, b)`, … | always a value; wraps around in two's complement |
| Saturating | `Int.saturatingAdd(a, b)`, … | always a value; clamps at the type's bounds |

```fortis
// accept
import fortis.host { Console }

recovers func main(console: Console) {
  let big: Int = 9223372036854775807
  let checked: Int = big + 1 or recover -1
  console.print("checked ${checked}")
  console.print("wrapping ${Int.wrappingAdd(big, 1)}")
  console.print("saturating ${Int.saturatingAdd(big, 1)}")
  let q: Int = Int.div(-7, 2) or recover 0
  let r: Int = Int.rem(-7, 2) or recover 0
  console.print("-7 / 2 is ${q}, remainder ${r}")
}
```

```output
checked -1
wrapping -9223372036854775808
saturating 9223372036854775807
-7 / 2 is -3, remainder -1
```

Division truncates toward zero and the remainder takes the sign of the dividend; `>>` is arithmetic on signed
types and logical on unsigned ones; `a += b` means `a = a + b` and needs the same proof. None of this depends on
the build mode (LR 6.4). `main` is declared `recovers` here because `or recover` discards failures without
reporting them; [Effects and contracts](11-effects.md) explains why that has to be written down.

## Floating point and `Decimal`

`Float` and `Float32` follow IEEE 754 strictly: every operator is total (no overflow failure, no proof), the
compiler never reorders or fuses operations, and every NaN is canonicalised, so `+ - * /` and `Float.sqrt` give
the same bits on every backend (LR 6.5). A float displays as the shortest text that reads back as the same
value, always with a fractional part (LR 6.5):

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  console.print("${0.1 + 0.2} ${2e3} ${1.5}")
  let root = Float.sqrt(-1.0)
  console.print("${root} is NaN: ${Float.isNaN(root)}")
}
```

```output
0.30000000000000004 2000.0 1.5
NaN is NaN: true
```

Because NaN is not equal to itself, floats do not implement `Equatable`, `Hashable` or `Comparable` (LR 6.5).
`==` on two floats is still allowed and means IEEE equality, but a float cannot be a map key or a field of a
class that derives `Equatable` (`E-TYP-090`, LR 6.10); convert with `Float.totalOrder(x)` or compare bit patterns
with `Float.bitsEqual(a, b)` when you need those (LR 6.5).

```fortis
// reject: E-TYP-090
class Reading derive Equatable {    // reject: E-TYP-090
  export celsius: Float
}

func main() {
}
```

`Decimal` is exact: a sign, a 96-bit coefficient and a scale of 0 to 28 digits after the point (LR 6.5). It has no
NaN and no infinity, and in Language 1.0 it has no operators: arithmetic goes through the checked functions
`Decimal.add`, `sub`, `mul`, `div` and `rem`, which fail with `numeric.overflow` or `numeric.divide_by_zero`
(LR 6.5). A `Decimal` displays with exactly its scale, so `1.50d` prints as `1.50`. `add` and `sub` give the
larger operand scale, `mul` the sum of the scales and `div` the largest scale that fits; a result that would need
more than 28 fractional digits or more than 96 coefficient bits is rounded half to even, and if its integer part
still does not fit, the operation fails. Rounding to a scale of your choosing is explicit:
`Decimal.round(value, scale, mode)` with a `RoundingMode` (LR 6.5).

```fortis
// accept
import fortis.host { Console }

try func main(console: Console) {
  let price = 1.50d
  let total = Decimal.add(price, 2.25d) or fail
  let product = Decimal.mul(1.5d, 1.25d) or fail
  let rounded = Decimal.round(2.345d, 2, HalfEven)
  console.print("${price} ${total} ${product} ${rounded}")
}
```

```output
1.50 3.75 1.875 2.34
```

## Text and views

`String` is owned, growable UTF-8 text; `StringView` is a borrowed, read-only view of text owned by something
else; `Bytes` and `BytesView` are the byte equivalents (LR 6.6). A string literal is a `StringView` of text
stored in the program, so it can be used wherever a view is expected; where a `String` is expected, the literal
is copied into a new `String`. That copy is the one implicit conversion in Fortis, and because it allocates it
is refused in `noalloc` code. `String.from(view)` makes the copy explicit (LR 6.6).

A string with `${…}` holes is different: it builds a fresh `String` every time it runs, which a call may borrow
for the statement but which a binding declared as `StringView` cannot keep, because there would be nothing for
the view to borrow from (`E-TYP-040`, LR 3.5):

```fortis
// reject: E-TYP-040
func main() {
  let count = 3
  let label: StringView = "count ${count}"      // reject: E-TYP-040
}
```

There is no indexing of text by integer, because UTF-8 scalars have different widths. Instead there are separate
operations whose cost you can see (LR 6.6): `byteCount()` is constant time, `scalarCount()` walks the text,
`slice(from, to)` takes scalar offsets and returns an outcome (it fails on bad bounds), and `scalarAt(i)` returns
the scalar at an offset as an outcome. `text.Builder` accumulates text.

```fortis
// accept
import fortis.host { Console }
import fortis.text

try func main(console: Console) {
  let word: StringView = "naïve"
  console.print("${word.byteCount()} bytes, ${word.scalarCount()} scalars")
  let start = word.slice(0, 3) or fail
  let third = word.scalarAt(2) or fail
  let mut out = text.Builder.create()
  out.append(start)
  out.append(" / ")
  out.appendScalar(third)
  console.print(out.finish())
  let copied = String.from(word)
  console.print("an owned copy: ${copied}")
}
```

```output
6 bytes, 5 scalars
naï / ï
an owned copy: naïve
```

`ï` takes two bytes, which is why the word has six bytes but five scalars.

## Collections

`List[T]` is an owned, growable sequence; `Array[T, N]` has a fixed size; `ListView[T]` borrows a contiguous run
of elements; `Map[K, V]` and `Set[T]` hash their keys, which must be `Hashable` and `Equatable`; `OrderedMap`
needs `Comparable` keys; `FixedList` and `FixedMap` have a capacity fixed in the type and never allocate
(LR 6.7).

Reading an element by index with `list[i]` must be proved in bounds ([Proving arithmetic and
indexing](06-proofs.md)). Without a proof, `list.get(i)` returns an optional and `list.at(i)` returns an outcome
(LR 6.7). Here are the list operations you will use most:

```fortis
// accept
import fortis.host { Console }

try func main(console: Console) {
  let mut items = List.of(3, 1, 2)
  items.push(5)
  let last = items.pop() ?? 0
  let second = items.get(1) ?? 0
  let third = items.at(2) or fail
  console.print("${items.count()} items; popped ${last}, second ${second}, third ${third}")
  items.set(0, 10) or fail
  let doubled = items.map(fn(x: Int) Int => Int.saturatingMul(x, 2))
  let large = items.filter(fn(x: Int) Bool => x > 1)
  console.print("${doubled.count()} doubled, ${large.count()} above one, ${items.countWhere(fn(x: Int) Bool => x > 1)} counted")
  for x in items.reversed() {
    console.print("${x}")
  }
}
```

```output
3 items; popped 5, second 1, third 2
3 doubled, 2 above one, 2 counted
2
1
10
```

`map`, `filter` and `countWhere` take a `pure` callback; `map` and `filter` allocate the new collection they
return, while `countWhere` returns a count. `filter`, `reversed` and `concat` copy elements, so the element type
must be copyable (LR 6.7). Loops that do something for each element, such as printing,
are `for` loops (LR 6.7).

`Map` and `Set` use a hash function fixed for each compiler release and iterate in first-insertion order;
replacing a value keeps its key where it was. So their order is deterministic on every backend, and they may be
used in `pure` functions (LR 6.7):

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let mut ages: Map[String, Int] = Map.default()
  ages.set("brian", 41)
  ages.set("ada", 36)
  ages.set("brian", 42)
  console.print("${ages.count()} people, ada is ${ages.get("ada") ?? 0}")
  for entry in ages {
    console.print("${entry.0}: ${entry.1}")
  }
  let mut seen: Set[Int] = Set.default()
  let first = seen.add(4)
  let again = seen.add(4)
  console.print("${first} ${again} ${seen.contains(4)}")
}
```

```output
2 people, ada is 36
brian: 42
ada: 36
true false true
```

The fixed capacity of `FixedList` and `FixedMap` means inserting past it fails with `collection.full` rather
than allocating (LR 6.7).

Iterating a `Map` gives each entry as a `(key, value)` tuple, read through `entry.0` and `entry.1`. The LR names
`Map.iterate()` without saying what an entry is, and the library does not declare `iterate` yet (D-58, X-154),
so treat this as the toolchain's current behaviour. A `for` header binds one name, not a pattern (LR 5.3); to take
an entry apart, match it: `match entry { (name, age) => … }` (LR 9.8). Today's grammar does not accept a tuple
element after a marker, so `copy entry.0` is refused although LR 6.9 calls `t.0` a field (X-153); copy through
the match's bindings instead.

A list literal takes its element type from its elements: `List.of("pear", "apple")` is a
`List[StringView]`, because a string literal is a `StringView` (LR 3.5, LR 6.7). Annotate the binding when you
want owned strings: `let fruit: List[String] = List.of("pear", "apple")` (LR 6.6). A refinement is a subtype of
its base where a single value is used (LR 6.3); whether a `List[Int<0..9>]` may stand for a `ListView[Int]` the
LR does not say, and today's checkers say no (X-154), so convert with `map` when you need the wider element type.

## Optional values

`T?` is either `null` or a present `T` (LR 6.8). A `T` is accepted wherever a `T?` is expected; nothing goes the
other way without a test. `a ?? b` gives `a` when it is present and otherwise evaluates `b`; `a?.field` is `null`
when `a` is; and `match` uses the patterns `present(x)` and `null`.

Tests on `null` narrow the type for the code they guard (LR 6.8): inside `if n != null { … }` the place `n` is a
plain `T`; after `if n == null { return … }` it is a `T` for the rest of the block; and in `a != null && e`, `a`
is a `T` inside `e`.

```fortis
// accept
import fortis.host { Console }

class Address {
  export postcode: Int
}

class Person {
  export name: String
  export address: Address?
}

pure func describe(n: Int?) String {
  if n == null {
    return "nothing"
  }
  return "the number ${n}"
}

pure func postcodeOf(p: Person) Int => p.address?.postcode ?? 0

func main(console: Console) {
  console.print(describe(null))
  console.print(describe(4))
  let ada = Person(name: "Ada", address: Address(postcode: 10115))
  let bo = Person(name: "Bo", address: null)
  console.print("${postcodeOf(ada)} ${postcodeOf(bo)}")
  let maybe: Int? = null
  let label = match maybe {
    present(v) => "got ${v}"
    null => "got nothing"
  }
  console.print(label)
}
```

```output
nothing
the number 4
10115 0
got nothing
```

An optional is never an error and never a Boolean: `if maybe { … }` is `E-TYP-010` (LR 6.8). Absence, falsehood
and failure are three different things in Fortis, with three different operators: `??`, `||` and `or`
(LR 6.8, LR 1).

## Tuples, records, variant maps and newtypes

A tuple `(A, B)` is read with `.0` and `.1`; an anonymous record `{ name: A, other: B }` has named fields; a
`VariantMap[E, V]` maps every variant of an enum `E` to a value, written `{ Found: 200, NotFound: 404 }`, and the
checker insists on exactly one entry per variant (LR 6.9).

```fortis
// accept
import fortis.host { Console }

enum Lookup {
  Found
  NotFound
}

func main(console: Console) {
  let pair = (3, "three")
  let person = { name: "Ada", age: 36 }
  let statuses: VariantMap[Lookup, Int] = { Found: 200, NotFound: 404 }
  console.print("${pair.0} is ${pair.1}; ${person.name} is ${person.age}")
}
```

```output
3 is three; Ada is 36
```

Tuples, records and variant maps are local conveniences: they are structural and cannot appear in an exported
signature, where a named class says more (`E-TYP-030`, LR 6.9). The two exceptions are aspect parameters and the
signatures of the `fortis` package itself, such as `join`, which returns a tuple (LR 6.9). A type may be put in parentheses for grouping,
which is how an optional function is written: `(fn(Int) Bool)?` (LR 6.9).

```fortis
// reject: E-TYP-030
export pure func bounds() (Int, Int) => (1, 2)     // reject: E-TYP-030

func main() {
}
```

`type Name = Representation` declares a newtype, a new nominal type with a private representation (LR 6.9). It is
the only meaning `type` has: there are no transparent aliases in Language 1.0. A newtype value is made only by
`Name.validate(raw)` or by a literal the checker proves in range, written `Name(42)`. A newtype over a trivial
representation implements `Equatable`, `Hashable`, `Comparable` and `Displayable` exactly when its
representation does, displaying the underlying value (LR 6.9). So a newtype over `Int` can be interpolated and
used as a map key with no further declaration, while one over `Float` displays but cannot be a map key, because
`Float` is not `Hashable` (LR 6.5). The representation is private (LR 6.9): there is no accessor that turns a
`LineNo` back into an `Int`, and arithmetic on a newtype is not defined. Keep the raw value where you compute,
and make the newtype with `validate` (or a proved literal) where the value crosses into the part of the program
that should not mix it up with other numbers (LR 6.9).

## Generics

Functions, classes, enums and interfaces take type parameters in square brackets, constrained with `where`
after the return type (LR 6.10). A generic body is checked once, against its bounds, so a call at any type that
satisfies them cannot fail to compile.

```fortis
// accept
import fortis.host { Console }

pure func occurrences[T](items: List[T], wanted: T) Int where T implements Equatable {
  let mut n = 0
  for item in items {
    if item == wanted {
      n = Int.saturatingAdd(n, 1)
    }
  }
  return n
}

pure func larger[T](a: T, b: T) T where T implements Comparable, T implements Copyable {
  if a > b {
    return copy a
  }
  return copy b
}

func main(console: Console) {
  console.print("${occurrences(List.of(1, 2, 1), 1)} ${occurrences(List.of("a", "b"), "c")}")
  let pear: String = "pear"
  let apple: String = "apple"
  console.print("${larger(3, 8)} ${larger(pear, apple)}")
}
```

```output
2 0
8 pear
```

Type arguments are never written at a call: they are inferred from the arguments, or from the type the call's
result is expected to have, such as an annotated `let`, a return or a parameter (LR 6.10). When neither fixes
them, the call is `E-TYP-160`, and the repair is to annotate the binding:

```fortis
// reject: E-TYP-160
func main() {
  let names = List.default()        // reject: E-TYP-160
}
```

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let mut names: List[String] = List.default()
  names.push("ada")
  console.print("${names.count()}")
}
```

```output
1
```

A generic function is compiled once per type it is used at. Higher-kinded types, associated types and
overloading are not in Language 1.0 (LR 6.10).

## The built-in interfaces and `derive`

The operators and interpolation rest on a small set of interfaces in the prelude (LR 6.10):

| Interface | What it gives | Notes |
|---|---|---|
| `Equatable` | `==` and `!=` | |
| `Hashable` | use as a `Map` key or `Set` element | requires `Equatable` |
| `Comparable` | `<` `<=` `>` `>=` | requires `Equatable` |
| `Displayable` | `${value}` in a string | |
| `Iterable`, `Iterator`, `Consumable`, `Consumer` | `for` loops | LR 5.3 |
| `Copyable` | `copy` | structural: no declaration needed |
| `Shareable` | crossing to another thread | structural, LR 12.7 |
| `Abandonable` | being live when a task is cancelled | LR 12.4 |
| `Default` | `Type.default()` | |
| `Encodable`, `Decodable`, `Codec` | the format-free value tree | `Codec` is both |
| `Authority` | marks capabilities | LR 11.6 |
| `From[S]` | `Type.from(source)` and `source.to()` | |

There is no operator overloading: `Money + Money` is spelled `Money.add(a, b)`, and the comparison operators
dispatch only to these interfaces (LR 6.10).

`derive` on a class or enum generates `Equatable`, `Hashable`, `Comparable`, `Displayable`, `Encodable`,
`Decodable` or `Codec` field by field, and is accepted only when every field implements the interface
(`E-TYP-090` otherwise, LR 6.10). A derived `Comparable` orders by fields, and variants, in declaration order.

```fortis
// accept
import fortis.host { Console }

class Point derive Equatable, Displayable {
  export x: Int
  export y: Int
}

enum Size derive Equatable, Comparable, Displayable {
  Small
  Medium
  Large
}

func main(console: Console) {
  let p = Point(x: 1, y: 2)
  console.print("${p}, equal to itself: ${p == Point(x: 1, y: 2)}")
  console.print("${Size.Medium}; small before large: ${Size.Small < Large}")
}
```

```output
Point(x: 1, y: 2), equal to itself: true
Medium; small before large: true
```

`derive Default` builds the value from each field's declared initialiser, and from the field type's own default
only for fields without one (LR 6.10). It gives the class `Name.default()`, with no arguments, so a class whose fields
all start empty needs no hand-written constructor, and the header says so; a field whose type has no default, such
as a class that does not derive it or `Int<1..10>`, makes the derivation an error (`E-TYP-090`). `derive From[S]` copies every field of the target from the field of the
same name in `S`, which must exist and be copyable; then `Summary.from(customer)` builds the value, and
`customer.to()` does the same wherever the target type is already known (LR 6.10):

```fortis
// accept
class Customer {
  export name: String
  export age: Int
  export email: String
}

class Summary derive From[Customer] {
  export name: String
  export age: Int
}

test "a summary keeps the fields it names" {
  let ada = Customer(name: "Ada", age: 36, email: "ada@example.org")
  let summary: Summary = ada.to()
  expect summary.name == "Ada"
  expect summary.age == 36
}
```

## Encoding values: the canonical codec

A type that derives `Codec` (so `Encodable` and `Decodable`) gets two canonical encodings with no format package
at all (LR 6.10): `toText()` and `fromText(input)` read and write JSON with keys in declaration order and no extra
whitespace, and `toBytes()` and `fromBytes(input)` a deterministic binary form, identical on every backend.
Decoding follows the expected type and runs every refinement and constructor check. It fails with `codec.invalid`
for malformed input, a value that does not fit its type, a missing field that has neither an initialiser nor an
optional type, or trailing data, and with `codec.unknown_field` for a text key the type does not declare; a value
outside a refinement fails with that refinement's own failure, exactly as `validate` would (LR 6.10).

```fortis
// accept
import fortis.host { Console }

class Customer derive Codec, Equatable {
  export name: String
  export age: Int<0..150>
}

try func main(console: Console) {
  let ada = Customer(name: "Ada", age: 36)
  let json = ada.toText()
  console.print(json)
  let back = Customer.fromText(json) or fail
  console.print("round trip equal: ${back == ada}")
  let fromBinary = Customer.fromBytes(ada.toBytes()) or fail
  console.print("binary: ${fromBinary.name}")
  let broken = Customer.fromText("{\"name\": \"Bo\"}") catch e {
    console.print("decoding failed: ${e.code}")
    report e
    recover Customer(name: "nobody", age: 0)
  }
  console.print(broken.name)
}
```

```output
{"name":"Ada","age":36}
round trip equal: true
binary: Ada
decoding failed: codec.invalid
nobody
```

Deriving these interfaces needs every field to be exported and encodable in turn; secrets, capabilities, loans,
closures, futures and tasks implement none of them, so they can never end up in an encoded value by accident
(LR 6.10). `${value}` is a human rendering and is never the encoded form.

## Trivial and copyable types

A **trivial** type is duplicated by plain assignment, so it never needs an ownership marker (LR 6.11). The set is
closed: `Void`, `Never`, `Bool`, the integers, floats, `Decimal`, `Scalar`, `Duration`, `Instant`, `Timestamp`;
newtypes over those; enums whose payloads are all trivial; optionals, tuples and records of trivial types; and a
class you declare `trivial`, which is rejected if any field is not trivial (`E-OWN-030`, LR 6.11).

```fortis
// accept
import fortis.host { Console }

trivial class Pixel derive Equatable {
  export x: Int
  export y: Int
}

func main(console: Console) {
  let p = Pixel(x: 1, y: 2)
  let q = p
  console.print("${q == p}")
}
```

```output
true
```

```fortis
// reject: E-OWN-030
trivial class Tag {                 // reject: E-OWN-030
  export name: String
}

func main() {
}
```

Everything else is ownership-bearing and needs `copy`, `alias` or `move` when an existing value is assigned,
returned or handed over; a class is ownership-bearing unless declared `trivial`, so adding a field never changes
what an assignment elsewhere means (LR 6.11). [Ownership](07-ownership.md) covers those markers.

A **copyable** type supports `copy`: trivial types, `String`, `Bytes`, `BigInt`, collections of copyable
elements, and classes, enums, tuples and records whose fields are all copyable (LR 6.11). Shared handles,
mutexes, atomics, loans, resources, futures, tasks, outcomes, failures and closures are not, and `copy` of one is
`E-OWN-002` (LR 7.5). A copy is field by field, cannot fail, and allocates when the value owns heap memory, so it
is refused inside `noalloc` code (LR 6.11).

## Linear types

A **linear** value must be used up exactly once on every path (LR 6.12). The linear types are `Outcome[T]`,
`Failure`, `Future[T]`, `Task[T]`, the zero-sized witness `Must`, closures declared `fn once` that capture one,
and anything that can contain one: an optional, class, enum, tuple, record or collection over a linear type.
Dropping one at the end of a block, overwriting it or `discard`ing it is `E-OBL-001` (LR 6.12). The one
exception is a future that was never polled: `discard` of it is allowed when its captured environment holds no
linear value and no exclusive loan (LR 12.3, [Cleanup, async and concurrency](12-async.md)):

```fortis
// reject: E-OBL-001
func main() {
  let token = Must.create()         // reject: E-OBL-001
}
```

```fortis
// accept
func main() {
  let token = Must.create()
  Must.discharge(move token)
}
```

`Must` exists so that a library can make a handle linear: a transaction that holds a private `Must` field cannot
be forgotten, only committed or rolled back (LR 6.12). Most linear values you meet are outcomes, and
[Outcomes and failures](10-outcomes.md) shows every way to consume them.

A type parameter is not linear unless it is declared `T: linear`. That matters only where a generic function
holds a `T` by value; a function that only borrows its `T` can be used with linear types freely, because the
obligation stays with the caller (LR 6.12).

## Looking up a text key

`Map.get` only reads its key, so a `Map[String, V]` takes the key as a `StringView`: a view of some text, a literal
or a `String` all look the entry up, and nothing is copied to make a key that is thrown away (LR 6.7, X-345).
`set` still takes an owned `String`, because the map keeps that one:

```fortis
// accept
pure func countOf(counts: Map[String, Int], word: StringView) Int {
  return counts.get(word) ?? 0
}

test "a view finds the entry" {
  let mut counts: Map[String, Int] = Map.default()
  counts.set("cat", 2)
  let word: StringView = "cat"
  expect countOf(counts, word) == 2
  expect countOf(counts, "dog") == 0
}
```
