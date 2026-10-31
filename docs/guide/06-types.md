# Types

<!-- covers: LR 6 -->
*Human mode. Agent mode: [Types, by rule](../agent/06-types.md). Source of truth: LR 6.*

> Preview: normalisation, Unicode case mapping and the collections `OrderedMap`, `FixedList`, `FixedMap` and
> `VariantMap` beyond their syntax (LR 6.6, LR 6.7) arrive in v0.4. Refinements (LR 6.3), newtypes (LR 6.9),
> generics (LR 6.10) and conversions (LR 6.13) are in v0.1; `derive Default` (LR 6.10) is in v0.0.2.

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

That subtyping stops at the type itself: the type arguments of a container, a view, an optional, a tuple or any
other constructed type are **invariant** in their refinement, so a `List[Int<0..5>]` is not a `List[Int]`,
whether it is lent, moved or copied (`E-TYP-040`), since through an alias with the wider element type a 9 could
be stored. The other direction is a proof, not a subtype either: a `List[Int]` where a `List[Int<0..5>]` is
expected is `E-REF-001` (unproved) unless every element is proved, which for a value no literal built never is,
so validate element by element (LR 6.3, X-514).

```fortis
// reject: E-TYP-040
func main() {
  let small: List[Int<0..5>] = List.of(1, 2)
  let any: List[Int] = move small     // reject: E-TYP-040 — type arguments are invariant
}
```

To go from a wider type to a narrower one, ask: `x as Int<0..100>` is the pure fallible conversion of a base
type into a refinement of it, of type `Outcome[Int<0..100>]`, failing with `refinement.out_of_range` (LR 6.13);
`raw as Percent` for a named type is total when the checker proves the representation's range and fallible
otherwise (LR 6.9). The older call spelling `Int<0..100>.validate(x)` remains accepted in v0.1 and `fortis fmt`
rewrites it to the `as` form. The outcome must be handled like any other
([Outcomes and failures](10-outcomes.md)).

```fortis
// accept
import fortis.host { Console }

type Percent = Int<0..100>

try func main(console: Console) {
  let fine = 75 as Percent
  console.print("accepted ${fine}")
  let tooBig = 140 as Percent catch e {
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
a bound, the `as` conversion and the checked operations below are the explicit fallback (LR 6.3).

Each bound is an integer literal or a `const` of the base type, resolved at compile time (LR 6.3, X-512): a
constant whose initialiser is an integer literal, another such constant, or `-`, parentheses and `+ - * / %`
over them, named plainly or through a module alias.

```fortis
// accept
const top: Int = 100

pure func clamped(n: Int<0..top>) Int => n

test "a const bound is folded at compile time" {
  expect clamped(80) == 80
}
```

A refinement is well formed only in the forms the table lists: a range over an integer type, `scalars` over
`String` or `StringView`, `length` over `Bytes`, `BytesView`, `List[T]` or `ListView[T]`. Anything else is
rejected with `E-TYP-020`, read as "not a refinement of this base" (LR 6.3): a float base (`Float<0..1>`, not
supported in v1, because NaN and rounding make ranges unreliable), a base the table does not name (`Bool<0..1>`,
a newtype, a type parameter), a measure the base lacks (`String<length: 0..3>`), a bound that is not resolved at
compile time, and a range that is empty or inverted (`Int<5..1>`, `Int<0..<0>`) or lies outside the base's own
range (`UInt8<0..300>`), or, for a measure, outside `0..`.

```fortis
// reject: E-TYP-020
type Ratio = Float<0..1>              // reject: E-TYP-020 — a float cannot be refined
type Empty = Int<5..1>                // reject: E-TYP-020 — the range is inverted
type Wide = UInt8<0..300>             // reject: E-TYP-020 — outside UInt8's own range

func main() {
}
```

A collection refined by its length is a property of a value, so it may not be mutated: change an unrefined
collection and convert the result with `as` (`E-REF-003`, LR 6.3).

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
is refused in `noalloc` code. `view as String` makes the allocation explicit (LR 6.13); `String.from(view)` is
retired, remains accepted in v0.1 and is rewritten by `fortis fmt` (LR 6.6).

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
  let copied = word as String
  console.print("an owned copy: ${copied}")
}
```

```output
6 bytes, 5 scalars
naï / ï
an owned copy: naïve
```

`ï` takes two bytes, which is why the word has six bytes but five scalars.

### Text essentials

`String` and `StringView` both have the everyday text methods below, so a program trims and splits an owned
string or a view alike; none of them can fail, and none indexes by integer (LR 6.6, X-595). Positions are scalar
offsets, the unit `slice` and `scalarAt` take, and "whitespace" is the Unicode `White_Space` property:

| Method | Result |
|---|---|
| `trim()`, `trimStart()`, `trimEnd()` | `StringView from self`: the text without its leading and trailing, leading only, or trailing only whitespace; a view, no allocation |
| `find(needle)` | `Int<0..>?`: the scalar offset of the first occurrence, `null` if there is none; the empty needle is found at 0 |
| `contains(needle)` | `Bool`: `find(needle) != null` |
| `startsWith(prefix)`, `endsWith(suffix)` | `Bool`; the empty prefix or suffix always matches |
| `split(separator)` | `List[String]`: the pieces between the non-overlapping occurrences of the separator, left to right; allocates |
| `lines()` | `List[String]`: the lines of the text; allocates |
| `replace(old, new)` | `String`: a copy with every non-overlapping occurrence of `old`, left to right, replaced by `new`; allocates |
| `asciiUpper()`, `asciiLower()` | `String`: a copy with the ASCII letters mapped, every other scalar unchanged; allocates |
| `repeat(times)` | `String`: the text written `times` times in a row; allocates |
| `String.join(parts, separator)` | `String`: the parts in order with the separator between neighbours; allocates |

