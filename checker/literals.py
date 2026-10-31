"""Literal typing and assignability (DEL-11: Language Reference 3.5, 6.3 subtyping, 6.6, X-17).

A literal's type comes from its token: an integer literal is the exact interval `Int<v..v>` (so
it is assignable to every integer type and refinement whose range contains v, and `E-REF-001`
otherwise, reported as *violated*); a float literal is `Float` (`Float32` with the `f32` suffix); a
decimal literal is `Decimal`; a duration literal is `Duration`; a string literal without
interpolation is `StringView from static` and converts implicitly to `String` (the one implicit
conversion); with interpolation it is `String` and every interpolated expression must be
`Displayable`. `assignable(dc, have, want)` is the structural rule of 6.3 plus optional injection."""
from __future__ import annotations

from typing import Optional, Tuple

from checker.types import (BOOL, DECIMAL, DURATION, FLOAT, INT, INT_TYPES, STRING, STRING_VIEW, DynamicT, FnT, LoanT, Named, OptionalT,
                           RecordT, Refined, TupleT, Type, TypeParam, Unknown, int_bounds)

STRLIT = Named("$strlit")
INTERP = Named("$interp")            # an interpolated literal: a fresh String (loanable as a view for a statement)
VIEW_OF = {("String", "StringView"), ("Bytes", "BytesView"), ("List", "ListView"), ("Array", "ListView"), ("FixedList", "ListView")}            # a string literal without interpolation: view or String by context
FLOAT32 = Named("Float32")


def is_strlit(t: Type) -> bool:
    return t == STRLIT or (isinstance(t, Refined) and t.base == STRLIT)


def scalar_count(text: str) -> int:
    """Unicode scalars of a string literal's content (escapes resolved; no interpolation)."""
    body, i, n = text, 0, 0
    while i < len(body):
        if body[i] == "\\":
            i += 2
        else:
            i += 1
        n += 1
    return n


def literal_type(tok) -> Type:
    k = tok.kind
    if k == "INT":
        t = tok.text.replace("_", "")
        v = int(t, 0) if t[:2].lower() in ("0x", "0b", "0o") else int(t)
        return Refined(INT, v, v, "exact")
    if k == "FLOAT":
        return FLOAT32 if tok.text.endswith("f32") else FLOAT
    if k == "DEC": return DECIMAL
    if k == "DUR": return DURATION
    if k in ("STR", "RAW"):
        if k == "STR" and "${" in tok.text:
            return INTERP
        n = scalar_count(tok.text[1:-1] if k == "STR" else tok.text[3:-3])
        return Refined(STRLIT, n, n, "scalars")          # a literal's scalar count is a fact (6.3)
    if k == "BYTES": return Named("BytesView")
    if k == "KW" and tok.text in ("true", "false"): return BOOL
    return Unknown("literal")


def negate(t: Type) -> Type:
    if isinstance(t, Refined) and t.base == INT and t.lo == t.hi and t.lo is not None:
        return Refined(INT, -t.lo, -t.lo, "exact")
    return t


def known(t: Type) -> bool:
    return not isinstance(t, (Unknown, TypeParam))


def strip(t: Type) -> Type:
    return t.inner if isinstance(t, LoanT) else t


def concrete(t: Type) -> Type:
    """A literal's type once the context has not decided: a plain literal is a view, an interpolated one a String."""
    if is_strlit(t): return STRING_VIEW
    if t == INTERP: return STRING
    return t


CLOSURE_WHY = "each lambda has its own closure type (8.4): this position already holds one lambda's closure, and a second, different lambda needs a `dynamic fn(...)` position"


def _closure_id(t: Type):
    """A non-dynamic lambda's identity, or None when this type is not one closure."""
    t = strip(t)
    if isinstance(t, FnT) and t.identity and not t.dynamic:
        return t.identity
    return None


