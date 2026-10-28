"""The canonical codec (Language Reference 6.10; DEL-29): `toText`/`fromText` and `toBytes`/`fromBytes`.

Both formats are published in `conformance/codec/FORMAT.md`. Encoding follows the value; decoding follows the
expected type (a descriptor built by the interpreter from the declaration), running refinements and nominal
validation, and fails with `codec.invalid`, `codec.unknown_field` or the refinement's own failure.

Descriptors are tuples:
  ("int", lo, hi) ("bool",) ("float",) ("float32",) ("decimal",) ("string", lo, hi) ("scalar",) ("bytes",)
  ("duration",) ("timestamp",) ("list", d) ("array", d, n) ("set", d) ("map", kd, vd) ("opt", d) ("tuple", [d])
  ("record", [(name, d)]) ("class", ClassInfo, {field: d}) ("enum", EnumInfo, {variant: [(field, d)]})
  ("newtype", name, d, lo, hi) ("sized", d, lo, hi) for a length-refined collection
"""
from __future__ import annotations

import base64
import math
import struct
from typing import Any, Callable, Dict, List, Optional, Tuple

from interpreter import numeric
from interpreter.values import EnumValue, Failure, FortisError, Instance, ListVal

MAGIC = b"FTB\x01"                      # "FTB", format version 1

# binary tags
T_NULL, T_FALSE, T_TRUE, T_INT, T_FLOAT, T_DECIMAL, T_STRING, T_BYTES = 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07
T_LIST, T_MAP, T_RECORD, T_VARIANT, T_DURATION, T_TIMESTAMP, T_SCALAR, T_FLOAT32, T_SET, T_TUPLE = 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11


class Invalid(Exception):
    def __init__(self, code: str, detail: str):
        super().__init__(detail)
        self.code, self.detail = code, detail


def bad(detail: str):
    raise Invalid("codec.invalid", detail)


# ---------------------------------------------------------------- values the codec meets
def _kinds():
    from interpreter import intrinsics as I
    from interpreter.eval import Scalar
    return I, Scalar


def float_text(x: float) -> str:
    if math.isnan(x): return '"NaN"'
    if math.isinf(x): return '"Infinity"' if x > 0 else '"-Infinity"'
    return numeric.fmt_float(x)


def json_string(s: str) -> str:
    out = ['"']
    for c in s:
        o = ord(c)
        if c == '"': out.append('\\"')
        elif c == "\\": out.append("\\\\")
        elif c == "\n": out.append("\\n")
        elif c == "\r": out.append("\\r")
        elif c == "\t": out.append("\\t")
        elif c == "\b": out.append("\\b")
        elif c == "\f": out.append("\\f")
        elif o < 0x20: out.append(f"\\u{o:04x}")
        else: out.append(c)
    out.append('"')
    return "".join(out)


# ---------------------------------------------------------------- canonical text
def to_text(v) -> str:
    I, Scalar = _kinds()
    if v is None: return "null"
    if isinstance(v, bool): return "true" if v else "false"
    if isinstance(v, int): return str(v)
    if isinstance(v, float): return float_text(v)
    if isinstance(v, numeric.F32): return float_text(v.v)
    if isinstance(v, str): return json_string(v)
    if isinstance(v, Scalar): return json_string(v.text)
    if isinstance(v, numeric.Dec): return json_string(v.display())
    if isinstance(v, numeric.Dur): return str(v.ns)
    if isinstance(v, I.TimestampVal): return json_string(v.display())
    if isinstance(v, I.BytesVal): return json_string(base64.b64encode(v.data).decode("ascii"))
    if isinstance(v, I.MapVal): return "[" + ",".join(f"[{to_text(k)},{to_text(x)}]" for k, x in v.entries.values()) + "]"
    if isinstance(v, I.SetVal): return "[" + ",".join(to_text(x) for x in v.items.values()) + "]"
    if isinstance(v, ListVal): return "[" + ",".join(to_text(x) for x in v.items) + "]"
    if isinstance(v, tuple): return "[" + ",".join(to_text(x) for x in v) + "]"
    if isinstance(v, dict): return "{" + ",".join(f"{json_string(k)}:{to_text(x)}" for k, x in v.items()) + "}"
    if isinstance(v, Instance): return "{" + ",".join(f"{json_string(k)}:{to_text(v.fields[k])}" for k in v.cls.fields) + "}"
    if isinstance(v, EnumValue):
        if not v.payload: return json_string(v.variant)
        return "{" + json_string(v.variant) + ":{" + ",".join(f"{json_string(k)}:{to_text(x)}" for k, x in v.payload.items()) + "}}"
    raise FortisError(f"{type(v).__name__} is not Encodable")


