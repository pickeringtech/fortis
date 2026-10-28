"""The interval domain of Language Reference 6.3 (DEL-13), exactly as listed and nothing more:
intervals over integer places, interval arithmetic for the proof-required operators (6.4), narrowing
by the listed comparison forms, hull at joins, widening at loop headers, invalidation by exclusive
access, length facts for indexing (6.7) and the three record statuses (violated / unproved /
unknown, PROG-DIA-02). `Facts` is the flow-sensitive state the typer threads through a body."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, Optional, Tuple

from checker.types import INT_TYPES, Named, OptionalT, Refined, Type, int_bounds

Interval = Tuple[int, int]

# SP-05b candidate rule (not in the published reference): a ranged `for i in a..<p.count()` establishes
# the length fact `i < p` for the body when `p` is a place the body neither assigns nor loans editable.
RANGED_LENGTH_FACT = True        # adopted by X-93 (SP-05b); `measure.py --published` turns the two rules off
# SP-05b candidate rule 2: the success value of a checked integer intrinsic (`Int.add(a, b)` ...) has the
# interval of the operation clipped to the type's range, since the call fails instead of overflowing.
CHECKED_RESULT_INTERVAL = True


# Every place whose facts were discarded, in order (D-74, D-77): a narrowing derived from an operand read before a
# later operand discarded the place's facts must not be installed. The typer marks the log's length before an operand
# and reads what was invalidated after it; it clears the log at the start of each function body.
LOG: list = []


def overlaps(a: str, b: str) -> bool:
    """The place `a` is `b`, a prefix of it, or a field of it: a write to one may change the other."""
    return a == b or a.startswith(b + ".") or b.startswith(a + ".")


def hull(a: Optional[Interval], b: Optional[Interval]) -> Optional[Interval]:
    if a is None or b is None:
        return None
    return (min(a[0], b[0]), max(a[1], b[1]))


def meet(a: Interval, b: Interval) -> Optional[Interval]:
    lo, hi = max(a[0], b[0]), min(a[1], b[1])
    return (lo, hi) if lo <= hi else None


def arith(op: str, a: Interval, b: Interval, width: int = 64) -> Optional[Interval]:
    """Interval arithmetic of 6.3/6.4; None when the rules give no result (division by a zero-containing divisor, shift out of range)."""
    if op == "+": return (a[0] + b[0], a[1] + b[1])
    if op == "-": return (a[0] - b[1], a[1] - b[0])
    if op == "*":
        ps = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]]
        return (min(ps), max(ps))
    if op == "/":
        if b[0] <= 0 <= b[1]:
            return None
        qs = [tdiv(x, y) for x in a for y in b]
        return (min(qs), max(qs))
    if op == "%":
        if b[0] <= 0 <= b[1]:
            return None
        m = max(abs(b[0]), abs(b[1]))
        if a[0] >= 0: return (0, m - 1)
        if a[1] <= 0: return (-(m - 1), 0)
        return (-(m - 1), m - 1)
    if op == "<<":
        if b[0] < 0 or b[1] > width - 1:
            return None
        vals = [a[0] << b[0], a[0] << b[1], a[1] << b[0], a[1] << b[1]]
        return (min(vals), max(vals))
    if op == ">>":
        if b[0] < 0 or b[1] > width - 1:
            return None
        vals = [a[0] >> b[0], a[0] >> b[1], a[1] >> b[0], a[1] >> b[1]]
        return (min(vals), max(vals))
    return None


def tdiv(x: int, y: int) -> int:
    q = abs(x) // abs(y)
    return q if (x >= 0) == (y >= 0) else -q


def narrow(op: str, p: Interval, c: Interval) -> Optional[Interval]:
    """The interval of a place p after `p op c` holds."""
    if op == "<": return meet(p, (p[0], c[1] - 1))
    if op == "<=": return meet(p, (p[0], c[1]))
    if op == ">": return meet(p, (c[0] + 1, p[1]))
    if op == ">=": return meet(p, (c[0], p[1]))
    if op == "==": return meet(p, c)
    if op == "!=":
        if c[0] == c[1]:
            if p[0] == c[0]: return meet(p, (p[0] + 1, p[1]))
            if p[1] == c[0]: return meet(p, (p[0], p[1] - 1))
        return p
    return p


NEGATE = {"<": ">=", "<=": ">", ">": "<=", ">=": "<", "==": "!=", "!=": "=="}
FLIP = {"<": ">", "<=": ">=", ">": "<", ">=": "<=", "==": "==", "!=": "!="}


@dataclass
class Facts:
    """Flow-sensitive facts about places: integer intervals, presence of optionals, index-below-length facts."""
    ints: Dict[str, Interval] = field(default_factory=dict)
    present: Dict[str, bool] = field(default_factory=dict)
    below: Dict[str, str] = field(default_factory=dict)          # index place -> collection place whose length bounds it

    def copy(self) -> "Facts":
        return Facts(dict(self.ints), dict(self.present), dict(self.below))

    @staticmethod
    def join_all(states: list) -> "Facts":
        """The hull of every incoming path (6.3); an empty list has no incoming path."""
        out = states[0].copy()
        for s in states[1:]:
            out = out.join(s)
        return out

    def join(self, other: "Facts") -> "Facts":
        ints = {}
        for k in set(self.ints) & set(other.ints):
            h = hull(self.ints[k], other.ints[k])
            if h is not None:
                ints[k] = h
        present = {k: True for k in set(self.present) & set(other.present) if self.present[k] and other.present[k]}
        below = {k: v for k, v in self.below.items() if other.below.get(k) == v}
        return Facts(ints, present, below)

    def invalidate(self, place: str):
        """Every fact about the place, a prefix of it, or a field of it is discarded (6.3)."""
        LOG.append(place)
        for d in (self.ints, self.present, self.below):
            for k in list(d):
                if k == place or k.startswith(place + ".") or place.startswith(k + "."):
                    del d[k]
        for k, v in list(self.below.items()):
            if v == place or v.startswith(place + ".") or place.startswith(v + "."):
                del self.below[k]

    def split_off_names(self, names) -> "Facts":
        """Remove facts about `names` (each place, a prefix of it, or a field of it) and return them.

        A match arm uses this so a guard fact stays with the binding that established it (6.8, 9.8)."""
        names = list(names)

        def hit(place: str) -> bool:
            return any(overlaps(place, n) for n in names)

        saved = Facts()
        for src, dst in ((self.ints, saved.ints), (self.present, saved.present)):
            for k in [k for k in src if hit(k)]:
                dst[k] = src.pop(k)
        for k in [k for k, v in self.below.items() if hit(k) or hit(v)]:
            saved.below[k] = self.below.pop(k)
        return saved

    def restore_names(self, names, saved: "Facts") -> None:
        """Drop facts about `names`, then install `saved` (what those names meant for an outer binding)."""
        self.split_off_names(names)
        self.ints.update(saved.ints)
        self.present.update(saved.present)
        self.below.update(saved.below)

    def sealed(self, names, outer: "Facts") -> "Facts":
        """A copy that carries `outer` for `names` instead of this state's facts about them."""
        f = self.copy()
        f.restore_names(names, outer)
        return f


def declared(t: Type) -> Optional[Interval]:
    return int_bounds(t)


def classify(have: Interval, want: Interval) -> str:
    """violated when the intervals are disjoint, unproved when they overlap but have is not contained."""
    if have[1] < want[0] or have[0] > want[1]:
        return "violated"
    return "unproved"