def declared_vs_lambda(have: Type, want: Type) -> bool:
    """True when `want` is one lambda's closure and `have` is a declared function at that position (8.4).

    A lambda may still fill a declared `fn` (the other direction). Two lambdas stay with
    `closure_conflict`. Optional injection and a loan are the same position as their inner closure."""
    have, want = strip(have), strip(want)
    if isinstance(want, OptionalT):
        inner = have.inner if isinstance(have, OptionalT) else have
        return declared_vs_lambda(inner, want.inner)
    if isinstance(want, LoanT):
        inner = have.inner if isinstance(have, LoanT) else have
        return declared_vs_lambda(inner, want.inner)
    if _closure_id(want) and isinstance(have, FnT) and not have.dynamic and not have.identity:
        return True
    if isinstance(have, Named) and isinstance(want, Named) and have.name == want.name:
        return any(declared_vs_lambda(a, b) for a, b in zip(have.args, want.args))
    if isinstance(have, TupleT) and isinstance(want, TupleT):
        return any(declared_vs_lambda(a, b) for a, b in zip(have.items, want.items))
    if isinstance(have, RecordT) and isinstance(want, RecordT):
        wm = dict(want.fields)
        return any(k in wm and declared_vs_lambda(ft, wm[k]) for k, ft in have.fields)
    return False


def closure_conflict(have: Type, want: Type) -> bool:
    """True when both sides are one closure type and they are different lambdas (8.4).

    A declared function type has no identity, and `dynamic fn` is the boxed position that
    may hold any lambda of the signature, so neither is a conflict. An optional position
    conflicts when its inner closure does (present injection, 6.8), so the diagnostic is
    LANG-08-04 rather than a bare optional mismatch."""
    have, want = strip(have), strip(want)
    if isinstance(want, OptionalT):
        inner = have.inner if isinstance(have, OptionalT) else have
        return closure_conflict(inner, want.inner)
    hid, wid = _closure_id(have), _closure_id(want)
    if hid and wid and hid != wid:
        return True
    if isinstance(have, Named) and isinstance(want, Named) and have.name == want.name:
        return any(closure_conflict(a, b) for a, b in zip(have.args, want.args))
    if isinstance(have, TupleT) and isinstance(want, TupleT):
        return any(closure_conflict(a, b) for a, b in zip(have.items, want.items))
    if isinstance(have, RecordT) and isinstance(want, RecordT):
        wm = dict(want.fields)
        return any(k in wm and closure_conflict(ft, wm[k]) for k, ft in have.fields)
    return False


def contains_dynamic_fn(t: Type) -> bool:
    t = strip(t)
    if isinstance(t, FnT):
        return t.dynamic or any(contains_dynamic_fn(p) for p in t.params) or contains_dynamic_fn(t.ret)
    if isinstance(t, Named):
        return any(contains_dynamic_fn(a) for a in t.args)
    if isinstance(t, (OptionalT, LoanT)):
        return contains_dynamic_fn(t.inner)
    if isinstance(t, TupleT):
        return any(contains_dynamic_fn(x) for x in t.items)
    if isinstance(t, RecordT):
        return any(contains_dynamic_fn(x) for _, x in t.fields)
    return False


def dynamic_covers(expected: Optional[Type], value: Type) -> bool:
    """True when every closure identity in `value` sits in a `dynamic fn` position of `expected`."""
    if expected is None:
        return False
    e, v = strip(expected), strip(value)
    # a present value covers an optional expected type by covering its inner (6.8)
    if isinstance(e, OptionalT) and not isinstance(v, OptionalT):
        return dynamic_covers(e.inner, v)
    if isinstance(v, FnT) and v.identity and not v.dynamic:
        return isinstance(e, FnT) and e.dynamic
    if isinstance(e, OptionalT) and isinstance(v, OptionalT):
        return dynamic_covers(e.inner, v.inner)
    if isinstance(e, Named) and isinstance(v, Named) and e.name == v.name and len(e.args) == len(v.args):
        return all(dynamic_covers(a, b) for a, b in zip(e.args, v.args))
    if isinstance(e, TupleT) and isinstance(v, TupleT) and len(e.items) == len(v.items):
        return all(dynamic_covers(a, b) for a, b in zip(e.items, v.items))
    if isinstance(e, RecordT) and isinstance(v, RecordT):
        em = dict(e.fields)
        return all(k in em and dynamic_covers(em[k], ft) for k, ft in v.fields)
    return _closure_id(v) is None and not _nested_identity(v)


