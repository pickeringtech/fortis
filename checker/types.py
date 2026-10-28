"""Type representations and the subtyping relation (Language Reference 6.1–6.3, 6.8, 6.9, 6.11)."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

INT_TYPES = {"Int": (-(2 ** 63), 2 ** 63 - 1), "Int64": (-(2 ** 63), 2 ** 63 - 1), "Int32": (-(2 ** 31), 2 ** 31 - 1),
             "Int16": (-(2 ** 15), 2 ** 15 - 1), "Int8": (-128, 127), "UInt": (0, 2 ** 64 - 1), "UInt64": (0, 2 ** 64 - 1),
             "UInt32": (0, 2 ** 32 - 1), "UInt16": (0, 65535), "UInt8": (0, 255), "Byte": (0, 255)}
FLOAT_TYPES = {"Float", "Float32"}
TRIVIAL_BUILTINS = {"Void", "Never", "Bool", "Float32", "Float", "Decimal", "Byte", "Duration", "Instant", "Timestamp", "Scalar", "Ordering"} | set(INT_TYPES)
COPYABLE_HEAP = {"String", "Bytes", "BigInt"}
COPYABLE_CONTAINERS = {"List", "Array", "Map", "OrderedMap", "Set", "FixedList", "FixedMap"}
NOT_COPYABLE = {"Shared", "Weak", "Mutex", "MutexGuard", "AtomicInt", "AtomicBool", "Future", "Task", "Outcome", "Failure", "StringView", "BytesView", "ListView"}
LINEAR_BUILTINS = {"Outcome", "Failure", "Future", "Task", "Must"}


class Type:
    def __repr__(self): return self.show()
    def show(self) -> str: raise NotImplementedError


@dataclass(frozen=True)
class Named(Type):
    name: str
    args: Tuple[Type, ...] = ()
    package: str = ""          # nominal identity includes the declaring package@version (LANG-04-04)
    # Declaring module of a core type whose package identity is empty (`fortis.host`). Empty for prelude
    # types and for names not yet resolved. Not part of equality: identity stays `package` (LANG-04-04).
    origin: str = field(default="", compare=False, hash=False)

    def show(self):
        a = f"[{', '.join(x.show() for x in self.args)}]" if self.args else ""
        return self.name + a


@dataclass(frozen=True)
class Refined(Type):
    base: Named
    lo: Optional[int]
    hi: Optional[int]
    kind: str = "range"        # range | scalars | length

    def show(self):
        lo = "" if self.lo is None else str(self.lo)
        hi = "" if self.hi is None else str(self.hi)
        prefix = "" if self.kind == "range" else f"{self.kind}: "
        return f"{self.base.show()}<{prefix}{lo}..{hi}>"


@dataclass(frozen=True)
class OptionalT(Type):
    inner: Type
    def show(self): return self.inner.show() + "?"


@dataclass(frozen=True)
class TupleT(Type):
    items: Tuple[Type, ...]
    def show(self): return "(" + ", ".join(t.show() for t in self.items) + ")"


@dataclass(frozen=True)
class RecordT(Type):
    fields: Tuple[Tuple[str, Type], ...]
    def show(self): return "{ " + ", ".join(f"{k}: {t.show()}" for k, t in self.fields) + " }"


@dataclass(frozen=True)
class FnT(Type):
    params: Tuple[Type, ...]
    ret: Type
    kind: str = "fn"           # fn | fn mut | fn once
    modifiers: Tuple[str, ...] = ()
    origin: str = ""           # the region of a loan-holding closure in an owning position (8.3): `fn(...) T from p`
    dynamic: bool = False      # declared `dynamic fn`: a boxed closure, the one heterogeneous position (8.4)
    identity: str = ""         # the lambda literal this closure type is, empty for a declared function type
    def show(self):
        m = " ".join(self.modifiers) + " " if self.modifiers else ""
        k = self.kind if self.kind == "fn" else self.kind
        d = "dynamic " if self.dynamic else ""
        return f"{d}{m}{k}({', '.join(p.show() for p in self.params)}) {self.ret.show()}" + (f" from {self.origin}" if self.origin else "")


@dataclass(frozen=True)
class LoanT(Type):
    inner: Type
    mode: str                  # alias | editable
    origin: str
    def show(self): return f"{self.mode} {self.inner.show()} from {self.origin}"


@dataclass(frozen=True)
class DynamicT(Type):
    iface: Named
    def show(self): return f"dynamic {self.iface.show()}"


@dataclass(frozen=True)
class TypeParam(Type):
    name: str
    def show(self): return self.name


@dataclass(frozen=True)
class Unknown(Type):
    """A type the checker cannot see (a kit signature fixture or an unimplemented construct); it
    unifies with anything and is reported as `unknown` (PROG-DIA-02), never as a proved violation."""
    why: str = ""
    def show(self): return "<unknown>"


VOID = Named("Void"); NEVER = Named("Never"); BOOL = Named("Bool"); INT = Named("Int"); STRING = Named("String")
STRING_VIEW = Named("StringView"); FLOAT = Named("Float"); DECIMAL = Named("Decimal"); DURATION = Named("Duration")
NULL_T = OptionalT(NEVER)


def base_name(t: Type) -> Optional[str]:
    if isinstance(t, Named): return t.name
    if isinstance(t, Refined): return t.base.name
    return None


def int_bounds(t: Type) -> Optional[Tuple[Optional[int], Optional[int]]]:
    """The declared interval of an integer type (None for non-integers)."""
    if isinstance(t, Refined) and t.kind in ("range", "exact") and t.base.name in INT_TYPES:
        lo, hi = INT_TYPES[t.base.name]
        return (lo if t.lo is None else t.lo, hi if t.hi is None else t.hi)
    if isinstance(t, Named) and t.name in INT_TYPES:
        return INT_TYPES[t.name]
    return None


def strip_loan(t: Type) -> Type:
    return t.inner if isinstance(t, LoanT) else t
