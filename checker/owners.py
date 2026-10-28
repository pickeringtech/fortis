"""Lexical loans, moves and markers (DEL-14: Language Reference 7.2–7.6, 5.7, 9.2).

`Owners` is the flow-sensitive ownership state the typer threads through a body beside its facts:
which places have been moved out (on some path), which loans are active (owner path, mode, holder
binding, the block depth at which they end), which bindings hold loans (paths to an owner) and the
reservations of the call being evaluated (two-phase calls, 5.7). Every check reports one of the
codes E-OWN-001 (marker required or not permitted), E-OWN-002 (copy of a non-copyable place),
E-OWN-010 (conflicting loan), E-OWN-011 (owner used while loaned), E-OWN-012 (broken reservation),
E-OWN-020 (use after move), E-OWN-040 (partial move)."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional, Set, Tuple


@dataclass
class Loan:
    owner: str                 # the owner place path (a binding root, or a field path of one)
    mode: str                  # alias | editable
    holder: Optional[str]      # the binding that holds it, or None for a statement/call temporary
    depth: int                 # block depth at which a held loan ends; temporaries end with the statement
    line: int = 0


def overlaps(a: str, b: str) -> bool:
    """Two place paths overlap when one is a prefix of the other (7.3: disjoint field paths coexist)."""
    return a == b or a.startswith(b + ".") or b.startswith(a + ".")


def root_of(path: str) -> str:
    return path.split(".")[0].split("[")[0]


@dataclass
class Owners:
    moved: Set[str] = field(default_factory=set)                       # places moved out on some path
    loans: List[Loan] = field(default_factory=list)
    holders: Dict[str, Loan] = field(default_factory=dict)             # loan-holding bindings -> the loan they hold
    reservations: List[Tuple[str, str]] = field(default_factory=list)  # (path, mode) reserved by the call being evaluated
    obligations: Dict[str, Tuple[object, int]] = field(default_factory=dict)   # linear place -> (origin token, block depth) (10.1)
    half: List[Tuple[str, object]] = field(default_factory=list)       # obligations consumed on one incoming path only (filled by join)
    captured: Dict[str, str] = field(default_factory=dict)             # observation frame field -> "all" (every path) | "some" (13)

    def copy(self) -> "Owners":
        return Owners(set(self.moved), list(self.loans), dict(self.holders), list(self.reservations), dict(self.obligations), [], dict(self.captured))

    def join(self, other: "Owners") -> "Owners":
        """A place is initialised only when it is on every path; a loan is live when live on any path; an
        obligation open on one path and consumed on the other is a violation the typer reports (10.1)."""
        loans = list(self.loans)
        for l in other.loans:
            if not any(l.owner == m.owner and l.mode == m.mode and l.holder == m.holder for m in loans):
                loans.append(l)
        holders = dict(self.holders)
        holders.update(other.holders)
        obligations = {}
        half = []
        for p in set(self.obligations) | set(other.obligations):
            a, b = self.obligations.get(p), other.obligations.get(p)
            if a is not None and b is not None:
                obligations[p] = a
            else:
                half.append((p, (a or b)[0]))
        captured = {}
        for f in set(self.captured) | set(other.captured):
            a, b = self.captured.get(f), other.captured.get(f)
            captured[f] = "all" if a == "all" and b == "all" else "some"
        return Owners(self.moved | other.moved, loans, holders, [], obligations, half, captured)

    # ---- queries -------------------------------------------------------------------------
    def owner_path(self, path: str) -> str:
        """Loans are recorded against the owner, not the path: resolve a loan-holding root to its owner."""
        root = root_of(path)
        h = self.holders.get(root)
        if h is not None:
            rest = path[len(root):]
            return h.owner + rest
        return path

    def active(self, path: str) -> List[Loan]:
        p = self.owner_path(path)
        return [l for l in self.loans if overlaps(l.owner, p) or root_of(l.owner) == root_of(p) and l.mode == "editable"]

    def exclusive_by_other(self, path: str, holder: Optional[str]) -> Optional[Loan]:
        p = self.owner_path(path)
        for l in self.loans:
            if l.mode == "editable" and l.holder != holder and (overlaps(l.owner, p) or root_of(l.owner) == root_of(p)):
                return l
        return None

    def is_moved(self, path: str) -> bool:
        return any(path == m or path.startswith(m + ".") or m.startswith(path + ".") for m in self.moved)

    # ---- transitions -----------------------------------------------------------------------
    def end_block(self, depth: int):
        self.loans = [l for l in self.loans if l.holder is None or l.depth < depth]
        self.holders = {h: l for h, l in self.holders.items() if l.depth < depth}

    def end_statement(self):
        self.loans = [l for l in self.loans if l.holder is not None]
        self.reservations = []

    def add_loan(self, path: str, mode: str, holder: Optional[str], depth: int, line: int) -> Loan:
        l = Loan(self.owner_path(path), mode, holder, depth, line)
        self.loans.append(l)
        if holder is not None:
            self.holders[holder] = l
        return l

    def move(self, path: str):
        self.moved.add(path)
        self.obligations.pop(path, None)                   # a move transfers the obligation (10.2)

    def reinit(self, path: str):
        self.moved = {m for m in self.moved if not (m == path or m.startswith(path + "."))}
