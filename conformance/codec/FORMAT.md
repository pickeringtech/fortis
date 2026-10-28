# Fortis canonical formats, version 1

The two canonical encodings of Language Reference 6.10, published with the conformance suite. Both are
deterministic: one value has exactly one encoding. Encoding follows the value; decoding follows the expected
type, runs refinements and nominal validation, and fails with `codec.invalid` (malformed input, a value that does
not fit its type, trailing data), `codec.unknown_field` (text only: an object key the type does not declare) or
the refinement's own failure (`refinement.out_of_range`). The reference implementation is `interpreter/codec.py`;
golden files are in `conformance/codec/golden/`.

## Canonical text (`toText` / `fromText`)

JSON (RFC 8259) with no insignificant whitespace. Decoding accepts any RFC 8259 whitespace; encoding never emits it.

| Fortis type | Text |
|---|---|
| `Int`, `Int8`…`Int64`, `UInt`, `UInt8`…`UInt64`, `Byte` | integer, decimal digits, `-` for negatives |
| `Bool` | `true` / `false` |
| `Float`, `Float32` | the shortest digits that read back to the same value, with a fractional part (`2000.0`, `0.1`); `"NaN"`, `"Infinity"`, `"-Infinity"` as strings |
| `Decimal` | string of the value at its exact scale (`"12.50"`) |
| `String` | string; only `"`, `\` and U+0000–U+001F are escaped (`\"`, `\\`, `\n`, `\r`, `\t`, `\b`, `\f`, else `\u00XX`); every other scalar is written as UTF-8 |
| `Scalar` | string of one scalar |
| `Bytes` | string, base64 (RFC 4648 §4, with padding) |
| `Duration` | integer nanoseconds |
| `Timestamp` | string, RFC 3339 in UTC, seconds always, fraction only when non-zero with trailing zeros removed (`"1970-01-01T00:00:01.5Z"`) |
| `List[T]`, `Array[T, N]`, `Set[T]`, `FixedList[T, N]` | array; a set in its iteration order (first insertion) |
| `Map[K, V]`, `OrderedMap[K, V]`, `FixedMap[K, V, N]` | array of `[key, value]` pairs in iteration order (keys need not be strings) |
| `T?` | `null` or the value (a nested optional `(T?)?` is not Encodable) |
| `(A, B, …)` | array |
| record `{ a: A, … }`, class | object, keys in declaration order |
| enum variant without payload | string of the variant name |
| enum variant with payload | object with one key, the variant name, whose value is the payload object |
| `type N = R` | the encoding of `R`; decoding validates the refinement |

A class field missing from the input takes its declared initialiser, else `null` when its type is optional,
else decoding fails `codec.invalid`.

## Canonical binary (`toBytes` / `fromBytes`)

A 4-byte header `46 54 42 01` (`FTB`, version 1), then one tagged value. Integers in tags and lengths are unsigned
LEB128; signed integers are zigzag-encoded first (`n >= 0 → 2n`, `n < 0 → -2n - 1`).

| Tag | Value | Body |
|---|---|---|
| `00` | null (`T?` absent) | — |
| `01` / `02` | `false` / `true` | — |
| `03` | integer (every integer type) | zigzag LEB128 |
| `04` | `Float` | 8 bytes IEEE 754 binary64, little-endian |
| `05` | `Decimal` | sign byte (0/1), scale byte (0–28), coefficient LEB128 |
| `06` | `String` | byte length LEB128, UTF-8 |
| `07` | `Bytes` | length LEB128, bytes |
| `08` | `List`, `Array`, `FixedList` | count LEB128, elements |
| `09` | `Map`, `OrderedMap`, `FixedMap` | count LEB128, then key and value per entry in iteration order |
| `0A` | record, class | field count LEB128, values in declaration order (no names) |
| `0B` | enum variant | variant index in declaration order LEB128, payload count LEB128, payload values in order |
| `0C` | `Duration` | zigzag LEB128 nanoseconds |
| `0D` | `Timestamp` | zigzag LEB128 nanoseconds since the epoch |
| `0E` | `Scalar` | code point LEB128 |
| `0F` | `Float32` | 4 bytes IEEE 754 binary32, little-endian |
| `10` | `Set` | count LEB128, elements in iteration order |
| `11` | tuple | arity LEB128, elements |

A present optional is the value's own encoding (no wrapper); a newtype is its representation. Decoding checks
every tag against the expected type, every count against the type (tuple arity, array size, field count),
and rejects a missing header, truncated input, invalid UTF-8, a non-scalar code point, duplicate set
elements or map keys, and trailing bytes, all with `codec.invalid`.

## Types without values yet, and types outside the codec

`FixedList`, `FixedMap` and `OrderedMap` are codec-eligible, with the mappings above, but the core library
declares no operations for them yet, so no value of them can be constructed and their golden files do not exist.
`BigInt` is declared without operations and is not Encodable in version 1. `Instant` is a monotonic clock reading
that means nothing outside its process and is never Encodable. The excluded kinds of Language Reference 6.10
(`Secret[T]`, capabilities, loans and views, resources such as `File`, closures, futures and tasks) are rejected
at derivation with `E-TYP-090`; the fixtures are `conformance/programs/codec_reject_*.ft`.