class _Reader:
    """A strict JSON reader: the canonical text has no insignificant whitespace, but decoding accepts RFC 8259 input."""
    def __init__(self, s: str):
        self.s, self.i = s, 0

    def ws(self):
        while self.i < len(self.s) and self.s[self.i] in " \t\r\n":
            self.i += 1

    def value(self):
        self.ws()
        if self.i >= len(self.s): bad("unexpected end of input")
        c = self.s[self.i]
        if c == "{": return self.obj()
        if c == "[": return self.arr()
        if c == '"': return self.string()
        if self.s.startswith("true", self.i): self.i += 4; return True
        if self.s.startswith("false", self.i): self.i += 5; return False
        if self.s.startswith("null", self.i): self.i += 4; return None
        return self.number()

    def obj(self):
        self.i += 1
        out: List[Tuple[str, Any]] = []
        self.ws()
        if self.s[self.i:self.i + 1] == "}":
            self.i += 1
            return ("object", out)
        while True:
            self.ws()
            k = self.string()
            self.ws()
            if self.s[self.i:self.i + 1] != ":": bad(f"expected ':' at {self.i}")
            self.i += 1
            out.append((k, self.value()))
            self.ws()
            c = self.s[self.i:self.i + 1]
            self.i += 1
            if c == "}": return ("object", out)
            if c != ",": bad(f"expected ',' or '}}' at {self.i - 1}")

    def arr(self):
        self.i += 1
        out = []
        self.ws()
        if self.s[self.i:self.i + 1] == "]":
            self.i += 1
            return out
        while True:
            out.append(self.value())
            self.ws()
            c = self.s[self.i:self.i + 1]
            self.i += 1
            if c == "]": return out
            if c != ",": bad(f"expected ',' or ']' at {self.i - 1}")

    def string(self):
        if self.s[self.i:self.i + 1] != '"': bad(f"expected a string at {self.i}")
        self.i += 1
        out = []
        while True:
            if self.i >= len(self.s): bad("unterminated string")
            c = self.s[self.i]
            if c == '"':
                self.i += 1
                return "".join(out)
            if c == "\\":
                e = self.s[self.i + 1:self.i + 2]
                m = {'"': '"', "\\": "\\", "/": "/", "b": "\b", "f": "\f", "n": "\n", "r": "\r", "t": "\t"}
                if e in m:
                    out.append(m[e]); self.i += 2
                elif e == "u":
                    h = self.s[self.i + 2:self.i + 6]
                    try:
                        cp = int(h, 16)
                    except ValueError:
                        bad("bad \\u escape")
                    self.i += 6
                    if 0xD800 <= cp < 0xDC00 and self.s[self.i:self.i + 2] == "\\u":
                        lo = int(self.s[self.i + 2:self.i + 6], 16)
                        cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)
                        self.i += 6
                    out.append(chr(cp))
                else:
                    bad("bad escape")
            elif ord(c) < 0x20:
                bad("control character in string")
            else:
                out.append(c); self.i += 1

    def number(self):
        j = self.i
        while j < len(self.s) and self.s[j] in "+-0123456789.eE":
            j += 1
        t = self.s[self.i:j]
        if not t: bad(f"unexpected {self.s[self.i]!r} at {self.i}")
        self.i = j
        try:
            if any(ch in t for ch in ".eE"):
                return ("float", float(t))
            return int(t)
        except ValueError:
            bad(f"bad number {t!r}")