`split` keeps every piece, so a separator at either end, or two in a row, gives an empty piece there, and a
text without the separator is one piece (itself). A line ends at a line feed, and a carriage return directly
before it ends with it (`\r\n`); the text after the last terminator is a line only when it is not empty, so
`"".lines()` is empty and `"a\n".lines()` is one line. `trim`, `trimStart` and `trimEnd` return views, so the
result is a loan of its text and cannot outlive its owner (LR 7.6); `split` and `lines` return owned strings,
because a `List` of views would store loans, which Language 1.0 does not have (LR 7.7). Case mapping is ASCII
only in v0.1: normalisation and full Unicode case mapping arrive in a later release (LR 6.6, X-596). Parsing is
not a text method: `text as Int or fail` is the conversion of LR 6.13.

```fortis
// accept
import fortis.host { Console }

try func main(console: Console) {
  let line: StringView = "  large, 2  "
  let parts = line.trim().split(",")
  console.print("${parts.count()} parts")
  let size = parts.get(0) ?? ""
  let count = (parts.get(1) ?? "").trim() as Int or fail
  console.print("${size} x ${count}")
  console.print("${"tea".startsWith("t")} ${"tea".endsWith("a")} ${"tea".contains("e")} ${"tea".find("a") ?? 0}")
  let joined = String.join(List.of("a" as String, "b" as String), "-")
  console.print(joined.asciiUpper())
  console.print("${"ab".repeat(3)} / ${" a b ".replace(" ", "")}")
  for word in "one\ntwo".lines() {
    console.print(word)
  }
}
```

```output
2 parts
large x 2
true true true 2
A-B
ababab / ab
one
two
```

## Collections

`List[T]` is an owned, growable sequence; `Array[T, N]` has a fixed size; `ListView[T]` borrows a contiguous run
of elements; `Map[K, V]` and `Set[T]` hash their keys, which must be `Hashable` and `Equatable`; `OrderedMap`
needs `Comparable` keys; `FixedList` and `FixedMap` have a capacity fixed in the type and never allocate
(LR 6.7). Two more collection types are written in Fortis in `fortis.collections` and are imported from it:
`Deque[T]`, a double-ended queue, and `SortedSet[T]`, a set kept in ascending order (LR 6.7).

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
must be copyable (LR 6.7).

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

Iterating a `Map` gives each entry as a `(key, value)` tuple, read through `entry.0` and `entry.1`; a `for`
header binds one name, not a pattern (LR 5.3), so to take an entry apart, match it: `match entry { (name, age)
=> … }` (LR 9.8). Neither `Map` nor `Set` has an `iterate()` method or a cursor type of its own in v0.1
(LR 6.7, D-58, D-535). Today's grammar does not accept a tuple element after a marker, so `copy entry.0` is
refused although LR 6.9 calls `t.0` a field (X-153); copy through the match's bindings instead.

A list takes its element type from its elements: `List.of("pear", "apple")` would infer
`List[StringView]`, which stores loans and is rejected (`E-OWN-053`, LR 7.7). Annotate the binding for
owned strings: `let fruit: List[String] = List.of("pear", "apple")` (LR 6.6). A refinement is a subtype of
its base where a single value is used, but type arguments are invariant in their refinement, so a
`List[Int<0..9>]` may not stand for a `ListView[Int]` (`E-TYP-040`, LR 6.3, X-514); convert with `map` when you
need the wider element type.

## Collections in both styles

Beside the `for` loops and cursors, which stay, `List[T]` and `ListView[T]` have the functional methods below,
and `Map` and `Set` have `each` (LR 6.7, X-479). They are written in Fortis in the standard library over a
`for` loop, so every backend has them. A callback that only computes is a `pure fn`, and a lambda that does I/O
cannot be passed to one (`E-TYP-040`); `each` and `tryEach` take a callback that may have effects, an `editable
fn mut`, so a lambda that changes a captured binding qualifies. Because a callback parameter contributes the
effects its function type permits, a `pure` function cannot call `each` or `tryEach`, whatever lambda it passes
(`E-EFF-001`, LR 6.7, LR 11.3). Every method that returns elements (`take`, `skip`, `enumerate`, `zip`,
`chunked`, `sortedBy`, `groupBy`, `keys`, `values` and the set operations) requires `Copyable` elements, as
`filter` does; a method that only looks at them (`each`, `fold`, `any`, `all`, `find`, `indexWhere`, `contains`)
does not, so a list of resources can be visited, folded and searched.

```fortis
// illustrative
…
// List[T] and ListView[T]
func each(self, visit: editable fn mut(T) Void)
try func tryEach(self, visit: editable fn mut(T) Outcome[Void])
pure func map[U](self, f: pure fn(T) U) List[U] where U: linear
pure try func tryMap[U](self, f: pure fn(T) Outcome[U]) List[U]
pure func filter(self, keep: pure fn(T) Bool) List[T] where T implements Copyable
pure func fold[A](self, initial: owned A, step: pure fn(A, T) A) A
pure try func tryFold[A](self, initial: owned A, step: pure fn(A, T) Outcome[A]) A
pure func any(self, holds: pure fn(T) Bool) Bool
pure func all(self, holds: pure fn(T) Bool) Bool
pure func find(self, holds: pure fn(T) Bool) (alias T from self)?
pure func indexWhere(self, holds: pure fn(T) Bool) Int?
pure func contains(self, target: T) Bool where T implements Equatable
pure func sortedBy[K](self, key: pure fn(T) K) List[T] where T implements Copyable, K implements Comparable
pure func enumerate(self) List[(Int, T)] where T implements Copyable
pure func zip[U](self, other: ListView[U]) List[(T, U)] where T implements Copyable, U implements Copyable
pure func take(self, n: Int) List[T] where T implements Copyable
pure func skip(self, n: Int) List[T] where T implements Copyable
pure func flatMap[U](self, f: pure fn(T) List[U]) List[U] where U: linear
pure func chunked(self, size: Int<1..>) List[List[T]] where T implements Copyable
pure func groupBy[K](self, key: pure fn(T) K) Map[K, List[T]] where T implements Copyable, K implements Copyable, K implements Hashable, K implements Equatable
// Map[K, V]
func each(self, visit: editable fn mut(K, V) Void) where K implements Hashable
pure func keys(self) List[K] where K implements Hashable, K implements Copyable
pure func values(self) List[V] where K implements Hashable, V implements Copyable
// Set[T]
func each(self, visit: editable fn mut(T) Void) where T implements Hashable
pure func union(self, other: Set[T]) Set[T] where T implements Hashable, T implements Copyable
pure func intersection(self, other: Set[T]) Set[T] where T implements Hashable, T implements Copyable
pure func difference(self, other: Set[T]) Set[T] where T implements Hashable, T implements Copyable
```