def _nested_identity(t: Type) -> bool:
    t = strip(t)
    if isinstance(t, FnT):
        return bool(t.identity) or any(_nested_identity(p) for p in t.params) or _nested_identity(t.ret)
    if isinstance(t, Named):
        return any(_nested_identity(a) for a in t.args)
    if isinstance(t, (OptionalT, LoanT)):
        return _nested_identity(t.inner)
    if isinstance(t, TupleT):
        return any(_nested_identity(x) for x in t.items)
    if isinstance(t, RecordT):
        return any(_nested_identity(x) for _, x in t.fields)
    return False


def origin_clash(have: Type, want: Type) -> str:
    """Why these types name different declarations, or "" when every origin agrees.

    Equality ignores origin. Assignability asks this before it accepts on equality, so a type
    argument or a callback parameter cannot swap a user's class for the core type of the same name.
    A type parameter or an unknown type agrees: the caller treats that as unproved."""
    have, want = strip(have), strip(want)
    if isinstance(have, Refined):
        have = have.base
    if isinstance(want, Refined):
        want = want.base
    if not known(have) or not known(want):
        return ""
    if isinstance(have, Named) and isinstance(want, Named):
        if have.name == want.name and (have.package != want.package or have.origin != want.origin):
            from checker.types import diagnostic_types
            actual, expected = diagnostic_types(have, want)
            return f"{actual} is not {expected}"
        if have.name == want.name:
            for a, b in zip(have.args, want.args):
                found = origin_clash(a, b)
                if found:
                    return found
        return ""
    if isinstance(have, FnT) and isinstance(want, FnT):
        for a, b in zip(have.params, want.params):
            found = origin_clash(a, b)
            if found:
                return found
        return origin_clash(have.ret, want.ret)
    if isinstance(have, OptionalT) and isinstance(want, OptionalT):
        return origin_clash(have.inner, want.inner)
    if isinstance(have, TupleT) and isinstance(want, TupleT):
        for a, b in zip(have.items, want.items):
            found = origin_clash(a, b)
            if found:
                return found
        return ""
    if isinstance(have, RecordT) and isinstance(want, RecordT):
        for (hk, ht), (wk, wt) in zip(have.fields, want.fields):
            if hk != wk:
                continue
            found = origin_clash(ht, wt)
            if found:
                return found
        return ""
    if isinstance(have, LoanT) and isinstance(want, LoanT):
        return origin_clash(have.inner, want.inner)
    if isinstance(have, DynamicT) and isinstance(want, DynamicT):
        return origin_clash(have.iface, want.iface)
    return ""


def unrefined_full(t: Type) -> Type:
    """An integer refinement spanning its base's whole range is the base itself: `Int<min..max>` is `Int` (6.3)."""
    if isinstance(t, Refined) and t.kind == "range" and t.base.name in INT_TYPES and int_bounds(t) == INT_TYPES[t.base.name]:
        return t.base
    return t


