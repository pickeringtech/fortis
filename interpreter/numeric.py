"""Numeric value semantics of the definitional interpreter (Language Reference 3.5, 6.4, 6.5, 15):
Float (binary64, strict IEEE, canonical NaN), Float32, Decimal (sign, 96-bit coefficient, scale
0..28, rust_decimal scale rules) and Duration (nanoseconds), with their display forms and the
checked intrinsics. Pure Python; every result is defined bit-exactly so that the VM and native
backends can be compared against it (LANG-06-04)."""
from __future__ import annotations

import math
import struct
from dataclasses import dataclass
from decimal import ROUND_HALF_EVEN, ROUND_HALF_UP, ROUND_DOWN, ROUND_UP, Decimal as PyDec, localcontext
from typing import Any, Optional

MAX_COEFF = 2 ** 96 - 1
MAX_SCALE = 28
INT64_MIN, INT64_MAX = -(2 ** 63), 2 ** 63 - 1


class Failure(Exception):
    """Raised by the intrinsics; the interpreter turns it into a Failure value."""
    def __init__(self, code: str, payload=None):
        super().__init__(code)
        self.code = code
        self.payload = payload or {}


# ---- Float ----------------------------------------------------------------------------
def canon(x: float) -> float:
    """Every NaN result is the positive quiet NaN with zero payload (6.5)."""
    return float("nan") if math.isnan(x) else x


def f64_op(op: str, a: float, b: float) -> float:
    if op == "+": return canon(a + b)
    if op == "-": return canon(a - b)
    if op == "*": return canon(a * b)
    if op == "/":
        if b == 0.0:
            if a == 0.0 or math.isnan(a): return float("nan")
            return math.copysign(float("inf"), a) * math.copysign(1.0, b)
        return canon(a / b)
    raise ValueError(op)


def f64_sqrt(x: float) -> float:
    return float("nan") if (x < 0 or math.isnan(x)) else math.sqrt(x)


def f64_bits(x: float) -> int:
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def total_order(x: float) -> int:
    """IEEE 754 totalOrder as a signed 64-bit integer key (6.5)."""
    b = f64_bits(x)
    if b >> 63:
        b ^= 0x7FFFFFFFFFFFFFFF
    return b - 2 ** 64 if b >= 2 ** 63 else b


def fmt_float(x: float) -> str:
    """Shortest round-trip digits, plain notation, always a fractional part; NaN, Infinity, -Infinity."""
    if math.isnan(x): return "NaN"
    if math.isinf(x): return "Infinity" if x > 0 else "-Infinity"
    r = repr(x)
    if "e" in r or "E" in r:
        d = PyDec(r)
        r = format(d, "f")
    if "." not in r:
        r += ".0"
    if r == "-0.0":
        return "-0.0"
    return r


@dataclass(frozen=True)
class F32:
    v: float

    @staticmethod
    def of(x: float) -> "F32":
        return F32(canon(struct.unpack("<f", struct.pack("<f", x))[0]))

    def display(self) -> str:
        x = self.v
        if math.isnan(x): return "NaN"
        if math.isinf(x): return "Infinity" if x > 0 else "-Infinity"
        for p in range(1, 10):
            s = format(x, f".{p}g")
            if struct.unpack("<f", struct.pack("<f", float(s)))[0] == x:
                break
        if "e" in s:
            s = format(PyDec(s), "f")
        if "." not in s:
            s += ".0"
        return s


def f32_op(op: str, a: F32, b: F32) -> F32:
    return F32.of(f64_op(op, a.v, b.v))


