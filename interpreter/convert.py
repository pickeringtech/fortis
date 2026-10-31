"""The built-in conversions of the `as` operator, as the definitional interpreter runs them (Language Reference
6.13, X-475; the table is checker/conversions.py).

The checker has resolved every `x as T` to one row before anything runs; the interpreter runs the row the operand's
value and the target name select, which is the same row: the integer rows differ only in the target's bounds (a
widening never fails its range check), the text rows parse the canonical form of the target type, and the rest
are one value shape each. A total row gives the value; a fallible row gives the value or a `Failure`, as a `try`
call does (10.2), with the failures 6.13 names: `conversion.out_of_range`, `conversion.not_integral`,
`conversion.not_exact`, `conversion.parse` and `conversion.not_utf8`. The VM and the native runtime build the same
failures with the same payloads.
"""
from __future__ import annotations

import re
from typing import Any, Callable

from checker.types import INT_TYPES
from interpreter import numeric
from interpreter.values import Failure, FortisError, display

# the canonical text of an integer: what `Displayable` prints (no sign on zero, no leading zeros, no `+`)
INT_TEXT = re.compile(r"0|-?[1-9][0-9]*")
# the canonical text of a Decimal: an integer part as above and exactly the scale's fractional digits
DEC_TEXT = re.compile(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?")
MAX_SCALAR = 0x10FFFF


def failure(code: str, payload: dict, origin: str) -> Failure:
    return Failure(code, payload, origin=origin)


def is_int(v: Any) -> bool:
    return isinstance(v, int) and not isinstance(v, bool)


def to_int(x: int, target: str, origin: str):
    """An integer into the integer type `target`: the value when it lies in the target's range (a widening always
    does), else `conversion.out_of_range`."""
    lo, hi = INT_TYPES[target]
    if x < lo or x > hi:
        return failure("conversion.out_of_range", {"value": x, "low": lo, "high": hi}, origin)
    return x


def parse_decimal(text: str, origin: str):
    if not DEC_TEXT.fullmatch(text):
        return failure("conversion.parse", {"text": text}, origin)
    neg = text.startswith("-")
    body = text.lstrip("-")
    ip, _, fp = body.partition(".")
    # Bound the work before int(): arbitrary input must not hit Python's digit limit.
    if len(ip) + len(fp) > 29 or len(fp) > numeric.MAX_SCALE:
        return failure("conversion.parse", {"text": text}, origin)
    coeff, scale = int(ip + fp), len(fp)
    # a Decimal displays no negative zero, more than 28 fractional digits or a coefficient beyond 96 bits: such a
    # text is the canonical form of no Decimal
    if (neg and coeff == 0) or coeff > numeric.MAX_COEFF or scale > numeric.MAX_SCALE:
        return failure("conversion.parse", {"text": text}, origin)
    return numeric.Dec(neg, coeff, scale)


def convert(v: Any, target: str, origin: str, scalar: Callable[[str], Any], bytes_of: Callable[[bytes], Any]):
    """`v as target` for a built-in target type named `target` (no refinement): the row the pair selects."""
    if is_int(v):
        if target in INT_TYPES:
            return to_int(v, target, origin)
        if target == "Decimal":
            return numeric.Dec.make(v < 0, abs(v), 0)
        if target == "Float":
            f = float(v)
            if int(f) != v:
                return failure("conversion.not_exact", {"value": v}, origin)
            return f
        if target == "String":
            return str(v)
        if target == "Scalar":
            if 0 <= v <= MAX_SCALAR and not 0xD800 <= v <= 0xDFFF:
                return scalar(chr(v))
            return failure("conversion.out_of_range", {"value": v, "low": 0, "high": MAX_SCALAR}, origin)
    elif isinstance(v, bool):
        if target == "String":
            return "true" if v else "false"
    elif isinstance(v, str):
        if target in INT_TYPES:
            if not INT_TEXT.fullmatch(v):
                return failure("conversion.parse", {"text": v}, origin)
            # The shared Rust carrier is i128. Preserve larger canonical integers as text in
            # the failure payload, without parsing an unbounded integer (LR 6.13, X-690).
            digits = v.removeprefix("-")
            n = int(v) if len(digits) <= 39 else None
            if n is None or not -(2 ** 127) <= n < 2 ** 127:
                lo, hi = INT_TYPES[target]
                return failure("conversion.out_of_range", {"value": v, "low": lo, "high": hi}, origin)
            return to_int(n, target, origin)
        if target == "Decimal":
            return parse_decimal(v, origin)
        if target == "Bool":
            if v in ("true", "false"):
                return v == "true"
            return failure("conversion.parse", {"text": v}, origin)
        if target == "String":
            return str(v)                                   # an owned copy of the view (6.6)
        if target == "Bytes":
            return bytes_of(v.encode("utf-8"))
    elif isinstance(v, numeric.Dec):
        if target in INT_TYPES:
            q, r = divmod(v.coeff, 10 ** v.scale)
            if r != 0:
                return failure("conversion.not_integral", {"value": v.display()}, origin)
            return to_int(-q if v.neg else q, target, origin)
        if target == "String":
            return v.display()
    elif isinstance(v, (float, numeric.F32)):
        if target == "String":
            return display(v)
    elif hasattr(v, "data") and isinstance(getattr(v, "data"), bytes):
        if target == "String":
            try:
                return v.data.decode("utf-8")
            except UnicodeDecodeError as e:
                return failure("conversion.not_utf8", {"position": e.start}, origin)
    elif hasattr(v, "text") and isinstance(getattr(v, "text"), str) and len(v.text) == 1:
        if target == "Int":
            return ord(v.text)
        if target == "String":
            return v.text
    raise FortisError(f"no built-in conversion of {display(v)} to {target}: the checker resolves every `as` (6.13)")