def refinement_differs(a: Type, b: Type) -> bool:
    """Two type arguments that are assignable one way but differ in a refinement (6.3, X-514): `Int<0..3>` against
    `Int<0..5>` or `Int`. A literal's exact type and a string literal are not refinements a value carries."""
    a, b = unrefined_full(strip(a)), unrefined_full(strip(b))
    if a == b or is_strlit(a) or is_strlit(b):
        return False
    ra = isinstance(a, Refined) and a.kind != "exact"
    rb = isinstance(b, Refined) and b.kind != "exact"
    if ra or rb:
        return True
    if isinstance(a, OptionalT) and isinstance(b, OptionalT):
        return refinement_differs(a.inner, b.inner)
    if isinstance(a, TupleT) and isinstance(b, TupleT):
        return any(refinement_differs(x, y) for x, y in zip(a.items, b.items))
    if isinstance(a, RecordT) and isinstance(b, RecordT):
        bm = dict(b.fields)
        return any(k in bm and refinement_differs(t, bm[k]) for k, t in a.fields)
    if isinstance(a, Named) and isinstance(b, Named):
        return any(refinement_differs(x, y) for x, y in zip(a.args, b.args))
    return False


def invariant_assignable(dc, have: Type, want: Type) -> Tuple[Optional[bool], str]:
    """A writable place has the same type in both directions (6.3, 6.9; D-707)."""
    have, want = strip(have), strip(want)
    forward = assignable(dc, have, want)
    if forward[0] is False:
        return forward
    backward = assignable(dc, want, have)
    if backward[0] is False:
        return False, f"writable types are invariant: {have.show()} is not {want.show()} (6.9)"
    pairs = None
    if isinstance(have, TupleT) and isinstance(want, TupleT):
        pairs = zip(have.items, want.items)
    elif isinstance(have, RecordT) and isinstance(want, RecordT):
        wm = dict(want.fields)
        pairs = ((t, wm[k]) for k, t in have.fields)
    elif isinstance(have, OptionalT) and isinstance(want, OptionalT):
        pairs = [(have.inner, want.inner)]
    elif isinstance(have, Named) and isinstance(want, Named) and have.name == want.name and len(have.args) == len(want.args):
        pairs = zip(have.args, want.args)
    elif isinstance(have, FnT) and isinstance(want, FnT):
        pairs = list(zip(have.params, want.params)) + [(have.ret, want.ret)]
    if pairs is not None:
        unknown = False
        for a, b in pairs:
            verdict, why = invariant_assignable(dc, a, b)
            if verdict is False:
                return False, why
            unknown |= verdict is None
        return (None, "") if unknown else (True, "")
    if isinstance(have, DynamicT) or isinstance(want, DynamicT):
        if have == want:
            return True, ""
        if not isinstance(have, (Unknown, TypeParam)) and not isinstance(want, (Unknown, TypeParam)):
            return False, f"writable types are invariant: {have.show()} is not {want.show()} (6.9)"
    if refinement_differs(have, want):
        return False, f"writable types are invariant: {have.show()} is not {want.show()} (6.9)"
    return (None, "") if forward[0] is None or backward[0] is None else (True, "")


def contains_structural(t: Type) -> bool:
    """Whether an editable loan exposes a record or tuple, including inside a container."""
    t = strip(t)
    if isinstance(t, (RecordT, TupleT)):
        return True
    if isinstance(t, Refined):
        return contains_structural(t.base)
    if isinstance(t, OptionalT):
        return contains_structural(t.inner)
    return isinstance(t, Named) and any(contains_structural(a) for a in t.args)