# ---- Decimal ---------------------------------------------------------------------------
@dataclass(frozen=True)
class Dec:
    """sign, coefficient (0..2^96-1) and scale (0..28); value = ±coefficient / 10^scale."""
    neg: bool
    coeff: int
    scale: int

    @staticmethod
    def parse(text: str) -> "Dec":
        t = text.rstrip("d").replace("_", "")
        neg = t.startswith("-")
        t = t.lstrip("-")
        if "." in t:
            ip, fp = t.split(".")
            return Dec.make(neg, int(ip + fp), len(fp))
        return Dec.make(neg, int(t), 0)

    @staticmethod
    def make(neg: bool, coeff: int, scale: int) -> "Dec":
        if coeff > MAX_COEFF or scale > MAX_SCALE:
            raise Failure("numeric.overflow", {"operation": "decimal"})
        return Dec(neg and coeff != 0, coeff, scale)

    def value(self) -> PyDec:
        v = PyDec(self.coeff).scaleb(-self.scale)
        return -v if self.neg else v

    @staticmethod
    def from_value(v: PyDec, scale: int, mode=ROUND_HALF_EVEN) -> "Dec":
        """Round v to `scale` fractional digits; fail if the coefficient does not fit 96 bits."""
        with localcontext() as ctx:
            ctx.prec = 80
            q = v.quantize(PyDec(1).scaleb(-scale), rounding=mode)
        neg = q < 0
        coeff = int(abs(q).scaleb(scale))
        return Dec.make(neg, coeff, scale)

    @staticmethod
    def fit(v: PyDec, scale: int) -> "Dec":
        """The largest scale <= `scale` whose coefficient fits (div, and overflow handling of add/mul)."""
        s = min(scale, MAX_SCALE)
        while s >= 0:
            with localcontext() as ctx:
                ctx.prec = 80
                q = v.quantize(PyDec(1).scaleb(-s), rounding=ROUND_HALF_EVEN)
            if int(abs(q).scaleb(s)) <= MAX_COEFF:
                return Dec(q < 0, int(abs(q).scaleb(s)), s)
            s -= 1
        raise Failure("numeric.overflow", {"operation": "decimal"})

    def display(self) -> str:
        digits = str(self.coeff).rjust(self.scale + 1, "0")
        if self.scale:
            digits = digits[:-self.scale] + "." + digits[-self.scale:]
        return ("-" if self.neg else "") + digits


ROUNDING = {"HalfEven": ROUND_HALF_EVEN, "HalfAwayFromZero": ROUND_HALF_UP, "TowardZero": ROUND_DOWN, "AwayFromZero": ROUND_UP}


def dec_op(op: str, a: Dec, b: Dec) -> Dec:
    with localcontext() as ctx:
        ctx.prec = 80
        va, vb = a.value(), b.value()
        if op == "add": return Dec.fit(va + vb, max(a.scale, b.scale))
        if op == "sub": return Dec.fit(va - vb, max(a.scale, b.scale))
        if op == "mul": return Dec.fit(va * vb, a.scale + b.scale)
        if op == "div":
            if b.coeff == 0: raise Failure("numeric.divide_by_zero", {})
            return Dec.fit(va / vb, MAX_SCALE)
        if op == "rem":
            if b.coeff == 0: raise Failure("numeric.divide_by_zero", {})
            q = (va / vb).to_integral_value(rounding=ROUND_DOWN)
            return Dec.fit(va - q * vb, max(a.scale, b.scale))
    raise ValueError(op)


def dec_round(a: Dec, scale: int, mode: str) -> Dec:
    if not 0 <= scale <= MAX_SCALE:
        raise Failure("numeric.overflow", {"operation": "round"})
    return Dec.from_value(a.value(), scale, ROUNDING[mode])


def dec_from_float(x: float, scale: int, mode: str) -> Dec:
    if math.isnan(x) or math.isinf(x) or not 0 <= scale <= MAX_SCALE:
        raise Failure("numeric.overflow", {"operation": "fromFloat"})
    return Dec.from_value(PyDec(repr(x)), scale, ROUNDING[mode])


def dec_cmp(a: Dec, b: Dec) -> int:
    va, vb = a.value(), b.value()
    return (va > vb) - (va < vb)


# ---- Duration --------------------------------------------------------------------------
UNITS = {"ns": 1, "us": 1_000, "ms": 1_000_000, "s": 1_000_000_000, "min": 60_000_000_000, "h": 3_600_000_000_000}


@dataclass(frozen=True)
class Dur:
    ns: int

    @staticmethod
    def parse(text: str) -> "Dur":
        t = text.replace("_", "")
        for u in ("min", "ms", "us", "ns", "h", "s"):
            if t.endswith(u):
                return Dur.make(int(t[:-len(u)]) * UNITS[u])
        raise ValueError(text)

    @staticmethod
    def make(ns: int) -> "Dur":
        if ns < INT64_MIN or ns > INT64_MAX:
            raise Failure("numeric.overflow", {"operation": "duration"})
        return Dur(ns)

    def display(self) -> str:
        """The largest unit that divides the value exactly (0 displays as 0s)."""
        if self.ns == 0:
            return "0s"
        for u in ("h", "min", "s", "ms", "us", "ns"):
            if self.ns % UNITS[u] == 0:
                return f"{self.ns // UNITS[u]}{u}"
        return f"{self.ns}ns"


def dur_op(op: str, a: Dur, b: Any) -> Dur:
    if op == "add": return Dur.make(a.ns + b.ns)
    if op == "sub": return Dur.make(a.ns - b.ns)
    if op == "multiply": return Dur.make(a.ns * b)
    raise ValueError(op)