Each method's callback sees its element by read loan (LR 6.7), so a method that builds a new value from an element
copies it and says so in its signature. `find` lends the first element the callback accepts (`null` when none
does) and `indexWhere` gives its index; `any`, `all`, `find`, `indexWhere`, `contains` and the `try` methods
stop early. `fold` passes the accumulator by read loan and takes the next one from the callback's result, so it
is for values the callback builds anew (a sum, a longer text); accumulating into a collection is `each` over a
`let mut`. `sortedBy` is a stable merge sort over the keys, which it computes once each, so equal keys keep
their order; `groupBy` gathers copies under their key in a `Map` in first-appearance order; `take(n)` and
`skip(n)` never fail (an `n` of 0 or less empties `take` and keeps everything for `skip`, an `n` past the end keeps everything for `take` and empties
`skip`); `chunked(size)` gives lists of `size` elements, the last one shorter, and its size is an `Int<1..>`, so
`chunked(0)` is `E-REF-001`.

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let names: List[String] = List.of("ada", "bo", "cy", "dee")
  let long = names.filter(fn(n) => n.scalarCount() > 2)
  let shouted = names.map(fn(n) => n.asciiUpper())
  let total = names.fold(0, fn(sum, n) => Int.saturatingAdd(sum, n.scalarCount()))
  console.print("${long.count()} long, ${shouted.first() ?? "none"}, ${total} letters")
  console.print("any short? ${names.any(fn(n) => n.scalarCount() < 3)}, all short? ${names.all(fn(n) => n.scalarCount() < 3)}")
  let found = names.find(fn(n) => n == "cy") ?? "none"
  let idx = names.indexWhere(fn(n) => n == "cy") ?? -1
  console.print("found ${found} at ${idx}, has bo: ${names.contains("bo")}")
  let sorted = names.sortedBy(fn(n) => copy n)
  let pairs = names.enumerate()
  let zipped = names.zip(sorted)
  console.print("sorted ${sorted.first() ?? "none"}, ${pairs.count()} enumerated, ${zipped.count()} zipped")
  console.print("took ${names.take(2).count()}, skipped ${names.skip(3).count()}, chunks ${names.chunked(3).count()}")
  let byLength = names.groupBy(fn(n) => n.scalarCount())
  console.print("${byLength.count()} lengths, flat ${names.flatMap(fn(n) => n.split("")).count()}")
  let mut printed = 0
  names.each(fn(n) {
    printed = Int.saturatingAdd(printed, 1)
  })
  console.print("visited ${printed}")
}
```

```output
2 long, ADA, 10 letters
any short? true, all short? false
found cy at 2, has bo: true
sorted ada, 4 enumerated, 4 zipped
took 2, skipped 1, chunks 2
2 lengths, flat 4
visited 4
```

The `try` methods take a callback that returns an outcome, which is what a lambda whose body is a call of a
`try` function, or a fallible `as`, has when it is written as it reads: `lines.tryMap(fn(line) => line as Int)
or fail`. They stop at the first failure, in element order, and return it; the later elements are not visited,
and a list `tryMap` was building is dropped. They are `try` functions, so their call is handled on the line
([Outcomes and failures](10-outcomes.md)); a callback that returns a plain value is no `try` callback
(`E-TYP-040`, naming the `Outcome[…]` it must return, LR 6.7).

```fortis
// accept
import fortis.host { Console }

try func main(console: Console) {
  let texts: List[String] = List.of("1", "2", "3")
  let numbers = texts.tryMap(fn(t) => t as Int) or fail
  console.print("parsed ${numbers.count()} numbers, first ${numbers.first() ?? 0}")
  let sum = numbers.tryFold(0, fn(a, n) => Int.add(a, n)) or fail
  console.print("sum ${sum}")
  let bad: List[String] = List.of("4", "five", "6")
  let parsed = bad.tryMap(fn(t) => t as Int) catch e {
    console.print("stopped at ${e.code}")
    report e
    recover List.default()
  }
  console.print("recovered with ${parsed.count()}")
}
```

```output
parsed 3 numbers, first 1
sum 6
stopped at conversion.parse
recovered with 0
```

`Map.each` and `Set.each` visit in first-insertion order; `keys` and `values` copy; `union` gives self's items
and then other's that self lacks, and `intersection` and `difference` keep self's order, each building a new set
(LR 6.7).

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let mut ages: Map[String, Int] = Map.default()
  ages.set("ada", 36)
  ages.set("bo", 41)
  ages.each(fn(name, age) => console.print("${name} is ${age}"))
  console.print("${ages.keys().count()} keys, ${ages.values().count()} values")
  let mut seen: Set[Int] = Set.default()
  discard seen.add(1)
  discard seen.add(2)
  let mut other: Set[Int] = Set.default()
  discard other.add(2)
  discard other.add(3)
  console.print("union ${seen.union(other).count()}, both ${seen.intersection(other).count()}, only ${seen.difference(other).count()}")
}
```

```output
ada is 36
bo is 41
2 keys, 2 values
union 3, both 1, only 1
```

A double-ended queue and a sorted set are written in Fortis in `fortis.collections` and imported from it
(LR 6.7, X-576, X-577). A `Deque[T]` pushes and pops at both ends in amortised constant time and holds
resources; a `SortedSet[T]` keeps its members in ascending order, each once, ordered by `compare`, so it needs
`T implements Comparable`. Neither is `Copyable` (`copy` of one is `E-OWN-002`). `toList()` copies the items;
`fromList(items: owned List[T])` consumes an input list. Neither accepts linear elements in v0.1. A `for` over either walks it in its order (front to back for the queue, smallest to largest
for the set), as each method does, because both implement `Iterable` and a `for` over any class that implements
`Iterable[T, I]` walks the cursor its `iterate()` returns (LR 5.3, LR 6.7).

