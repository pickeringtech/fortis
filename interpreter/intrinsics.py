"""Kernel intrinsics of the core library (DEL-26).

Every `trusted` signature of `fortis.prelude`, `fortis.text`, `fortis.collections` and `fortis.numeric` that has no
Fortis body is implemented here, in one registry the coverage check reads (`conformance/tools/library_check.py`):

  METHODS[(Type, name)]  a method: fn(interp, receiver, positional args) -> value
  ASSOC[(Type, name)]    an associated function: fn(interp, positional args) -> value
  FUNCS[name]            a module-level function: fn(interp, positional args) -> value

Each implementation honours the contract written above its signature in `library/fortis/*.ft`. A fallible
intrinsic returns a `Failure` value (the dynamic form of `Outcome`); a dynamic check outside the kernel's rules
raises `FortisError`. Derived operations with Fortis bodies (List.reversed, map, filter, …) are not here: the
interpreter runs their bodies.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, Tuple

from interpreter import numeric
from interpreter.values import (VOID, BuilderVal, Capability, ClassInfo, EnumValue, Failure, FortisError, Instance, ListVal, display)

METHODS: Dict[Tuple[str, str], Callable] = {}
ASSOC: Dict[Tuple[str, str], Callable] = {}
FUNCS: Dict[str, Callable] = {}

INT_MIN, INT_MAX = -(2 ** 63), 2 ** 63 - 1


def method(t: str, *names: str):
    def deco(fn):
        for n in names:
            METHODS[(t, n)] = fn
        return fn
    return deco


def assoc(t: str, *names: str):
    def deco(fn):
        for n in names:
            ASSOC[(t, n)] = fn
        return fn
    return deco


def func(*names: str):
    def deco(fn):
        for n in names:
            FUNCS[n] = fn
        return fn
    return deco


# ---------------------------------------------------------------- values the core adds
@dataclass
class MapVal:
    """`Map[K, V]`: a hash container; iteration order is first-insertion order (deterministic per build, 6.7)."""
    entries: Dict[Any, Tuple[Any, Any]] = field(default_factory=dict)
    custom_seq: int = 0     # the next key of an entry whose type writes its own equality: never reused after a removal


@dataclass
class SetVal:
    items: Dict[Any, Any] = field(default_factory=dict)
    custom_seq: int = 0


@dataclass
class ArrayVal(ListVal):
    """`Array[T, N]`: fixed size N."""


@dataclass
class CursorVal:
    items: List[Any]
    pos: int = 0


@dataclass
class AtomicVal:
    kind: str
    value: Any


@dataclass
class MutexVal:
    value: Any
    held: bool = False


@dataclass
class GuardVal:
    mutex: MutexVal


@dataclass
class TimestampVal:
    ns: int

    def display(self) -> str:
        """RFC 3339 in UTC, with a fractional second only when the instant has one (15)."""
        import datetime
        secs, nanos = divmod(self.ns, 1_000_000_000)
        base = (datetime.datetime(1970, 1, 1) + datetime.timedelta(seconds=secs)).strftime("%Y-%m-%dT%H:%M:%S")
        return base + (("." + f"{nanos:09d}".rstrip("0")) if nanos else "") + "Z"


@dataclass
class BytesVal:
    data: bytes


class ReadyVal:
    """The future of an `async` intrinsic that completes without suspending (Mutex.lock with the lock free)."""
    def __init__(self, value):
        self.value = value


def type_name(v) -> Optional[str]:
    """The core type of a runtime value, for intrinsic dispatch."""
    if isinstance(v, ArrayVal): return "Array"
    if isinstance(v, ListVal): return "List"
    if isinstance(v, bool): return "Bool"
    if isinstance(v, int): return "Int"
    if isinstance(v, str): return "String"
    if isinstance(v, BuilderVal): return "Builder"
    if isinstance(v, MapVal): return "Map"
    if isinstance(v, SetVal): return "Set"
    if isinstance(v, CursorVal): return "ListCursor"
    if isinstance(v, AtomicVal): return v.kind
    if isinstance(v, MutexVal): return "Mutex"
    if isinstance(v, GuardVal): return "MutexGuard"
    if isinstance(v, TimestampVal): return "Timestamp"
    if isinstance(v, BytesVal): return "BytesView"
    if isinstance(v, numeric.Dec): return "Decimal"
    if isinstance(v, numeric.Dur): return "Duration"
    if isinstance(v, Failure): return "Failure"
    if isinstance(v, Capability): return CAPABILITY_TYPES.get(v.kind)
    if isinstance(v, PathVal): return "Path"
    if isinstance(v, SecretVal): return "Secret"
    if isinstance(v, FileVal): return "File"
    n = type(v).__name__
    return {"Scalar": "Scalar", "SharedVal": "Shared", "WeakVal": "Weak", "ScopeVal": "Scope", "TaskVal": "Task"}.get(n)


ALIASES = {"String": ("StringView",), "List": ("ListView",), "Array": ("ListView",)}      # an `Array` read as a view (X-578)


def lookup_method(v, name: str) -> Optional[Callable]:
    t = type_name(v)
    if t is None:
        return None
    for tn in (t,) + ALIASES.get(t, ()):
        fn = METHODS.get((tn, name))
        if fn is not None:
            return fn
    return None


def freeze(v):
    """A hashable key with the equality of derived Equatable/Hashable (6.10): structural over fields and payloads."""
    if v is None or isinstance(v, (bool, int, str)):
        return (type(v).__name__, v)
    if isinstance(v, ListVal):
        return ("list", tuple(freeze(x) for x in v.items))
    if isinstance(v, tuple):
        return ("tuple", tuple(freeze(x) for x in v))
    if isinstance(v, EnumValue):
        return ("enum", v.variant, tuple((k, freeze(x)) for k, x in v.payload.items()))   # one key type per map
    if isinstance(v, Instance):
        if "equals" in v.cls.methods or "hash" in v.cls.methods:
            raise FortisError(f"{v.cls.name} implements Hashable by hand; hand-written hash is outside the kernel")
        return ("inst", v.cls.name, tuple((k, freeze(x)) for k, x in v.fields.items()))
    if isinstance(v, numeric.Dec):
        return ("dec", v.neg, v.coeff, v.scale)
    if isinstance(v, numeric.Dur):
        return ("dur", v.ns)
    if type(v).__name__ == "Scalar":
        return ("scalar", v.text)
    raise FortisError(f"{display(v)} is not Hashable")


def own_equality(it, v) -> bool:
    """Whether `v`'s type writes its own `equals` or `hash`, or nests a value that does: a container then finds the
    key by that equality, not by a structural key, because a hand-written `hash` writes nothing (`Hasher` has no
    members in Language 1.0) and equal values must still be found by `equals` (6.10, X-516; D-84)."""
    if isinstance(v, Instance):
        if "equals" in v.cls.methods or "hash" in v.cls.methods:
            return True
        return any(own_equality(it, x) for x in v.fields.values())
    if isinstance(v, EnumValue):
        en = it.enums.get(v.enum) if v.enum else None
        if en is None and not v.enum:
            owners = [e for e in it.enums.values() if v.variant in e.variants]
            en = owners[0] if len(owners) == 1 else None
        if en is not None and ("equals" in en.methods or "hash" in en.methods):
            return True
        return any(own_equality(it, x) for x in v.payload.values())
    if isinstance(v, ListVal):
        return any(own_equality(it, x) for x in v.items)
    if isinstance(v, tuple):
        return any(own_equality(it, x) for x in v)
    return False



def fail(code: str, **payload) -> Failure:
    return Failure(code, dict(payload))


# ---------------------------------------------------------------- Int (6.4)
def _check(r: int, op: str):
    return fail("numeric.overflow", operation=op) if r < INT_MIN or r > INT_MAX else r


@assoc("Int", "add")
def int_add(it, p): return _check(p[0] + p[1], "add")


@assoc("Int", "sub")
def int_sub(it, p): return _check(p[0] - p[1], "sub")


@assoc("Int", "mul")
def int_mul(it, p): return _check(p[0] * p[1], "mul")


@assoc("Int", "div")
def int_div(it, p):
    a, b = p
    if b == 0: return fail("numeric.divide_by_zero")
    q = abs(a) // abs(b)
    return _check(q if (a >= 0) == (b >= 0) else -q, "div")


@assoc("Int", "rem")
def int_rem(it, p):
    a, b = p
    if b == 0: return fail("numeric.divide_by_zero")
    r = abs(a) % abs(b)
    return r if a >= 0 else -r


@assoc("Int", "shiftLeft")
def int_shl(it, p):
    a, b = p
    if not 0 <= b < 64: return fail("numeric.shift_range", bits=b)
    return _check(a << b, "shiftLeft")


def _wrap(r: int) -> int:
    return (r + 2 ** 63) % 2 ** 64 - 2 ** 63


def _sat(r: int) -> int:
    return max(INT_MIN, min(INT_MAX, r))


ASSOC[("Int", "wrappingAdd")] = lambda it, p: _wrap(p[0] + p[1])
ASSOC[("Int", "wrappingSub")] = lambda it, p: _wrap(p[0] - p[1])
ASSOC[("Int", "wrappingMul")] = lambda it, p: _wrap(p[0] * p[1])
ASSOC[("Int", "saturatingAdd")] = lambda it, p: _sat(p[0] + p[1])
ASSOC[("Int", "saturatingSub")] = lambda it, p: _sat(p[0] - p[1])
ASSOC[("Int", "saturatingMul")] = lambda it, p: _sat(p[0] * p[1])
ASSOC[("Int", "validate")] = lambda it, p: p[0]          # unrefined Int: every value; a refinement is checked by the caller's bounds


# ---------------------------------------------------------------- floats (6.5)
ASSOC[("Float", "sqrt")] = lambda it, p: numeric.f64_sqrt(p[0])
ASSOC[("Float", "isNaN")] = lambda it, p: p[0] != p[0]
ASSOC[("Float", "totalOrder")] = lambda it, p: numeric.total_order(p[0])
ASSOC[("Float", "bitsEqual")] = lambda it, p: numeric.f64_bits(p[0]) == numeric.f64_bits(p[1])
ASSOC[("Float32", "sqrt")] = lambda it, p: numeric.F32.of(numeric.f64_sqrt(p[0].v))
ASSOC[("Float32", "isNaN")] = lambda it, p: p[0].v != p[0].v


# ---------------------------------------------------------------- Decimal (6.4)
for _op in ("add", "sub", "mul", "div", "rem"):
    ASSOC[("Decimal", _op)] = (lambda op: lambda it, p: it.numeric_call(numeric.dec_op, op, p[0], p[1]))(_op)
ASSOC[("Decimal", "round")] = lambda it, p: it.numeric_call(numeric.dec_round, p[0], p[1], it.variant_name(p[2]))
ASSOC[("Decimal", "fromFloat")] = lambda it, p: it.numeric_call(numeric.dec_from_float, p[0], p[1], it.variant_name(p[2]))
METHODS[("Decimal", "scale")] = lambda it, v, p: v.scale


# ---------------------------------------------------------------- text (6.6)
ASSOC[("String", "from")] = lambda it, p: str(p[0])
for _t in ("String", "StringView"):
    METHODS[(_t, "scalarCount")] = lambda it, v, p: len(v)
    METHODS[(_t, "byteCount")] = lambda it, v, p: len(v.encode("utf-8"))


@method("StringView", "slice")
def sv_slice(it, v, p):
    a, b = p
    if 0 <= a <= b <= len(v): return v[a:b]
    return fail("text.slice_bounds", start=a, end=b)


@method("StringView", "scalarAt")
def sv_scalar_at(it, v, p):
    from interpreter.eval import Scalar
    i = p[0]
    return Scalar(v[i]) if 0 <= i < len(v) else fail("text.index", index=i)


# Unicode White_Space (the property Rust's `char::is_whitespace` uses), spelled out because Python's `str.isspace`
# and `str.split()` also take U+001C to U+001F: the three backends must trim and split at the same scalars (X-479).
WHITE_SPACE = "\t\n\x0b\x0c\r \x85\xa0                　"
ASCII_UPPER = {c: c - 32 for c in range(ord("a"), ord("z") + 1)}
ASCII_LOWER = {c: c + 32 for c in range(ord("A"), ord("Z") + 1)}


def split_lines(v: str) -> list:
    """The lines of `v` (6.6): a line ends at `\\n`, and one `\\r` before that `\\n` ends with it; the text after the last
    terminator is a line only when it is not empty. A `\\r` not before a `\\n` stays in the line."""
    pieces = v.split("\n")
    last = pieces.pop()
    out = [x[:-1] if x.endswith("\r") else x for x in pieces]
    if last:
        out.append(last)
    return out


for _t in ("String", "StringView"):
    METHODS[(_t, "trim")] = lambda it, v, p: v.strip(WHITE_SPACE)
    METHODS[(_t, "trimStart")] = lambda it, v, p: v.lstrip(WHITE_SPACE)
    METHODS[(_t, "trimEnd")] = lambda it, v, p: v.rstrip(WHITE_SPACE)
    METHODS[(_t, "find")] = lambda it, v, p: (lambda i: None if i < 0 else i)(v.find(p[0]))
    METHODS[(_t, "startsWith")] = lambda it, v, p: v.startswith(p[0])
    METHODS[(_t, "endsWith")] = lambda it, v, p: v.endswith(p[0])
    METHODS[(_t, "split")] = lambda it, v, p: ListVal([v] if p[0] == "" else v.split(p[0]))
    METHODS[(_t, "lines")] = lambda it, v, p: ListVal(split_lines(v))
    METHODS[(_t, "replace")] = lambda it, v, p: v if p[0] == "" else v.replace(p[0], p[1])
    METHODS[(_t, "asciiUpper")] = lambda it, v, p: v.translate(ASCII_UPPER)
    METHODS[(_t, "asciiLower")] = lambda it, v, p: v.translate(ASCII_LOWER)

METHODS[("BytesView", "byteCount")] = lambda it, v, p: len(v.data)
METHODS[("Bytes", "byteCount")] = lambda it, v, p: len(v.data)
METHODS[("BytesView", "count")] = lambda it, v, p: len(v.data)
METHODS[("BytesView", "get")] = lambda it, v, p: v.data[p[0]] if isinstance(p[0], int) and 0 <= p[0] < len(v.data) else None
METHODS[("Bytes", "get")] = METHODS[("BytesView", "get")]


def _consume_each(it, v, p):
    items, v.items = list(v.items), []
    for x in items:                                  # in order, each element transferred to the callback (10.7)
        it.call_value(p[0], [(None, x)])
    return None


METHODS[("List", "consumeEach")] = _consume_each


@assoc("Scalar", "of")
def scalar_of(it, p):
    from interpreter.eval import Scalar
    if len(p[0]) != 1:
        raise FortisError("Scalar.of of a view that is not one scalar")
    return Scalar(p[0])


ASSOC[("Scalar", "digitValue")] = lambda it, p: int(p[0].text) if len(p[0].text) == 1 and "0" <= p[0].text <= "9" else None
ASSOC[("Scalar", "hexValue")] = lambda it, p: int(p[0].text, 16) if len(p[0].text) == 1 and p[0].text.lower() in "0123456789abcdef" else None

ASSOC[("Builder", "create")] = lambda it, p: BuilderVal()
METHODS[("Builder", "append")] = lambda it, v, p: (v.parts.append(display(p[0])), VOID)[1]
METHODS[("Builder", "appendLine")] = lambda it, v, p: (v.parts.append(display(p[0]) + "\n"), VOID)[1]
METHODS[("Builder", "appendScalar")] = lambda it, v, p: (v.parts.append(p[0].text), VOID)[1]
METHODS[("Builder", "finish")] = lambda it, v, p: "".join(v.parts)


@func("graphemes")
def graphemes(it, p):
    from interpreter.graphemes import count
    return count(p[0])


WORD = re.compile("[^" + "".join(re.escape(c) for c in WHITE_SPACE) + "]+")
ASSOC[("Text", "words")] = lambda it, p: ListVal(WORD.findall(p[0]))


# ---------------------------------------------------------------- time (6.2; the full module is DEL-27)
_UNITS = {"days": 86_400_000_000_000, "hours": 3_600_000_000_000, "minutes": 60_000_000_000, "seconds": 1_000_000_000, "milliseconds": 1_000_000}
for _u, _ns in _UNITS.items():
    ASSOC[("Duration", _u)] = (lambda ns: lambda it, p: it.numeric_call(numeric.Dur.make, p[0] * ns))(_ns)
for _op in ("add", "sub", "multiply"):
    ASSOC[("Duration", _op)] = (lambda op: lambda it, p: it.numeric_call(numeric.dur_op, op, p[0], p[1]))(_op)
METHODS[("Duration", "nanoseconds")] = lambda it, v, p: v.ns
ASSOC[("Timestamp", "epoch")] = lambda it, p: TimestampVal(0)


@assoc("Timestamp", "add")
def ts_add(it, p):
    r = p[0].ns + p[1].ns
    return fail("numeric.overflow", operation="Timestamp.add") if r < INT_MIN or r > INT_MAX else TimestampVal(r)


ASSOC[("Timestamp", "after")] = lambda it, p: p[0].ns > p[1].ns
ASSOC[("Timestamp", "before")] = lambda it, p: p[0].ns < p[1].ns


# ---------------------------------------------------------------- money (15; DEL-27)
def _money(it, currency, amount):
    cls = it.classes.get("Money")
    if cls is None:
        raise FortisError("fortis.money is not loaded")
    return Instance(cls, {"currency": currency, "amount": amount})


@assoc("Money", "multiply")
def money_multiply(it, p):
    m, factor = p
    r = it.numeric_call(numeric.dec_op, "mul", m.fields["amount"], numeric.Dec(factor < 0, abs(factor), 0))
    return r if isinstance(r, Failure) else _money(it, m.fields["currency"], r)


@assoc("Money", "divide")
def money_divide(it, p):
    m, divisor = p
    r = it.numeric_call(numeric.dec_op, "div", m.fields["amount"], numeric.Dec(divisor < 0, abs(divisor), 0))
    return r if isinstance(r, Failure) else _money(it, m.fields["currency"], r)


# ---------------------------------------------------------------- collections (6.7)
ASSOC[("List", "default")] = lambda it, p: ListVal([])
ASSOC[("List", "of")] = lambda it, p: ListVal(list(p))
for _t in ("List", "ListView"):
    METHODS[(_t, "count")] = lambda it, v, p: len(v.items)
    METHODS[(_t, "get")] = lambda it, v, p: v.items[p[0]] if 0 <= p[0] < len(v.items) else None
METHODS[("List", "push")] = lambda it, v, p: (v.items.append(p[0]), VOID)[1]
METHODS[("List", "pop")] = lambda it, v, p: v.items.pop() if v.items else None
METHODS[("List", "at")] = lambda it, v, p: v.items[p[0]] if 0 <= p[0] < len(v.items) else fail("collection.index", index=p[0])


@method("List", "set")
def list_set(it, v, p):
    i, x = p
    if not 0 <= i < len(v.items):
        return fail("collection.index", index=i)
    it.destroy(v.items[i])
    v.items[i] = x
    return VOID


@method("List", "clear")
def list_clear(it, v, p):
    for x in v.items:
        it.destroy(x)
    v.items.clear()
    return VOID


METHODS[("List", "iterate")] = lambda it, v, p: CursorVal(v.items)


@method("ListCursor", "next")
def cursor_next(it, v, p):
    if v.pos >= len(v.items):
        return None
    v.pos += 1
    return v.items[v.pos - 1]


@assoc("Array", "filled")
def array_filled(it, p, n: Optional[int] = None):
    if n is None:
        raise FortisError("Array.filled needs the size N from the type (Array[T, N])")
    from interpreter.values import deep_copy
    return ArrayVal([deep_copy(p[0]) for _ in range(n)])


# `Deque.iterate` (X-576): the library class keeps its items in two lists, `front` with the first item last and `back`
# with the last item last, and the cursor walks them front to back
METHODS[("Deque", "iterate")] = lambda it, v, p: CursorVal(list(reversed(v.fields["front"].items)) + list(v.fields["back"].items))
METHODS[("Array", "count")] = lambda it, v, p: len(v.items)
METHODS[("Array", "get")] = lambda it, v, p: v.items[p[0]] if 0 <= p[0] < len(v.items) else None
METHODS[("Array", "asView")] = lambda it, v, p: v      # a view reads the array's own elements (X-578); ListView's methods reach it by ALIASES


@method("Array", "set")
def array_set(it, v, p):
    i, x = p
    if not 0 <= i < len(v.items):
        raise FortisError(f"Array.set index {i} outside 0..{len(v.items)} (proof-required)")
    it.destroy(v.items[i])
    v.items[i] = x
    return VOID


ASSOC[("Map", "default")] = lambda it, p: MapVal()


@method("Map", "get")
def map_get(it, v, p):
    if own_equality(it, p[0]):
        e = next((e for e in v.entries.values() if it.equal(e[0], p[0])), None)
        return None if e is None else e[1]
    e = v.entries.get(freeze(p[0]))
    return None if e is None else e[1]


@method("Map", "set")
def map_set(it, v, p):
    if own_equality(it, p[0]):
        hit = next((fk for fk, e in v.entries.items() if it.equal(e[0], p[0])), None)
        if hit is not None:
            it.destroy(v.entries[hit][1])
            v.entries[hit] = (v.entries[hit][0], p[1])     # replacing keeps the first insertion's position
        else:
            v.entries[("custom", v.custom_seq)] = (p[0], p[1])
            v.custom_seq += 1
        return VOID
    k = freeze(p[0])
    old = v.entries.get(k)
    if old is not None:
        it.destroy(old[1])
        v.entries[k] = (old[0], p[1])                 # replacing keeps the first insertion's position
    else:
        v.entries[k] = (p[0], p[1])
    return VOID


METHODS[("Map", "count")] = lambda it, v, p: len(v.entries)


def _map_slot(it, v, key):
    """The dict key of the entry for `key`, or None: by its own equality when the type writes one, else structurally."""
    if own_equality(it, key):
        return next((fk for fk, e in v.entries.items() if it.equal(e[0], key)), None)
    k = freeze(key)
    return k if k in v.entries else None


@method("Map", "containsKey")
def map_contains_key(it, v, p):
    return _map_slot(it, v, p[0]) is not None


@method("Map", "remove")
def map_remove(it, v, p):
    """The removed entry's value, now the caller's; the key it held is dropped. The rest keep their order (6.7)."""
    slot = _map_slot(it, v, p[0])
    if slot is None:
        return None
    k, x = v.entries.pop(slot)
    it.destroy(k)
    return x
ASSOC[("Set", "default")] = lambda it, p: SetVal()


@method("Set", "add")
def set_add(it, v, p):
    if own_equality(it, p[0]):
        if any(it.equal(x, p[0]) for x in v.items.values()):
            return False
        v.items[("custom", v.custom_seq)] = p[0]
        v.custom_seq += 1
        return True
    k = freeze(p[0])
    if k in v.items:
        return False
    v.items[k] = p[0]
    return True


METHODS[("Set", "contains")] = lambda it, v, p: (any(it.equal(x, p[0]) for x in v.items.values()) if own_equality(it, p[0]) else freeze(p[0]) in v.items)
METHODS[("Set", "count")] = lambda it, v, p: len(v.items)


@method("Set", "remove")
def set_remove(it, v, p):
    if own_equality(it, p[0]):
        slot = next((fk for fk, x in v.items.items() if it.equal(x, p[0])), None)
    else:
        k = freeze(p[0])
        slot = k if k in v.items else None
    if slot is None:
        return False
    it.destroy(v.items.pop(slot))
    return True


# ---------------------------------------------------------------- outcomes (10)
ASSOC[("Must", "create")] = lambda it, p: VOID
ASSOC[("Must", "discharge")] = lambda it, p: VOID
ASSOC[("Outcome", "success")] = lambda it, p: p[0]
ASSOC[("Outcome", "failure")] = lambda it, p: p[0]


@assoc("Outcome", "collect")
def outcome_collect(it, p):
    items = p[0].items if isinstance(p[0], ListVal) else []
    first = next((x for x in items if isinstance(x, Failure)), None)
    return first if first is not None else ListVal(list(items))


@assoc("Failures", "aggregate")
def failures_aggregate(it, p):
    fs = [f for f in p[0].items if isinstance(f, Failure)]
    agg = Failure("failure.aggregate", {"count": len(fs)}, origin=it.stack[-1] if it.stack else "main")
    prev = agg
    for f in fs:
        prev.cause = f
        prev = f
    return agg


@func("annotate")
def annotate(it, p):
    f = p[0]
    f.annotations.append(p[1])
    return f


# ---------------------------------------------------------------- concurrency (12)
@assoc("Shared", "create")
def shared_create(it, p):
    from interpreter.values import SharedBox, SharedVal
    return SharedVal(SharedBox(p[0]))


@method("Shared", "retain")
def shared_retain(it, v, p):
    from interpreter.values import SharedVal
    v.box.strong += 1
    return SharedVal(v.box)


@method("Shared", "downgrade")
def shared_downgrade(it, v, p):
    from interpreter.values import WeakVal
    return WeakVal(v.box)


@method("Weak", "upgrade")
def weak_upgrade(it, v, p):
    from interpreter.values import SharedVal
    if v.box.strong == 0:
        return None
    v.box.strong += 1
    return SharedVal(v.box)


@method("Weak", "retain")
def weak_retain(it, v, p):
    from interpreter.values import WeakVal
    return WeakVal(v.box)


ASSOC[("Mutex", "create")] = lambda it, p: MutexVal(p[0])


@method("Mutex", "lockBlocking")
def mutex_lock_blocking(it, v, p):
    if v.held:
        raise FortisError("lockBlocking on a held mutex would block forever on the single-threaded interpreter")
    v.held = True
    return GuardVal(v)


METHODS[("Mutex", "lock")] = lambda it, v, p: ReadyVal(mutex_lock_blocking(it, v, p))


# the protected value, through the exclusive guard (12.7; D-23)
@method("MutexGuard", "get")
def guard_get(it, v, p):
    from interpreter.values import deep_copy
    return deep_copy(v.mutex.value)


@method("MutexGuard", "set")
def guard_set(it, v, p):
    old, v.mutex.value = v.mutex.value, p[0]
    it.destroy(old)                                   # the old value is dropped, as the VM and native code drop it (7.9)
    return VOID


@method("MutexGuard", "replace")
def guard_replace(it, v, p):
    old, v.mutex.value = v.mutex.value, p[0]
    return old
ASSOC[("AtomicInt", "create")] = lambda it, p: AtomicVal("AtomicInt", p[0])
ASSOC[("AtomicBool", "create")] = lambda it, p: AtomicVal("AtomicBool", p[0])
for _t in ("AtomicInt", "AtomicBool"):
    METHODS[(_t, "load")] = lambda it, v, p: v.value
    METHODS[(_t, "store")] = lambda it, v, p: (setattr(v, "value", p[0]), VOID)[1]


@method("AtomicInt", "fetchAdd")
def atomic_fetch_add(it, v, p):
    old = v.value
    v.value = _wrap(old + p[0])
    return old


@method("Scope", "launch", "launchParallel", "launchBlocking")
def scope_launch(it, v, p, mname=None):
    raise AssertionError("dispatched by the interpreter (needs the method name)")


@method("Scope", "detach")
def scope_detach(it, v, p):
    raise AssertionError("dispatched by the interpreter")


@method("Task", "abandon")
def task_abandon(it, v, p):
    it.sched.cancel(v)
    it.sched.join(v)
    return VOID


@func("join")
def join(it, p):
    from interpreter.executor import JoinVal
    return JoinVal(list(p))


@func("checkpoint")
def checkpoint(it, p):
    it.sched.checkpoint()
    return VOID


@func("injectDefect")
def inject_defect(it, p):
    from interpreter.executor import DefectSignal
    raise DefectSignal(p[0])


# ---------------------------------------------------------------- host: development adapters (15, 11.6; DEL-28)
CAPABILITY_TYPES = {"fileSystem": "FileSystem", "network": "Network", "clock": "Clock", "random": "Random", "environment": "Environment",
                    "process": "Process", "supervisor": "Supervisor", "console": "Console", "secretAccess": "SecretAccess", "rawSql": "RawSql"}


@dataclass
class PathVal:
    text: str

    def display(self) -> str:
        return self.text


class SecretVal:
    """`Secret[T]`: displays as a fingerprint, never the value; not encodable, not capturable (15)."""
    _key = None

    def __init__(self, value):
        self.value = value

    def display(self) -> str:
        import hashlib
        import hmac
        import os as _os
        if SecretVal._key is None:
            SecretVal._key = _os.urandom(16)               # per process: fingerprints compare within a run only
        digest = hmac.new(SecretVal._key, display(self.value).encode("utf-8"), hashlib.sha256).hexdigest()[:8]
        return f"Secret({digest})"


@dataclass
class FileVal:
    path: str
    mode: str                     # r | w
    closed: bool = False
    pos: int = 0


def _dev_root(it) -> str:
    import os as _os
    import tempfile
    if getattr(it, "dev_root", None) is None:
        it.dev_root = _os.environ.get("FORTIS_DEV_ROOT") or tempfile.mkdtemp(prefix="fortis-dev-")
    return it.dev_root


@assoc("Host", "capabilities")
def host_capabilities(it, p):
    import os as _os
    # the capabilities are created once per run (11.6): the checker admits one direct call, in the entry `main`; a
    # second creation means that guarantee is broken, which is the runtime-invariant defect (10.8; D-110)
    if getattr(it, "capabilities_made", False):
        from interpreter.executor import DefectSignal
        raise DefectSignal("runtime_invariant")
    it.capabilities_made = True
    caps = ClassInfo("Capabilities", {}, {}, {})
    fields = {k: Capability(k) for k in CAPABILITY_TYPES}
    fields["fileSystem"].state["root"] = _dev_root(it)
    fields["random"].state["seed"] = int(_os.environ.get("FORTIS_DEV_SEED", "0"))
    return Instance(caps, fields)


@assoc("Path", "parse")
def path_parse(it, p):
    t = p[0]
    if t == "" or "\0" in t:
        return fail("path.invalid", text=t)
    return PathVal(t)


@func("reveal")
def reveal(it, p):
    from interpreter.values import deep_copy
    return deep_copy(p[0].value)


def _fs_resolve(cap: Capability, path: PathVal):
    import os as _os
    root = cap.state.get("root")
    if root is None:
        return None, fail("fs.outside_root", path=path.text)
    t = path.text
    norm = _os.path.normpath(t)
    if _os.path.isabs(t) or norm == ".." or norm.startswith(".." + _os.sep):
        return None, fail("fs.outside_root", path=t)
    return _os.path.join(root, norm), None


@method("FileSystem", "readText")
def fs_read_text(it, v, p):
    import os as _os
    full, err = _fs_resolve(v, p[0])
    if err: return err
    if not _os.path.exists(full): return fail("fs.not_found", path=p[0].text)
    if not _os.path.isfile(full): return fail("fs.not_a_file", path=p[0].text)
    try:
        return open(full, encoding="utf-8").read()
    except UnicodeDecodeError:
        return fail("fs.invalid_text", path=p[0].text)


@method("FileSystem", "writeAll")
def fs_write_all(it, v, p):
    import os as _os
    full, err = _fs_resolve(v, p[0])
    if err: return err
    if not _os.path.isdir(_os.path.dirname(full) or "."): return fail("fs.not_found", path=p[0].text)
    open(full, "w", encoding="utf-8").write(p[1])
    return VOID


@method("FileSystem", "readAll")
def fs_read_all(it, v, p):
    import os as _os
    full, err = _fs_resolve(v, p[0])
    if err: return err
    if not _os.path.exists(full): return fail("fs.not_found", path=p[0].text)
    if not _os.path.isfile(full): return fail("fs.not_a_file", path=p[0].text)
    return BytesVal(open(full, "rb").read())


@method("FileSystem", "open")
def fs_open(it, v, p):
    import os as _os
    full, err = _fs_resolve(v, p[0])
    if err: return err
    if not _os.path.isfile(full): return fail("fs.not_found", path=p[0].text)
    return FileVal(full, "r")


@method("FileSystem", "create")
def fs_create(it, v, p):
    import os as _os
    full, err = _fs_resolve(v, p[0])
    if err: return err
    if not _os.path.isdir(_os.path.dirname(full) or "."): return fail("fs.not_found", path=p[0].text)
    open(full, "wb").close()
    return FileVal(full, "w")


@method("FileSystem", "subtree")
def fs_subtree(it, v, p):
    import os as _os
    full, err = _fs_resolve(v, p[0])
    sub = Capability("fileSystem")
    if err is None:
        _os.makedirs(full, exist_ok=True)
        sub.state["root"] = full
    return sub                                          # an escaping path gives a confinement that refuses everything


@method("File", "readAll")
def file_read_all(it, v, p):
    if v.closed: return fail("fs.closed")
    if v.mode != "r": return fail("fs.wrong_mode")
    data = open(v.path, "rb").read()[v.pos:]
    v.pos += len(data)
    return BytesVal(data)


@method("File", "writeAll")
def file_write_all(it, v, p):
    if v.closed: return fail("fs.closed")
    if v.mode != "w": return fail("fs.wrong_mode")
    open(v.path, "ab").write(p[0].data)
    return VOID


@method("File", "close")
def file_close(it, v, p):
    v.closed = True
    return VOID


@method("Network", "resolve")
def net_resolve(it, v, p):
    return "127.0.0.1" if p[0] == "localhost" else fail("network.unreachable", host=p[0])


@method("Clock", "sleep")
def clock_sleep(it, v, p):
    from interpreter.executor import SleepVal
    return SleepVal(p[0].ns)


@method("Clock", "now")
def clock_now(it, v, p):
    import time as _time
    if "frozen" in v.state:
        return v.state["frozen"]
    return TimestampVal(_time.time_ns())


@method("Clock", "frozen")
def clock_frozen(it, v, p):
    c = Capability("clock")
    c.state["frozen"] = p[0]
    return c


@method("Clock", "elapsedSince")
def clock_elapsed(it, v, p):
    now = clock_now(it, v, [])
    if p[0].ns > now.ns:
        return fail("clock.future")
    return it.numeric_call(numeric.Dur.make, now.ns - p[0].ns)


def _splitmix(state: int):
    state = (state + 0x9E3779B97F4A7C15) % 2 ** 64
    z = state
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) % 2 ** 64
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) % 2 ** 64
    return state, z ^ (z >> 31)


@method("Random", "below")
def random_below(it, v, p):
    bound = p[0]
    if bound < 1:
        raise FortisError("Random.below needs bound >= 1 (proof-required)")
    limit = (2 ** 64 // bound) * bound                  # rejection sampling: no modulo bias
    while True:
        v.state["seed"], z = _splitmix(v.state.get("seed", 0))
        if z < limit:
            return z % bound


@method("Random", "seeded")
def random_seeded(it, v, p):
    r = Capability("random")
    r.state["seed"] = p[0] % 2 ** 64
    return r


@method("Environment", "get")
def env_get(it, v, p):
    import os as _os
    name = p[0]
    if name == "" or "=" in name:
        return None
    val = _os.environ.get(name)
    return None if val is None else SecretVal(val)


@method("Process", "run")
def process_run(it, v, p):
    program = p[0]
    if program == "true": return 0
    if program == "false": return 1
    return fail("process.denied", program=program)


@method("Console", "print")
def console_print(it, v, p):
    line = display(p[0])
    it.out.append(line)
    if getattr(it, "stream", None) is not None:
        it.stream.write(line + "\n")          # written as printed: a run that ends in a defect keeps its output (10.8)
        it.stream.flush()                       # and before any record a later line's statement writes to stderr
    return VOID


@method("RawSql", "execute")
def sql_execute(it, v, p):
    return fail("sql.unavailable")
