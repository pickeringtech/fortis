"""Runtime values of the kernel."""
from __future__ import annotations

import copy
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

INT_MIN, INT_MAX = -(2 ** 63), 2 ** 63 - 1


class FortisError(Exception):
    """A dynamic check failed: the program is outside the kernel's rules (a static layer will
    reject it later). Never a Fortis `Failure`."""


class FailureSignal(Exception):
    """A Fortis `Failure` propagating out of a `try` function (the dynamic form of `Outcome`)."""
    def __init__(self, failure: "Failure"):
        super().__init__(failure.code)
        self.failure = failure


@dataclass
class Void:
    def __repr__(self): return "Void"


VOID = Void()
NULL = None


@dataclass
class Failure:
    code: str
    payload: Dict[str, Any] = field(default_factory=dict)
    origin: str = ""
    cause: Optional["Failure"] = None
    message: str = ""
    annotations: List[str] = field(default_factory=list)     # notes added by `annotate` (10.6)


@dataclass
class Report:
    code: str


@dataclass
class Instance:
    cls: "ClassInfo"
    fields: Dict[str, Any]
    # its owner has destroyed it: its cleanup ran and its fields were destroyed. A second destruction, which only a
    # program the checker should have refused can reach (two owners of one value, D-352), does nothing, so cleanup
    # runs exactly once (7.9, 12.2; D-348)
    destroyed: bool = field(default=False, compare=False, repr=False)

    def display(self) -> str:
        if self.cls.name == "Scalar":
            return self.fields["text"]
        if "display" in self.cls.methods:
            return "<display>"
        return f"{self.cls.name}(" + ", ".join(f"{k}: {display(v)}" for k, v in self.fields.items()) + ")"


class LentTuple(tuple):
    """A tuple that holds a loan beside the elements it owns (`(xs.get(0), make())`): `lent` are the indices of the
    elements it borrows. Destroying it destroys only the others (7.9; D-349). A copy copies the elements it owns and
    holds the same loans, as a copy of a loan is that loan."""
    def __new__(cls, items, lent):
        t = super().__new__(cls, items)
        t.lent = frozenset(lent)
        return t

    def __deepcopy__(self, memo):
        return LentTuple([x if i in self.lent else copy.deepcopy(x, memo) for i, x in enumerate(self)], self.lent)


class LentRecord(dict):
    """A record literal that holds a loan beside the fields it owns (`{ item: xs.get(0), n: 1 }`): `lent` are the
    names of the fields it borrows. Destroying it destroys only the others (7.9; D-349). A copy copies the fields it
    owns and holds the same loans."""
    def __init__(self, items, lent):
        super().__init__(items)
        self.lent = frozenset(lent)

    def __deepcopy__(self, memo):
        return LentRecord({k: x if k in self.lent else copy.deepcopy(x, memo) for k, x in self.items()}, self.lent)


def lent_parts(v):
    """The indices or field names an aggregate borrows rather than owns (7.9; D-349)."""
    return getattr(v, "lent", frozenset())


def retuple(old, items):
    """A tuple rebuilt from `old` with new elements in the same places: it borrows what `old` borrowed."""
    lent = lent_parts(old)
    return LentTuple(items, lent) if lent else tuple(items)


@dataclass
class EnumValue:
    enum: str
    variant: str
    payload: Dict[str, Any] = field(default_factory=dict)

    def display(self) -> str:
        if not self.payload:
            return self.variant
        return f"{self.variant}(" + ", ".join(f"{k}: {display(v)}" for k, v in self.payload.items()) + ")"


@dataclass
class ListVal:
    items: List[Any]


@dataclass
class BuilderVal:
    parts: List[str] = field(default_factory=list)


@dataclass
class Closure:
    params: List[str]
    body: Any
    env: Any
    kind: str = "fn"
    is_async: bool = False          # an `async fn` lambda: a call binds the arguments and returns a Future (12.3)
    ptypes: List[Any] = field(default_factory=list)   # the declared parameter types (paramType nodes or None)
    ret: Any = None                 # the declared result type node
    # the captures the closure owns (`copy`, `move` and bare ones), in capture-list order: destroying the closure
    # destroys those its body has not moved out (7.9, X-250; D-30)
    owned: List[str] = field(default_factory=list)
    # its `owned` parameters, in order: a call destroys those its body has not moved, last (7.9; D-152)
    owned_params: List[str] = field(default_factory=list)
    # the function a log record in its body names: `lambda@LINE:COL` of its `fn`, as the IR names it (13; D-323)
    label: str = "lambda"
    is_try: bool = False            # a `try fn` lambda: a failure in its body is the call's outcome (10.2, 8.2)


