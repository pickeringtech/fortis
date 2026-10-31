# Types (LR 6)

<!-- covers: LR 6 -->
*Agent mode. Human mode: [Types](../guide/06-types.md). Source of truth: LR 6. Proof rules for integer
operators and indexing (LR 6.3, 6.4, 6.7) are in [Proofs](06-proofs.md), the complete reference for Language 1.0's
interval domain; SP-18's further rules are a later edition (X-146).*

> Preview: normalisation, Unicode case mapping and the collections `OrderedMap`, `FixedList`, `FixedMap` and `VariantMap` beyond their syntax (LR 6.6, LR 6.7) arrive in v0.4. Refinements (LR 6.3), newtypes (LR 6.9), generics (LR 6.10) and conversions (LR 6.13) are in v0.1; `derive Default` (LR 6.10) is in v0.0.2.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-06-01 | Nominal identity survives calls, storage, serialisation and FFI: distinct declarations never interchange, whatever their representation. | LR 6.1, 6.9 |
| LANG-06-02 | A refinement is a structural subtype of its base and of every refinement containing it; the reverse is never implicit; the interval domain is exactly LR 6.3's, with no further inference. | LR 6.3 |
| LANG-06-03 | Untrusted values pass the checked `as` conversion (also spelled `validate` in v0.1) before holding a refined or nominal type. | LR 6.3, 6.9, 6.13 |
| LANG-06-04 | Integer, `Decimal` and the listed float operations agree bit-exactly across VM and native. | LR 6.4, 6.5 |
| LANG-06-05 | Float operators are total, strict IEEE 754; float refinements are rejected. | LR 6.3, 6.5 |
| LANG-06-06 | `Map`/`Set` keys implement Hashable and Equatable; iteration is deterministic per build (first-insertion order). | LR 6.7 |
| LANG-06-07 | The trivial and copyable sets are exactly those of LR 6.11. | LR 6.11 |
| LANG-06-08 | Optional, `Bool` and `Outcome` never convert into one another. | LR 6.8 |
| LANG-06-09 | Linear values are consumed exactly once on every path. | LR 6.12 |
| LANG-06-10 | Coherence: an implementation of an interface for a type lives in the package defining one of them; overlaps are rejected. | LR 6.10, 9.5 |
| LANG-06-11 | A call whose type arguments are fixed neither by its arguments nor by its expected type is rejected. | LR 6.10 |
| LANG-06-12 | No associated call on a type parameter (`T.default()`, `T.create()`): `E-TYP-170`; a call of a method of `T`'s interface bound runs on every backend (X-180). | LR 6.10 |
| LANG-06-13 | An `implement` block's target is a non-generic class or enum the program declares (`E-TYP-171`); an accepted block behaves as its target listing the interface (X-180). | LR 9.5 |
| LANG-06-14 | `x as T` resolves at compile time to exactly one conversion for the pair, a table row or a declared one (`E-TYP-180`); an identity is `E-TYP-181`; a built-in conversion is total or fails exactly as the table lists, alike on every backend; a newtype unwrap keeps its representation refinement and is `E-NAM-004` outside its declaring module (X-475, X-552). | LR 6.13 |
| LANG-06-15 | A class or enum declares or explicitly derives its conversions, one per pair, keyed by the whole type: a pair declared twice is `E-TYP-182`, a member for a pair the table decides is `E-TYP-183`, a generic class or enum and a newtype have no `convert` member (`E-TYP-184`); a consuming member takes `move x as T` (X-475, X-551). | LR 6.13 |
| LANG-06-16 | The functional methods take the callback types their signatures state: `pure fn` where the method only computes (`E-TYP-040` for a lambda that does not qualify), `fn mut` for `each`, an outcome-returning function for the `try` methods; a method returning elements requires `Copyable`; a `pure` function never calls `each` or `tryEach` (`E-EFF-001`); every backend runs them with the same results and visit order. | LR 6.7 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-TYP-040 | an `Outcome[T]` passed where `T` is expected | consume the outcome first (`let v = f() or fail`) | LR 10.1 |
| E-TYP-040 | `?.method()` calls an editable, consuming or `try` method | test for null, then call with the required mode and handle any outcome | LR 6.8 |
| E-TYP-090 | enum text derivation has payloads, type parameters, ASCII-case-clashing names or a source other than `String` | use a non-generic payload-free enum with distinct names, or write the conversion | LR 6.13 |
| E-TYP-040 | an interpolated string bound to a `StringView` binding | annotate the binding `String`, or drop the annotation | LR 3.5 |
| E-TYP-040 | a container or view whose type argument differs only in its refinement, in any direction (`List[Int<0..5>]` as a `List[Int]`, or the reverse as `E-REF-001`) | keep one refinement per container type; convert element by element with `map`, or validate each element (X-514) | LR 6.3 |
| E-REF-001 | a literal outside its target refinement, a value the interval rules do not prove, or a `List[Int]` where a `List[Int<0..5>]` is expected (its elements are never proved) | widen the declared type, or convert with `x as T` and handle the outcome | LR 3.5, 6.3 |
| E-TYP-020 | a refinement that is not of its base: a float base (`Float<0..1>`), a base the forms do not name, a measure the base lacks, a bound that does not fold at compile time, an empty or inverted range (`Int<5..1>`), or one outside the base's own range (`UInt8<0..300>`) | refine an integer representation (for example a count of thousandths), fix the range, or use a `const` bound of the base type (X-512) | LR 6.3 |
| E-REF-003 | a length-refined collection loaned `editable` or mutated | mutate an unrefined collection, then convert with `move c as T` | LR 6.3 |
| E-TYP-030 | a tuple, record or variant map in an exported signature (aspect parameters and the `fortis` package's own signatures excepted), or a non-exported type in one | declare and export a named class | LR 6.9, 4.4 |
| E-TYP-090 | `derive I` where a field does not implement `I`; a derivation rule not met; the codec pair on a non-`Encodable`/`Decodable` type; a `Set`/`Map` whose key lacks `Hashable` and `Equatable` (X-615) | implement or derive the interface for the field or key type, change the type, or write the implementation by hand | LR 6.10, 6.7 |
| E-TYP-160 | type arguments not fixed (`let names = List.default()`; `null` fixes nothing) | annotate the receiving binding: `let names: List[String] = List.default()` | LR 6.10 |
| E-TYP-170 | an associated call on a type parameter (`T.default()`, `T.fromText(text)`, `T.create()`) | take a value of `T`, or a function that makes one (`make: fn() T`), as a parameter (X-180) | LR 6.10 |
| E-TYP-180 | `x as T` with no conversion for the pair: no row of the built-in table and no declared member; conversions never chain by search | convert through the named function the record cites (`Decimal.fromFloat(x, scale, mode)`), or in steps the table has (`s as Int as Decimal`); declare a `convert` member for a class or enum pair | LR 6.13 |
| E-TYP-181 | `x as T` where `x` already has type `T`: two names of one range, a refinement to its base or a wider refinement | drop the `as`: the value is already a `T` | LR 6.13 |
| E-TYP-182 | two conversions declared for one pair (a `convert to` and a `convert from` naming each other, or two members) | delete one: a pair converts by one declaration, in the body of its source or its target | LR 6.13 |
| E-TYP-183 | a `convert` member for a pair the built-in table already decides, the identity included | delete the member: the pair converts without a declaration | LR 6.13 |
| E-TYP-184 | a `convert` member on a generic class or enum or a newtype, or one whose other type is a refinement | wrap the value in a non-generic class the program declares; a refinement target stays the table's `validate` row | LR 6.13 |
| E-NAM-004 | a newtype unwrap (`p as Int`) outside its declaring module, or an editing or consuming method of a collection newtype called outside it (X-552, X-581) | unwrap inside the declaring module, or add a reading method there; edit a collection newtype inside its module | LR 6.9 |
| E-OWN-002 | `copy x` of a type parameter without `T implements Copyable`, or a loaned `T` a call returns made owned without that bound (X-513) | add the bound, or take and return the `T` by loan only | LR 6.10, 6.12 |
| E-OWN-030 | `trivial class` with a non-trivial field (reported at the class) | drop `trivial`, or make every field trivial | LR 6.11 |
| E-OWN-002 | `copy` of a non-copyable type (`Shared`, `Weak`, `Mutex`, loans, futures, closures…) | `retain()` for `Shared`/`Weak`; otherwise move or loan | LR 6.11, 7.5 |
| E-OBL-001 | a linear value dropped, overwritten or discarded (except `discard` of a never-polled future whose environment holds no linear value and no exclusive loan, LR 12.3) | consume it (`Must.discharge(move m)`; outcomes: `or fail`, `or recover`, `catch`, `match`) | LR 6.12, 12.3 |
| E-OBL-003 | a generic function holding its own `T` by value, or a user generic class or enum (X-611), instantiated with a linear type without declaring `T: linear` | declare `where T: linear` and consume the value, or take the `T` by loan only; a class or enum writes the bound in a `where` clause at the end of its head | LR 6.12, 10.7 |
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
  `Bytes<length: 0..65536>`, `List[T]<length: 0..100>`; type position only (LR 6.3). Each bound is an integer
  literal or a `const` of the base type folded at compile time (`Int<0..top>`, X-512); an empty or inverted
  range, one outside the base's own range, a base or measure the forms do not name, or a bound that does not
  fold, is `E-TYP-020`. Type arguments of a container or view are invariant in their refinement (X-514). Optional values,
  tuple elements and record fields may widen as values; editable loans preserve their types invariantly
  through every member (LR 6.3, 6.9; X-685, X-686, X-712).
- `x as T` is the checked conversion (LR 6.13): into a refinement always fallible (`Outcome[T]`,
  `refinement.out_of_range`); into a newtype total when the representation is proved, fallible otherwise.
  `T.validate(x)` and `Int<0..100>.validate(x)` are the retired call spelling, still accepted, always an
  outcome; `fortis fmt` rewrites them (X-552, X-553).
- Integer arithmetic (LR 6.4): an operator the checker cannot prove safe is written as the left operand of an
  outcome operator, `a * b + c or fail`, and is the checked operation, failing `numeric.overflow`,
  `numeric.divide_by_zero`, `numeric.shift_range` (X-345). The named families, on `Int` only (the
  sized integer types have none yet; v0.4): checked `Int.add/sub/mul/div/rem/shiftLeft` → `Outcome` (the same
  operations; `fortis fmt` writes the operator where the two are equivalent); `wrapping…` and `saturating…` are total. Division truncates toward zero; `%` has the dividend's sign; `>>` arithmetic on signed, logical on unsigned.
- The success value of a checked intrinsic has the interval of the operation on its operands (LR 6.3, X-93), so
  `Int.div(-7, 2) or recover 0` is `E-REF-001` (0 is outside `[-3, -3]`); annotate the binding `Int`.
- Floats (LR 6.5): no `Equatable`/`Hashable`/`Comparable`; `==` is IEEE; keys via `Float.totalOrder(x)` or
  `Float.bitsEqual(a, b)`; display is shortest round-trip with a fractional part (`2000.0`), `NaN`, `Infinity`.
- `Decimal` (LR 6.5): no operators; `Decimal.add/sub/mul/div/rem` → `Outcome[Decimal]`; scale rules: add/sub larger
  scale, mul sum of scales, div largest that fits, half-even when over 28 digits; `Decimal.round(v, scale, mode)`
  with `RoundingMode.HalfEven|HalfAwayFromZero|TowardZero|AwayFromZero`; displays with exactly its scale.
- Text (LR 6.6): literal is `StringView from static`; implicit literal-to-`String` conversion allocates (refused
  in `noalloc`); `view as String` is the explicit copy (`String.from(view)` retired, accepted, fmt rewrites it);
  no integer indexing; `byteCount()` O(1), `scalarCount()` O(n),
  `slice(from, to)` → `Outcome[StringView from view]`, `scalarAt(i)` → `Outcome[Scalar]`; `text.Builder`
  `create/append/appendLine/appendScalar/finish`.
- Text essentials on `String` and `StringView` alike (X-595; none fails, none indexes by integer):
  `trim`/`trimStart`/`trimEnd` → `StringView from self` (no allocation; a loan of the text), `find(needle)` →
  `Int<0..>?` (a scalar offset; the empty needle is found at 0), `contains`, `startsWith`, `endsWith` → `Bool`,
  `split(sep)` → `List[String]` (every piece kept; an empty separator gives one piece), `lines()` →
  `List[String]` (a line ends at `\n`, a `\r\n` pair ends together; text after the last terminator is a line
  only when not empty), `replace(old, new)` → `String`, `asciiUpper`/`asciiLower` → `String` (ASCII only,
  X-596), `repeat(times: Int<0..>)` → `String`, `String.join(parts, sep)` → `String`; split, lines, replace,
  case, repeat and join allocate. Parsing is the conversion `text as Int or fail` (LR 6.13), never a text
  method.
- Collections (LR 6.7): `list[i]` needs a proof ([Proofs](06-proofs.md)); otherwise `get(i)` → `(alias T from
  self)?`, `at(i)` → `Outcome[T]`; `set(i, v)` → `Outcome[Void]`; `push(item)` → `Void`; `pop()` → `T?`;
  `clear()` empties the list and is rejected for linear elements (`E-OBL-001`, LR 10.7). `Map.set/get/count`,
  `Set.add` (→ `Bool`, true when new) `/contains/count`; iteration in first-insertion order, replacement keeps the
  position; neither has an `iterate()` or a cursor of its own in v0.1 (D-58, D-535); `for entry in map` binds a
  `(key, value)` tuple by read loan. `get` on a `Map[String, V]` takes a `StringView` key, so `counts.get(word)`
  takes a view or a `String` with no `copy`; `set` takes an owned key.
- Collections in a functional style (LR 6.7, X-479; `List`, `ListView`, beside the loops, which stay): `each`,
  `tryEach` (callback `editable fn mut`, effects allowed), `map`, `tryMap`, `filter`, `fold`, `tryFold`, `any`,
  `all`, `find`, `indexWhere`, `contains`, `sortedBy`, `enumerate`, `zip`, `take`, `skip`, `flatMap`,
  `chunked`, `groupBy`. Callbacks that only compute are `pure fn` (an I/O lambda is `E-TYP-040`); a `pure`
  function cannot call `each`/`tryEach` (`E-EFF-001`, LR 11.3); methods returning elements require
  `T implements Copyable` (`take`, `skip`, `enumerate`, `zip`, `chunked`, `sortedBy`, `groupBy`, `keys`, `values`, the
  set operations, `filter`); methods that only look do not. `try` callbacks return `Outcome[U]` (a plain value
  is `E-TYP-040`) and stop at the first failure; the call is handled on the line. `Map.each/keys/values`;
  `Set.each/union/intersection/difference`. `sortedBy` is stable; `groupBy` keys in first-appearance order;
  `chunked(size: Int<1..>)`. Reading traversals keep obligations in their owner; bind a fresh linear list
  before borrowing it. Non-failing map may produce linear results; tryMap results and fold accumulators
  must be non-linear (LR 10.7).
- `Deque[T]` and `SortedSet[T]` are written in Fortis in `fortis.collections` and imported from it (LR 6.7, X-576,
  X-577): built with `default()`/`fromList(items)` (variadic `of` is `List`'s alone); not `Copyable` (`copy` is
  `E-OWN-002`; `toList()` copies the items and `fromList` consumes an owned list); `Deque` pushes and pops at both ends amortised O(1) and holds
  resources; `SortedSet` needs `T implements Comparable`, keeps members ascending, each once, with `between(low, high)`,
  `union`, `intersection`, `difference`; a `for` over either, or over any class implementing `Iterable[T, I]`,
  walks its cursor (D-545). `Array.get(i)` → `(alias T from self)?` (null outside `[0, N)`), `asView()` →
  `ListView[T] from self` (X-578); `Array.filled` writes its size on the call, `Array[Int, 3].filled(7)`
  (D-399).
- Keys (LR 6.7, X-615): every written or inferred `Set[T]`, `Map[K, V]` requires `Hashable` and `Equatable` on the key
  or element, `E-TYP-090` even when empty, inside nested types and after instantiation; generic code needs
  `where K implements Hashable` (which grants `Equatable`); a class has no interface `where` bound in v0.1, so
  a field `Map[K, V]` over the class's own unbounded parameter is rejected; tuples and anonymous records are
  never keys; an enum is a key only by derivation or implementation. Make a key with `derive Equatable,
  Hashable`; a hand-written `hash` writes nothing (`Hasher` has no members in Language 1.0), so membership is
  decided by `equals` alone (X-516).
- Optionals (LR 6.8): `T` → `T?` implicit; `??` lazy; `a?.f`; `match` with `present(x)` / `null`; narrowing after
  `x != null`, after `if x == null { return … }`, and in `x != null && e`; `maybe.map(fn(x) => …)` → `U?` is
  the one method called with `.` on a `T?` without narrowing (`null` stays `null`; X-479).
- Optional method calls (LR 6.8, X-632): `a?.m(args)` evaluates `a` once; null skips arguments and returns null.
  Present calls `m`; `U` becomes `U?`, `U?` stays `U?`. Chains, mixin and interface methods resolve as after a
  null test. Only read receivers and non-`try` methods in v0.1 (`E-TYP-040` otherwise). Effects still count;
  returned loans and views retain the receiver's owner through optional injection and chaining (LR 7.6,
  LR 7.7). The formatter keeps `?.m()` and `map(fn(x) => x.m())` as written.
