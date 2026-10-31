"""The built-in conversion table of the `as` operator (Language Reference 6.13, X-475).

`x as T` names exactly one conversion for the pair (type of `x`, `T`): a row of this table for two built-in types,
or a `convert` member declared for the pair. The table is the whole of what converts automatically: a conversion
that cannot lose information is total and has type `T`; one that could lose information is fallible, has type
`Outcome[T]` and fails when it would. Nothing here rounds: a conversion that needs a rounding choice is a named
function with the choice written (`Decimal.fromFloat(x, scale, mode)`), and the table has no row for it.

The production frontend has the same table (compiler/crates/fortis-types/src/conversions.rs); the two agree row for
row, and the backends execute a row by its `kind`.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Tuple

from checker.types import INT_TYPES, Named, Refined, Type

# the integer values a binary64 float holds exactly: every value of a type within this range converts totally
FLOAT_EXACT = (-(2 ** 53), 2 ** 53)
TEXT = ("String", "StringView")
BYTES = ("Bytes", "BytesView")
# the types whose canonical text (what `Displayable` prints) a `String` conversion produces
DISPLAYED = set(INT_TYPES) | {"Decimal", "Bool", "Scalar", "Float", "Float32"}
# the kinds whose result is a new `String` or `Bytes`: the conversion allocates (6.6)
ALLOCATING = {"to_string", "view_to_string", "bytes_to_string", "string_to_bytes"}


@dataclass(frozen=True)
class Conversion:
    kind: str            # the row: int_widen, int_narrow, int_to_decimal, decimal_to_int, int_to_float, to_string,
                         # parse_int, parse_decimal, parse_bool, view_to_string, bytes_to_string, string_to_bytes,
                         # scalar_to_int, int_to_scalar
    source: str          # the base name of the source type
    target: str          # the base name of the target type
    fallible: bool

    @property
    def allocates(self) -> bool:
        return self.kind in ALLOCATING

    @property
    def intrinsic(self) -> str:
        """The backends' name for the row: `as:<kind>:<source>:<target>`."""
        return f"as:{self.kind}:{self.source}:{self.target}"


def base_of(t: Type) -> Optional[str]:
    """The base name a conversion is keyed by: a refinement converts as its base (6.3, a refinement is a subtype)."""
    if isinstance(t, Refined):
        return t.base.name
    if isinstance(t, Named) and not t.args:
        return t.name
    return None


def same_builtin(a: str, b: str) -> bool:
    """One built-in type under two names: `Int` and `Int64`, `UInt` and `UInt64`, `Byte` and `UInt8` (6.2)."""
    if a == b:
        return True
    return a in INT_TYPES and b in INT_TYPES and INT_TYPES[a] == INT_TYPES[b]


def rounding_hint(src: str, dst: str) -> Optional[str]:
    """The named function a `Float` source needs: a conversion would have to choose how to round (6.5)."""
    if src in ("Float", "Float32") and (dst in INT_TYPES or dst == "Decimal"):
        return "a Float rounds by a written choice: `Decimal.fromFloat(x, scale, mode)` (6.5), and from the Decimal onward"
    return None


def resolve(src: str, dst: str) -> Optional[Conversion]:
    """The built-in conversion from `src` to `dst` (base names), or None. The identity is not a conversion: the
    caller rejects it before asking (E-TYP-181)."""
    if src in INT_TYPES and dst in INT_TYPES:
        (slo, shi), (dlo, dhi) = INT_TYPES[src], INT_TYPES[dst]
        if dlo <= slo and shi <= dhi:
            return Conversion("int_widen", src, dst, False)
        return Conversion("int_narrow", src, dst, True)
    if src in INT_TYPES and dst == "Decimal":
        return Conversion("int_to_decimal", src, dst, False)
    if src == "Decimal" and dst in INT_TYPES:
        return Conversion("decimal_to_int", src, dst, True)
    if src in INT_TYPES and dst == "Float":
        lo, hi = INT_TYPES[src]
        exact = FLOAT_EXACT[0] <= lo and hi <= FLOAT_EXACT[1]
        return Conversion("int_to_float", src, dst, not exact)
    if src in DISPLAYED and dst == "String":
        return Conversion("to_string", src, dst, False)
    if src in TEXT and dst in INT_TYPES:
        return Conversion("parse_int", src, dst, True)
    if src in TEXT and dst == "Decimal":
        return Conversion("parse_decimal", src, dst, True)
    if src in TEXT and dst == "Bool":
        return Conversion("parse_bool", src, dst, True)
    if src == "StringView" and dst == "String":
        return Conversion("view_to_string", src, dst, False)
    if src in BYTES and dst == "String":
        return Conversion("bytes_to_string", src, dst, True)
    if src in TEXT and dst == "Bytes":
        return Conversion("string_to_bytes", src, dst, False)
    if src == "Scalar" and dst == "Int":
        return Conversion("scalar_to_int", src, dst, False)
    if src == "Int" and dst == "Scalar":
        return Conversion("int_to_scalar", src, dst, True)
    return None


def int_bounds_of(name: str) -> Tuple[int, int]:
    return INT_TYPES[name]