```fortis
// accept
import fortis.host { Console }
import fortis.collections { Deque, SortedSet }

func main(console: Console) {
  let mut queue: Deque[String] = Deque.default()
  queue.pushBack("bo" as String)
  queue.pushFront("ada" as String)
  queue.pushBack("cy" as String)
  console.print("front ${queue.peekFront() ?? "none"}, back ${queue.peekBack() ?? "none"}, ${queue.count()} waiting")
  for name in queue {
    console.print("serving ${name}")
  }
  let sizes = SortedSet.fromList(List.of(3, 1, 2, 1))
  console.print("${sizes.count()} sizes, first ${sizes.first() ?? 0}, between 1 and 3: ${sizes.between(1, 3).count()}")
  sizes.each(fn(s) => console.print("size ${s}"))
}
```

```output
front ada, back cy, 3 waiting
serving ada
serving bo
serving cy
3 sizes, first 1, between 1 and 3: 2
size 1
size 2
size 3
```

An `Array[T, N]` reads an element by `get(index)`, an optional that is `null` outside `[0, N)`, and lends its
elements as a view by `asView()`, which has every method a `ListView` has (LR 6.7, X-578).

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let grid = Array[Int, 3].filled(7)
  console.print("${grid.get(1) ?? 0}, ${grid.get(5) ?? -1}")
  let bumped = grid.asView().map(fn(n) => Int.saturatingAdd(n, 1))
  console.print("${bumped.first() ?? 0} bumped")
}
```

```output
7, -1
8 bumped
```

## Keys of maps and sets

Every written or inferred `Set[T]` and `Map[K, V]` requires its element or key type to implement both `Hashable`
and `Equatable`; an unmet requirement is `E-TYP-090`, even for an empty container (LR 6.7, X-615, D-549). The
check applies inside nested types, to fields, parameters and results, and after generic instantiation. In
generic code the requirement must follow from the declared `where` bounds: `where K implements Hashable`
suffices, because `Hashable` requires `Equatable` (LR 6.10). A class has no interface `where` bound in v0.1
(`where T: linear` declares linearity only, LR 6.12), so a field `Map[K, V]` whose `K` is the class's unbounded
parameter is rejected; use a concrete key type or a type parameter for the whole container. Triviality and
copyability (LR 6.11) do not imply either interface, and tuples and anonymous records do not satisfy interfaces
(LR 6.9), so they are not keys; an enum whose payloads are all trivial is still not a key unless it derives or
implements them.

To make a type a key, `derive Equatable, Hashable` (accepted when every field or payload implements them), or
write `equals` and `hash` by hand. A hand-written `hash` writes nothing, because `Hasher` has no members in
Language 1.0, so every value of the type hashes alike and the container decides membership by `equals` alone
(LR 6.10, X-516). A newtype over a `Hashable`, `Equatable` representation is a key with no declaration, because
it forwards both (LR 6.9). Core `hash` is not a callable library method; use the containers for hashing
and the ordinary `equals` and `compare` methods for direct comparisons (X-516, LR 6.10).

```fortis
// accept
class Sku derive Equatable, Hashable {
  export code: String
}

test "a derived key finds its entry" {
  let mut stock: Map[Sku, Int] = Map.default()
  stock.set(Sku(code: "A1"), 3)
  expect (stock.get(Sku(code: "A1")) ?? 0) == 3
  expect stock.containsKey(Sku(code: "A1"))
}
```

```fortis
// reject: E-TYP-090
class Name {
  export text: String
}

func registry() Set[Name] => Set.default()   // reject: E-TYP-090 — Name is not Hashable and Equatable

func main() {
}
```

Reading traversal methods retain the elements' obligations in the collection. Bind a fresh linear list before
borrowing it, then consume it. A non-failing `map` may produce linear elements, but `tryMap` results and fold
accumulators must be non-linear; `filter` requires Copyable elements (LR 10.7). See
[linear containers and consuming loops](10-outcomes.md#consuming-every-loop-item).

## Optional values

`T?` is either `null` or a present `T` (LR 6.8). A `T` is accepted wherever a `T?` is expected; nothing goes the
other way without a test. `a ?? b` gives `a` when it is present and otherwise evaluates `b`; `a?.field` is `null`
when `a` is; and `match` uses the patterns `present(x)` and `null`.

`a?.method(args)` calls a method of the present value (LR 6.8). It evaluates `a` once; if absent, it returns
`null` without evaluating any arguments. A result `U` becomes `U?`, and an already optional result stays
optional without another layer. Chains such as `a?.child()?.label()` apply this rule at each call, including
mixin and interface methods. In v0.1 the method must take a read receiver and must not be `try`. For an
editable, consuming or fallible method, test for null first, call with the required receiver mode, and handle
its outcome. The method's effects still count in the caller even when the receiver might be absent.

A returned loan or view retains the receiver's owner through the optional and any following calls, so the
ordinary lifetime and conflict rules still apply (LR 7.6, LR 7.7). `maybe.map(fn(x) => ...)` applies an arbitrary
pure function to a present value; `?.` calls its member. The formatter keeps either spelling as written.

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

### Newtypes

`type Name = Representation` declares a newtype, a new nominal type with a private representation (LR 6.9). It
is the only meaning `type` has: there are no transparent aliases in Language 1.0.

A newtype value is made in three ways (LR 6.9, LR 6.13):

- `raw as Name`, the conversion: pure, and fallible, an outcome failing with `refinement.out_of_range`, unless
  the checker proves the representation's range, in which case it gives a `Name` directly.
- `Name(x)`, judged as a call of `func Name(value: Representation) Name` (LR 6.9): `x` is a literal the checker proves
  in range (`Name(42)`), a value of the representation type, or a value whose interval the proof rules show
  within the representation's refinement. A literal or value the rules do not prove is `E-REF-001` (`Percent(140)`
  for `type Percent = Int<0..100>`, or `Small(n)` for an `Int` nothing has narrowed), and a value of another
  type is `E-TYP-040` (`Small("five")`).
- the retired spelling `Name.validate(raw)`, which stays accepted in v0.1 and keeps its outcome result;
  `fortis fmt` rewrites it to the `as` form (LR 6.13).

A newtype implements `Equatable`, `Hashable`, `Comparable` and `Displayable` exactly when its representation
does, whatever the representation, and display shows the underlying value (LR 6.9, X-511). So a newtype over
`Int` can be interpolated and used as a map key with no further declaration, and so can a `type Code =
String<scalars: 1..10>`; one over `Float` displays but cannot be a map key, because `Float` is not `Hashable`
(LR 6.5). A newtype over a trivial representation is itself trivial (LR 6.11).

Out again goes through `as`, inside the declaring module only (LR 6.9, X-552): `p as Int` unwraps a `Pence`
totally and has type `Int<0..>`, keeping the representation's refinement even when the target writes only its
base. Outside that module the same unwrap is `E-NAM-004`. An unwrap is one step to the declared representation,
never a search through nested newtypes, and a conversion cannot turn a loan into an owner: an
ownership-bearing representation needs `move` or `copy` on a place in either direction (LR 7.5). Newtypes
declare no conversion members (LR 6.13).

```fortis
// accept
import fortis.host { Console }