- Tuples `t.0`; records `{ name: a }`; `VariantMap[E, V]` literal `{ Found: 200, NotFound: 404 }` with one entry
  per variant; `(fn(Int) Bool)?` is an optional function (LR 6.9).
- Newtypes (LR 6.9): `type Name = Repr` is always nominal with a private representation. Build with `raw as
  Name` (fallible unless proved, LR 6.13) or `Name(x)`, judged as a call: a literal proved in range, a value of
  the representation type, or a value whose interval is proved; unproved is `E-REF-001`, another type
  `E-TYP-040` (X-511); the retired `Name.validate(raw)` stays accepted. Implements Equatable, Hashable,
  Comparable and Displayable exactly when the representation does, whatever it is (X-511); trivial over a
  trivial representation. Unwrap with `name as Rep` in the declaring module only (total, keeps the
  representation's refinement; `E-NAM-004` outside; X-552); a linear representation wraps only when proved
  (`E-OBL-001` otherwise). Collection newtypes (X-581): reading methods (`self`) visible everywhere, editing
  and consuming ones (`editable self`, `owned self`) only inside the declaring module (`E-NAM-004` outside);
  reading iteration follows the representation; a method returning `List[T]` returns `List[T]`; no associated
  functions, indexing, conversion or interchange. A newtype declares no `convert` member.
- Generics (LR 6.10): `[T]`, `where T implements I` after the return type; type arguments are always inferred,
  never written at a call. A written expected type fixes the parameters it names first; arguments fix the
  rest (disagreeing arguments join to their common type, two refinements of one base to the hull, else
  `E-TYP-040`; `null` fixes nothing). A parameter the result names inside a type argument takes an integer argument's
  base type: `List.of(small)` with `small: Int<0..3>` is a `List[Int]`; write `let xs: List[Int<0..9>] =
  List.of(small, big)` to keep a refinement (X-513). In a body `x: T` is a loan: returning, storing or passing
  it `owned` needs a marker (`E-OWN-001`); `copy x` needs `T implements Copyable` (`E-OWN-002` without); a
  loaned `T` a call returns never becomes owned without that bound. No associated call on `T` (`E-TYP-170`,
  X-180). Native lowering gives `T` exactly the Rust bounds its `where` names (X-515). Instantiation depth
  budget 32 (`E-LIM-001`). A generic class's constructor takes its arguments from the written type, else the
  expected type.
- Derivable (LR 6.10): `Equatable`, `Hashable`, `Comparable`, `Displayable`, `Encodable`,
  `Decodable`, `Codec`; derived `Comparable` orders a class by its fields in declaration order and an enum by
  variant order, then payload fields (X-516); `Default` (field initialisers first; a class gets `Name.default()`,
  no arguments; written without its call, `Name.default` is that function's value, X-516); `derive convert
  from S` (total borrowed field-wise; a missing, private or non-`Copyable` field or a fallible or consuming
  field conversion is `E-TYP-090`). `==`/`!=` call `equals`, `< <= > >=` call `compare`, and containers find a
  key by the same `equals`/`hash`, on every backend; a core type's `equals`/`compare` are ordinary callable
  methods; core `hash` is not callable (X-516). The retired `From[S]`/`to()`/`derive From[S]` stay accepted; `fortis fmt` rewrites them
  (LR 6.13).
- Canonical codec (LR 6.10): `toText()`/`fromText(input)` (JSON, declaration order), `toBytes()`/
  `fromBytes(input)`; failures `codec.invalid`, `codec.unknown_field`, or the refinement's own failure. Encodable
  derivation needs exported, encodable fields; secrets, authorities, loans, closures, futures and tasks are never
  encodable.
- Trivial (LR 6.11): primitives, `Decimal`, `Scalar`, time types, newtypes over trivial, enums/optionals/tuples/
  records of trivial, `trivial class`. Copyable: trivial, `String`, `Bytes`, `BigInt`, collections and
  aggregates of copyable. Not copyable: `Shared`, `Weak`, `Mutex`, atomics, loans, views, resources, `Future`,
  `Task`, `Outcome`, `Failure`, closures.
- Linear (LR 6.12): `Outcome[T]`, `Failure`, `Future[T]`, `Task[T]`, `Must`, `fn once` capturing one, and any
  type that can contain one. Type parameters are non-linear unless `T: linear`; a generic function holding a
  `T` by value declares it and consumes the value, and a user generic class or enum is instantiated at a linear
  argument only with `T: linear` on the corresponding parameter (`E-OBL-003`), written in a `where` clause at
  the end of its head, which accepts linear bounds only (X-611).

## Conversions (LR 6.13)

- One operator: `expression as Type`; binds tighter than arithmetic and comparison (`a as Decimal * b` is
  `(a as Decimal) * b`), chains left to right. A `<` directly after the target opens a refinement, so a
  comparison writes parentheses: `(n as Int) < 5` (X-550).
- Resolution is static, to exactly one conversion for the pair: a table row, the base-into-refinement row, or a
  member or explicit derivation a class or enum declares. No conversion is `E-TYP-180` (conversions never chain
  by search); an identity, two names of one range and a refinement to its base or a wider one are `E-TYP-181`.
- Total conversions have type `T`; fallible ones `Outcome[T]`, handled on the line (`or fail`, `or recover`,
  `catch`, `match`). Nothing rounds: `Float` into an integer or `Decimal` has no conversion; write
  `Decimal.fromFloat(x, scale, mode)`.
- Built-in rows: integer widening total, narrowing `conversion.out_of_range`; integer → `Decimal` total;
  `Decimal` → integer `conversion.not_integral`/`conversion.out_of_range`; `Int32` and smaller → `Float` total,
  `Int`/`Int64`/`UInt`/`UInt64` → `Float` `conversion.not_exact`; `Float`/`Float32` → `String` total, and there
  is no `String` → `Float` row; the integers, `Decimal`, `Bool`, `Scalar`, `Float`, `Float32` → `String` total
  (canonical text, allocates); `String`/`StringView` → an integer type, `Decimal`, `Bool` fallible
  (`conversion.parse`, `conversion.out_of_range`); `StringView` → `String` total (allocates);
  `Bytes`/`BytesView` → `String` `conversion.not_utf8`; `String`/`StringView` → `Bytes` total; `Scalar` → `Int`
  total; `Int` → `Scalar` `conversion.out_of_range`; base → refinement `refinement.out_of_range` (fallible even
  for a proved value); base → newtype total when proved, else `refinement.out_of_range`; newtype → its
  representation total, declaring module only (`E-NAM-004` outside).
- Failure payloads: `conversion.out_of_range` (`value`, `low`, `high`), `conversion.not_integral` (`value`),
  `conversion.not_exact` (`value`), `conversion.parse` (`text`), `conversion.not_utf8` (`position`).
- The operand is passed as to a read-loan parameter: a bare place is loaned, `move x as T` moves in, `copy x as
  T` converts a copy; into a new `String`/`Bytes` allocates (refused in `noalloc`).
- Members on a class or enum: `convert to T`, `try convert to T`, `convert from S(s)`, `try convert from S(s)`;
  `try` makes `x as T` an `Outcome[T]`; the body is a method body whose returns give the target (the declaring
  type for `convert from`); `self`/`s` is a read loan unless the head writes `owned`, and a consuming member is
  used as `move x as T`; visibility as a method's (no `export`: module only). One conversion per pair keyed by
  the whole type (`E-TYP-182`); a member for a pair the table decides is `E-TYP-183`; a generic class or enum
  and a newtype declare none (`E-TYP-184`); a pair of two foreign types has no body to hold a member.
- Retired, accepted in v0.1, rewritten by `fortis fmt` resolving by identity (X-552, X-553): `From[S]` (a
  legacy implementation supplies the pair of a total borrowed `convert from S`), `T.from(x)`, `x.to()` (kept
  with a note where no expected target is written), `derive From[S]` → `derive convert from S`,
  `String.from(view)` → `view as String`, `T.validate(x)` → `x as T`. A directly enclosing failure handler
  is dropped when the formatter proves the conversion always succeeds, leaving `x as T` or the plain value
  for a proved structural refinement. An unproved conversion keeps its handler. Only an outcome-valued position
  keeps the wrapper: `Outcome.success(x as T)` when total, `Outcome.success(x)` for a refinement already proved.

## Enum text derivation (LR 6.13)

X-631: `derive convert from String` on a non-generic payload-free enum
generates exact fallible borrowed pairs for both `String` and `StringView`; literals use the latter. Match
the whole name, ASCII case-insensitive, without trimming; call `trim()` explicitly. Failure is
`conversion.parse` with owned `text` and `variants` (names in declaration order, separated by `, `).
Non-ASCII scalars compare exactly. Either generated pair conflicts with a member for that pair
(`E-TYP-182`); both have the enum's visibility. No reverse conversion is generated; use Displayable for
interpolation. Payloads, generic enums, clashing names and other source types are `E-TYP-090`.
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
  let id = raw as CustomerId or fail
  return lookup(id)
}

test "lookup" {
  let found = fromInput(5)
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
  items.push(1)                       // reject: E-REF-003
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
  items.push(1)
  return move items as List[Int]<length: 0..3> or fail
}

test "three" {
  let items = three()
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

A `Set` element or `Map` key needs `Hashable` and `Equatable` even when the container is empty (LR 6.7).
Derive both on a class or enum whose fields support them. In generic functions, add `where K implements Hashable`;
that bound also grants its required `Equatable` interface.

```fortis
// reject: E-TYP-090
class Key { export n: Int }
func empty() Map[Key, Int] => Map.default()    // reject: E-TYP-090
```

```fortis
// accept
class Key derive Equatable, Hashable { export n: Int }
func empty() Map[Key, Int] => Map.default()
test "empty map" { expect empty().count() == 0 }
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
  let half = 5000 as BasisPoints
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

### E-TYP-090: enum names must be distinct under ASCII case comparison (LR 6.13)

```fortis
// reject: E-TYP-090
enum Size derive convert from String { // reject: E-TYP-090
  Large
  LARGE
}
```

```fortis
// accept
enum Size derive Equatable, convert from String {
  Small
  Large
}

test "trim and parse a size" {
  let size = "  LARGE  ".trim() as Size or recover Small
  expect size == Large
}
```

### E-TYP-040: test for null before editing the present value (LR 6.8)

```fortis
// reject: E-TYP-040
class Cell {
  func change(editable self) {}
}

func main() {
  let mut cell: Cell? = Cell()
  discard cell?.change() // reject: E-TYP-040
}
```

```fortis
// accept
class Cell {
  func change(editable self) {}
}

func main() {
  let mut cell: Cell? = Cell()
  if cell != null {
    cell.change()
  }
}
```

### E-OBL-001: consume the obligation on each optional path (LR 6.8)

Optional method arguments run only when the receiver is present. An argument cannot move an existing linear
value, including through a nested call or into a closure capture. Test for null and consume it on both branches.

```fortis
// reject: E-OBL-001
class Cell {
  func take(self, m: owned Must) Int { Must.discharge(move m)
    return 1 }
}
func main() {
  let cell: Cell? = null
  let m = Must.create()
  let n = cell?.take(move m) // reject: E-OBL-001
}
```

```fortis
// accept
class Cell {
  func take(self, m: owned Must) Int { Must.discharge(move m)
    return 1 }
}
func main() {
  let cell: Cell? = null
  let m = Must.create()
  match cell {
    present(c) => { let n = c.take(move m) }
    null => Must.discharge(move m)
  }
}
```