def from_text(d, text: str, it):
    r = _Reader(text)
    tree = r.value()
    r.ws()
    if r.i != len(text): bad("trailing characters after the value")
    return _build_text(d, tree, it)


def _check_int(d, n):
    """("int", type_lo, type_hi, refined_lo, refined_hi): outside the type's range the input is malformed;
    outside a refinement the value fails the refinement (6.3)."""
    tlo, thi = d[1], d[2]
    rlo, rhi = (d[3], d[4]) if len(d) > 3 else (None, None)
    if (tlo is not None and n < tlo) or (thi is not None and n > thi):
        raise Invalid("codec.invalid", f"{n} does not fit {tlo}..{thi}")
    if (rlo is not None and n < rlo) or (rhi is not None and n > rhi):
        raise Invalid("refinement.out_of_range", f"{n} outside {rlo}..{rhi}")
    return n


def _sized(d, v):
    I, _ = _kinds()
    n = len(v.items) if isinstance(v, (ListVal, I.SetVal)) else len(v.entries)
    if (d[2] is not None and n < d[2]) or (d[3] is not None and n > d[3]):
        raise Invalid("refinement.out_of_range", f"{n} elements outside {d[2]}..{d[3]}")
    return v


def _build_text(d, t, it):
    I, Scalar = _kinds()
    k = d[0]
    if k == "sized":
        return _sized(d, _build_text(d[1], t, it))
    if k == "opt":
        return None if t is None else _build_text(d[1], t, it)
    if t is None: bad(f"null where {k} is expected")
    if k == "int":
        if not isinstance(t, int) or isinstance(t, bool): bad("expected an integer")
        return _check_int(d, t)
    if k == "bool":
        if not isinstance(t, bool): bad("expected true or false")
        return t
    if k in ("float", "float32"):
        if isinstance(t, tuple) and t[0] == "float": x = t[1]
        elif isinstance(t, int) and not isinstance(t, bool): x = float(t)
        elif t in ("NaN", "Infinity", "-Infinity"): x = float(t.replace("Infinity", "inf"))
        else: bad("expected a number")
        return x if k == "float" else numeric.F32.of(x)
    if k == "decimal":
        if not isinstance(t, str): bad("expected a decimal string")
        try:
            return numeric.Dec.parse(t + "d")
        except Exception:  # noqa: BLE001
            bad(f"bad decimal {t!r}")
    if k == "string":
        if not isinstance(t, str): bad("expected a string")
        n = len(t)
        if (d[1] is not None and n < d[1]) or (d[2] is not None and n > d[2]):
            raise Invalid("refinement.out_of_range", f"string of {n} scalars outside {d[1]}..{d[2]}")
        return t
    if k == "scalar":
        if not isinstance(t, str) or len(t) != 1: bad("expected a one-scalar string")
        return Scalar(t)
    if k == "bytes":
        if not isinstance(t, str): bad("expected base64 text")
        try:
            return I.BytesVal(base64.b64decode(t, validate=True))
        except Exception:  # noqa: BLE001
            bad("bad base64")
    if k == "duration":
        if not isinstance(t, int) or isinstance(t, bool): bad("expected nanoseconds")
        try:
            return numeric.Dur.make(t)
        except numeric.Failure:
            raise Invalid("numeric.overflow", "duration out of range")
    if k == "timestamp":
        if not isinstance(t, str): bad("expected an RFC 3339 string")
        return I.TimestampVal(parse_rfc3339(t))
    if k in ("list", "array", "set"):
        if not isinstance(t, list): bad("expected an array")
        items = [_build_text(d[1], x, it) for x in t]
        if k == "array":
            if len(items) != d[2]: bad(f"expected {d[2]} elements")
            return I.ArrayVal(items)
        if k == "set":
            s = I.SetVal()
            for x in items:
                if not I.set_add(it, s, [x]): bad("duplicate set element")
            return s
        return ListVal(items)
    if k == "map":
        if not isinstance(t, list): bad("expected an array of pairs")
        m = I.MapVal()
        for pair in t:
            if not isinstance(pair, list) or len(pair) != 2: bad("expected a [key, value] pair")
            key = _build_text(d[1], pair[0], it)
            if I.freeze(key) in m.entries: bad("duplicate map key")
            I.map_set(it, m, [key, _build_text(d[2], pair[1], it)])
        return m
    if k == "tuple":
        if not isinstance(t, list) or len(t) != len(d[1]): bad(f"expected {len(d[1])} elements")
        return tuple(_build_text(x, y, it) for x, y in zip(d[1], t))
    if k == "record":
        return _fields_text(dict(d[1]), t, it, lambda f: dict(f))
    if k == "class":
        cls = d[1]
        return _fields_text(d[2], t, it, lambda f: Instance(cls, f), defaults=lambda n: it.class_default(cls, n))
    if k == "enum":
        en = d[1]
        if isinstance(t, str):
            if t not in en.variants or d[2].get(t): bad(f"no payload-free variant {t!r}")
            return EnumValue(en.name, t, {})
        if isinstance(t, tuple) and t[0] == "object" and len(t[1]) == 1:
            vname, body = t[1][0]
            if vname not in en.variants: bad(f"no variant {vname!r}")
            payload = _fields_text(dict(d[2][vname]), body, it, lambda f: f)
            return EnumValue(en.name, vname, payload)
        bad("expected a variant")
    if k == "newtype":
        inner = _build_text(d[2], t, it)
        return _check_int(("int", None, None, d[3], d[4]), inner) if isinstance(inner, int) and not isinstance(inner, bool) else inner
    raise FortisError(f"no decoder for {k}")