type Pence = Int<0..>

try func main(console: Console) {
  let paid = 42 as Pence                      // proved: total, no outcome
  let raw = Int.saturatingAdd(400, 300)       // a plain Int: its range is not proved
  let alsoPaid = raw as Pence or fail         // unproved: an outcome, handled on the line
  console.print("${paid as Int} ${alsoPaid as Int}")   // unwrapping: this is the declaring module
}
```

```output
42 700
```

```fortis
// reject: E-REF-001, E-TYP-040
type Percent = Int<0..100>

func main() {
  let big = Percent(140)          // reject: E-REF-001 — 140 is not proved in range
  let word = Percent("forty")     // reject: E-TYP-040 — a String is not the representation
  discard big
  discard word
}
```

A representation that is linear (LR 6.12), including a collection of linear elements, may be wrapped only when
the refinement is proved: an unproved `as` or `validate` call is `E-OBL-001`, because its failure would discard
the representation without consuming its obligations (LR 10.7). A proved `as` remains total, and an unwrap
transfers the obligations to the representation (LR 6.9).

### Newtypes over collections

A newtype whose representation is a collection offers that collection's instance methods, with the
representation's type arguments and result types (LR 6.9, X-581). A reading method (`self`) is visible outside
the declaring module; an editing or consuming method (`editable self`, `owned self`) is visible only inside it,
and a call outside is `E-NAM-004`, naming the method. Reading iteration (`for item in basket`) follows the
representation too; consuming iteration is private like a consuming method. A method returning `List[T]`
returns `List[T]`, never the newtype. The rule looks through a refinement of a collection, but not through
another newtype. It exposes no associated functions, fields, indexing, conversion or interchange with the
representation: construction stays `Name(x)` or `raw as Name`, and the private unwrap is unchanged.

```fortis
// accept
import fortis.host { Console }

type Basket = List[String]

func main(console: Console) {
  let mut basket = Basket(List.of("coffee" as String, "tea" as String))
  basket.push("cake" as String)          // allowed: this is the declaring module
  basket.each(fn(item) => console.print(item))
  for item in basket {
    discard item
  }
  console.print("${basket.count()} items, first ${basket.first() ?? "none"}")
}
```

```output
coffee
tea
cake
3 items, first coffee
```

From another module, the same `basket.push(…)` is refused, while the reading methods keep working:

```yaml
# file: guide.stock/project.yaml
schema: fortis.project/v1
name: guide.stock
version: 1.0.0
entry: app
exports: []
dependencies:
  fortis: "1.0"
```

```fortis
// reject: E-NAM-004
// file: guide.stock/src/basket.ft
export type Basket = List[String]

export func empty() Basket => Basket(List.default())
```

```fortis
// reject: E-NAM-004
// file: guide.stock/src/app.ft
import fortis.host { Console }
import guide.stock.basket { empty }

func main(console: Console) {
  let mut basket = empty()
  basket.push("coffee" as String)      // reject: E-NAM-004 — editing only inside the declaring module
  console.print("${basket.count()} items")
}
```

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
  console.print("${occurrences(List.of(1, 2, 1), 1)} ${occurrences(List[String].of("a", "b"), "c" as String)}")
  let pear: String = "pear"
  let apple: String = "apple"
  console.print("${larger(3, 8)} ${larger(pear, apple)}")
}
```

```output
2 0
8 pear
```

Type arguments are never written at a call: there is no `f[Int](x)` syntax in v1 (a lowercase name followed by
`[` is an index); they are always inferred (LR 6.10). Without an expected type, an argument of the parameter's
type fixes it, and arguments that disagree are joined to their common type, two integer refinements of one base
to the hull of their intervals (`pick(small, big)` with `small: Int<0..3>` and `big: Int<0..9>` is an
`Int<0..9>`) and a narrower type to the wider one that contains it; arguments with no common type are
`E-TYP-040`. `null`, the absent case of any `T?`, fixes nothing. The call's expected type, an annotated
binding, a return, a parameter, the success type a `catch` recovers to, or the type a `match` or if-expression
gives, fixes the parameters it names first, and every argument is checked against them. Parameters not
named by that expected type are fixed by the arguments. This is how `let xs: List[dynamic Shape] = List.of(Circle(…), Square(…))` boxes each element as it is made (LR 6.10).
When nothing fixes a parameter, the call is `E-TYP-160`, and the repair is to annotate the binding:

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

