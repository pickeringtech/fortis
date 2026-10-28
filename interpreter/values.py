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

    def display(self) -> str:
        if self.cls.name == "Scalar":
            return self.fields["text"]
        if "display" in self.cls.methods:
            return "<display>"
        return f"{self.cls.name}(" + ", ".join(f"{k}: {display(v)}" for k, v in self.fields.items()) + ")"


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


def deep_copy(v):
    if isinstance(v, (int, str, bool)) or v is None:
        return v
    return copy.deepcopy(v)


def check_int(n: int, what: str):
    if n < INT_MIN or n > INT_MAX:
        raise FortisError(f"integer overflow in {what} (the operation is proof-required; use Int.{what})")
    return n