def _fields_text(fields: Dict[str, Any], t, it, make: Callable, defaults: Callable = None):
    if not (isinstance(t, tuple) and t[0] == "object"): bad("expected an object")
    got: Dict[str, Any] = {}
    for key, val in t[1]:
        if key not in fields:
            raise Invalid("codec.unknown_field", key)
        if key in got: bad(f"duplicate field {key!r}")
        got[key] = _build_text(fields[key], val, it)
    out = {}
    for name_, fd in fields.items():
        if name_ in got:
            out[name_] = got[name_]
        elif defaults is not None and defaults(name_) is not None:
            out[name_] = defaults(name_)
        elif fd[0] == "opt":
            out[name_] = None
        else:
            bad(f"missing field {name_!r}")
    return make(out)


def parse_rfc3339(t: str) -> int:
    import datetime
    import re
    m = re.fullmatch(r"(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z", t)
    if not m: bad(f"not an RFC 3339 UTC timestamp: {t!r}")
    y, mo, da, h, mi, s, frac = m.groups()
    try:
        base = datetime.datetime(int(y), int(mo), int(da), int(h), int(mi), int(s))
    except ValueError:
        bad(f"bad date {t!r}")
    secs = int((base - datetime.datetime(1970, 1, 1)).total_seconds())
    return secs * 1_000_000_000 + (int(frac.ljust(9, "0")) if frac else 0)


# ---------------------------------------------------------------- canonical binary
def _varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _zigzag(n: int) -> int:
    return (n << 1) if n >= 0 else ((-n << 1) - 1)


def to_bytes(v, it) -> bytes:
    return MAGIC + _enc(v, it)