A parameter that only the arguments fix, and that the result names inside a type argument, takes an integer
argument's base type, as an unannotated `let` widens its initialiser: the interval stays a fact of the argument
(LR 6.10, X-513). So `let xs = List.of(small)` with `small: Int<0..3>` is a `List[Int]`, not a
`List[Int<0..3>]`, and `List.of(small, big)` is a `List[Int]` too; to keep the refinement, write the expected
type. A result that is the parameter itself, like `pick` or a `T?`, keeps the argument's interval, since the
value passes through.

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let small: Int<0..3> = 2
  let big: Int<0..9> = 7
  let xs = List.of(small, big)                   // a List[Int]: the result names T inside a type argument
  let ys: List[Int<0..9>] = List.of(small, big)  // the expected type keeps the refinement
  console.print("${xs.count()} ${ys.count()}")
}
```

```output
2 2
```

Inside a generic body, a `T` is ownership-bearing whatever it is instantiated at: a parameter `x: T` is a loan,
so returning it, storing it or passing it to an `owned` position needs a marker like any loan (`E-OWN-001`), and
`copy x` is allowed only with `T implements Copyable`; without that bound it is `E-OWN-002`, and a loaned `T` a
call returns (`xs.get(0)` in a `first[T]`) cannot become owned without that bound. A function that only takes
`T` by loan does not own the caller's value (LR 6.10, LR 6.12, X-513).

```fortis
// reject: E-OWN-002
pure func duplicate[T](x: T) List[T] {
  return List.of(copy x)          // reject: E-OWN-002 — `copy x` needs T implements Copyable
}

func main() {
}
```

The bound is the repair, and the native lowering gives a type parameter exactly the Rust bounds its `where`
clause names (`Copyable` is `Clone`, `Equatable` is `PartialEq`, `Comparable` adds `PartialOrd`, `Hashable`
adds `Eq` and `Hash`, `Displayable` is the display trait, and a program's interface is its trait) and no other,
which is what makes a use at a type that satisfies the bounds build (LR 6.10, X-515).

```fortis
// accept
pure func duplicate[T](x: T) List[T] where T implements Copyable {
  return List.of(copy x)
}

test "a Copyable T can be copied" {
  expect duplicate(3).count() == 1
}
```

One thing a generic body cannot do in Language 1.0 is call a type parameter's associated functions: `T.default()`,
`T.fromText(text)` or an interface's own associated function is `E-TYP-170`, in a body, a field default, a
constant or a parameter default alike, because the call would need the type argument at run time (LR 6.10,
X-180). Take a value of `T`, or a function that makes one (`make: fn() T`), as a parameter instead; methods of
`T`'s bounds with a receiver are unaffected, and a concrete type's associated functions stay callable.

```fortis
// reject: E-TYP-170
pure func fresh[T]() T {
  return T.default()              // reject: E-TYP-170 — no associated call on a type parameter
}

func main() {
}
```

A generic class's constructor takes its type arguments from the written type, else from the expected type, and
checks each argument against them: `let p: Pair[Int, String] = Pair(a: 1, b: "x")` (LR 6.10).

A generic function is compiled once per type it is used at. Higher-kinded types, recursive associated types and
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
| `From[S]` (retired) | `Type.from(source)` and `source.to()` | accepted in v0.1; the conversion is written `source as Self` and `fortis fmt` rewrites the old forms (LR 6.13) |

There is no operator overloading: `Money + Money` is spelled `Money.add(a, b)`, and the comparison operators
dispatch only to these interfaces (LR 6.10). `==` and `!=` call the type's `equals` and `< <= > >=` its
`compare`, on every backend, whether the implementation is hand-written or derived, and a `Map` or `Set` finds a
key by the same `equals` and `hash`. A core type that implements `Equatable` or `Comparable` provides `equals`
and `compare` as ordinary methods, callable directly (`"a".compare("b")`, `a.equals(b)`) and through a `where`
bound, on every backend. `Hasher` has no members in Language 1.0, so a hand-written `hash` writes nothing and
every value of the type hashes alike: the containers then decide by `equals` alone, and a hand-written `equals`
is what `==`, a container lookup and a `where T implements Equatable` body call (LR 6.10, X-516).

`derive` on a class or enum generates `Equatable`, `Hashable`, `Comparable`, `Displayable`, `Encodable`,
`Decodable` or `Codec` field by field, and is accepted only when every field implements the interface
(`E-TYP-090` otherwise, LR 6.10). Derived `Comparable` orders by declaration order: a class compares its fields
in order and the first difference decides, and an enum orders by variant declaration order first and, within one
variant, by the payload's fields in order (LR 6.10, X-516).

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
as a class that does not derive it or `Int<1..10>`, makes the derivation an error (`E-TYP-090`). Written without
its call, `Name.default` is that function's value (`let make = HttpLimits.default`), which builds the value when
it is called (LR 6.10, X-516). `derive convert from S` copies every field of the target from the field of the
same name in `S`, which must be visible and copyable and have an equal type or a total borrowed conversion.
Then `customer as Summary` builds the value (LR 6.10, LR 6.13):

```fortis
// accept
class Customer {
  export name: String
  export age: Int
  export email: String
}

class Summary derive convert from Customer {
  export name: String
  export age: Int
}

