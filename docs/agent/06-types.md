# Types (LR 6)

<!-- covers: LR 6 -->
*Agent mode. Human mode: [Types](../guide/06-types.md). Source of truth: LR 6. Proof rules for integer
operators and indexing (LR 6.3, 6.4, 6.7) are in [Proofs](06-proofs.md), the complete reference for Language 1.0's
interval domain; SP-18's further rules are a later edition (X-146).*

> Preview: refinements (LR 6.3), newtypes (LR 6.9) and generics (LR 6.10) arrive in v0.1. Normalisation, case mapping and the collections beyond `List`, `Map` and `Set` (LR 6.6, LR 6.7) arrive in v0.4.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-06-01 | Nominal identity survives calls, storage, serialisation and FFI: distinct declarations never interchange, whatever their representation. | LR 6.1, 6.9 |
| LANG-06-02 | A refinement is a structural subtype of its base and of every refinement containing it; the reverse is never implicit; the interval domain is exactly LR 6.3's, with no further inference. | LR 6.3 |
| LANG-06-03 | Untrusted values pass `validate` before holding a refined or nominal type. | LR 6.3, 6.9 |
| LANG-06-04 | Integer, `Decimal` and the listed float operations agree bit-exactly across VM and native. | LR 6.4, 6.5 |
| LANG-06-05 | Float operators are total, strict IEEE 754; float refinements are rejected. | LR 6.3, 6.5 |
| LANG-06-06 | `Map`/`Set` iteration is deterministic per build (first-insertion order). | LR 6.7 |
| LANG-06-07 | The trivial and copyable sets are exactly those of LR 6.11. | LR 6.11 |
| LANG-06-08 | Optional, `Bool` and `Outcome` never convert into one another. | LR 6.8 |
| LANG-06-09 | Linear values are consumed exactly once on every path. | LR 6.12 |
| LANG-06-10 | Coherence: an implementation of an interface for a type lives in the package defining one of them; overlaps are rejected. | LR 6.10, 9.5 |
| LANG-06-11 | A call whose type arguments are fixed neither by its arguments nor by its expected type is rejected. | LR 6.10 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-TYP-040 | an `Outcome[T]` passed where `T` is expected | consume the outcome first (`let v = f() or fail`) | LR 10.1 |
| E-TYP-040 | an interpolated string bound to a `StringView` binding | annotate the binding `String`, or drop the annotation | LR 3.5 |
| E-REF-001 | a literal outside its target refinement | widen the declared type, or `validate` | LR 3.5, 6.3 |
| E-TYP-020 | a float refinement (`Float<0..1>`) | refine an integer representation (for example a count of thousandths), or check the bound in an associated constructor function | LR 6.3 |
| E-REF-003 | a length-refined collection loaned `editable` or mutated | mutate an unrefined collection, then `T.validate(move c)` | LR 6.3 |
| E-TYP-030 | a tuple, record or variant map in an exported signature (aspect parameters and the `fortis` package's own signatures excepted), or a non-exported type in one | declare and export a named class | LR 6.9, 4.4 |
| E-TYP-090 | `derive I` where a field does not implement `I`; a derivation rule not met; the codec pair on a non-`Encodable`/`Decodable` type | implement `I` for the field type, change the field type, or write the implementation by hand | LR 6.10 |
| E-TYP-160 | type arguments not fixed (`let names = List.default()`) | annotate the receiving binding: `let names: List[String] = List.default()` | LR 6.10 |
| E-OWN-030 | `trivial class` with a non-trivial field (reported at the class) | drop `trivial`, or make every field trivial | LR 6.11 |
| E-OWN-002 | `copy` of a non-copyable type (`Shared`, `Weak`, `Mutex`, loans, futures, closures…) | `retain()` for `Shared`/`Weak`; otherwise move or loan | LR 6.11, 7.5 |
| E-OBL-001 | a linear value dropped, overwritten or discarded (except `discard` of a never-polled future whose environment holds no linear value and no exclusive loan, LR 12.3) | consume it (`Must.discharge(move m)`; outcomes: `or fail`, `or recover`, `catch`, `match`) | LR 6.12, 12.3 |
| E-TYP-010 | an optional used as a condition | `x != null` | LR 6.8 |
| E-MIX-001 | deriving an interface the class also implements by hand | keep one of the two | LR 6.10 |

## Built-in types (LR 6.2)

`Void` `Never` `Bool`; `Int8` `Int16` `Int32` `Int64` `UInt8` `UInt16` `UInt32` `UInt64` (`Int` = `Int64`,
`UInt` = `UInt64`, `Byte` = `UInt8`); `Float32` `Float`; `Decimal` `BigInt`; `String` `StringView` `Bytes`
`BytesView`; `Duration` `Instant` `Timestamp`; `List[T]` `Array[T, N]` `ListView[T]` `Map[K, V]`
`OrderedMap[K, V]` `Set[T]` `FixedList[T, N]` `FixedMap[K, V, N]`; tuples, anonymous records,
`VariantMap[E, V]`; `T?`; `Outcome[T]` `Failure` `Report`; `Future[T]` `Task[T]` `TaskCompletion[T]` `Shared[T]`
`Weak[T]` `Mutex[T]` `AtomicInt` `AtomicBool`; `fn`/`fn mut`/`fn once` types. `N` is an integer literal; no
generics over `N`. `List.of(...items: T)` is the only variadic form (standard library only) (LR 6.2).

## Facts used when writing code

- Refinement forms: `Int<0..100>`, `Int<0..<100>`, `Int<1..>`, `Int<..0>`, `String<scalars: 1..120>`,
  `Bytes<length: 0..65536>`, `List[T]<length: 0..100>`; type position only (LR 6.3).
- `T.validate(x)` and `Int<0..100>.validate(x)` are pure, return `Outcome[T]`, fail (by the prelude contract) `refinement.out_of_range`
  (code observed from the toolchain; LR 6.3 names only the outcome).
- Integer families (LR 6.4): checked `Int.add/sub/mul/div/rem/shiftLeft` → `Outcome`, failing
  `numeric.overflow`, `numeric.divide_by_zero`, `numeric.shift_range`; `wrapping…` and `saturating…` are total.
  Division truncates toward zero; `%` has the dividend's sign; `>>` arithmetic on signed, logical on unsigned.
- The success value of a checked intrinsic has the interval of the operation on its operands (LR 6.3, X-93), so
  `Int.div(-7, 2) or recover 0` is `E-REF-001` (0 is outside `[-3, -3]`); annotate the binding `Int`.
- Floats (LR 6.5): no `Equatable`/`Hashable`/`Comparable`; `==` is IEEE; keys via `Float.totalOrder(x)` or
  `Float.bitsEqual(a, b)`; display is shortest round-trip with a fractional part (`2000.0`), `NaN`, `Infinity`.
- `Decimal` (LR 6.5): no operators; `Decimal.add/sub/mul/div/rem` → `Outcome[Decimal]`; scale rules: add/sub larger
  scale, mul sum of scales, div largest that fits, half-even when over 28 digits; `Decimal.round(v, scale, mode)`
  with `RoundingMode.HalfEven|HalfAwayFromZero|TowardZero|AwayFromZero`; displays with exactly its scale.
- Text (LR 6.6): literal is `StringView from static`; implicit literal-to-`String` conversion allocates (refused
  in `noalloc`); `String.from(view)`; no integer indexing; `byteCount()` O(1), `scalarCount()` O(n),
  `slice(from, to)` → `Outcome[StringView from view]`, `scalarAt(i)` → `Outcome[Scalar]`; `text.Builder`
  `create/append/appendLine/appendScalar/finish`.
- Collections (LR 6.7): `list[i]` needs a proof ([Proofs](06-proofs.md)); otherwise `get(i)` → `(alias T from
  self)?`, `at(i)` → `Outcome[T]`; `set(i, v)` → `Outcome[Void]`; `push(item)` → `Void`; `pop()` → `T?`; `map`/`filter`/
  `countWhere` take `pure fn`; `filter`/`reversed`/`concat` need `T: Copyable`. `Map.set/get/count`,
  `Set.add` (→ `Bool`, true when new) `/contains/count`; iteration in first-insertion order, replacement keeps the
  position.
- Optionals (LR 6.8): `T` → `T?` implicit; `??` lazy; `a?.f`; `match` with `present(x)` / `null`; narrowing after
  `x != null`, after `if x == null { return … }`, and in `x != null && e`.
- Tuples `t.0`; records `{ name: a }`; `VariantMap[E, V]` literal `{ Found: 200, NotFound: 404 }` with one entry
  per variant; `(fn(Int) Bool)?` is an optional function (LR 6.9).
- Newtypes (LR 6.9): `type Name = Repr` is always nominal with a private representation; values only from
  `Name.validate(raw)` or a proved literal `Name(42)`; over a trivial representation each of Equatable,
  Hashable, Comparable and Displayable holds exactly when it holds for the representation (a newtype over
  `Float` is not Hashable, LR 6.5).
- Generics (LR 6.10): `[T]`, `where T implements I` after the return type; type arguments are always inferred, never written at a call.
- Derivable (LR 6.10): `Equatable`, `Hashable`, `Comparable` (declaration order), `Displayable`, `Encodable`,
  `Decodable`, `Codec`; `Default` (field initialisers first); `From[S]` (fields copied by name). `x.to()` where the
  target type is known, else `T.from(x)`.
- Canonical codec (LR 6.10): `toText()`/`fromText(input)` (JSON, declaration order), `toBytes()`/
  `fromBytes(input)`; failures `codec.invalid`, `codec.unknown_field`, or the refinement's own failure. Encodable
  derivation needs exported, encodable fields; secrets, authorities, loans, closures, futures and tasks are never
  encodable.
- Trivial (LR 6.11): primitives, `Decimal`, `Scalar`, time types, newtypes over trivial, enums/optionals/tuples/
  records of trivial, `trivial class`. Copyable: trivial, `String`, `Bytes`, `BigInt`, collections and
  aggregates of copyable. Not copyable: `Shared`, `Weak`, `Mutex`, atomics, loans, views, resources, `Future`,
  `Task`, `Outcome`, `Failure`, closures.
- Linear (LR 6.12): `Outcome[T]`, `Failure`, `Future[T]`, `Task[T]`, `Must`, `fn once` capturing one, and any
  type that can contain one. Type parameters are non-linear unless `T: linear`.

## Correction patterns

### E-TYP-040: a raw value where a newtype is expected (LR 6.9, LR 10.1)

```fortis
// reject: E-TYP-040
type CustomerId = Int<1..>

pure func lookup(id: CustomerId) Int => 1

pure func fromInput(raw: Int) Int {
  return lookup(raw)                  // reject: E-TYP-040
}

test "lookup" {
  expect fromInput(5) == 1
}
```

```fortis
// accept
type CustomerId = Int<1..>

pure func lookup(id: CustomerId) Int => 1

pure try func fromInput(raw: Int) Int {
  let id = CustomerId.validate(raw) or fail
  return lookup(id)
}

test "lookup" {
  let found = fromInput(5) or fail
  expect found == 1
}
```

### E-REF-001: a literal outside its refinement (LR 3.5, LR 6.3)

```fortis
// reject: E-REF-001
pure func limit() Int<0..100> {
  let score: Int<0..100> = 140        // reject: E-REF-001
  return 100
}

test "limit" {
  expect limit() == 100
}
```

```fortis
// accept
pure func limit() Int<0..100> {
  let score: Int<0..100> = 100
  return score
}

test "limit" {
  expect limit() == 100
}
```

### E-REF-003: mutating a length-refined collection (LR 6.3)

```fortis
// reject: E-REF-003
pure func grow(start: List[Int]<length: 0..3>) Int {
  let mut items = copy start
  editable items.push(1)              // reject: E-REF-003
  return items.count()
}

test "grow" {
  expect true
}
```

```fortis
// accept
pure try func three() List[Int]<length: 0..3> {
  let mut items: List[Int] = List.default()
  editable items.push(1)
  return List[Int]<length: 0..3>.validate(move items) or fail
}

test "three" {
  let items = three() or fail
  expect items.count() == 1
}
```

### E-TYP-160: type arguments not fixed (LR 6.10)

```fortis
// reject: E-TYP-160
pure func names() Int {
  let found = List.default()          // reject: E-TYP-160
  return found.count()
}

test "names" {
  expect names() == 0
}
```

```fortis
// accept
pure func names() Int {
  let found: List[String] = List.default()
  return found.count()
}

test "names" {
  expect names() == 0
}
```

### E-TYP-030: a structural type in an exported signature (LR 6.9)

```fortis
// reject: E-TYP-030
export pure func bounds() (Int, Int) => (1, 2)     // reject: E-TYP-030

test "bounds" {
  expect bounds().0 == 1
}
```

```fortis
// accept
export class Bounds {
  export low: Int
  export high: Int
}

export pure func bounds() Bounds => Bounds(low: 1, high: 2)

test "bounds" {
  expect bounds().low == 1
}
```

### E-TYP-090: deriving an interface a field lacks (LR 6.10)

```fortis
// reject: E-TYP-090
class Reading derive Equatable {    // reject: E-TYP-090
  export celsius: Float
}

test "reading" {
  expect Reading(celsius: 1.5).celsius > 1.0
}
```

```fortis
// accept
class Reading derive Equatable {
  export celsius: Decimal
}

test "reading" {
  expect Reading(celsius: 1.5d) == Reading(celsius: 1.5d)
}
```

### E-TYP-020: a float refinement (LR 6.3)

```fortis
// reject: E-TYP-020
type Ratio = Float<0..1>              // reject: E-TYP-020

test "ratio" {
  expect true
}
```

```fortis
// accept
type BasisPoints = Int<0..10000>

test "ratio" {
  let half = BasisPoints.validate(5000) or fail
  expect half == BasisPoints(5000)
}
```

### E-OWN-030: a trivial class with a non-trivial field (LR 6.11)

```fortis
// reject: E-OWN-030
trivial class Tag {                   // reject: E-OWN-030
  export name: String
}

test "tag" {
  expect Tag(name: "a").name == "a"
}
```

```fortis
// accept
class Tag {
  export name: String
}

test "tag" {
  expect Tag(name: "a").name == "a"
}
```

### E-OBL-001: a linear value dropped (LR 6.12)

```fortis
// reject: E-OBL-001
pure func witness() Int {
  let token = Must.create()           // reject: E-OBL-001
  return 1
}

test "witness" {
  expect witness() == 1
}
```

```fortis
// accept
pure func witness() Int {
  let token = Must.create()
  Must.discharge(move token)
  return 1
}

test "witness" {
  expect witness() == 1
}
```

### E-TYP-040: an interpolated string held as a view (LR 3.5, LR 6.6)

```fortis
// reject: E-TYP-040
pure func label(n: Int) Int {
  let text: StringView = "count ${n}"      // reject: E-TYP-040
  return text.scalarCount()
}

test "label" {
  expect label(3) == 7
}
```

```fortis
// accept
pure func label(n: Int) Int {
  let text: String = "count ${n}"
  return text.scalarCount()
}

test "label" {
  expect label(3) == 7
}
```