def assignable(dc, have: Type, want: Type) -> Tuple[Optional[bool], str]:
    """(True, "") when have is assignable to want; (False, why) when not; (None, "") when unknown."""
    if (isinstance(have, LoanT) and have.mode == "editable") or (isinstance(want, LoanT) and want.mode == "editable"):
        # A length-refined collection is never editable: its existing E-REF-003 is decisive.
        # Avoid a second type-mismatch record for the same forbidden loan.
        core = strip(have)
        if (contains_structural(have) or contains_structural(want)) and not (isinstance(core, Refined) and core.kind == "length"):
            return invariant_assignable(dc, core, strip(want))
    have, want = strip(have), strip(want)
    if isinstance(have, (Unknown, TypeParam)) or isinstance(want, (Unknown, TypeParam)):
        return None, ""
    if found := origin_clash(have, want):
        return False, found
    if closure_conflict(have, want):
        return False, CLOSURE_WHY
    if isinstance(want, OptionalT):
        if isinstance(have, OptionalT):
            return assignable(dc, have.inner, want.inner)
        if isinstance(have, Named) and not have.package and have.name == "Never":      # null
            return True, ""
        return assignable(dc, have, want.inner)
    if isinstance(have, Named) and not have.package and have.name == "Never":
        return True, ""
    if is_strlit(have):
        if isinstance(want, Refined) and want.kind == "scalars" and want.base in (STRING, STRING_VIEW) and isinstance(have, Refined):
            lo, hi = want.lo if want.lo is not None else 0, want.hi if want.hi is not None else have.hi
            return (True, "") if lo <= have.lo <= hi else (False, f"the literal has {have.lo} scalars, outside {want.show()}")
        if want in (STRING_VIEW, STRING): return True, ""
        if isinstance(want, Refined) and want.base in (STRING_VIEW, STRING): return None, ""
        return False, f"a string literal is a StringView (or a String where one is expected), not {want.show()}"
    if have == INTERP:
        return assignable(dc, STRING, want)
    hb, wb = int_bounds(have), int_bounds(want)
    if hb is not None and wb is not None:
        if isinstance(want, Refined) and want.kind != "range":
            return None, ""
        ok = wb[0] <= hb[0] and hb[1] <= wb[1]
        if ok:
            return True, ""
        if isinstance(have, Refined) and have.kind == "exact":
            return False, f"interval [{hb[0]}, {hb[1]}] is outside [{wb[0]}, {wb[1]}]"
        if isinstance(have, Named) and isinstance(want, Named):
            return False, f"{have.show()} is not {want.show()} (distinct integer types)"
        return None, ""                 # a place's interval is a fact of the interval domain (DEL-13)
    if hb is not None or wb is not None:
        if isinstance(have, Refined) and isinstance(want, Refined) and have.kind == want.kind and have.base == want.base:
            return None, ""     # scalars/length refinements: DEL-13
        if isinstance(have, Refined) and have.base == want: return True, ""
        if isinstance(have, Named) and isinstance(want, Refined) and have == want.base: return None, ""   # unproved: DEL-13
        return False, f"{have.show()} is not {want.show()}"
    if isinstance(have, Refined) and isinstance(want, Refined):
        if have.base == want.base and have.kind == want.kind and have.kind in ("scalars", "length"):
            if (want.lo is None or (have.lo is not None and have.lo >= want.lo)) and (want.hi is None or (have.hi is not None and have.hi <= want.hi)):
                return True, ""
        return None, ""
    if isinstance(have, Refined): return assignable(dc, have.base, want)
    if isinstance(want, Refined): return None, ""
    if isinstance(have, Named) and isinstance(want, Named):
        if have.name == want.name and (have.package != want.package or have.origin != want.origin):
            return False, origin_clash(have, want)
        if have == want:
            # Equality ignores origins nested in the arguments. Reject before accepting.
            found = origin_clash(have, want)
            if found:
                return False, found
            return True, ""
        if not have.package and have.name == "Never": return True, ""
        if not have.package and not want.package and (have.name, want.name) in VIEW_OF and (not have.args or not want.args or have.args[0] == want.args[0]):
            if have.args and want.args:          # an owned value where its view is expected (6.6, 7.4); the element still has an origin
                found = origin_clash(have.args[0], want.args[0])
                if found:
                    return False, found
            return True, ""
        if have.name == want.name and have.origin == want.origin and have.package == want.package:
            # arguments are invariant in the proved direction: one false argument makes the whole type false,
            # so `List[Bool]` is not `List[Int]` even when another argument is still a type parameter (6.1, 6.7).
            # Unknown stays unknown only while an argument itself is unproved (a type parameter, a tuple).
            if len(have.args) != len(want.args):
                # A constructor's result names the class (`Hold`) while the signature names `Hold[Tree]`,
                # and those argument counts were never proved apart. Same-arity arguments are folded below,
                # so `List[Bool]` is still not `List[Int]`.
                return None, ""
            unknown = False
            for a, b in zip(have.args, want.args):
                found = origin_clash(a, b)
                if found:
                    return False, found
                verdict, why = assignable(dc, a, b)
                if verdict is False:
                    return False, why
                if verdict is True and refinement_differs(a, b):
                    # a type argument is invariant in its refinement (6.3, X-514; D-465): through an alias with the wider
                    # element type a value the narrower one excludes could be stored
                    return False, f"type arguments are invariant in their refinement: {a.show()} is not {b.show()} (6.3)"
                if verdict is None:
                    unknown = True
            if unknown:
                return None, ""
            return True, ""
        if have.name == want.name: return False, f"{have.name} of {have.package or 'this package'} is not {want.name} of {want.package or 'this package'}"
        if want.name == "Self" or have.name == "Self": return None, ""
        return False, f"{have.show()} is not {want.show()}"
    if isinstance(have, (TupleT, RecordT)) and type(have) is type(want):
        if isinstance(have, TupleT):
            if len(have.items) != len(want.items):
                return False, "tuple element count differs"
            pairs = zip(have.items, want.items)
        else:
            hm, wm = dict(have.fields), dict(want.fields)
            if hm.keys() != wm.keys():
                return False, "record field names differ"
            pairs = ((hm[k], wm[k]) for k in hm)
        unknown = False
        for a, b in pairs:
            verdict, why = assignable(dc, a, b)
            if verdict is False:
                return False, why
            unknown |= verdict is None
        return (None, "") if unknown else (True, "")
    if isinstance(have, FnT) and isinstance(want, FnT):
        order = {"fn": 0, "fn mut": 1, "fn once": 2}
        if order[have.kind] > order[want.kind]:
            return False, f"a `{have.kind}` closure cannot be used as `{want.kind}` (8.4)"
        if len(have.params) != len(want.params):
            return False, "parameter count differs"
        for i, (hp, wp) in enumerate(zip(have.params, want.params)):
            if have.mode(i) != want.mode(i):
                return False, f"parameter {i + 1} mode {have.mode(i) or 'read'} differs from {want.mode(i) or 'read'} (8.4)"
            if known(hp) and known(wp) and strip(hp) != strip(wp):
                return False, f"parameter {hp.show()} differs from {wp.show()}"
            if known(hp) and known(wp):
                found = origin_clash(hp, wp)     # equality ignores an origin nested in the parameter
                if found:
                    return False, f"parameter {found}"
        hm, wm = set(have.modifiers), set(want.modifiers)
        if "async" in wm and "async" not in hm:
            return False, "the position requires an async callable: declare the function `async`, or wrap the call in an async lambda (8.4, 12.3)"
        for claim in ("pure", "noalloc"):
            if claim in wm and claim not in hm:
                return False, f"the position requires `{claim}`"
        for weak in ("try", "async", "recovers"):
            if weak in hm and weak not in wm:
                return False, f"the closure is `{weak}` but the position is not"
        r = assignable(dc, have.ret, want.ret)
        return (True, "") if r[0] is not False else (False, "result " + r[1])
    if isinstance(have, OptionalT) and not isinstance(want, OptionalT):
        return False, f"{have.show()} is optional; {want.show()} is not (6.8)"
    if type(have) is not type(want):
        if isinstance(want, DynamicT) or isinstance(have, DynamicT): return None, ""
        # two known types of different kinds (`Light` against `fn`, `fn` against `Never?`) are not the same type (6.1)
        return False, f"{have.show()} is not {want.show()}"
    return None, ""