def _enc(v, it) -> bytes:
    I, Scalar = _kinds()
    if v is None: return bytes([T_NULL])
    if isinstance(v, bool): return bytes([T_TRUE if v else T_FALSE])
    if isinstance(v, int): return bytes([T_INT]) + _varint(_zigzag(v))
    if isinstance(v, float): return bytes([T_FLOAT]) + struct.pack("<d", v)
    if isinstance(v, numeric.F32): return bytes([T_FLOAT32]) + struct.pack("<f", v.v)
    if isinstance(v, str):
        b = v.encode("utf-8")
        return bytes([T_STRING]) + _varint(len(b)) + b
    if isinstance(v, Scalar): return bytes([T_SCALAR]) + _varint(ord(v.text))
    if isinstance(v, numeric.Dec): return bytes([T_DECIMAL, 1 if v.neg else 0, v.scale]) + _varint(v.coeff)
    if isinstance(v, numeric.Dur): return bytes([T_DURATION]) + _varint(_zigzag(v.ns))
    if isinstance(v, I.TimestampVal): return bytes([T_TIMESTAMP]) + _varint(_zigzag(v.ns))
    if isinstance(v, I.BytesVal): return bytes([T_BYTES]) + _varint(len(v.data)) + v.data
    if isinstance(v, I.MapVal): return bytes([T_MAP]) + _varint(len(v.entries)) + b"".join(_enc(k, it) + _enc(x, it) for k, x in v.entries.values())
    if isinstance(v, I.SetVal): return bytes([T_SET]) + _varint(len(v.items)) + b"".join(_enc(x, it) for x in v.items.values())
    if isinstance(v, ListVal): return bytes([T_LIST]) + _varint(len(v.items)) + b"".join(_enc(x, it) for x in v.items)
    if isinstance(v, tuple): return bytes([T_TUPLE]) + _varint(len(v)) + b"".join(_enc(x, it) for x in v)
    if isinstance(v, dict): return bytes([T_RECORD]) + _varint(len(v)) + b"".join(_enc(x, it) for x in v.values())
    if isinstance(v, Instance): return bytes([T_RECORD]) + _varint(len(v.cls.fields)) + b"".join(_enc(v.fields[k], it) for k in v.cls.fields)
    if isinstance(v, EnumValue):
        owners = [it.enums[v.enum]] if v.enum else [e for e in it.enums.values() if v.variant in e.variants]
        idxs = {list(e.variants).index(v.variant) for e in owners}
        if len(idxs) != 1:
            raise FortisError(f"the unprefixed variant {v.variant} reached the codec without its enum")
        idx = idxs.pop()
        return bytes([T_VARIANT]) + _varint(idx) + _varint(len(v.payload)) + b"".join(_enc(x, it) for x in v.payload.values())
    raise FortisError(f"{type(v).__name__} is not Encodable")


class _Bin:
    def __init__(self, data: bytes):
        self.d, self.i = data, 0

    def byte(self) -> int:
        if self.i >= len(self.d): bad("truncated input")
        b = self.d[self.i]
        self.i += 1
        return b

    def take(self, n: int) -> bytes:
        if self.i + n > len(self.d): bad("truncated input")
        out = self.d[self.i:self.i + n]
        self.i += n
        return out

    def varint(self) -> int:
        n, shift = 0, 0
        while True:
            b = self.byte()
            n |= (b & 0x7F) << shift
            if not b & 0x80:
                return n
            shift += 7
            if shift > 70: bad("varint too long")

    def zigzag(self) -> int:
        n = self.varint()
        return (n >> 1) if not n & 1 else -((n + 1) >> 1)

    def expect(self, tag: int, what: str):
        b = self.byte()
        if b != tag: bad(f"expected {what} (tag {tag:#04x}), found tag {b:#04x}")


def from_bytes(d, data: bytes, it):
    if data[:4] != MAGIC: bad("missing canonical binary header FTB\\x01")
    r = _Bin(data)
    r.i = 4
    v = _dec(d, r, it)
    if r.i != len(data): bad("trailing bytes after the value")
    return v