@dataclass
class ClassInfo:
    name: str
    fields: Dict[str, Any]          # name -> default constExpr node or None
    field_exported: Dict[str, bool]
    methods: Dict[str, Any]         # name -> (methodHead node, funcBody node, receiver mode or None)
    node: Any = None
    cleanup: Any = None             # the cleanup block, run when a value is destroyed (7.9)
    implements: List[str] = field(default_factory=list)
    field_types: Dict[str, Any] = field(default_factory=dict)   # name -> declared type node (decoding, 6.10)
    type_params: List[str] = field(default_factory=list)
    consts: Dict[str, Any] = field(default_factory=dict)        # associated constant -> its constExpr node (9.3)
    # declared `trivial class` (6.11): its values are copied, never moved, as the checkers' `is_trivial` says (D-411)
    trivial: bool = False

    def __deepcopy__(self, memo):
        return self          # declaration metadata: a copied value keeps its class (equality compares it by identity)


class SharedBox:
    """The reference-counted cell behind Shared and Weak handles (7.8)."""
    __slots__ = ("value", "strong")
    def __init__(self, value):
        self.value, self.strong = value, 1


@dataclass
class SharedVal:
    box: SharedBox
    def display(self) -> str: return display(self.box.value)


@dataclass
class WeakVal:
    box: SharedBox
    def display(self) -> str: return "Weak"


@dataclass
class EnumInfo:
    name: str
    variants: Dict[str, List[str]]  # variant -> payload field names
    methods: Dict[str, Any]
    payload_types: Dict[str, List[Any]] = field(default_factory=dict)   # variant -> [(field, type node)]
    type_params: List[str] = field(default_factory=list)
    consts: Dict[str, Any] = field(default_factory=dict)        # associated constant -> its constExpr node (9.3)

    def __deepcopy__(self, memo):
        return self          # declaration metadata: a copied value keeps its class (equality compares it by identity)


@dataclass
class ErrorInfo:
    name: str
    code: str
    payload: List[str]
    message: Optional[str]

    def __deepcopy__(self, memo):
        return self          # declaration metadata: a copied value keeps its class (equality compares it by identity)


@dataclass
class Capability:
    kind: str
    sink: Any = None
    state: Dict[str, Any] = field(default_factory=dict)     # the development adapter's state (DEL-28)

    def display(self) -> str:
        return self.kind[0].upper() + self.kind[1:]


def display(v) -> str:
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        from interpreter.numeric import fmt_float
        return fmt_float(v)
    if isinstance(v, str):
        return v
    if isinstance(v, (Instance, EnumValue)):
        return v.display()
    if isinstance(v, ListVal):
        return "[" + ", ".join(display(x) for x in v.items) + "]"
    if isinstance(v, Failure):
        return f"Failure({v.code})"
    if hasattr(v, "display"):
        return v.display()
    if isinstance(v, Void):
        return "Void"
    return repr(v)


def render_message(template: str, payload: Dict[str, str]) -> str:
    """A failure's message (10.3; D-340): the `message` of an `error` declaration is a template, stored as written
    between its quotes and rendered when the failure is displayed. Each `${name}` that names a payload field becomes
    that field's text, as the record's `payload` shows it; any other text is kept as it is. The VM and native code
    render through fortis-runtime's `render_message`, the same rule."""
    out, rest = [], template
    while "${" in rest:
        at = rest.index("${")
        out.append(rest[:at])
        after = rest[at + 2:]
        end = after.find("}")
        if end < 0:
            out.append(rest[at:])
            rest = ""
            break
        name = after[:end]
        out.append(payload[name] if name in payload else "${" + name + "}")
        rest = after[end + 1:]
    out.append(rest)
    return "".join(out)


def deep_copy(v):
    if isinstance(v, (int, str, bool)) or v is None:
        return v
    return copy.deepcopy(v)


def check_int(n: int, what: str):
    if n < INT_MIN or n > INT_MAX:
        raise FortisError(f"integer overflow in {what} (the operation is proof-required; use Int.{what})")
    return n