test "a summary keeps the fields it names" {
  let ada = Customer(name: "Ada", age: 36, email: "ada@example.org")
  let summary = ada as Summary
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
holds a `T` by value: such a function cannot be instantiated with a linear type unless it says `T: linear` and
consumes the value (`E-OBL-003`). A function that only borrows its `T` can be used with linear types freely,
because the obligation stays with the caller (LR 6.12).

```fortis
// accept
import fortis.host { Console }

func id[T](x: owned T) T where T: linear {
  return move x                      // returning it forwards the obligation to the caller
}

recovers func main(console: Console) {
  let r = Outcome.success(3)         // an outcome is a linear value
  match id(move r) {
    success(n) => console.print("${n}")
    failure(f) => { report f }
  }
}
```

```output
3
```

A user generic type follows the same declaration rule for a linear type argument: a class or enum may be
instantiated at one only if its corresponding parameter declares `T: linear`, or the instantiation is
`E-OBL-003` (LR 6.12, X-611). A class or enum writes the bound in a `where` clause at the end of its head,
before its body (`class Bin[T] where T: linear { … }`), and this clause on a class or enum accepts linear bounds
only. The rule applies to explicit and inferred arguments alike, including arguments inferred from a later use
or passed through a generic function, and a body under `T: linear` must consume what it owns (LR 10.7).

```fortis
// reject: E-OBL-003, E-OBL-001
class Bin[T] {
  item: T
}

func main() {
  let absent: Bin[Outcome[Int]]? = null    // reject: E-OBL-003, E-OBL-001 — no T: linear bound; a linear optional dropped
}
```

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

## Conversions

One operator converts a value from one type to another: `expression as Type` (LR 6.13). It binds tighter than
the arithmetic and comparison operators, so `a as Decimal * b` is `(a as Decimal) * b`, and it chains left to
right: `n as Int as Decimal` widens, then converts. A `<` directly after the target opens a refinement, so a
converted value is compared in parentheses: `(n as Int) < 5` (LR 6.13, X-550). The checker resolves every `as`
at compile time to exactly one conversion for the pair: a pair with no conversion is rejected (`E-TYP-180`),
and so is an identity, `n as Int` where `n` is already an `Int` (`E-TYP-181`). Conversions never chain by
search: `s as Decimal` for a `Scalar` is rejected although `Scalar` converts to `Int` and `Int` to `Decimal`;
write the steps yourself, `s as Int as Decimal`, which converts left to right.

A conversion that cannot lose information is **total** and has type `T`. One that could lose information is
**fallible**, has type `Outcome[T]` (LR 6.13), and is handled on the line like any outcome (`or fail`, `or recover`,
`catch`; LR 10.2); left unhandled it is caught by the outcome rules (an `Outcome` dropped is `E-OBL-001`, one
used as a `T` is `E-TYP-040`). Nothing rounds: turning a `Float` into an integer or a `Decimal` needs a
rounding choice, and that is a named function with the choice written, `Decimal.fromFloat(x, scale, mode)`
(LR 6.5), so `f as Int` is rejected.

Built-in types convert automatically, and this table is the whole of what converts without a declaration
(LR 6.13):

| From | To | Kind |
|---|---|---|
| an integer type | an integer type whose range contains its own (`Int8` to `Int`, `UInt8` to `Int16`) | total |
| an integer type | an integer type whose range does not contain its own (`Int` to `Int8`, `Int` to `UInt`, `UInt64` to `Int`) | fallible: `conversion.out_of_range` |
| an integer type | `Decimal` | total |
| `Decimal` | an integer type | fallible: `conversion.not_integral` for a fractional part, `conversion.out_of_range` beyond the type |
| `Int32` and the smaller integer types | `Float` | total: every value is exactly representable |
| `Int`, `Int64`, `UInt`, `UInt64` | `Float` | fallible: `conversion.not_exact` when the value is not exactly representable |
| `Float`, `Float32` | an integer type, `Decimal` | no conversion: rounding is a choice, and `Decimal.fromFloat(x, scale, mode)` writes it |
| the integer types, `Decimal`, `Bool`, `Scalar`, `Float`, `Float32` | `String` | total: the canonical text, what `Displayable` prints (allocates) |
| `String`, `StringView` | an integer type, `Decimal`, `Bool` | fallible: `conversion.parse` for any text but the canonical form, `conversion.out_of_range` for an integer beyond the type |
| `StringView` | `String` | total: an owned copy (allocates) |
| `Bytes`, `BytesView` | `String` | fallible: `conversion.not_utf8` at the first byte that is not UTF-8 (allocates) |
| `String`, `StringView` | `Bytes` | total: the UTF-8 bytes (allocates) |
| `Scalar` | `Int` | total (`Byte` to `Int` is a widening above) |
| `Int` | `Scalar` | fallible: `conversion.out_of_range` for a surrogate or a value above U+10FFFF |
| a base type | a refinement of it (`n as Int<0..100>`) | fallible: `refinement.out_of_range`, the conversion `T.validate(x)` spells (LR 6.3) |
| a base type | a newtype of it (`raw as Pence`) | total when the representation is proved; otherwise fallible: `refinement.out_of_range` (LR 6.9) |
| a newtype | its representation (`p as Int`) | total; preserves the representation refinement; visible only in its declaring module, otherwise `E-NAM-004` (LR 6.9) |
| a refinement | its base or a wider refinement | not a conversion: a structural subtype, free (LR 6.3, LR 6.13); `E-TYP-181` |

Two names of one type (`Int` and `Int64`, `UInt` and `UInt64`, `Byte` and `UInt8`) are the identity. Parsing
covers the integer types, `Decimal` and `Bool`, and there is no row into `Float`, while `Float` and `Float32`
into `String` is total. A fallible built-in conversion fails with one of five codes, each with a payload the
backends build alike: `conversion.out_of_range` (`value`, `low`, `high`), `conversion.not_integral` (`value`),
`conversion.not_exact` (`value`), `conversion.parse` (`text`) and `conversion.not_utf8` (`position`)
(LR 6.13).

```fortis
// accept
import fortis.host { Console }

try recovers func main(console: Console) {
  let small: Int8 = 7
  console.print(small as Int as Decimal as String)     // total: widening, then an integer into a Decimal
  let parsed = "42" as Int or fail                     // fallible: the canonical text, or conversion.parse
  console.print(parsed as String)
  let over = 300 as Int8 catch e {                     // fallible: 300 does not fit an Int8
    console.print(e.code)
    recover small
  }
  discard over
  let pct = 50 as Int<0..100> or fail                  // the refinement's own check
  console.print(pct as String)
}
```

```output
7
42
conversion.out_of_range
50
```

```fortis
// reject: E-TYP-180, E-TYP-181
func main() {
  let n = 3
  let rounded = 1.5 as Int          // reject: E-TYP-180
  let same = n as Int               // reject: E-TYP-181
}
```

`x as T` passes `x` as a call passes an argument to a parameter taken by read loan: a bare ownership-bearing
place is loaned, `move x as T` moves the value in and `copy x as T` converts a copy; a conversion into a new
`String` or `Bytes` allocates and is refused in `noalloc` code (LR 6.13, LR 7.5).

Nothing converts a class or an enum value automatically: a type explicitly derives conversions or declares them as
`convert` members (LR 6.13). `convert to T` converts the declaring type into a `T`; `convert from S(s)` converts
an `S`, named `s`, into a value of the declaring type. `try` makes the conversion fallible: `x as T` has type
`Outcome[T]`, handled on the line. The body is a method body whose returns give the target type, and `self` (or
`s`) is a read loan of the source unless the head writes `owned self` / `owned s` — then the conversion consumes
the value and the use site writes `move x as T`. One conversion per pair, keyed by the whole type as `implement`
coherence keys its pairs: two members for one pair are rejected (`E-TYP-182`), a member for a pair the built-in
table already decides duplicates it (`E-TYP-183`), and a generic class or enum declares no member — the other
type a member names is a nominal type, never a refinement (`E-TYP-184`).

```fortis
// accept
import fortis.host { Console }

class Celsius {
  degrees: Float

  export convert to Fahrenheit {
    return Fahrenheit(degrees: self.degrees * 9.0 / 5.0 + 32.0)
  }
  export convert from Fahrenheit(f) {
    return Celsius(degrees: (f.degrees - 32.0) * 5.0 / 9.0)
  }
}

class Fahrenheit {
  degrees: Float
}

try recovers func main(console: Console) {
  let hot = Celsius(degrees: 100.0) as Fahrenheit      // the `convert to` member on Celsius
  console.print(hot.degrees as String)
  let cold = hot as Celsius                            // the `convert from` member on Celsius
  console.print(cold.degrees as String)
}
```

```output
212.0
100.0
```

```fortis
// reject: E-TYP-182, E-TYP-183, E-TYP-184
class Celsius {
  degrees: Float

  export convert to Fahrenheit {
    return Fahrenheit(degrees: 0.0)
  }
  export convert to Celsius {       // reject: E-TYP-183 — the identity: the table already decides it
    return Celsius(degrees: 0.0)
  }
}

class Fahrenheit {
  degrees: Float

  export convert from Celsius(c) {  // reject: E-TYP-182 — (Celsius, Fahrenheit) is declared twice
    return Fahrenheit(degrees: 0.0)
  }
}

class Pair[T] {
  first: T

  export convert to Text {          // reject: E-TYP-184 — a generic class declares no convert member
    return Text(body: "")
  }
}

class Text {
  body: String
}
```


A newtype uses the same operator (LR 6.9, LR 6.13). An unproved value needs a range check and returns an
outcome; a proved value constructs the newtype directly. Inside the module that declares it, `p as Int`
unwraps a `Pence` and keeps its `Int<0..>` refinement. An unwrap outside that module is `E-NAM-004`.
An ownership-bearing representation needs `move` or `copy` on a place in either direction (LR 7.5).

```fortis
// accept
import fortis.host { Console }

type Pence = Int<0..>

try func checked(raw: Int) Pence {
  return raw as Pence or fail
}

try func main(console: Console) {
  let paid = checked(42) or fail
  let zero = 0 as Pence
  console.print("${paid as Int} ${zero as Int}")
}
```

```output
42 0
```

The old forms `From[S]`, `T.from(x)`, `x.to()`, `derive From[S]`, `String.from(view)` and `T.validate(x)`
are retired but remain accepted in v0.1 (LR 6.3, LR 6.6, LR 6.10, LR 6.13). `fortis fmt --write` rewrites them
by their resolved declaration. A user function merely named `from` or `validate` stays as it is. `to()` needs
a written expected target in a parameter, return, field or annotated binding; without one the formatter keeps
it and prints a note. A retired `validate` call still returns an outcome even when its argument is proved, so
its rewrite keeps the outcome: `Outcome.success(x as T)` when the newtype conversion is total, and
`Outcome.success(x)` for a refinement already proved (LR 6.13, X-553). `derive convert from S` is the explicit
opt-in for a borrowed field-wise conversion.
A field needing a fallible or consuming conversion makes that derivation `E-TYP-090`; write an explicit
conversion body to handle it (LR 6.13).

## Transform a present value

`T?.map(f)` applies its `pure fn(T) U` callback only to a present value, lent by read loan, and returns
`U?`. Its result parameter permits `U: linear`, and the caller must consume such a result (LR 10.7, X-621); `null` stays `null` without calling the callback (LR 6.8). The operators still supply the other
operations: `??` gives a default, `?.` reaches a member, and a null test or `match` narrows the value.
See [Outcomes](10-outcomes.md) for the corresponding success-only `Outcome.map`.

```fortis
// accept
pure func double(n: Int?) Int? {
  return n.map(fn(value) => Int.saturatingMul(value, 2))
}

test "optional map" {
  expect double(3) == 6
  expect double(null) == null
}
```


## Enum names from text

A non-generic enum whose variants have no payload can opt into `derive convert from String` (LR 6.13).
This supplies fallible conversions from both `String` and `StringView`, including literals and views from
`trim()`. It compares the whole name ignoring ASCII case, compares other scalars exactly, and does not trim
for you. A failed conversion reports `conversion.parse` with owned `text` and `variants`; the latter lists
the variant names in declaration order separated by `, `.

Names that clash after ASCII case folding, payload variants, generic enums and any other source type are
rejected with `E-TYP-090`. The pairs have the enum's visibility and cannot also be declared by a member
(`E-TYP-182`). This derives no reverse conversion; Displayable still supplies interpolation (LR 6.13).
The [coffee walkthrough](worked-example.md) uses the derivation with an explicit trim at its input boundary.

### Optional calls and writable places (LR 6.7 to 6.9)

The arguments of `cell?.take(value)` are evaluated only when `cell` is present. Moving a linear value such as
`Must` or `Outcome` in an argument is therefore rejected with `E-OBL-001`: the null path would leave it
unconsumed. Use `match` or `if let` and consume the obligation on both paths. Owned non-linear arguments such
as `String` remain permitted (X-710).

A List index assignment checks the element type just as `set` does. For example, storing `9` in a
`List[Int<0..3>]` is `E-REF-001` (X-711). An editable record or tuple must keep the exact corresponding member
types, including optional refinements and nested records. Passing an editable `{ n: Int<0..3> }` to an
editable `{ n: Int }` parameter would allow an invalid write and is `E-TYP-040`. Read loans and owned values
can still widen their fields (X-712).