def _dec(d, r: _Bin, it):
    I, Scalar = _kinds()
    k = d[0]
    if k == "sized":
        return _sized(d, _dec(d[1], r, it))
    if k == "opt":
        if r.d[r.i:r.i + 1] == bytes([T_NULL]):
            r.i += 1
            return None
        return _dec(d[1], r, it)
    if k == "int":
        r.expect(T_INT, "an integer")
        return _check_int(d, r.zigzag())
    if k == "bool":
        b = r.byte()
        if b not in (T_FALSE, T_TRUE): bad("expected a boolean")
        return b == T_TRUE
    if k == "float":
        r.expect(T_FLOAT, "a float")
        return struct.unpack("<d", r.take(8))[0]
    if k == "float32":
        r.expect(T_FLOAT32, "a float32")
        return numeric.F32.of(struct.unpack("<f", r.take(4))[0])
    if k == "decimal":
        r.expect(T_DECIMAL, "a decimal")
        neg, scale = r.byte(), r.byte()
        coeff = r.varint()
        if neg not in (0, 1) or scale > 28 or coeff > numeric.MAX_COEFF: bad("bad decimal")
        return numeric.Dec(bool(neg) and coeff != 0, coeff, scale)
    if k == "string":
        r.expect(T_STRING, "a string")
        try:
            s = r.take(r.varint()).decode("utf-8")
        except UnicodeDecodeError:
            bad("invalid UTF-8")
        if (d[1] is not None and len(s) < d[1]) or (d[2] is not None and len(s) > d[2]):
            raise Invalid("refinement.out_of_range", f"string of {len(s)} scalars outside {d[1]}..{d[2]}")
        return s
    if k == "scalar":
        r.expect(T_SCALAR, "a scalar")
        cp = r.varint()
        if cp > 0x10FFFF or 0xD800 <= cp <= 0xDFFF: bad("not a Unicode scalar")
        return Scalar(chr(cp))
    if k == "bytes":
        r.expect(T_BYTES, "bytes")
        return I.BytesVal(r.take(r.varint()))
    if k == "duration":
        r.expect(T_DURATION, "a duration")
        try:
            return numeric.Dur.make(r.zigzag())
        except numeric.Failure:
            raise Invalid("numeric.overflow", "duration out of range")
    if k == "timestamp":
        r.expect(T_TIMESTAMP, "a timestamp")
        return I.TimestampVal(r.zigzag())
    if k in ("list", "array"):
        r.expect(T_LIST, "a list")
        items = [_dec(d[1], r, it) for _ in range(r.varint())]
        if k == "array":
            if len(items) != d[2]: bad(f"expected {d[2]} elements")
            return I.ArrayVal(items)
        return ListVal(items)
    if k == "set":
        r.expect(T_SET, "a set")
        s = I.SetVal()
        for _ in range(r.varint()):
            if not I.set_add(it, s, [_dec(d[1], r, it)]): bad("duplicate set element")
        return s
    if k == "map":
        r.expect(T_MAP, "a map")
        m = I.MapVal()
        for _ in range(r.varint()):
            key = _dec(d[1], r, it)
            if I.freeze(key) in m.entries: bad("duplicate map key")
            I.map_set(it, m, [key, _dec(d[2], r, it)])
        return m
    if k == "tuple":
        r.expect(T_TUPLE, "a tuple")
        if r.varint() != len(d[1]): bad("wrong tuple arity")
        return tuple(_dec(x, r, it) for x in d[1])
    if k in ("record", "class"):
        r.expect(T_RECORD, "a record")
        fields = dict(d[1]) if k == "record" else d[2]
        if r.varint() != len(fields): bad("wrong field count")
        vals = {n: _dec(fd, r, it) for n, fd in fields.items()}
        return dict(vals) if k == "record" else Instance(d[1], vals)
    if k == "enum":
        r.expect(T_VARIANT, "a variant")
        en = d[1]
        idx = r.varint()
        names = list(en.variants)
        if idx >= len(names): bad("variant index out of range")
        vname = names[idx]
        fields = d[2].get(vname, [])
        if r.varint() != len(fields): bad("wrong payload count")
        return EnumValue(en.name, vname, {n: _dec(fd, r, it) for n, fd in fields})
    if k == "newtype":
        inner = _dec(d[2], r, it)
        return _check_int(("int", None, None, d[3], d[4]), inner) if isinstance(inner, int) and not isinstance(inner, bool) else inner
    raise FortisError(f"no decoder for {k}")
