"""Statement and expression typing over declaration tables (DEL-07 visibility, DEL-11 literals,
DEL-12 expressions, statements, patterns and function exits: Language Reference 4.4, 4.5, 5.1–5.6,
5.8, 6.8, 6.9, 9.2, 9.8, 10.4).

One walk per function body with an environment of bindings (type, mutability, kind). Every
expression form has its rule here; what the rule cannot decide (a generic result the arguments do
not fix, a kit type, a form a later layer owns) is `Unknown` and produces no record: the
checker never reports a proved violation it has not proved (PROG-DIA-02). Codes: E-NAM-004,
E-TYP-010/030/040/050/110/120/130, E-NAM-002, E-MAT-001/002, E-REF-001 (literals). Plug-in points
for later layers are marked `# plug-in:`."""
from __future__ import annotations

import dataclasses
import re
from dataclasses import dataclass
from typing import Dict, List, Optional, Sequence, Set, Tuple

import lexer as lexer_mod
import tree as tree_mod
from lexer import Token
from tree import ARITH_RULES, children, name, direct_tokens, find, find_all, lambda_body, pun_name, region_operators, tokens

from checker.decls import Decl, DeclChecker, Sig, standard
from reading import CAPABILITY_TYPES, LOG_LEVELS, call_name_token, log_call, outcome_lets, postfix_call_token
from checker import intervals as iv
from checker.intervals import Facts
from checker.owners import Loan, Owners, overlaps, root_of
from checker import effects as fx
from checker.effects import Summary
from checker.literals import CLOSURE_WHY, INTERP, STRLIT, assignable, closure_conflict, concrete, contains_dynamic_fn, declared_vs_lambda, dynamic_covers, is_strlit, literal_type, negate
from checker.types import (BOOL, DECIMAL, DURATION, FLOAT, INT, INT_TYPES, NEVER, STRING, STRING_VIEW, VOID, DynamicT, FnT, LoanT, Named,
                           OptionalT, RecordT, Refined, TupleT, Type, TypeParam, Unknown, base_name, int_bounds)

STRUCTURAL = (TupleT, RecordT)
CHECKED_OPS = ("+", "-", "*", "/", "%", "<<")     # the operators with a checked `Int` operation (6.4, X-345)


FLOAT32 = Named("Float32")
FAILURE = Named("Failure")
ITERABLE = {"List", "ListView", "Array", "Set", "FixedList", "OrderedMap", "Map"}
OPEN_SCRUTINEE = {"Int", "String", "StringView", "Failure", "Scalar", "Byte", "Decimal", "Duration", "Timestamp"} | set(INT_TYPES)


@dataclass
class Binding:
    type: Type
    mutable: bool = False
    kind: str = "let"            # let | mut | param | editable | owned | self | pattern | for | catch | capture | scope
    depth: int = 0               # block depth of the declaration (0: a parameter)


class Env:
    def __init__(self, parent: Optional["Env"] = None):
        self.parent = parent
        self.vars: Dict[str, Binding] = {}

    def lookup(self, n: str) -> Optional[Binding]:
        e = self
        while e is not None:
            if n in e.vars:
                return e.vars[n]
            e = e.parent
        return None

    def get(self, n: str) -> Optional[Type]:
        b = self.lookup(n)
        return b.type if b else None

    def set(self, n: str, t: Type, mutable=False, kind="let"):
        self.vars[n] = Binding(t, mutable, kind)


def base_type(t: Type) -> Type:
    t = t.inner if isinstance(t, LoanT) else t
    while isinstance(t, Refined):
        t = t.base
    return t


def strip(t: Type) -> Type:
    return t.inner if isinstance(t, LoanT) else t


def known(t: Type) -> bool:
    return not isinstance(t, (Unknown, TypeParam))


def hole_sources(tk: Token) -> List[str]:
    """The source of each `${…}` hole of a string token, in order: the lexer's record, which skips strings nested in a
    hole (D-161), else (a synthesised token) a scan of its text."""
    return [s for s, _, _ in tk.interps] if getattr(tk, "interps", None) else holes_of(tk.text)


def holes_of(text: str) -> List[str]:
    """The source of each `${…}` hole of a string literal's text, in order (braces only; see `hole_sources`)."""
    body, i, out = text[1:-1], 0, []
    while i < len(body):
        if body[i] == "\\":
            i += 2
            continue
        if body.startswith("${", i):
            depth, j = 1, i + 2
            while j < len(body) and depth:
                if body[j] == "{":
                    depth += 1
                elif body[j] == "}":
                    depth -= 1
                j += 1
            out.append(body[i + 2:j - 1])
            i = j
        else:
            i += 1
    return out


def lower_names(toks: Sequence[Token]) -> Set[str]:
    """The text of every LOWER token among `toks`, those inside the holes of a string token included: a hole is an
    expression evaluated where its string is (3.6), so a place named only in `"${t.label}"` is named (D-391)."""
    out: Set[str] = set()
    for t in toks:
        if t.kind == "LOWER":
            out.add(t.text)
        elif t.kind == "STR" and "${" in t.text:
            for src in hole_sources(t):
                try:
                    out |= lower_names(lexer_mod.lex(src))
                except Exception:  # noqa: BLE001
                    continue
    return out


class Typer:
    def __init__(self, dc: DeclChecker):
        self.dc = dc
        self.table = dc.table
        self.module = dc.module_name
        self.package = dc.package
        self.entry_module: Optional[str] = dc.entry_module   # the program's entry module (4.6), whose `main` creates the capabilities
        self.records = dc.records
        self.aliases: Dict[str, str] = {}
        self.self_type: Optional[Named] = None
        self.self_mode: str = "self"
        self.ret_type: Optional[Type] = None
        self.in_try: bool = False
        self.ctx: List[str] = []          # "arm" | "ifexpr" | "catch" | "loop" | "whilecond" | "lambda" | "hook" | "function"
        self.arm_types: List[List[Type]] = []
        self.loop_labels: List[Optional[str]] = []
        self.give_seen: List[bool] = []
        self.facts: Facts = Facts()
        self.diverged: bool = False
        self.coll_path: Optional[str] = None
        self.own: Owners = Owners()
        self.depth: int = 0
        self.arg_mode: Optional[str] = None       # parameter mode of the argument being typed ("" | "editable" | "owned" | "from")
        self.receiver: Optional[Tuple[str, str]] = None   # (place path, marker) of a marked receiver for the next call
        self.in_marker: bool = False
        self.pending: Optional[Tuple[str, str, Token]] = None   # a marked place awaiting its binding (let) or position
        self.copy_refused: Set[Tuple[int, int]] = set()          # `copy` markers already E-OWN-002, by line and column (D-341)
        self.res_stack: List[List[Tuple[str, str]]] = []       # reservations of the enclosing calls being evaluated (5.7)
        self.lambda_kind: List[str] = []                        # kinds of the lambdas being typed (8.2)
        self.last_closure: Optional[dict] = None                # captures of the lambda just typed, awaiting its binding
        self.closures: Dict[str, dict] = {}                     # closure binding -> {"kind", "captures": [(path, marker)]}
        self.discharges: List[dict] = []                        # failures discharged without a report (for the recovers effect, DEL-19)
        self.eff: Summary = Summary()                           # the effect summary of the body being typed (11.1)
        self.fresh: Set[str] = set()                             # locals whose storage is fresh in this activation (11.3)
        self.editable_params: Set[str] = set()
        self.summaries: Dict[str, Summary] = {}                  # function/method label -> summary (fixed point across the module)
        self.current_label: str = ""
        self.async_body: bool = False
        self.call_decl: Optional[Decl] = None
        self.call_label: str = ""
        self.final_pass: bool = True
        self.reported: Set[str] = set()
        self.schema: Optional[Decl] = None                       # the observation schema attached by @observe (13)
        self.schema_unknown: bool = False
        self.current_aspects: list = []                          # aspectUse nodes of the function being typed (11.8)
        self.caps_calls: int = 0                                 # Host.capabilities() calls in the body (11.6)
        self.call_sigs: Dict[Tuple[int, int], Sig] = {}          # a call's callee by its name token's position (5.5, `@ignorable`)
        self.test_body: bool = False                             # typing a test or property body: `or fail` is implied (14, X-345)
        self.consumed_ops: Set[int] = set()                      # nodes that `or`, `catch` or `match` consume (14)
        self.implied_fails: List[Tuple[int, int]] = []           # positions of calls with an implied `or fail` (14)
        self.implied_regions: Set[Tuple[int, int]] = set()       # first operators of a test's checked regions (14, 6.4)
        self.tparams_in_scope: Set[str] = set()                  # the type parameters of the body being typed (6.10)
        self.where_in_scope: list = []                           # its owner's and its own `where` bounds, with arguments
        self.pending_owner_where: tuple = ()                     # a generic class's bounds, for the method call checked next
        self.const_stack: Set[str] = set()                       # constants whose value is being typed (a cycle guard)
        self.elaborations: List[str] = []                        # aspect elaboration records (golden tests, LANG-11-09)
        self.defer_depth: int = 0                                # > 0 while typing a deferred block (12.1)
        self.node_types: Optional[Dict[int, Type]] = None        # id(expr node) -> inferred type when recording (DEL-25)
        self.branch_expected: List[Optional[Type]] = []           # the type a `give`/`recover` value joins, when known
        self.branch_target = False                                 # typing a branch value whose target type is known
        self.closure_info: Dict[int, dict] = {}                  # id(lambda node) -> {kind, captures}
        self.regions: List[dict] = []                            # checked-arithmetic regions being typed (6.4, X-345)
        self.unresolved_args: int = 0                            # > 0 while typing an unresolved call's arguments (D-304)
        self.checked_ops: Set[Tuple[int, int]] = set()           # (line, column) of each checked operator (6.4, X-345)
        self.guards: List[Tuple[str, int, Token]] = []           # MutexGuard bindings (name, depth, token) live in the body (12.3)
        self.scope_depths: List[Tuple[str, int]] = []            # (scope name, block depth) of enclosing scope blocks (12.5)
        self.deferred: List[Tuple[int, Set[str], Token]] = []    # (block depth, places the defer names, token) of active defers
        self.defer_bodies: List = []           # (block depth, deferred block) registered in the blocks being walked (D-75)
        self.exit_frames: List = []            # (depth, states) per if-expression branch, match arm and catch block: the
                                               # states at each `give`/`recover`, after the cleanup they run (D-76)
        self.break_frames: List = []           # (label, depth, states) per loop: the states at each `break` (D-72)
        self.block_rest: List = []             # the statements after the one being walked, in its block (D-75)
        self.catch_bound: List[str] = []                        # failure bindings of the enclosing catch blocks
        self.carried: List = []                                  # loans a call result carries, awaiting their binding
        self.carried_temps: List[Tuple[Token, int]] = []         # temporaries an async call's future lends, with the call's depth (12.3; D-31)
        self.call_depth = 0                                      # calls being checked, innermost last
        self.use_ok: bool = True
        self.field_closures = self.dc.session.field_closures  # shared across modules and both passes (8.4)
        self.pending_call_expected: Optional[Type] = None        # expected type of the call about to be checked, consumed by it
        self.result_expected: Optional[Type] = None              # expected type of a chain's last call, copied across at the call itself

    # ---- records ----------------------------------------------------------------------
    def rec(self, code, msg, rule, tok, fields):
        self.dc.rec(code, msg, rule, tok, fields)

    def typ(self, code, msg, tok, expected, actual, span, rule="LANG-06-01"):
        self.rec(code, msg, rule, tok, {"expected_type": expected, "actual_type": actual, "primary_span": span})

    def decl_of(self, t: Type) -> Optional[Decl]:
        """The declaration of a named type. A type is known by its identity (package@version and module, LANG-04-04),
        computed in the module that declares it, not by a same-named declaration this module happens to import:
        `import acme.left.items as left` enters `left.Item`, whose values are `Item`."""
        t = strip(t)
        if isinstance(t, (Named, Refined)):
            return self.dc.resolve_type(t)
        return None

    def foreign(self, d: Decl) -> bool:
        return d.module != self.module

    # ---- visibility (4.4) -------------------------------------------------------------
    def member_visible(self, d: Decl, member: str) -> bool:
        if member in d.fields:
            if d.fields[member][1]: return True
        elif member in d.methods:
            if d.methods[member].exported: return True
        else:
            return True
        for iface in d.implements:
            idecl = self.dc.resolve_type(iface)
            if idecl is not None and (member in idecl.methods or member in idecl.constants):
                return True
            if iface.name == "Abandonable" and member == "abandon":
                return True
        for mx in d.mixins:
            mdecl = self.dc.resolve_type(mx)
            if mdecl is not None and ("requires:" + member) in mdecl.methods:
                return True
        return False

    def check_member(self, d: Decl, member: str, tok: Token, what: str = "member"):
        if self.foreign(d) and not self.member_visible(d, member):
            self.rec("E-NAM-004", f"{what} `{member}` of {d.name} is not exported; {d.name} is declared in {d.module}", "LANG-04-05" if what != "field" else "LANG-09-02",
                     tok, {"name": member, "candidates": sorted(k for k, v in d.fields.items() if v[1]) + sorted(k for k, s in d.methods.items() if s.exported)})

    # ---- the interval domain (6.3) ------------------------------------------------------
    def path_of_place(self, pl) -> Optional[str]:
        ts = direct_tokens(pl)
        parts = [ts[0].text]
        for k in children(pl)[1:]:
            if name(k) == "memberName":
                parts.append(tokens(k)[0].text)
            elif isinstance(k, Token) and k.kind == "INT":
                parts.append("[" + k.text + "]")
        return ".".join(parts)

    def path_of_expr(self, e) -> Optional[str]:
        """The place path of an expression that is a place (a binding, self, or a member path), else None."""
        if e is None:
            return None
        n = name(e)
        kids = children(e)
        if n in ("constExpr", "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr",
                 "shiftExpr", "additiveExpr", "multiplicativeExpr", "unaryExpr", "exprStmt") and len(kids) == 1:
            return self.path_of_expr(kids[0])
        if n == "place":
            return self.path_of_place(e)
        if n == "markerExpr" and len(kids) == 2:
            return self.path_of_place(find(e, "place"))
        if n == "primaryExpr":
            if len(kids) == 1 and isinstance(kids[0], Token) and (kids[0].kind == "LOWER" or kids[0].text == "self"):
                return kids[0].text
            if len(kids) == 3 and isinstance(kids[0], Token) and kids[0].text == "(":
                return self.path_of_expr(kids[1])
            return None
        if n == "postfixExpr":
            root = self.path_of_expr(kids[0])
            if root is None:
                return None
            for pf in kids[1:]:
                k = children(pf)
                if k and isinstance(k[0], Token) and k[0].text == "." and len(k) > 1 and name(k[1]) == "memberName":
                    root += "." + tokens(k[1])[0].text
                else:
                    return None
            return root
        return None

    def interval_of(self, t: Type) -> Optional[iv.Interval]:
        return int_bounds(strip(t))

    def fact_type(self, path: Optional[str], t: Type) -> Type:
        """A place's type sharpened by the current facts."""
        if path is None:
            return t
        s = strip(t)
        if isinstance(s, OptionalT) and self.facts.present.get(path):
            return s.inner
        if path in self.facts.ints and self.interval_of(s) is not None:
            lo, hi = self.facts.ints[path]
            base = s.base if isinstance(s, Refined) else s
            return Refined(base, lo, hi)
        return t

    def is_int(self, t: Type) -> bool:
        return self.interval_of(t) is not None

    def ref(self, code, msg, tok, status, required="", proved="", widening=""):
        self.rec(code, msg, "LANG-06-02", tok, {"required_range": required, "proved_interval": proved, "widening_point": widening})
        self.records[-1]["status"] = status

    def show_iv(self, i: Optional[iv.Interval]) -> str:
        return "?" if i is None else f"[{i[0]}, {i[1]}]"

    def set_fact(self, path: Optional[str], t: Type):
        """After `let`/assignment: the place has the interval of the assigned expression (or presence)."""
        if path is None:
            return
        self.facts.invalidate(path)
        i = self.interval_of(t)
        if i is not None:
            self.facts.ints[path] = i
        s = strip(t)
        if isinstance(s, Named) and s.name != "Never" and not isinstance(s, OptionalT):
            pass

    def assigned_places(self, block) -> Set[str]:
        """Places assigned or exclusively accessed anywhere in a loop body (widened at the header, 6.3)."""
        return {o for o in self.writes_in(block) if o}

    def writes_in(self, node) -> List[str]:
        """Every place the node assigns or loans exclusively, in order and with repeats. A lambda body is a separate
        activation and is not entered: the lambda writes outside itself only through its `editable` captures, which count
        as writes where it is made; a marked use of a closure binding counts its `editable` captures as written there; an
        interpolation hole is an expression evaluated where its string is (3.6), so its writes count there."""
        out: List[str] = []
        for c in children(node):
            out += self.writes_of(c)
        return out

    def writes_of(self, c) -> List[str]:
        """The writes of one child: the node itself (see `writes_in`), then what it contains."""
        out: List[str] = []
        if isinstance(c, Token):
            if c.kind == "STR" and "${" in c.text:
                for src in hole_sources(c):
                    try:
                        et = self.dc.lib.rec.parse(lexer_mod.lex(src), "expr")
                    except Exception:  # noqa: BLE001
                        continue
                    out += self.writes_of(et)
            return out
        n = name(c)
        if n == "lambda":
            cl = find(c, "captureList")
            for cap in (find_all(cl, "capture") if cl is not None else []):
                pl = find(cap, "place")
                if pl is not None and tokens(cap)[0].text == "editable":
                    out.append(self.path_of_place(pl))
            return out
        if n == "assignStmt":
            out.append(self.path_of_place(find(c, "place")))
        elif n == "markerExpr":
            path = self.marked_path(c)
            if tokens(c)[0].text == "editable":
                out.append(path)
            for cp, cm in self.closures.get(root_of(path), {}).get("captures", []):
                if cm == "editable":
                    out.append(cp)
        return out + self.writes_in(c)

    def marked_path(self, m) -> str:
        """The place a marker loans: in `editable p.m(...)` the greedy place took the method name, so the loaned place is
        the receiver `p` (D-71: `editable xs.pop()` loans `xs`, not a place `xs.pop`)."""
        pl = find(m, "place")
        kids = children(m)
        members = [k for k in children(pl) if name(k) == "memberName"]
        if len(kids) > 2 and isinstance(kids[2], Token) and kids[2].text == "(" and members:
            return ".".join([direct_tokens(pl)[0].text] + [tokens(k)[0].text for k in members[:-1]])
        return self.path_of_place(pl)

    def deferred_effects(self, facts: Facts, blk):
        """A deferred body's effect on the facts where it runs (D-75): every place it assigns or loans exclusively loses
        its facts, except that a place written once in the body, by a top-level `place = <integer literal>` (or its
        negation) with no other write to it, a prefix of it or a field of it, holds the literal afterwards (the body has no
        exits, E-DEF-001)."""
        writes = [w for w in self.writes_in(blk) if w]
        for p in sorted(set(writes)):
            facts.invalidate(p)
        for st in [c for c in children(blk) if name(c) == "statement"]:
            inner = children(st)[0]
            if name(inner) != "assignStmt":
                continue
            ts = direct_tokens(inner)
            e = find(inner, "expr")
            path = self.path_of_place(find(inner, "place"))
            ets = tokens(e) if e is not None else []
            # the literal holds only if this is the body's one write to the place, a prefix of it or a field of it
            if not path or sum(1 for q in writes if iv.overlaps(q, path)) != 1 or not any(t.text == "=" for t in ts):
                continue
            lit = ets[1:] if len(ets) == 2 and ets[0].text == "-" else ets
            if len(lit) == 1 and lit[0].kind == "INT":
                i = self.interval_of(literal_type(lit[0]))
                if i is not None and lit is not ets:
                    i = iv.arith("-", (0, 0), i)
                if i is not None:
                    facts.ints[path] = i

    def cleanup_state(self, depth: int) -> Facts:
        """The state at an exit that leaves every block deeper than `depth`: their pending deferred bodies run (D-75)."""
        s = self.facts.copy()
        for d, blk in reversed(self.defer_bodies):
            if d > depth:
                self.deferred_effects(s, blk)
        return s

    def condition_states(self, cond, env: Env) -> Tuple[Facts, Facts, bool]:
        """Type a condition and return (true state, false state, exact): the condition's effects are applied once, from
        the state before it, and both states carry them (D-73)."""
        pre = self.facts.copy()
        self.condition(cond, env)
        self.facts = pre
        return self.narrow(cond, env)

    def narrow(self, cond, env: Env) -> Tuple[Facts, Facts, bool]:
        """(facts when true, facts when false, whether the false side is the exact negation)."""
        base = self.facts
        n = name(cond)
        kids = children(cond)
        if n in ("constExpr", "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr",
                 "shiftExpr", "additiveExpr", "multiplicativeExpr", "unaryExpr", "exprStmt") and len(kids) == 1:
            return self.narrow(kids[0], env)
        if n == "primaryExpr" and len(kids) == 3 and isinstance(kids[0], Token) and kids[0].text == "(":
            return self.narrow(kids[1], env)
        if n == "andExpr" and len(kids) > 1:
            # each conjunct is narrowed in the state where the earlier ones held, with its own effects; the expression is
            # false when some conjunct is false, so the false state joins each conjunct's false state (D-73)
            t = base.copy()
            falses = []
            for k in kids[::2]:
                self.facts = t
                t, f, _ = self.narrow(k, env)
                falses.append(f)
            self.facts = base
            return t, Facts.join_all(falses), False
        if n in ("equalityExpr", "relationalExpr") and len(kids) == 3:
            op = kids[1].text
            lp, rp = self.path_of_expr(kids[0]), self.path_of_expr(kids[2])
            lt = self.infer(kids[0], env)
            mark = len(iv.LOG)
            rt = self.infer(kids[2], env, expected=self.enum_operand(lt))   # `s == Error` (9.7, X-345)
            # D-74, D-77: the left operand's value was read before the right operand ran; if the right operand discarded the
            # left place's facts (a write, an editable loan), the comparison says nothing about the place's current value
            if lp is not None and any(iv.overlaps(q, lp) for q in iv.LOG[mark:]):
                lp = None
            cur = self.facts                       # the state after both operands' effects
            # null tests (6.8)
            if n == "equalityExpr":
                for p, t in ((lp, strip(lt)), (rp, strip(rt))):
                    other = rt if p == lp else lt
                    if p is not None and isinstance(t, OptionalT) and strip(other) == OptionalT(NEVER):
                        yes, no = cur.copy(), cur.copy()
                        (no if op == "==" else yes).present[p] = True
                        return yes, no, True
            # length facts (6.7): i < p.count(); the else branch of `i >= p.count()` gives `i < p`, and the else branch of
            # `i > p.count()` gives only `i <= p`, which does not permit indexing (D-70)
            if n == "relationalExpr" and lp is not None and rp is None and self.is_int(lt):
                cp = self.count_receiver(kids[2])
                if cp is not None:
                    yes, no = cur.copy(), cur.copy()
                    if op == "<":
                        yes.below[lp] = cp
                    elif op == ">=":
                        no.below[lp] = cp
                    return yes, no, True
            li, ri = self.interval_of(lt), self.interval_of(rt)
            if li is None or ri is None:
                return cur.copy(), cur.copy(), False
            yes, no = cur.copy(), cur.copy()
            nop = iv.NEGATE[op]
            if lp is not None:
                a = iv.narrow(op, li, ri)
                b = iv.narrow(nop, li, ri)
                if a is not None: yes.ints[lp] = a
                if b is not None: no.ints[lp] = b
            if rp is not None:
                fop = iv.FLIP[op]
                a = iv.narrow(fop, ri, li)
                b = iv.narrow(iv.NEGATE[fop], ri, li)
                if a is not None: yes.ints[rp] = a
                if b is not None: no.ints[rp] = b
            return yes, no, (lp is not None or rp is not None)
        # no narrowing form: the condition still runs, so both branches see its effects (D-73: `c.v == 1 && reset(editable
        # c)` must not keep `c.v == 1`); its diagnostics were reported when the condition was typed
        entry = self.facts.copy()
        before = len(self.records)
        self.infer(cond, env)
        del self.records[before:]
        cur = self.facts
        self.facts = entry
        return cur.copy(), cur.copy(), False

    def count_receiver(self, e) -> Optional[str]:
        """The collection place of `p.count()`."""
        pe = find(e, "postfixExpr")
        if pe is None or len(tokens(e)) != len(tokens(pe)):
            return None
        kids = children(pe)
        if len(kids) < 2:
            return None
        last = kids[-1]
        mc = find(last, "methodCall")
        if mc is None or tokens(find(mc, "memberName"))[0].text != "count" or find(mc, "argList") is not None:
            return None
        prefix = kids[0] if len(kids) == 2 else None
        if prefix is None:
            root = self.path_of_expr(kids[0])
            if root is None:
                return None
            for pf in kids[1:-1]:
                k = children(pf)
                if k and isinstance(k[0], Token) and k[0].text == "." and len(k) > 1 and name(k[1]) == "memberName":
                    root += "." + tokens(k[1])[0].text
                else:
                    return None
            return root
        return self.path_of_expr(prefix)

    # ---- ownership (7.2–7.6, 5.7) ----------------------------------------------------------
    def own_err(self, code: str, msg: str, tok: Token, place: str, conflict: str, rule: str = "LANG-07-04", region: str = ""):
        self.rec(code, msg, rule, tok, {"place": place, "conflict": conflict, "spans": place, "loan_region": region})

    def is_bearing(self, t: Type) -> bool:
        """Ownership-bearing for the marker rules: a known type outside the trivial set (6.11) that is not
        itself a loan or a view (a loan is duplicated as a loan; X-95); literals and unknowns are not."""
        if isinstance(t, LoanT):
            return False
        s = strip(t)
        if not known(s) or is_strlit(s) or s == INTERP:
            return False
        if isinstance(s, Named) and s.name in ("StringView", "BytesView", "ListView"):
            return False
        return not self.dc.is_trivial(s)

    @staticmethod
    def lent(t: Type) -> Optional[Type]:
        """What a loan lends, as the owned value a copy of it is: `alias T` gives `T` and `(alias T)?` gives `T?`; None
        for a type that is not a loan."""
        if isinstance(t, LoanT):
            return t.inner
        if isinstance(t, OptionalT):
            inner = Typer.lent(t.inner)
            return OptionalT(inner) if inner is not None else None
        if isinstance(t, TupleT):
            items = [Typer.lent(x) for x in t.items]
            return TupleT(tuple(x if l is None else l for x, l in zip(t.items, items))) if any(l is not None for l in items) else None
        if isinstance(t, RecordT):
            fs = [(k, Typer.lent(x)) for k, x in t.fields]
            return RecordT(tuple((k, x if l is None else l) for (k, x), (_, l) in zip(t.fields, fs))) if any(l is not None for _, l in fs) else None
        return None

    def lent_operand(self, node, env: Env) -> Optional[str]:
        """The origin of a `??` operand that is a place read through a loan, written bare: a read or `editable`
        parameter, an `alias` capture, a binding that holds a loan, or `self` in a method that borrows it."""
        path = self.path_of_expr(node)
        if path is None or find(node, "markerExpr") is not None:
            return None
        root = root_of(path)
        kind = self.binding_kind(path, env)
        if kind in ("param", "editable", "capture-alias") or root in self.own.holders:
            return root
        if root == "self" and kind == "self" and self.self_mode in ("self", "editable self"):
            return root
        return None

    def coalesce_result(self, inner: Type, right: Type, left_node, right_node, env: Env) -> Type:
        """The type of `a ?? b`: `a`'s present type, and a loan of it when the result can be a loan (D-341): `a`
        lends, the fallback is a loan (a nested lent `??`, at any depth), or either operand is a place read through
        a loan. An owned value is then made from it only by `copy`, wherever it meets an owned expectation."""
        if isinstance(inner, LoanT) or not self.is_bearing(inner):
            return inner
        if isinstance(right, LoanT):
            return LoanT(inner, right.mode, right.origin)
        origin = self.lent_operand(left_node, env) or self.lent_operand(right_node, env)
        return LoanT(inner, "alias", origin) if origin is not None else inner

    @staticmethod
    def source_text(node) -> str:
        """An expression as the source spells it, for a message: `move d`, `make(console, "x")`."""
        s = " ".join(t.text for t in tokens(node))
        s = re.sub(r"\s+([,)\].])", r"\1", s)
        s = re.sub(r"(\w)\s+([(\[])", r"\1\2", s)
        return re.sub(r"([(\[.])\s+", r"\1", s)

    def owned_into_lent(self, have: Type, ret: Optional[Type], e, env: Env):
        """A value of the function's own where the result is declared a loan (7.6; D-373, D-377, D-381). A `from`
        result is a loan, so the expression that produces it must lend: what does not would be held by the caller as
        a loan of what ends with the call, as a loan of a local or a temporary returned is: E-OWN-051."""
        if ret is None or self.lent(ret) is None:
            return
        what = self.unlent_in(e, ret, env, have)
        if what is None:
            return
        owned = self.lent(ret)
        self.own_err("E-OWN-051", f"the result is declared as a loan, but `{what}` is a value of the function's own, which ends with the call: return an owned value from a function returning `{owned.show()}` (bind the loan first, `let v = …`, then `return copy v ?? …`), or `match` the loan",
                     tokens(e)[0], what, "owned value in a lent result", "LANG-07-06")

    UNLENT_WRAP = ("constExpr", "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr",
                   "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr", "unaryExpr", "postfixExpr", "primaryExpr", "exprStmt")
    UNLENT_OPS = ("orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr")

    def unlent_in(self, e, want: Type, env: Env, t: Optional[Type] = None) -> Optional[str]:
        """The first part of `e` that is a value of the function's own where `want`, a result declared `from` a
        parameter, is a loan, as the source spells it; None when the expression lends (7.6; D-381). What lends: a
        place read through a loan (a parameter, `self`, a binding that holds a loan, a field or element path through
        one), `alias p`, a call whose result lends, a `??` both of whose sides lend, an `if` or `match` every one of
        whose results lends, an outcome expression whose value side and every `recover` lend (D-385), `present` of
        one (an optional holds nothing of its own), parentheses, and a string literal without interpolation where a
        view is declared, a view of static storage (6.6). What does not: an interpolated string, a literal where a
        `String` loan is declared (it converts by allocating; D-386), a tuple or record literal, a constructor,
        `move x`, `copy x`, a call whose result is owned, a lambda, an arithmetic or string result. A trivial value
        holds nothing and is exempt. `t` is `e`'s type where the caller has it; otherwise the type recorded while `e`
        was inferred says."""
        w = want
        while isinstance(w, OptionalT):
            w = w.inner
        if not isinstance(w, LoanT):
            return None                                    # an owned result: a loan there is loan_into_owned's
        n = e
        while True:
            nm, kids = name(n), children(n)
            if nm in self.UNLENT_WRAP and len(kids) == 1 and not isinstance(kids[0], Token):
                n = kids[0]
            elif nm == "primaryExpr" and len(kids) == 3 and isinstance(kids[0], Token) and kids[0].text == "(":
                n = kids[1]
            else:
                break
        nm, kids = name(n), children(n)
        if nm == "outcomeExpr" and len(kids) > 1:
            # an outcome expression produces its value side, or what its handler recovers, so both must lend; `or
            # fail`, and a handler that fails or returns, produce nothing (10.2; D-385)
            hit = self.unlent_in(kids[0], want, env)
            if hit is not None:
                return hit
            words = [k.text for k in kids if isinstance(k, Token)]
            if "recover" in words:
                return self.unlent_in(kids[-1], want, env)
            if "catch" in words:
                return self.unlent_recovered(next(k for k in kids if name(k) == "block"), want, env)
            return None
        if nm == "coalesceExpr" and len(kids) == 3:
            return self.unlent_in(kids[0], want, env) or self.unlent_in(kids[2], want, env)
        if nm == "primaryExpr" and len(kids) == 4 and isinstance(kids[0], Token) and kids[0].text == "present":
            return self.unlent_in(kids[2], want, env)
        if nm == "matchExpr":
            for arm in (c for c in kids if name(c) == "matchArm"):
                body = [c for c in children(arm) if name(c) in ("expr", "block")][-1]
                hit = self.unlent_in(body, want, env) if name(body) == "expr" else self.unlent_given(body, want, env)
                if hit is not None:
                    return hit
            return None
        if nm == "ifExpr":
            for c in kids:
                if isinstance(c, Token) or name(c) == "expr":
                    continue                               # a condition decides nothing
                hit = self.unlent_given(c, want, env) if name(c) == "block" else self.unlent_in(c, want, env)
                if hit is not None:
                    return hit
            return None
        if t is None:
            t = self.recorded_type(e)
        s = strip(t) if t is not None else None
        if isinstance(s, Named) and s.name == "Outcome" and s.args:
            t = s.args[0]                                  # a `try` call produces its success value
        if nm == "literal" and isinstance(kids[0], Token) and kids[0].kind in ("STR", "RAW", "BYTES"):
            # text: a literal without interpolation is a view of static storage, which lends where a view is
            # declared (6.6); an interpolation allocates a fresh String, and a literal converts to one where a
            # `String` loan is declared: the function's own (D-386)
            core = want
            while isinstance(core, (OptionalT, LoanT)):
                core = core.inner
            core = strip(core)
            # "without interpolation" is decided by the lexer's recorded holes, as `lower_names` decides it: an escaped
            # `\${` opens no hole and prints literally on every backend (D-412)
            static = kids[0].kind != "STR" or not hole_sources(kids[0])
            if static and isinstance(core, Named) and core.name in ("StringView", "BytesView"):
                return None
            return self.source_text(n)
        if nm in self.UNLENT_OPS and len(kids) > 1:
            # computed here: a trivial result holds nothing, any other is the function's own
            return None if t is None or self.dc.is_trivial(strip(t)) else self.source_text(n)
        if t is not None and not self.is_bearing(self.lent(t) or t):
            return None                                    # trivial, a literal or unknown: it holds nothing
        if nm in ("tupleLit", "recordLit", "lambda"):
            return self.source_text(n)                     # constructed here, whatever its parts
        if t is None or self.lent(t) is not None or self.lent_operand(n, env) is not None:
            return None
        return self.source_text(n)

    def unlent_recovered(self, node, want: Type, env: Env) -> Optional[str]:
        """`unlent_in` over every `recover` of the handler `node`, at any depth of its statements: not one of a
        nested `catch`, whose handler recovers its own value, nor one inside a lambda (10.2; D-385)."""
        for c in children(node):
            if isinstance(c, Token):
                continue
            kids = children(c)
            if name(c) == "statement" and kids and isinstance(kids[0], Token) and kids[0].text == "recover":
                hit = self.unlent_in(next(k for k in kids if name(k) == "expr"), want, env)
            elif name(c) == "lambda" or (name(c) == "outcomeExpr" and any(isinstance(k, Token) and k.text == "catch" for k in kids)):
                hit = None
            else:
                hit = self.unlent_recovered(c, want, env)
            if hit is not None:
                return hit
        return None

    def unlent_given(self, node, want: Type, env: Env) -> Optional[str]:
        """`unlent_in` over every `give` that ends the arm or branch `node` is in, not one nested in an inner
        `if` or `match` expression (5.5)."""
        for c in children(node):
            if isinstance(c, Token):
                continue
            kids = children(c)
            if name(c) == "statement" and kids and isinstance(kids[0], Token) and kids[0].text == "give":
                hit = self.unlent_in(next(k for k in kids if name(k) == "expr"), want, env)
            elif name(c) not in ("matchExpr", "ifExpr", "lambda"):
                hit = self.unlent_given(c, want, env)
            else:
                hit = None
            if hit is not None:
                return hit
        return None

    def lends_result(self, ret: Optional[Type]) -> bool:
        """Start recording the types of the expressions inferred next when the result they produce is declared a
        loan, for `unlent_in`; True when this call started it, so its caller stops it. A recording already running
        (a lowering's, or an enclosing result's) is kept."""
        if self.node_types is not None or ret is None or self.lent(ret) is None:
            return False
        self.node_types = {}
        return True

    def recorded_type(self, e) -> Optional[Type]:
        """The type recorded for `e` while it was inferred, or for the one expression it wraps; None when none was."""
        if self.node_types is None:
            return None
        n = e
        while True:
            t = self.node_types.get(id(n))
            if t is not None:
                return t
            kids = children(n)
            if len(kids) != 1 or isinstance(kids[0], Token):
                return None
            n = kids[0]

    def loan_into_owned(self, have: Type, want: Optional[Type], tok: Token, what: str):
        """A loan where an owned value is expected (a return, an annotated binding, an assignment, an `owned` argument
        or field, a payload, a recovered value) stays a loan (X-95): an owned value is made from it
        only by `copy` (7.5, 7.6; D-341). A trivial value needs no marker. Without one, a copyable value is E-OWN-001
        (write `copy`); a value that is not Copyable has no owned copy at all, E-OWN-002. `want` is the expected type,
        or None where the position itself is owned; a loan or a view expected is lent on."""
        lent = self.lent(have)
        if lent is None:
            return
        w = want
        while isinstance(w, OptionalT):
            w = w.inner
        if isinstance(w, (LoanT, Unknown)) or (isinstance(w, Named) and w.name in ("StringView", "BytesView", "ListView", "ListCursor")):
            return
        core = lent
        while isinstance(core, OptionalT):
            core = core.inner
        if isinstance(core, Unknown) or self.dc.is_trivial(lent) or (tok.line, tok.col) in self.copy_refused:
            return
        if isinstance(core, Named) and core.name in ("StringView", "BytesView", "ListView", "ListCursor"):
            return                                    # a view held whole is a stored loan (E-OWN-053, E-OWN-090), not a copy
        if self.dc.is_copyable(lent):
            self.own_err("E-OWN-001", f"{what} must be owned, but it is a loan of {lent.show()}: write `copy` to make an owned copy (a call's result is bound first, `let v = …`, then `copy v`)",
                         tok, tok.text, "copy required", "LANG-07-01")
            return
        dup = " (a `Shared` or `Weak` is duplicated with `retain()`)" if isinstance(core, Named) and core.name in ("Shared", "Weak") else ""
        self.own_err("E-OWN-002", f"{what} must be owned, but it is a loan of {lent.show()}, which is not Copyable (6.11), so no owned copy can be made{dup}: keep the loan (a read parameter or a `from` result), or take the value out of its owner",
                     tok, tok.text, "loan where an owned value is required", "LANG-07-01")

    def binding_kind(self, path: str, env: Env) -> Optional[str]:
        b = env.lookup(root_of(path))
        return b.kind if b else ("self" if root_of(path) == "self" else None)

    # ---- markers the declaration already shows (X-345: R1, R2, R16) ------------------------------
    def editable_root(self, path: str, env: Env) -> bool:
        """Whether `editable p` may be written: `p` rooted in a `let mut` binding, an `editable` parameter, an
        `editable` or `move` capture, a binding that holds an exclusive loan, or the method's own `editable self`."""
        kind = self.binding_kind(path, env)
        holder = self.own.holders.get(root_of(path))
        return kind in ("mut", "editable", "capture-editable", "capture-move") or (holder is not None and holder.mode == "editable") \
            or (root_of(path) == "self" and self.self_mode == "editable self")

    def unmarked_editable_root(self, path: str, env: Env) -> bool:
        """The receiver of an `editable self` method, or a `fn mut` closure being called, needs no `editable` wherever
        `editable` could be written (7.5, X-345), whatever the receiver's type, and the call is then checked exactly as
        `editable p…`. On any other root it is rejected as the marked form is (D-303)."""
        return self.editable_root(path, env)

    def receiver_mode(self, recv: Type, m: str) -> Optional[str]:
        """The receiver mode of method `m` on `recv`, found as method_call_named finds it, recording nothing."""
        s = concrete(strip(recv))
        if isinstance(s, Refined):
            s = s.base
        if isinstance(s, Named) and s.name == "Shared" and s.args and m not in ("retain", "downgrade"):
            s = strip(s.args[0])
        if isinstance(s, OptionalT) or not known(s):
            return None
        d = self.dc.resolve_type(s.iface) if isinstance(s, DynamicT) else self.decl_of(s)
        if d is None:
            return None
        sig = d.methods.get(m)
        if sig is None and d.kind == "mixin":
            sig = d.methods.get("requires:" + m)
        if sig is None:
            for mx in d.mixins:
                md = self.dc.resolve_type(mx)
                if md is not None and m in md.methods:
                    sig = md.methods[m]
            for iface in d.implements:
                idecl = self.dc.resolve_type(iface)
                if sig is None and idecl is not None and m in idecl.methods:
                    sig = idecl.methods[m]
        return sig.receiver if sig is not None else None

    def unmarked_editable(self, path: str, tok: Token, env: Env):
        """An unmarked receiver or callee the declaration shows to be editable: the steps of `editable p` (marker)."""
        self.use_place(path, tok, "editable", env)
        self.facts.invalidate(path)
        self.receiver = (path, "editable")

    def returned_local(self, e, env: Env) -> Optional[str]:
        """`return x` moves `x` when `x` is a local `let` binding of the function: its use visibly ends there
        (7.5, X-345). A field, a parameter, a capture or any other expression keeps the markers of 7.5."""
        if e is None or find(e, "markerExpr") is not None:
            return None
        path = self.path_of_expr(e)
        if path is None or not path.isidentifier():
            return None
        b = env.lookup(path)
        if b is None or b.kind not in ("let", "mut") or b.depth <= 0:
            return None
        return path

    def set_last_status(self, status: str):
        """The status of the record just made (it was deduplicated away if the last record is another)."""
        if self.dc.records and self.dc.records[-1]["code"].startswith("E-EFF"):
            self.dc.records[-1]["status"] = status

    def eff_err(self, code: str, msg: str, tok: Token, required: str, effect: str, witness: str, rule: str = "LANG-11-01"):
        self.rec(code, msg, rule, tok, {"required_contract": required, "violating_effect": effect, "witness_path": witness})

    def provenance(self, path: str, env: Env) -> str:
        """fresh | footprint:<param> | mutate | none, for a write through the place (11.3)."""
        root = root_of(path)
        if root == "self":
            return "footprint:self" if self.self_mode == "editable self" else "none"   # a write through read self is E-NAM-002 already
        h = self.own.holders.get(root)
        if h is not None and root_of(h.owner) != root:
            return self.provenance(h.owner, env)
        b = env.lookup(root)
        if b is None:
            return "none"
        if b.kind == "editable":
            return f"footprint:{root}"
        if b.kind.startswith("capture-"):
            return "mutate" if b.kind in ("capture-editable",) else "fresh"
        if root in self.fresh or b.kind in ("mut", "let", "owned", "for", "pattern", "pattern-loan"):
            return "fresh"
        return "mutate"

    def note_write(self, path: str, tok: Token, env: Env):
        prov = self.provenance(path, env)
        if prov.startswith("footprint:"):
            self.eff.footprint.add(prov.split(":", 1)[1])
        elif prov == "mutate":
            self.eff.add("mutate", "proved", tok)

    def import_call(self, callee: Summary, tok: Token, receiver_path: Optional[str], arg_places: List[Tuple[str, str]], env: Env, is_async_callee: bool, callback_types: List[Type]):
        """Import a callee's summary: footprint parameters instantiated with the argument places (11.3)."""
        # `fail` is imported only where the outcome leaves the function unhandled: `or fail` and `fail f` add it
        # themselves (11.3); an async callee's suspension is imported by the await of its future
        skip = {"suspend", "fail"} if is_async_callee else {"fail"}
        self.eff.imports(callee, tok, skip)
        for fp in callee.footprint:
            place = receiver_path if fp == "self" else dict(arg_places).get(fp)
            if place is not None:
                self.note_write(place, tok, env)
        # callback arguments add nothing here: the callee's summary already counts what each callback
        # parameter's type permits (11.3), a superset of any lambda assignable to it

    def function_label(self, d: Decl) -> str:
        """The summary label of a call of function `d`: `function f` for the module's own function, which a unit of the
        module may have summarised; qualified by its module for another module's, whose summary its modifiers give
        (D-37: the labels of two modules never meet)."""
        if d.module == self.module or standard(d.module) or d.kind == "unknown":
            return f"function {d.name}"
        return f"function {d.module}::{d.name}"

    def method_label(self, d: Decl, m: str) -> str:
        """As `function_label`, for method `m` of type `d`."""
        if d.module == self.module or standard(d.module) or d.kind == "unknown":
            return f"method {d.name}.{m}"
        return f"method {d.module}::{d.name}.{m}"

    def callee_summary(self, d: Optional[Decl], sig: Sig, label: str) -> Summary:
        if label == "closure":
            return self.summaries["closure"]          # a callback contributes what its type permits (11.3)
        if sig.body is None or "trusted" in sig.modifiers:
            # the declaration the effect data names the operation by: the owning type of a method or associated
            # function, else the free function itself (an alias call names it; otherwise its label does)
            fd = d if d is not None and d.kind == "func" else None
            owner = d if d is not None and d.kind != "func" else None
            if fd is None and owner is None and label.startswith("function "):
                fd = self.table.get(label[len("function "):])
            home = owner if owner is not None else fd
            auth = owner is not None and (self.dc.implements_core_authority(owner) or (owner.name in fx.AUTHORITIES and owner.module.startswith("fortis")))
            s = fx.library_summary(home.module if home is not None else None, owner.name if owner is not None else None, sig.name, sig.modifiers, auth,
                                   sig.receiver, sorted(self.authority_params(sig)))
            # a foreign `trusted` function of the program (not the standard library, whose summaries are tabled):
            # what it does is behind a trust boundary except for the claims its signature writes (X-132)
            def user(x):
                return not standard(x.module) and x.kind != "unknown"
            if "trusted" not in sig.modifiers:
                foreign = False
            elif d is not None:
                foreign = user(d)
            else:
                f = self.table.get(label[len("function "):].rsplit("::", 1)[-1]) if label.startswith("function ") else None
                foreign = f is not None and f.kind == "func" and user(f)
            if foreign:
                if "pure" not in sig.modifiers:
                    for e in ("io", "mutate", "nondet"):
                        s.add_uncertain(e, "trusted", "unknown")
                if "noalloc" not in sig.modifiers:
                    s.add_uncertain("alloc", "trusted", "unknown")
            return s
        return self.summaries.get(label, self.summary_from_modifiers(sig))

    def summary_from_modifiers(self, sig: Sig) -> Summary:
        """Before a body has been analysed (or across a module boundary) only its written claims count (11.3)."""
        s = Summary()
        mods = set(sig.modifiers)
        if "try" in mods: s.add("fail")
        if "async" in mods: s.add("suspend")
        if "recovers" in mods: s.add("recovers")
        # a function value's effects are those its type permits: possible, not shown to happen (11.3; X-132)
        if "pure" not in mods:
            for e in ("io", "mutate", "nondet"):
                s.add_uncertain(e, "proved", "possible")
        if "noalloc" not in mods:
            s.add_uncertain("alloc", "proved", "possible")
        return s

    def expanded_mods(self, sig: Sig) -> Set[str]:
        """The written modifiers plus those of every named contract (11.5)."""
        mods = set(sig.modifiers)
        for c in sig.contracts:
            d = self.table.get(c)
            if d is not None and d.kind == "contract":
                mods |= d.items.get("modifiers", set())
        return mods

    @staticmethod
    def unshared(t: Type) -> Type:
        """A `Shared[A]` handle carries A's authority (11.6, X-137)."""
        s = strip(t)
        return strip(s.args[0]) if isinstance(s, Named) and s.name == "Shared" and len(s.args) == 1 else s

    def is_authority(self, t: Type) -> bool:
        """An Authority type (11.6), `Shared[A]` included, recognised by its declaration: a program's own class named
        like a capability is not one (review of D-145); a type whose declaration is unknown keeps the name rule."""
        s = self.unshared(t)
        if not isinstance(s, Named):
            return False
        d = self.decl_of(s)
        if d is not None and d.kind != "unknown":
            return self.dc.implements_core_authority(d)
        return s.name in fx.AUTHORITIES

    def authority_params(self, sig: Sig) -> Set[str]:
        out = set()
        for pn, pt, pm in sig.params:
            s = self.unshared(pt)
            if isinstance(s, Named) and self.is_authority(s):
                out.add(s.name)
        return out

    def receiver_authorities(self, t: Type) -> Set[str]:
        """The authorities a method's receiver of type `t` names (11.6; D-173): the receiver's own type when it is an
        authority, else each field of its class whose type is an authority or a `Shared` of one, as X-137's owners hold
        a `Shared[Console]`. A field of any other type names none: a `Capabilities` record names none, as a
        `Capabilities` parameter names none."""
        s = self.unshared(t)
        if not isinstance(s, Named):
            return set()
        if self.is_authority(s):
            return {s.name}
        d = self.decl_of(s)
        if d is None or d.kind != "class":
            return set()
        return {self.unshared(ft).name for ft, _, _ in d.fields.values() if self.is_authority(ft)}

    def is_entry_main(self, label: Optional[str] = None) -> bool:
        """The body being typed (or `label`) is the entry `main` (4.6): the function `main` of the entry module."""
        return (label if label is not None else self.current_label) == "function main" and self.entry_module is not None and self.module == self.entry_module

    def is_entry_main_decl(self, d: Optional[Decl]) -> bool:
        return d is not None and d.kind == "func" and d.name == "main" and self.entry_module is not None and d.module == self.entry_module

    def entry_main_ref(self, d: Optional[Decl], tok: Token):
        """The entry `main` is never referenced from Fortis code: only the runtime calls it, once, so the capabilities
        it creates are created once (11.6, LANG-11-07)."""
        if self.is_entry_main_decl(d):
            self.eff_err("E-EFF-010", "the entry `main` is referenced from Fortis code: only the runtime calls it, once, so the capabilities it creates are created once (11.6)",
                         tok, "no reference to the entry main", "io", self.current_label or "a declaration", "LANG-11-07")

    @staticmethod
    def is_host(d: Optional[Decl]) -> bool:
        """`Host` of `fortis.host`, not a program's own class of that name."""
        return d is not None and d.name == "Host" and d.kind == "class" and d.module == "fortis.host"

    def capabilities_use(self, tok: Token, called: bool):
        """`Host.capabilities` (11.6, LANG-11-07): only a direct call in the entry `main`'s own body, at most once, outside
        every loop (its condition included), lambda and aspect hook; never taken as a value."""
        where = self.current_label or "a declaration"
        if not called:
            self.eff_err("E-EFF-010", "`Host.capabilities` is taken as a value: the capabilities are created only by a direct call in the entry `main` (11.6)",
                         tok, "a direct call in the entry main", "io", where, "LANG-11-07")
            return
        if not self.is_entry_main():
            self.eff_err("E-EFF-010", "`Host.capabilities()` is called outside the entry `main` (11.6)", tok, "the entry main only", "io", where, "LANG-11-07")
            return
        md = self.table.get("main")
        if md is not None and md.sig is not None and any(isinstance(pt, Named) and pt.name in CAPABILITY_TYPES for _, pt, _ in md.sig.params):
            self.eff_err("E-EFF-010", "`Host.capabilities()` is called in a `main` that takes capability parameters: the runtime has already created the capabilities, once (4.6, 11.6)",
                         tok, "no Host.capabilities() beside capability parameters", "io", where, "LANG-11-07")
            return
        inside ="a lambda" if "lambda" in self.ctx else "an aspect's hook" if "hook" in self.ctx else "a loop" if ("loop" in self.ctx or "whilecond" in self.ctx) else None
        if inside is not None:
            self.eff_err("E-EFF-010", f"`Host.capabilities()` is called inside {inside}: the capabilities are created once, by a direct call in the entry `main`'s own body (11.6)",
                         tok, "a direct call in the entry main", "io", where, "LANG-11-07")
            return
        self.caps_calls += 1

    def check_contracts(self, sig: Sig, head_tok: Token, label: str):
        """Each named contract subtracts (11.5): forbidden effects, required and excluded authorities, named by origin."""
        e = self.eff
        params = {base_name(self.unshared(pt)) for _, pt, _ in sig.params}
        for c in sig.contracts:
            d = self.table.get(c)
            if d is None or d.kind != "contract":
                self.rec("E-NAM-001", f"`{c}` is not a declared contract", "LANG-11-06", head_tok, {"name": c, "candidates": sorted(k for k, v in self.table.decls.items() if v.kind == "contract")})
                continue
            for f in sorted(d.items.get("forbids", ())):
                if f in e.effects:
                    self.eff_err("E-EFF-001", f"{label} carries contract {c}, which forbids `{f}`, but its body has that effect", e.sites.get(f, head_tok), c, f, label, "LANG-11-06")
            for a in sorted(d.items.get("requires", ())):
                if a not in params:
                    self.eff_err("E-EFF-001", f"{label} carries contract {c}, which requires capability {a}, but takes no parameter of that type", head_tok, c, "io", label, "LANG-11-06")
            for a in sorted(d.items.get("excludes", ())):
                if a in params:
                    self.eff_err("E-EFF-001", f"{label} carries contract {c}, which excludes capability {a}, but takes a parameter of that type", head_tok, c, "io", label, "LANG-11-06")
                elif a in e.authorities:
                    self.eff_err("E-EFF-001", f"{label} carries contract {c}, which excludes capability {a}, but calls a function that requires it", head_tok, c, "io", label, "LANG-11-06")

    def check_claims(self, sig: Sig, head_tok: Token, label: str):
        """The body's summary against the modifiers (11.2): E-EFF-001; `block` in an async function: E-EFF-020."""
        mods = self.expanded_mods(sig)
        e = self.eff
        self.check_contracts(sig, head_tok, label)
        # the authority must be in this function's own signature (11.6): one a callee takes, reached through a value that
        # is not an Authority (`caps.clock` of a Capabilities), does not count; a lambda acts under its enclosing
        # function's (D-144)
        own = e.authorities if label == "lambda" else self.authority_params(sig)
        if label != "lambda" and sig.receiver is not None and self.self_type is not None:
            own = own | self.receiver_authorities(self.self_type)
        if e.needs_auth and not own and not self.is_entry_main(label):
            self.eff_err("E-EFF-010", f"{label} performs I/O but takes no Authority value: a capability or per-activation authority must be a parameter (11.6)", e.sites.get("io", head_tok), "an Authority parameter", "io", label, "LANG-11-07")
        if self.is_entry_main(label) and self.caps_calls > 1:
            self.eff_err("E-EFF-010", "`Host.capabilities()` is called more than once in main: the capabilities are created once (11.6)", head_tok, "one Host.capabilities() call", "io", label, "LANG-11-07")
        if "pure" in mods and label != "lambda":
            # a `pure` function may not take a capability parameter at all (11.5): the claim rules out the I/O it is for (D-145)
            for a in sorted(self.authority_params(sig)):
                self.eff_err("E-EFF-001", f"{label} is declared `pure` but takes the capability {a}: a pure function receives no authority (11.5)", head_tok, "pure", "io", label, "LANG-11-06")
        # the record's status says how certain the effect is (PROG-DIA-02, X-132)
        why = {"unproved": ": a callback's type permits it, so it may happen (write the claim in the function type)",
               "unknown": ": it comes through a foreign `trusted` function that does not claim its absence, so it is behind a trust boundary"}
        for effect in fx.PURE_FORBIDS:
            if "pure" in mods and effect in e.effects:
                st = e.status(effect)
                verb = "has" if st == "violated" else "may have"
                self.eff_err("E-EFF-001", f"{label} is declared `pure` but its body {verb} the `{effect}` effect{why.get(st, '')}", e.sites.get(effect, head_tok), "pure", effect, label, "LANG-11-02" if effect == "mutate" else "LANG-11-01")
                self.set_last_status(st)
        if "noalloc" in mods and "alloc" in e.effects:
            st = e.status("alloc")
            verb = "allocates" if st == "violated" else "may allocate"
            self.eff_err("E-EFF-001", f"{label} is declared `noalloc` but its body {verb}{why.get(st, '')}", e.sites.get("alloc", head_tok), "noalloc", "alloc", label, "LANG-11-03")
            self.set_last_status(st)
        if "suspend" in e.effects and "async" not in mods:
            self.eff_err("E-EFF-001", f"{label} suspends (`await`) but is not declared `async`", e.sites.get("suspend", head_tok), "async", "suspend", label, "LANG-11-05")
        if "recovers" in e.effects and "recovers" not in mods and label != "lambda":
            self.eff_err("E-EFF-001", f"{label} discharges a failure without reporting it but is not declared `recovers`", e.sites.get("recovers", head_tok), "recovers", "recovers", label, "LANG-10-05")
        if "block" in e.effects and "async" in mods:
            self.eff_err("E-EFF-020", f"{label} is `async` but its body may block the executor's thread", e.sites.get("block", head_tok), "async", "block", label, "LANG-11-01")

    # ---- aspects (11.8) ---------------------------------------------------------------
    def asp_err(self, msg: str, tok: Token, expected: str, actual: str, span: str):
        self.typ("E-TYP-150", msg, tok, expected, actual, span, "LANG-11-09")

    def aspect_args(self, al, env: Env) -> list:
        """[(label or None, expr node, is_rest, env)] of an application or specialisation argument list."""
        out = []
        if al is None:
            return out
        for a in [c for c in children(al) if name(c) == "arg"]:
            ats = direct_tokens(a)
            e = find(a, "expr")
            if ats and ats[0].text == "...":
                out.append((ats[1].text, None, True, env))
            elif len(ats) > 1 and ats[1].text == ":":
                out.append((ats[0].text, e, False, env))
            else:
                out.append((None, e, False, env))
        return out

    def resolve_aspect(self, d: Decl, args: list, tok: Token, seen: Set[str]):
        """Bind an application's arguments to an aspect's parameters, following specialisations to the base
        aspect; `...rest` forwards the remaining named arguments unchanged (11.8)."""
        a = d.aspect
        params = a["params"]
        names_ = [p[0] for p in params]
        bound: Dict[str, tuple] = {}
        extra = []
        positional = False
        for lbl, e, rest, env in args:
            if rest:
                self.asp_err(f"`...{lbl}` forwards arguments only inside a specialisation `aspect x(...) = base(...)`", tok, "named arguments", "...rest", d.name)
                continue
            if lbl is None:
                if positional or not params:
                    self.asp_err(f"@{d.name}: only the first declared parameter may be passed positionally", tokens(e)[0], f"a named argument of {names_}", "positional", d.name)
                    continue
                bound[names_[0]] = (e, env)
                positional = True
            elif lbl in names_:
                bound[lbl] = (e, env)
            elif a["rest"]:
                extra.append((lbl, e, False, env))
            else:
                self.asp_err(f"@{d.name} has no parameter `{lbl}`", tokens(e)[0] if e is not None else tok, f"one of {names_}", lbl, d.name)
        for pn, pt, has_default in params:
            if pn not in bound and not has_default:
                self.asp_err(f"@{d.name}: argument `{pn}` is required", tok, pn, "absent", d.name)
        level = Env()
        for pn, pt, _ in params:
            level.set(pn, pt, kind="let")
        if a["base"] is None:
            if extra:
                self.asp_err(f"@{d.name} has no parameter `{extra[0][0]}`", tok, f"one of {names_}", extra[0][0], d.name)
            return d, bound
        bname, bal, btok = a["base"]
        bd = a.get("base_decl") or self.table.get(bname)
        if bd is None or bd.kind != "aspect" or bname in seen:
            self.asp_err(f"aspect {d.name} specialises `{bname}`, which is not a declared aspect", btok, "an aspect", bname, d.name)
            return None, {}
        fargs = []
        for lbl, e, rest, env in self.aspect_args(bal, level):
            if rest:
                fargs.extend(extra)
            else:
                fargs.append((lbl, e, False, level))
        # the fixed arguments may name this level's parameters: type them with the application's values
        self.level_values = getattr(self, "level_values", [])
        self.level_values.append(bound)
        base, bbound = self.resolve_aspect(bd, fargs, tok, seen | {d.name})
        self.level_values.pop()
        return base, bbound

    def type_aspect_arg(self, pn: str, pt: Type, e, env: Env, aspect: str):
        if e is None:
            return
        ts = tokens(e)
        if isinstance(strip(pt), Named) and strip(pt).name == "ObservationSchema":
            d = self.table.get(ts[0].text) if len(ts) == 1 and ts[0].kind == "UPPER" else None
            if d is not None and d.kind == "unknown":
                self.schema_unknown = True              # a schema from a package outside the corpus: frame checks suspended
                return
            if d is None or d.kind != "observation":
                self.asp_err(f"@{aspect}: `{pn}` names an observation schema; `{' '.join(t.text for t in ts)}` is not one", ts[0], "an observation declaration", ts[0].text, pn)
            else:
                self.schema = d
            return
        if len(ts) == 1 and ts[0].kind == "LOWER" and env.lookup(ts[0].text) is not None:
            return                                       # a forwarded parameter of a specialisation
        if len(ts) == 1 and ts[0].kind == "UPPER" and self.table.get(ts[0].text) is not None:
            return                                       # a declaration name (a schema, a variant constructor, an enum)
        call = self.nonconstant_call(e)
        if call is not None:
            self.asp_err(f"@{aspect}: the argument `{pn}` makes a call (`{call.text}`); aspect arguments are compile-time constants, evaluated by no code (11.8)", call, "a compile-time constant", "a call", pn)
            return
        self.last_closure = None
        t = self.infer(e, env, expected=pt)
        if self.last_closure is not None and self.last_closure.get("captures"):
            self.asp_err(f"@{aspect}: the `fn` argument `{pn}` captures; aspect arguments are compile-time constants", ts[0], "a capture-free fn literal", "a closure with captures", pn)
        s = strip(pt)
        if known(t) and isinstance(s, Named) and not s.args and s.name not in ("VariantMap",) and not isinstance(pt, (OptionalT, FnT, DynamicT)):
            self.expect(t, pt, ts[0], f"aspect argument `{pn}`")

    def hook(self, blk, env: Env, ret: Type, in_try: bool, what: str, owned: Optional[str] = None, tok: Optional[Token] = None) -> Summary:
        """Type one hook block in the elaborated outer function: its own ownership state, the function's summary."""
        saved = (self.own, self.facts, self.ret_type, self.in_try, self.ctx, self.depth, self.eff)
        self.own, self.facts, self.ret_type, self.in_try, self.ctx, self.depth = Owners(), Facts(), ret, in_try, ["function", "hook"], 0
        self.eff = Summary()
        if owned is not None:
            self.own.obligations[owned] = (tok or tokens(blk)[0], 0)
        self.walk_block(blk, env)
        if ret != VOID and not self.terminates(blk):
            self.typ("E-TYP-110", f"the {what} must end every path in `return` or `fail`", tokens(blk)[0], ret.show(), "fall-through", what, "LANG-11-09")
        elif owned is not None and not self.terminates(blk):
            self.report_open(0, f"at the end of the {what}")
        out = self.eff
        self.own, self.facts, self.ret_type, self.in_try, self.ctx, self.depth, self.eff = saved
        self.eff.imports(out)
        for k, v in out.sites.items():
            self.eff.sites.setdefault(k, v)
        self.eff.footprint |= out.footprint
        return out

    def apply_aspects(self, sig: Sig, head_tok: Token, label: str):
        uses = self.current_aspects
        if not uses:
            return
        target_try = "try" in self.expanded_mods(sig)
        chain = []
        for use in uses:
            uts = direct_tokens(use)
            nm = uts[1]
            d = self.table.get(nm.text)
            if d is None or d.kind != "aspect":
                self.asp_err(f"`@{nm.text}` is not a declared aspect", nm, "an aspect", nm.text, nm.text)
                continue
            base, bound = self.resolve_aspect(d, self.aspect_args(find(use, "argList"), Env()), nm, set())
            if base is None:
                continue
            for pn, pt, _ in base.aspect["params"]:
                if pn in bound:
                    self.type_aspect_arg(pn, pt, bound[pn][0], bound[pn][1], nm.text)
            a = base.aspect
            for n2, ty, mode in a["needs"]:
                matches = [(pn, pm) for pn, pt, pm in sig.params if base_name(strip(pt)) == base_name(ty)]
                if len(matches) == 0:
                    self.asp_err(f"@{nm.text} needs a parameter of type {ty.show()} ({n2}); {label} has none", nm, ty.show(), "no such parameter", n2)
                elif len(matches) > 1:
                    self.asp_err(f"@{nm.text} needs exactly one parameter of type {ty.show()}; {label} has {' and '.join(m[0] for m in matches)}", nm, f"one {ty.show()} parameter", ", ".join(m[0] for m in matches), n2)
                elif mode == "editable" and matches[0][1].lstrip(".") != "editable":
                    self.asp_err(f"@{nm.text} needs `{n2}: editable {ty.show()}`; parameter `{matches[0][0]}` is not editable", nm, f"editable {ty.show()}", matches[0][1] or "read loan", n2)
            for c in a["needs_contracts"]:
                if c not in sig.contracts:
                    self.asp_err(f"@{nm.text} needs contract {c} on its target; {label} does not carry it", nm, c, "absent", c)
            chain.append((base, bound, nm))
        if not chain:
            return
        def hook_env(base, bound):
            e = self.params_env(sig, Env())
            for pn, pt, _ in base.aspect["params"]:
                e.set(pn, self.subst(pt, {"R": sig.ret}), kind="let")
            for n2, ty, mode in base.aspect["needs"]:
                m = next(((pn, pt, pm) for pn, pt, pm in sig.params if base_name(strip(pt)) == base_name(ty)), None)
                if m is not None:
                    e.set(n2, m[1], mutable=False, kind={"editable": "editable", "owned": "owned"}.get(m[2].lstrip("."), "param"))
            return e
        handler_above = False
        before_names, after_names, handler_names = [], [], []
        for base, bound, nm in chain:
            a = base.aspect
            if a["on_failure"] is not None:
                handler_above = True
            for blk in a["before"]:
                self.hook(blk, hook_env(base, bound), VOID, target_try or handler_above, f"before hook of @{nm.text}")
                before_names.append(nm.text)
        handler_at_or_above = [any(c[0].aspect["on_failure"] is not None for c in chain[:i + 1]) for i in range(len(chain))]
        for i in range(len(chain) - 1, -1, -1):
            base, bound, nm = chain[i]
            a = base.aspect
            if a["after"] is not None:
                rn, rt, blk = a["after"]
                e = hook_env(base, bound)
                e.set(rn, sig.ret, mutable=False, kind="param")
                out = self.hook(blk, e, VOID, target_try or handler_at_or_above[i], f"after hook of @{nm.text}")
                if "fail" in out.effects and self.is_linear(sig.ret):
                    self.obl_err("E-OBL-008", f"the after hook of @{nm.text} may fail, but the result type {sig.ret.show()} is linear: a failing `after` would leave the result with no consumer (11.8)", out.sites.get("fail", nm), f"{nm.line}:{nm.col}", rn, "LANG-11-09")
                after_names.append(nm.text)
        for i in range(len(chain) - 1, -1, -1):
            base, bound, nm = chain[i]
            a = base.aspect
            if a["on_failure"] is not None:
                fn, rt, blk, ftok = a["on_failure"]
                e = hook_env(base, bound)
                e.set(fn, Named("Failure"), mutable=False, kind="owned")
                self.hook(blk, e, sig.ret, True, f"failure handler of @{nm.text}", owned=fn, tok=ftok)
                handler_names.append(nm.text)
        if not target_try:
            self.eff.effects.pop("fail", None)
            self.eff.sites.pop("fail", None)
        self.elaborations.append(f"{label}: before {before_names} -> body -> after {after_names}; on failure {handler_names} (innermost first)")

    def check_aspect_decl(self, d: Decl):
        a = d.aspect
        if a["rest"] and a["base"] is None:
            self.asp_err(f"aspect {d.name} declares `...rest` but is not a specialisation", self.dc.tok_of(d), "aspect x(...) = base(...)", "a full declaration with ...rest", d.name)
        if a["base"] is not None:
            bd = a.get("base_decl") or self.table.get(a["base"][0])
            if bd is None or bd.kind != "aspect":
                self.asp_err(f"aspect {d.name} specialises `{a['base'][0]}`, which is not a declared aspect", a["base"][2], "an aspect", a["base"][0], d.name)

    # ---- observation (13) -------------------------------------------------------------
    def obs_err(self, code: str, msg: str, tok: Token, fld: str, exit_path: str, rule: str = "LANG-13-02"):
        self.rec(code, msg, rule, tok, {"frame_field": fld, "exit_path": exit_path})

    def pure_snapshot(self, e, env: Env, expected: Optional[Type], what: str, tok: Token, fld: str) -> Type:
        before = set(self.eff.effects)
        t = self.infer(e, env, expected=expected)
        new = set(self.eff.effects) - before
        bad = sorted(new & {"io", "mutate", "nondet", "suspend", "fail", "block"})
        if bad:
            self.obs_err("E-OBS-012", f"the {what} expression has the {', '.join(bad)} effect; it must be pure, infallible and non-suspending (13)", tok, fld, "statement", "LANG-13-01")
        return t

    def capture_stmt(self, inner, env: Env):
        ts = direct_tokens(inner)
        fld, e = ts[1], find(inner, "expr")
        if self.schema is None:
            if not self.schema_unknown:
                self.obs_err("E-OBS-012", f"`capture {fld.text}` needs an observation schema attached with @observe(Schema) (13)", fld, fld.text, "statement", "LANG-13-01")
            self.pure_snapshot(e, env, None, "capture", fld, fld.text)
            return
        if fld.text not in self.schema.obs_fields:
            self.obs_err("E-OBS-012", f"`{fld.text}` is not a field of observation schema {self.schema.name}", fld, fld.text, "statement", "LANG-13-01")
            self.infer(e, env)
            return
        ft, req = self.schema.obs_fields[fld.text]
        if isinstance(ft, Refined) and isinstance(ft.base, Named) and ft.base.name == "String":
            ft = ft.base                                 # the scalar bound is a frame bound (truncation), not a refinement (13)
        if "loop" in self.ctx:
            self.obs_err("E-OBS-011", f"`capture {fld.text}` inside a loop body; a repeated measurement is `record` (13)", fld, fld.text, "loop body")
        elif fld.text in self.own.captured:
            self.obs_err("E-OBS-010", f"`{fld.text}` is captured a second time on this path", fld, fld.text, "statement")
        t = self.pure_snapshot(e, env, ft, "capture", fld, fld.text)
        if known(t) and known(ft):
            self.expect(t, ft, tokens(e)[0], f"capture {fld.text}")
        self.own.captured[fld.text] = "all"

    def record_stmt(self, inner, env: Env):
        ts = direct_tokens(inner)
        nm = ts[1]
        d = self.table.get(nm.text)
        al = find(inner, "argList")
        if d is None or d.kind != "observation":
            self.obs_err("E-OBS-012", f"`record {nm.text}` names no observation schema", nm, nm.text, "statement", "LANG-13-01")
            if al is not None:
                self.infer(al, env)
            return
        for lbl, e, rest, _ in self.aspect_args(al, env):
            if lbl is None or lbl not in d.obs_fields:
                self.obs_err("E-OBS-012", f"`record {nm.text}`: `{lbl or 'positional'}` is not a field of the schema", tokens(e)[0] if e is not None else nm, lbl or "", "statement", "LANG-13-01")
                if e is not None:
                    self.infer(e, env)
                continue
            ft, _ = d.obs_fields[lbl]
            t = self.pure_snapshot(e, env, ft, "record", tokens(e)[0], lbl)
            if known(t) and known(ft):
                self.expect(t, ft, tokens(e)[0], f"record field {lbl}")

    def obs_exit(self, kind: str, tok: Token, where: str):
        """A success or failure exit of the activation: every applicable required field is definitely captured (13)."""
        if self.schema is None or "lambda" in self.ctx:
            return
        for fld, (ft, req) in self.schema.obs_fields.items():
            if req == "all" or req == kind:            # a cancellation edge applies the `all` class only (13)
                if self.own.captured.get(fld) != "all":
                    self.obs_err("E-OBS-001", f"required field `{fld}` ({req}) is not captured on every path before {where}", tok, fld, where)

    def asy_err(self, msg: str, tok: Token, statement: str):
        self.rec("E-ASY-003", msg, "LANG-12-05", tok, {"offending_statement": statement, "rule_reference": "12.7"})

    def check_pool_launch(self, m: str, al, env: Env, tok: Token):
        """12.5/12.7: only Shareable environments cross a pool launch; no loan crosses one. launchParallel takes a
        future whose arguments are shareable values (copied or moved); launchBlocking a `fn once` whose captures
        are copy/move of shareables."""
        args = [a for a in children(al) if name(a) == "arg"] if al is not None else []
        if not args:
            return
        e = find(args[0], "expr")
        if e is None:
            return
        lam = find(e, "lambda")
        if m == "launchBlocking" or lam is not None:
            if lam is None:
                self.asy_err("launchBlocking takes a synchronous `fn once` closure (12.5)", tokens(e)[0], m)
                return
            words = [t.text for t in direct_tokens(lam)]
            if "mut" in words:
                self.asy_err("a `fn mut` closure is not Shareable and cannot cross a pool launch (12.7)", tokens(lam)[0], m)
            cl = tree_mod.child(lam, "captureList")
            for cap in ([c for c in children(cl) if name(c) == "capture"] if cl is not None else []):
                ck = children(cap)
                if isinstance(ck[0], Token):
                    continue                                   # a bare name: a trivial value, copied
                marker = tokens(ck[0])[0].text
                root = tokens(ck[1])[0].text
                b = env.lookup(root)
                if marker in ("alias", "editable"):
                    self.asy_err(f"the closure captures a loan of `{root}` (`{marker}`): loans never cross a pool launch (12.5)", tokens(ck[0])[0], m)
                elif b is not None and known(strip(b.type)) and not self.dc.is_shareable(strip(b.type)):
                    self.asy_err(f"the closure captures `{root}` of type {strip(b.type).show()}, which is not Shareable (12.7)", tokens(ck[0])[0], m)
            return
        # launchParallel(future): the async call's arguments
        inner_al = find(e, "argList")
        for a in ([c for c in children(inner_al) if name(c) == "arg"] if inner_al is not None else []):
            ae = find(a, "expr")
            if ae is None:
                continue
            mk = find(ae, "markerExpr")
            pl = self.path_of_expr(ae)
            if mk is not None and tokens(mk)[0].text in ("alias", "editable"):
                self.asy_err(f"`{tokens(mk)[0].text} {pl}` is a loan: loans never cross a pool launch (12.5)", tokens(ae)[0], m)
                continue
            if pl is not None:
                b = env.lookup(root_of(pl))
                if find(ae, "place") is not None:
                    t = strip(self.declared_type(find(ae, "place"), env))
                elif b is not None and "." not in pl:
                    t = strip(b.type)
                else:
                    continue
                if b is not None and b.kind in ("param", "editable") and mk is None and self.is_bearing(t):
                    self.asy_err(f"`{pl}` is a loan parameter: loans never cross a pool launch (12.5)", tokens(ae)[0], m)
                elif mk is None and self.is_bearing(t):           # an unmarked bearing place is a read loan (7.5)
                    self.asy_err(f"`{pl}` is passed by loan: a pool task may not borrow; write `copy {pl}` or `move {pl}` of a Shareable value (12.5)", tokens(ae)[0], m)
                elif known(t) and not self.dc.is_shareable(t):
                    self.asy_err(f"`{pl}` has type {t.show()}, which is not Shareable (12.7)", tokens(ae)[0], m)

    def task_typed(self, e, env: Env) -> bool:
        saved = len(self.records)
        own = self.own.copy()
        t = strip(self.infer(e, env))
        del self.records[saved:]
        self.own = own
        return isinstance(t, Named) and t.name == "Task"

    def shape_err(self, msg: str, tok: Token, shape: str, alternative: str):
        """7.10 / C.2: an ownership shape with no safe lowering is rejected, never lowered with unsafe or erasure."""
        self.own_err("E-OWN-090", f"{msg}; alternative: {alternative} (C.2)", tok, shape, "unsupported shape", "LANG-07-09")

    def check_shared_shape(self, args_types, al, env: Env, tok: Token, what: str):
        """`Shared.create` / `Mutex.create`: a loan, a view or a `fn mut` closure cannot be stored (C.2)."""
        for t, e in args_types:
            s = t
            if isinstance(t, LoanT) or (isinstance(strip(t), Named) and strip(t).name in ("StringView", "BytesView", "ListView")):
                self.shape_err(f"a loan is stored in {what}", tokens(e)[0], f"loan in {what}", "own the value")
            elif isinstance(strip(t), FnT) and strip(t).kind == "fn mut":
                self.shape_err(f"a `fn mut` closure escapes into {what}", tokens(e)[0], f"fn mut in {what}", "a Mutex around the state and a `fn` closure")
            else:
                d = self.decl_of(strip(t)) if known(strip(t)) else None
                if d is not None and d.lifetime:
                    self.shape_err(f"a value of {d.name}, which carries a loan through its lifetime parameter, is stored in {what}", tokens(e)[0], f"loan-carrying value in {what}", "own the value")

    def def_err(self, code: str, msg: str, tok: Token, statement: str, rule: str = "LANG-12-01"):
        self.rec(code, msg, rule, tok, {"offending_statement": statement, "rule_reference": "12.1"})

    def defer_stmt(self, inner, env: Env):
        """12.1: a deferred block is typed at registration with `return`/`fail`/`give`/`break`/`continue`/`await`
        rejected (E-DEF-001); the places it names must stay initialised until the block exits (E-DEF-002)."""
        blk = find(inner, "block")
        roots = {n for n in lower_names(tokens(blk)) if env.lookup(n) is not None}   # the bindings the defer names, in holes too
        tok = direct_tokens(inner)[0]
        for r in sorted(roots):
            b = env.lookup(r)
            h = self.own.holders.get(r)
            if b is not None and h is not None and getattr(b, "depth", None) == self.depth and h.holder == r:
                self.own_err("E-OWN-052", f"`{r}` holds a loan that ends when this block exits, before its deferred block runs; the defer may not use it (12.1)", tok, r, "loan ended before the defer", "LANG-07-04")
        self.deferred.append((self.depth, roots, tok))
        self.defer_bodies.append((self.depth, blk))
        # D-75: the body runs at every exit of the enclosing block, after any of the statements that follow. It is
        # checked from the registration state without the facts of every place those statements (later defers included)
        # assign or loan exclusively, and its own effects do not happen here: they happen at the block's exit.
        rest = list(self.block_rest)
        saved_facts = self.facts
        self.facts = saved_facts.copy()
        for st in rest:
            for p in self.assigned_places(st):
                self.facts.invalidate(p)
        self.defer_depth += 1
        saved_try = self.in_try
        self.in_try = True                     # `fail` forms are E-DEF-001 here, not E-ERR-001
        try:
            self.walk_block(blk, env)
        finally:
            self.in_try = saved_try
            self.defer_depth -= 1
            self.facts = saved_facts

    def obl_err(self, code: str, msg: str, tok: Token, origin: str, path: str, rule: str = "LANG-10-01"):
        self.rec(code, msg, rule, tok, {"obligation_origin": origin, "unconsumed_path": path})

    def is_linear(self, t: Type) -> bool:
        s = strip(t) if not isinstance(t, LoanT) else None
        return s is not None and known(s) and self.dc.is_linear(s)

    def obligate(self, path: str, tok: Token):
        self.own.obligations[path] = (tok, self.depth)

    def consume(self, path: Optional[str]):
        if path is not None:
            self.own.obligations.pop(path, None)

    def report_open(self, min_depth: int, where: str):
        """Every obligation at or below min_depth that is still open when the block (or function) exits."""
        for p, (tok, d) in list(self.own.obligations.items()):
            if p in self.catch_bound and min_depth == 0:
                self.discharges.append({"line": tok.line, "kind": "catch left by return/fail/Never"})   # discharged (10.4)
                self.eff.add("recovers", "proved", tok)
                del self.own.obligations[p]
                continue
            if d >= min_depth:
                self.obl_err("E-OBL-001", f"the linear value in `{p}` is dropped {where} without being consumed (or fail, or recover, catch, match, report, move)", tok, f"{tok.line}:{tok.col}", p)
                del self.own.obligations[p]

    def unwind_point(self, tok: Token, what: str, env: Env, keep: Optional[str] = None):
        """A point the function may leave by unwinding (`or fail`, `fail`, cancellation at `await`, 12.4): an
        open linear value is abandoned when Abandonable, otherwise it has no consumer (E-OBL-005)."""
        for p, (tok0, d) in list(self.own.obligations.items()):
            if p == keep or p in self.catch_bound:
                continue
            t = env.get(p) if "." not in p else None
            if t is not None and known(strip(t)) and self.dc.implements(strip(t), "Abandonable"):
                continue                      # abandoned on the unwind path; a normal exit must still consume it
            if what == "await":
                self.obl_err("E-OBL-005", f"the linear value in `{p}` is live across `await`, a cancellation point, and its type is not Abandonable", tok, f"{tok0.line}:{tok0.col}", p, "LANG-06-09")
            else:
                self.obl_err("E-OBL-005", f"the linear value in `{p}` is live across `{what}`, which may unwind the function, and its type is not Abandonable", tok, f"{tok0.line}:{tok0.col}", p, "LANG-06-09")

    def report_half(self):
        for p, tok in self.own.half:
            self.obl_err("E-OBL-001", f"the linear value in `{p}` is consumed on one path only; every path must consume it", tok, f"{tok.line}:{tok.col}", p)
        self.own.half = []

    def join_conditional(self, skip: Owners):
        """A conditional operand may not run: the right of `&&` / `||` / `??`, or an `or recover` / `catch`
        handler (LR 5.7, 10.2). Join its ownership with the path that skips it and report a consume that
        happened on only one of them (LR 7.4, 10.1). The operand was typed in the post-left state, so a
        second move is still seen. Reservations belong to the call being typed, not to this path join.
        `Owners.join` drops an earlier half, so this reports immediately: a later `if` or `match` must not
        be the only place a one-sided consume is noticed, and must not report it twice."""
        reservations = self.own.reservations
        self.own = skip.join(self.own)
        self.own.reservations = reservations
        self.report_half()

    def holder_chain(self, path: str) -> Set[str]:
        """The loan-holding bindings a path goes through: the binding itself and the parents it is a path to."""
        chain, cur = set(), root_of(path)
        while cur and cur not in chain:
            chain.add(cur)
            h = self.own.holders.get(cur)
            cur = root_of(h.owner) if h is not None and root_of(h.owner) != cur else None
        return chain

    def foreign_loans(self, path: str) -> List:
        """The active loans of `path`'s owner held by other bindings. A loan drawn through a read path (an item a
        cursor's `next()` lent, 7.6) is the owner's, not the path's: using the path itself leaves it alone (D-348)."""
        chain = self.holder_chain(path)
        return [l for l in self.own.active(path) if l.holder is not None and l.holder not in chain and l.via not in chain]

    def use_place(self, path: str, tok: Token, how: str, env: Env):
        """A use of a place: how in read | write | move | alias | editable | copy. Sets use_ok."""
        self.use_ok = True
        if path is None:
            return
        if self.own.is_moved(path) and not (how == "write" and "." not in path):
            self.own_err("E-OWN-020", f"`{path}` was moved and is used again ({how})", tok, path, "use after move")
            self.use_ok = False
            return
        kind = self.binding_kind(path, env)
        if how in ("read", "copy"):
            l = next((x for x in self.foreign_loans(path) if x.mode == "editable"), None)
            if l is not None:
                self.own_err("E-OWN-011", f"`{path}` is suspended while `{l.holder}` holds an exclusive loan of `{l.owner}`", tok, path, f"exclusive loan held by {l.holder}", "LANG-07-05", f"until the end of {l.holder}'s block")
                self.use_ok = False
            return
        if how == "alias":
            l = next((x for x in self.foreign_loans(path) if x.mode == "editable"), None)
            if l is not None:
                self.own_err("E-OWN-010", f"read loan of `{path}` while `{l.holder}` holds an exclusive loan of `{l.owner}`, which freezes the owner", tok, path, f"exclusive loan held by {l.holder}", "LANG-07-05", f"until the end of {l.holder}'s block")
                self.use_ok = False
            return
        if how == "editable":
            if not self.editable_root(path, env):
                self.own_err("E-OWN-001", f"`editable {path}` needs a root that is a `let mut`, an `editable` parameter or an exclusive loan", tok, path, "editable on an immutable root", "LANG-07-01")
                self.use_ok = False
                return
            for l in self.foreign_loans(path):
                self.own_err("E-OWN-010", f"exclusive loan of `{path}` while `{l.holder}` holds a {l.mode} loan of `{l.owner}`", tok, path, f"{l.mode} loan held by {l.holder}", "LANG-07-05", f"until the end of {l.holder}'s block")
                self.use_ok = False
                return
            return
        if how == "write":
            for l in self.foreign_loans(path):
                self.own_err("E-OWN-011", f"`{path}` is assigned while `{l.holder}` holds a {l.mode} loan of `{l.owner}`", tok, path, f"{l.mode} loan held by {l.holder}", "LANG-07-05", f"until the end of {l.holder}'s block")
                self.use_ok = False
                return
            if root_of(path) in self.own.holders and "." not in path and kind not in ("param", "editable", "owned"):
                self.own_err("E-OWN-011", f"`{path}` holds a loan and cannot be reassigned", tok, path, "loan-holding binding", "LANG-07-05")
                self.use_ok = False
            return
        if how == "move":
            for d_depth, roots, dtok in self.deferred:
                if root_of(path) in roots and d_depth <= self.depth:
                    self.def_err("E-DEF-002", f"`{path}` is moved out after the defer at line {dtok.line} names it; a deferred block's places must be initialised at every exit (12.1)", tok, f"move {path}")
            if "." in path and root_of(path) not in self.own.holders and kind not in (None,):
                self.own_err("E-OWN-040", f"partial move of `{path}` out of `{root_of(path)}`, which still owns the rest; move the whole binding or restructure", tok, path, "partial move", "LANG-07-01")
                self.use_ok = False
                return
            if kind in ("param", "editable") and root_of(path) in self.own.holders:
                # a field of the loan too: the parameter owns nothing, so nothing in it can be moved out (D-33)
                self.own_err("E-OWN-001", f"`move {path}`: a plain or editable parameter holds a loan and owns nothing to move; take it `owned`", tok, path, "move of a loan-holding parameter", "LANG-07-06")
                self.use_ok = False
                return
            if kind == "capture-move" and self.lambda_kind and self.lambda_kind[-1] != "fn once":
                self.rec("E-FUN-010", f"this `{self.lambda_kind[-1]}` lambda moves its capture `{root_of(path)}` out: declare it `fn once`", "LANG-08-02", tok, {"violation": "move out of a capture", "capture_span": root_of(path), "use_span": path})
                self.use_ok = False
                return
            if kind in ("capture-alias", "capture-editable"):
                self.own_err("E-OWN-001", f"`move {path}`: an `{kind[8:]}` capture is a loan and owns nothing to move", tok, path, "move of a loan capture", "LANG-08-02")
                self.use_ok = False
                return
            if kind in ("pattern-loan", "for") and root_of(path) in self.own.holders:
                self.own_err("E-OWN-001", f"`move {path}`: a binding of `match p` or `for x in p` is a read loan; use `match move p` / `for x in move p` to own it (9.8)", tok, path, "move of a loaned binding", "LANG-09-06")
                self.use_ok = False
                return
            for l in self.foreign_loans(path):
                self.own_err("E-OWN-011", f"`{path}` is moved while `{l.holder}` holds a {l.mode} loan of `{l.owner}`", tok, path, f"{l.mode} loan held by {l.holder}", "LANG-07-05", f"until the end of {l.holder}'s block")
                self.use_ok = False
                return
            return

    def reserve(self, path: str, mode: str, tok: Token):
        """5.7 step 1: a marked or implicitly loaned argument reserves its place; a later argument that moves,
        assigns or exclusively loans a reserved place breaks the reservation (E-OWN-012)."""
        if mode in ("move", "editable"):
            for rp, rm in self.own.reservations + [r for level in self.res_stack for r in level]:
                if overlaps(rp, path) or overlaps(path, rp):
                    self.own_err("E-OWN-012", f"argument `{mode} {path}` breaks the reservation of `{rp}` ({rm}) taken by an earlier argument of this call", tok, path, f"reservation of {rp}", "LANG-07-04")
                    return
        self.own.reservations.append((path, mode))

    def activate(self, tok: Token, carries: bool, env: Env, keep: Optional[Dict[int, str]] = None):
        """5.7 step 2: reservations become loans at once; conflicts among them or with active loans are reported.
        Step 4: the loans end when the call returns unless the result carries them. A result that holds a loan inside
        an optional, a tuple or a record (`(alias T from self)?`) keeps, for each reservation `keep` names (the
        argument it is `from`), a loan of the mode it declares, carried `nested` to the binding that receives it (D-348)."""
        res, self.own.reservations = self.own.reservations, []
        added = []
        kept = []
        for i, (path, mode) in enumerate(res):
            if mode == "move":
                self.own.move(path)
                continue
            lmode = "editable" if mode == "editable" else "alias"
            for other, omode in res:
                if other is not path and (overlaps(other, path) or overlaps(path, other)) and (lmode == "editable" or omode in ("editable", "move")):
                    self.own_err("E-OWN-010", f"the call takes a {lmode} loan of `{path}` and a {omode} of `{other}` at once", tok, path, f"{omode} of {other}", "LANG-07-05")
            added.append(self.own.add_loan(path, lmode, None, self.depth, tok.line))
            if keep and i in keep:
                kept.append((path, keep[i]))
        if carries:
            self.carried += added
            return
        for l in added:
            if l in self.own.loans:
                self.own.loans.remove(l)
        for path, mode in kept:
            # the kept loan has the mode the result declares (7.6): `editable cursor.next()` lends a read loan, drawn
            # from what the cursor lends, so advancing the cursor leaves it alone; so does any use of a read path
            root = root_of(path)
            h = self.own.holders.get(root)
            rt = env.get(root)
            rt = strip(rt.inner if isinstance(rt, LoanT) else rt) if rt is not None else None
            cursor = isinstance(rt, Named) and rt.name in ("StringView", "BytesView", "ListView", "ListCursor")
            l = self.own.add_loan(path, mode, None, self.depth, tok.line)
            l.nested = True
            l.via = root if cursor or (h is not None and h.mode == "alias") else None
            self.carried.append(l)

    def result_carries(self, sig: Sig) -> bool:
        r = strip(sig.ret) if not isinstance(sig.ret, LoanT) else sig.ret
        return isinstance(sig.ret, LoanT) or (isinstance(r, Named) and r.name in ("StringView", "BytesView", "ListView", "Future")) or "async" in sig.modifiers

    @staticmethod
    def nested_origins(t: Type, top: bool = True) -> Dict[str, str]:
        """The origins of the loans a result holds inside an optional, a tuple or a record, with the mode each
        declares: `(alias T from self)?` gives {"self": "alias"} (D-348). A result that is itself a loan carries its
        loan whole (`result_carries`), so it gives none."""
        if isinstance(t, LoanT):
            return {} if top else {t.origin: t.mode}
        if isinstance(t, OptionalT):
            return Typer.nested_origins(t.inner, False)
        out: Dict[str, str] = {}
        if isinstance(t, TupleT):
            for x in t.items:
                out.update(Typer.nested_origins(x, False))
        elif isinstance(t, RecordT):
            for _, x in t.fields:
                out.update(Typer.nested_origins(x, False))
        return out

    def holds_loan(self, t: Type) -> bool:
        """Whether a value of type `t` holds a loan of an ownership-bearing value: a loan, or one inside an optional,
        a tuple or a record, at any depth (D-348). A loan of a trivial value is that value, and holds nothing."""
        if isinstance(t, LoanT):
            return self.is_bearing(t.inner) or self.holds_loan(t.inner)
        if isinstance(t, OptionalT):
            return self.holds_loan(t.inner)
        if isinstance(t, TupleT):
            return any(self.holds_loan(x) for x in t.items)
        if isinstance(t, RecordT):
            return any(self.holds_loan(x) for _, x in t.fields)
        return False

    def keep_nested(self, holder: str, depth: int, t: Type, e) -> List[Loan]:
        """A binding receives a value of type `t` from expression `e` (D-348). When `t` holds a loan of an
        ownership-bearing value, the binding holds, until the end of its block, the loans the value carries inside it:
        those of the calls in `e` (`xs.get(0)`, `(xs.get(0), 1)`, `xs.get(0) ?? alias d`) and those of the bindings `e`
        reads (`move t`, `t ?? u`). Otherwise those loans end with the statement. Returns the loans it holds."""
        keep = self.holds_loan(t)
        rest, held = [], []
        for l in self.carried:
            if not l.nested:
                rest.append(l)
            elif keep:
                l.holder, l.depth = holder, depth
                held.append(l)
            elif l in self.own.loans:
                self.own.loans.remove(l)
        self.carried = rest
        if keep and e is not None:
            names = {tk.text for tk in tokens(e) if tk.kind == "LOWER"}
            for l in list(self.own.loans):
                # a parameter's loan of its own argument is the caller's and outlives the function: not passed on
                if l.holder in names and l.holder != holder and root_of(l.owner) != l.holder:
                    via = l.via if l.via is not None else (l.holder if l.mode == "alias" else None)
                    c = Loan(l.owner, l.mode, holder, depth, l.line, True, via)
                    self.own.loans.append(c)
                    held.append(c)
        return held

    def carries_onward(self, t: Type) -> bool:
        """Whether a value of type `t` can carry a loan onward: a future, a task or a loan, also inside an optional, an
        outcome or a tuple (D-31)."""
        if self.loan_carrying(t):
            return True
        s = strip(t)
        if isinstance(s, OptionalT):
            return self.carries_onward(s.inner)
        if isinstance(s, Named):
            if s.name in ("Future", "Task"):
                return True
            if s.name == "Outcome" and s.args:
                return self.carries_onward(s.args[0])
        if isinstance(s, TupleT):
            return any(self.carries_onward(x) for x in s.items)
        return False

    @staticmethod
    def default_is_temporary(decl, pname: str) -> bool:
        """Whether a parameter's default makes a new value at each call (anything but a literal string, 5.8)."""
        for p in find_all(decl, "param"):
            ts = [t for t in direct_tokens(p) if t.kind == "LOWER"]
            if ts and ts[0].text == pname:
                d = find(p, "constExpr")
                if d is None:
                    return False
                dts = [t for t in tokens(d) if t.text not in ("(", ")")]
                if len(dts) == 1 and (dts[0].text == "null" or (dts[0].kind == "STR" and "${" not in dts[0].text)):
                    return False                            # `null` and a literal string lend nothing that expires
                return True
        return False

    def lends_temporary(self, e, t: Type) -> bool:
        """An argument that is a temporary of a type a read parameter lends (not a place, not a trivial value, not a
        literal string, which is static): a future keeps the loan (12.3; D-31)."""
        ts = [t for t in tokens(e) if t.text != "("]
        copied = find(e, "markerExpr") is not None and ts and ts[0].text == "copy"
        if self.path_of_expr(e) is not None and not copied:
            return False
        s = strip(t)
        return self.is_bearing(t) or s == INTERP

    def check_arg_marker(self, e, p_name: str, mode: str, t: Type, tok: Token, env: Env):
        """A bare ownership-bearing place where the parameter mode needs a marker (7.5)."""
        lent_marked = find(e, "markerExpr") is not None and tokens(e)[0].text in ("alias", "editable")
        if mode.lstrip(".") in ("owned", "payload") and not lent_marked:      # a written `alias` is E-OWN-001 at its marker
            self.loan_into_owned(t, None, tok, f"`{p_name}`")
        path = self.path_of_expr(e)
        if path is None:
            lit = tokens(e)
            static_literal = len(lit) == 1 and lit[0].kind == "STR" and "${" not in lit[0].text   # `StringView from static` (6.6)
            if mode == "from" and find(e, "markerExpr") is None and not static_literal:
                self.own_err("E-OWN-051", f"the result borrows from `{p_name}`, so its argument must be a place, not a temporary (7.6)", tok, p_name, "loan of a temporary", "LANG-07-06")
            return
        if mode.startswith("..."):
            mode = mode[3:]
        if not self.is_bearing(t):
            if mode in ("editable",) and find(e, "markerExpr") is None:
                self.own_err("E-OWN-001", f"argument `{p_name}` is an `editable` parameter: write `editable {path}`", tok, path, "editable marker required", "LANG-07-01")
            return
        if find(e, "markerExpr") is not None and len(tokens(e)) and tokens(e)[0].text in ("copy", "alias", "move", "editable"):
            return
        if mode == "owned":
            self.own_err("E-OWN-001", f"argument `{p_name}` is an `owned` parameter: write `move {path}` or `copy {path}`", tok, path, "move or copy required", "LANG-07-01")
        elif mode == "editable":
            self.own_err("E-OWN-001", f"argument `{p_name}` is an `editable` parameter: write `editable {path}`", tok, path, "editable marker required", "LANG-07-01")
        elif mode == "from":
            self.own_err("E-OWN-001", f"the result borrows from `{p_name}`: write `alias {path}` (or `editable {path}`) so the kept loan shows at the call", tok, path, "alias marker required", "LANG-07-06")
            return
        elif mode == "loan":
            self.own_err("E-OWN-001", f"`{p_name}` stores a loan: write `alias {path}` or `editable {path}`", tok, path, "alias marker required", "LANG-07-06")
        else:
            self.reserve(path, "alias", tok)       # implicit read loan for the call

    # ---- generics ---------------------------------------------------------------------
    def recv_subst(self, recv: Type, d: Decl) -> Dict[str, Type]:
        recv = strip(recv)
        if isinstance(recv, Refined): recv = recv.base
        m: Dict[str, Type] = {"Self": recv}
        if isinstance(recv, Named):
            for p, a in zip(d.type_params, recv.args):
                m[p] = a
        return m

    def unify(self, pattern: Type, actual: Type, m: Dict[str, Type], pin_empty: bool = True):
        if isinstance(pattern, TypeParam):
            # an argument of the body's own type parameter fixes the callee's: `label(x)` with `x: U` binds T to U, whose
            # bounds are then the body's (D-171)
            if pattern.name not in m and (known(actual) or (isinstance(actual, TypeParam) and actual.name in self.tparams_in_scope)):
                a = concrete(strip(actual))
                if isinstance(a, Refined) and a.kind == "exact": a = a.base
                # a declared function (empty identity, optional and loan wrappers included) must not pin T when a
                # pending lambda can constrain that parameter (8.4). The lambda pins the closure; the declared
                # function is then a second closure at that position. No shared parameter, and it still pins.
                if not pin_empty and self._empty_declared_fn(a):
                    return
                m[pattern.name] = a
            return
        if isinstance(pattern, Named) and pattern.name in m and not pattern.args:
            return
        actual = strip(actual)
        if isinstance(pattern, Named) and isinstance(actual, Named) and pattern.name == actual.name:
            for pa, aa in zip(pattern.args, actual.args):
                self.unify(pa, aa, m, pin_empty)
        elif isinstance(pattern, OptionalT) and isinstance(actual, OptionalT):
            self.unify(pattern.inner, actual.inner, m, pin_empty)
        elif isinstance(pattern, LoanT):
            self.unify(pattern.inner, actual, m, pin_empty)
        elif isinstance(pattern, FnT) and isinstance(actual, FnT):
            for pp, ap in zip(pattern.params, actual.params):
                self.unify(pp, ap, m, pin_empty)
            self.unify(pattern.ret, actual.ret, m, pin_empty)
        elif isinstance(pattern, TupleT) and isinstance(actual, TupleT):
            for pp, ap in zip(pattern.items, actual.items):
                self.unify(pp, ap, m, pin_empty)

    @staticmethod
    def _empty_declared_fn(t: Type) -> bool:
        """A non-dynamic function type with no closure identity: a declared function, not a lambda (8.4)."""
        while isinstance(t, (OptionalT, LoanT)):
            t = t.inner
        return isinstance(t, FnT) and not t.dynamic and not t.identity

    def subst(self, t: Type, m: Dict[str, Type]) -> Type:
        return self.dc.subst(t, m)

    def result_of(self, sig: Sig, m: Dict[str, Type], unknown_args: bool = False) -> Type:
        r = self.subst(sig.ret, m)
        if self.has_free(r):
            # an uninferred type argument is one the call leaves unbound; a parameter bound to an enclosing generic's
            # own parameter (a generic body) is not one, and neither is one an unknown argument left unbound
            unbound = self.free_names(sig.ret) - set(m)
            return Unknown("generic result" if unbound and not unknown_args else "generic body")
        if "try" in sig.modifiers:
            r = Named("Outcome", (r,))
        if "async" in sig.modifiers:
            r = Named("Future", (r,))
        return r

    def free_names(self, t: Type) -> Set[str]:
        """The type parameters a type mentions."""
        if isinstance(t, TypeParam): return {t.name}
        if isinstance(t, Named): return set().union(*[self.free_names(a) for a in t.args]) if t.args else set()
        if isinstance(t, (OptionalT, LoanT)): return self.free_names(t.inner)
        if isinstance(t, TupleT): return set().union(*[self.free_names(x) for x in t.items]) if t.items else set()
        if isinstance(t, FnT): return set().union(self.free_names(t.ret), *[self.free_names(x) for x in t.params])
        if isinstance(t, RecordT): return set().union(*[self.free_names(x) for _, x in t.fields]) if t.fields else set()
        return set()

    def tparams_of(self, t: Type) -> Set[str]:
        if isinstance(t, TypeParam): return {t.name}
        if isinstance(t, Named): return set().union(*[self.tparams_of(a) for a in t.args])
        if isinstance(t, (OptionalT, LoanT, Refined)): return self.tparams_of(t.inner if not isinstance(t, Refined) else t.base)
        if isinstance(t, TupleT): return set().union(*[self.tparams_of(x) for x in t.items])
        if isinstance(t, FnT): return set().union(*[self.tparams_of(x) for x in t.params], self.tparams_of(t.ret))
        if isinstance(t, RecordT): return set().union(*[self.tparams_of(x) for _, x in t.fields])
        return set()

    def grants(self, have: Named, want: Named, seen: tuple = ()) -> bool:
        """Does the bound `implements have` grant `implements want`: the same interface with the same arguments, or one
        it requires (6.10)."""
        if have.name == want.name:
            return not want.args or tuple(have.args) == tuple(want.args)
        if have.name in seen:
            return False
        hd = self.table.get(have.name)
        if hd is None or hd.kind != "interface":
            return False
        sub = dict(zip(hd.type_params, have.args))        # `J[Int] requires I[T]` grants I[Int]
        return any(self.grants(self.subst(r, sub), want, seen + (have.name,)) for r in hd.requires)

    def bound_holds(self, a: Type, want: Named) -> Optional[bool]:
        """Does `a` satisfy `implements want` (6.10)? None while a type variable of the call is unsolved. A type
        parameter of the body being typed satisfies only what its `where` bounds grant; an applied interface needs a
        declared implementation with the same arguments, and its own bounds hold for them (D-171)."""
        if isinstance(a, Unknown) or any(p not in self.tparams_in_scope for p in self.tparams_of(a) | self.tparams_of(want)):
            return None
        if want.name in ("Copyable", "Shareable"):
            return self.dc.implements(a, want.name)
        if isinstance(a, TypeParam):
            return any(self.grants(b, want) for tp, b in self.where_in_scope if tp == a.name)
        if not self.dc.implements(a, want.name):
            return False
        idecl = self.table.get(want.name)
        if idecl is not None and want.args:
            sub = dict(zip(idecl.type_params, want.args))
            for tp2, b2 in getattr(idecl, "where", []) or []:
                if tp2 in sub and self.bound_holds(strip(sub[tp2]), self.subst(b2, sub)) is False:
                    return False
        if want.args and isinstance(a, Named):
            d = self.decl_of(a)
            declared = list(d.implements if d is not None else []) + [i for i, tt, _, _ in self.table.implement_blocks if tt.name == a.name]
            same = [i for i in declared if i.name == want.name]
            if same:
                sub = dict(zip(d.type_params, a.args)) if d is not None else {}
                return any(tuple(self.subst(i, sub).args) == tuple(want.args) for i in same)
        return True

    def has_free(self, t: Type) -> bool:
        if isinstance(t, TypeParam): return True
        if isinstance(t, Named): return any(self.has_free(a) for a in t.args)
        if isinstance(t, (OptionalT, LoanT)): return self.has_free(t.inner)
        if isinstance(t, TupleT): return any(self.has_free(x) for x in t.items)
        if isinstance(t, FnT): return any(self.has_free(x) for x in t.params) or self.has_free(t.ret)
        if isinstance(t, RecordT): return any(self.has_free(x) for _, x in t.fields)
        return False

    # ---- calls (5.8) --------------------------------------------------------------------
    def check_call(self, sig: Sig, al, env: Env, m: Dict[str, Type], where: str, tok: Token) -> Type:
        expected, self.pending_call_expected = self.pending_call_expected, None
        self.call_depth += 1
        try:
            return self.check_call_at(sig, al, env, m, where, tok, expected)
        finally:
            self.call_depth -= 1
            self.call_sigs[(tok.line, tok.col)] = sig      # the callee at its name token, for `@ignorable` statements (5.5)

    def check_call_at(self, sig: Sig, al, env: Env, m: Dict[str, Type], where: str, tok: Token, expected: Optional[Type] = None) -> Type:
        """Bind arguments to parameters, type them, infer method type parameters, return the result type."""
        call_decl, call_label = self.call_decl, self.call_label   # the callee of this call; argument typing may call others
        args = self.arg_nodes(al)
        has_lambda = any(self.is_lambda(self.arg_expr(a)) for a in args)
        if expected is not None:
            # the expected result fixes type arguments before the arguments are typed (6.10). A function binding is
            # dropped only when a lambda is pending, so the lambda can pin the closure (8.4); `seed_dynamic_fn`
            # then puts a `dynamic fn` back. `List.of` does this too, and doing it again here is the same map.
            self.seed_expected_result(sig.ret, expected, m, has_lambda)
            self.seed_dynamic_fn(sig.ret, expected, m)   # `List[dynamic fn]` fixes T before a lambda argument pins it (8.4)
        params = list(sig.params)
        bound: Dict[str, object] = {}
        seen_named = False
        pairs: List[Tuple[Tuple[str, Type, str], object]] = []
        for i, a in enumerate(args):
            ts = direct_tokens(a)
            e = self.arg_expr(a)
            if ts and ts[0].text == "...":
                continue
            pun = pun_name(a) if seen_named else None       # `wanted` after a named argument is `wanted: wanted` (5.8, X-345)
            if pun is not None:
                ts = [pun, Token("PUNCT", ":", pun.line, pun.col)]
            if ts and ts[0].kind == "LOWER" and len(ts) > 1 and ts[1].text == ":":
                seen_named = True
                p = next((p for p in params if p[0] == ts[0].text), None)
                if p is None:
                    why = f" (`{ts[0].text}` after a named argument stands for `{ts[0].text}: {ts[0].text}`)" if pun is not None else ""
                    self.typ("E-TYP-130", f"{where} has no parameter `{ts[0].text}`{why}", ts[0], ", ".join(p[0] for p in params), ts[0].text, ts[0].text, "LANG-05-09")
                    self.infer(e, env)
                    continue
                if p[0] in bound:
                    self.typ("E-TYP-130", f"parameter `{p[0]}` of {where} is bound twice", ts[0], "each parameter once", p[0], p[0], "LANG-05-09")
                    continue
            else:
                if seen_named:
                    self.typ("E-TYP-130", f"positional argument after a named argument in {where}", tokens(a)[0], "named arguments last", "positional", where, "LANG-05-09")
                    self.infer(e, env)
                    continue
                if params and params[-1][2].startswith("...") and i >= len(params) - 1:
                    p = params[-1]
                elif i < len(params):
                    p = params[i]
                else:
                    self.typ("E-TYP-130", f"{where} takes {len(params)} argument{'s' if len(params) != 1 else ''}, {len(args)} given", tokens(a)[0], f"{len(params)} arguments", f"{len(args)}", where, "LANG-05-09")
                    self.infer(e, env)
                    continue
            if not p[2].startswith("..."):
                bound[p[0]] = a
            pairs.append((p, e))
        # two passes: non-lambda arguments fix the type parameters, then lambdas are typed with expectations
        arg_types: List[Tuple[Tuple[str, Type, str], Type, object]] = []
        deferred = []
        saved_res, self.own.reservations = self.own.reservations, []
        self.res_stack.append(saved_res)
        recv = self.receiver
        self.receiver = None
        spans: Dict[str, Tuple[int, int]] = {}         # the reservations each argument made, by parameter (D-348)
        if recv is not None:
            self.reserve(recv[0], recv[1] if recv[1] != "alias" else "alias", tok)
            spans["self"] = (0, len(self.own.reservations))
        origin = sig.ret.origin if isinstance(sig.ret, LoanT) else None
        temps: List[Token] = []
        # A pending lambda suppresses an empty-function pin only for a type parameter it can
        # constrain. `foo(move id, fn ...)` passes the lambda to a concrete `fn`; `id` still fixes `T` (8.4).
        pending_names: Set[str] = set()
        for p, e in pairs:
            if self.is_lambda(e):
                pending_names |= self.tparams_of(p[1])
        for p, e in pairs:
            if self.is_lambda(e):
                deferred.append((p, e))
                continue
            pmode = "from" if origin == p[0] else ("loan" if isinstance(p[1], LoanT) else p[2])
            self.arg_mode = pmode
            n0 = len(self.own.reservations)
            t = self.infer(e, env, expected=self.subst(p[1], m))
            self.arg_mode = None
            self.check_arg_marker(e, p[0], pmode, t, tokens(e)[0], env)
            spans[p[0]] = (n0, len(self.own.reservations))
            if "async" in sig.modifiers and pmode in ("", "loan") and self.lends_temporary(e, t):
                temps.append(tokens(e)[0])
            shared = self.tparams_of(p[1]) & pending_names
            self.unify(p[1], t, m, pin_empty=not any(tp not in m for tp in shared))
            arg_types.append((p, t, e))
        # an omitted parameter's default that is evaluated afresh (not a literal string) is a temporary too (D-31)
        if "async" in sig.modifiers and sig.node is not None:
            for pn, pt, pm in sig.params:
                if pn not in bound and pm == "" and self.is_bearing(pt) and self.default_is_temporary(sig.node, pn):
                    temps.append(tok)
        self.carried_temps += [(tk, self.call_depth) for tk in temps]
        for p, e in deferred:
            t = self.infer(e, env, expected=self.subst(p[1], m))
            self.unify(p[1], t, m)
            arg_types.append((p, t, e))
        carries = self.result_carries(sig)
        keep = {i: mode for o, mode in ({} if carries else self.nested_origins(sig.ret)).items() for i in range(*spans.get(o, (0, 0)))}
        self.activate(tok, carries, env, keep)
        self.res_stack.pop()
        self.own.reservations = saved_res
        self.call_decl, self.call_label = call_decl, call_label
        callee = self.callee_summary(call_decl, sig, call_label)
        arg_places = [(p[0], self.path_of_expr(e)) for p, t, e in arg_types if self.path_of_expr(e) is not None and p[2].lstrip(".") == "editable"]
        cb_types = [t for p, t, e in arg_types if isinstance(strip(t), FnT) and self.path_of_expr(e) is None]
        self.import_call(callee, tok, recv[0] if recv is not None else self.coll_path, arg_places, env, "async" in sig.modifiers, [strip(t) for t in cb_types])
        if self.call_label in ("method Shared.create", "method Mutex.create"):
            self.check_shared_shape([(t, e) for p, t, e in arg_types], al, env, tok, self.call_label.split(".")[0].split()[-1])
        for p, t, e in arg_types:
            want = self.subst(p[1], m)
            if not self.has_free(want):
                # a declared function against a lambda-pinned parameter is the same clash as two lambdas (8.4).
                # `expect` would accept it: an empty identity is not a `closure_conflict`. One record, not both.
                if declared_vs_lambda(t, want):
                    shown = t.show()
                    self.typ("E-TYP-040", f"argument `{p[0]}` has type {shown}, expected {want.show()}: {CLOSURE_WHY}", tokens(e)[0], want.show(), shown, f"argument `{p[0]}`", "LANG-08-04")
                else:
                    self.expect(t, want, tokens(e)[0], f"argument `{p[0]}`")
            if self.is_linear(t) and self.path_of_expr(e) is None and not p[2].lstrip(".").startswith("owned") and find(e, "markerExpr") is None and assignable(self.dc, t, want)[0] is not False:
                self.obl_err("E-OBL-001", f"the linear value passed to `{p[0]}` is only loaned for the call and dropped afterwards; the parameter must be `owned`", tokens(e)[0], f"{tokens(e)[0].line}:{tokens(e)[0].col}", f"argument {p[0]}")
        # the callee's `where` bounds, and a generic owner's, hold for the type arguments the call fixes (6.10):
        # `MutexGuard[T].get` needs a Copyable T (D-171)
        owner_where, self.pending_owner_where = self.pending_owner_where, ()
        for tp, iface in list(owner_where) + list(sig.where):
            a = strip(m[tp]) if tp in m else None
            if a is None:
                continue
            want = self.subst(iface, m)
            if self.bound_holds(a, want) is False:
                self.typ("E-TYP-090", f"{where} needs `{tp}` to implement {want.show()}; {a.show()} does not (6.10)", tok, want.show(), a.show(), where, "LANG-06-10")
        for p in params:
            if p[0] not in bound and not p[2].startswith("...") and not self.has_default(sig, p[0]):
                self.typ("E-TYP-050", f"{where} needs argument `{p[0]}`", tok, p[0], "absent", where, "LANG-05-09")
        return self.result_of(sig, m, unknown_args=any(not known(strip(t)) for _, t, _ in arg_types))

    def check_param_defaults(self, sig: Sig, label: str):
        """Each parameter default (5.8) is typed where it is declared, against the parameter's type, under a label of its
        own: outside every body, so it cannot create the capabilities either (D-149)."""
        if sig.node is None:
            return
        self.check_defaults(find_all(sig.node, "param"), {pn: pt for pn, pt, _ in sig.params}, label)

    def check_defaults(self, params: list, types: Dict[str, Type], label: str):
        for p in params:
            ce = tree_mod.child(p, "constExpr")
            ts = [t for t in direct_tokens(p) if t.text != "..."]
            if ce is None or not ts or ts[0].text not in types:
                continue
            self.decl_context(f"default of parameter `{ts[0].text}` of {label}")
            want = types[ts[0].text]
            fixed = known(strip(want)) and not self.has_free(want)
            t = self.infer(ce, Env(), expected=want if fixed else None)
            if fixed:
                self.expect(t, want, tokens(ce)[0], f"default of `{ts[0].text}`")

    @staticmethod
    def arg_nodes(al) -> list:
        """The arguments of an argument list: `arg` in an expression, `constArg` in a constant expression (D-149)."""
        return [a for a in children(al) if name(a) in ("arg", "constArg")] if al is not None else []

    @staticmethod
    def arg_expr(a):
        """An argument's expression: `expr`, or a constant argument's `constExpr`."""
        return find(a, "expr") if name(a) == "arg" else tree_mod.child(a, "constExpr")

    def const_term(self, node, env: Env, expected) -> Type:
        """A term of a constant expression (grammar constTerm: a constant, a field or parameter default), typed like the
        expression it spells; before, every one was left untyped (D-149)."""
        kids = children(node)
        first = kids[0]
        if isinstance(first, Token):
            if first.text == "null":
                return OptionalT(NEVER)
            if first.text == "(":
                return self.infer(tree_mod.child(node, "constExpr"), env, expected)
            if first.text == "-":
                return self.negated(first, kids[1], env, expected)
            if first.text == "{":
                ks = [tokens(k)[0] for k in kids if name(k) == "ident"] or [k for k in kids if isinstance(k, Token) and k.kind in ("LOWER", "UPPER", "KW")]
                es = [c for c in kids if name(c) == "constExpr"]
                if any(k.kind == "UPPER" for k in ks):
                    for e in es:
                        self.infer(e, env)
                    return Unknown("variant map")
                return RecordT(tuple((k.text, self.infer(e, env)) for k, e in zip(ks, es)))
            if first.kind == "LOWER":
                ms = [tokens(k)[0] for k in kids if name(k) == "memberName"]
                if not ms:
                    value = self.const_value_type(first.text)
                    if value is not None:
                        return value
                    return self.fact_type(first.text, self.root_type(first, env))
                if first.text in self.aliases and len(ms) == 1:
                    d = self.table.get(first.text + "." + ms[0].text)
                    self.entry_main_ref(d, ms[0])
                    if d is not None and d.kind == "const":
                        used = self.const_use_type(d)
                        if used is not None:
                            return used
                return Unknown("constant path")
            return Unknown("constant term")
        if name(first) == "literal":
            return self.infer(first, env, expected)
        if name(first) == "typeRef":
            bad = self.nonconstant_term(node)
            if bad is not None:
                self.typ("E-TYP-040", f"`{bad[1]}(…)` is not a constant expression (4.2): a constant, a field default or a parameter default calls only constructors, enum variants, the `fortis` package's `Type.of` constructors and a derived `Type.default()`", bad[0], "a constant expression", f"a call of {bad[1]}", bad[1], "LANG-04-02")
                return Unknown("not constant")
            return self.type_primary(node, env, expected)
        return Unknown("constant term")

    def const_arith(self, kids: list, env: Env) -> Type:
        """A constant expression's arithmetic (grammar constExpr: terms and operators in one flat list): `*`, `/` and `%`
        bind tighter than `+` and `-`, so each product is typed first, then the sum; every term is typed (D-149)."""
        groups, ops = [[kids[0]]], []
        for i in range(1, len(kids) - 1, 2):
            if kids[i].text in ("+", "-"):
                ops.append(kids[i])
                groups.append([kids[i + 1]])
            else:
                groups[-1] += [kids[i], kids[i + 1]]
        total = None
        for gi, g in enumerate(groups):
            t = self.infer(g[0], env)
            for j in range(1, len(g) - 1, 2):
                t = self.arith(g[j], t, self.infer(g[j + 1], env))
            total = t if total is None else self.arith(ops[gi - 1], total, t)
        return total

    def note_inferred_const(self, module: str, name: str, t: Type) -> None:
        """Remember an unannotated constant's initialiser type for every module of this check (D-54.4)."""
        if known(strip(t)):
            self.dc.session.inferred_consts[(module, name)] = t

    def inferred_const(self, module: str, name: str) -> Optional[Type]:
        t = self.dc.session.inferred_consts.get((module, name))
        if t is not None and known(strip(t)):
            return t
        return None

    def const_use_type(self, d) -> Optional[Type]:
        """The type of a constant at a use: its declared type, else the initialiser's type from this check."""
        if d.alias is not None and known(strip(d.alias)):
            return d.alias
        stored = self.inferred_const(d.module, d.name)
        if stored is not None:
            return stored
        if d.module == self.module:
            value = self.const_value_type(d.name)
            if value is not None:
                return value
        return d.alias

    def seed_consts(self, mtree) -> None:
        """Note this module's unannotated constants and discard the records. A package check runs this on every
        module before typing any of them (D-54.4)."""
        for imp in tree_mod.imports(mtree):
            if imp["members"] is None:
                self.aliases[imp["alias"] or imp["path"][-1]] = ".".join(imp["path"])
        for d in [d for d in self.table.decls.values() if d.kind == "const" and d.module == self.module]:
            self.const_value_type(d.name)
        del self.records[:]

    def const_value_type(self, nm: str) -> Optional[Type]:
        """Inside a constant expression a constant stands for its value (4.2: constants are built at load, before any
        code runs), so arithmetic over it is proved on the value: the type its initialiser gives, when that fits the
        declared type. Its initialiser's own records are reported where it is declared (D-149)."""
        d = self.table.get(nm)
        if d is None or d.kind != "const" or d.node is None or d.module != self.module or nm in self.const_stack:
            return None
        ce = tree_mod.child(d.node, "constExpr")
        if ce is None:
            return None
        self.const_stack.add(nm)
        before = len(self.records)
        try:
            t = self.infer(ce, Env())
        finally:
            del self.records[before:]
            self.const_stack.discard(nm)
        if not known(strip(t)):
            return None
        if tree_mod.child(d.node, "type") is not None and assignable(self.dc, t, d.alias)[0] is not True:
            return None
        # an unannotated constant's type is its initialiser's (4.2; D-54.4). The session keeps it: an import
        # holds a separate declaration, whose alias stays Unknown("inferred const").
        if tree_mod.child(d.node, "type") is None:
            d.alias = t
            self.note_inferred_const(d.module, d.name, t)
        return t

    def nonconstant_term(self, node) -> Optional[Tuple[Token, str]]:
        """A constant term `Type.member(args)` that is not a constant expression (4.2): only a constructor, an enum
        variant, a `Type.of` constructor of the `fortis` package and a derived `Type.default()` are (D-149)."""
        m = tree_mod.child(node, "memberName")
        tr = tree_mod.child(node, "typeRef")
        if m is None or tr is None or not any(t.text == "(" for t in direct_tokens(node)):
            return None
        mt = tokens(m)[0]
        d = self.decl_of(self.dc.type_of(tr))
        if mt.text == "of":
            ok = d is None or d.kind == "unknown" or d.module.startswith("fortis")
        elif mt.text == "default":
            ok = d is None or d.kind == "unknown" or any(n == "Default" for n, _ in d.derives)
        else:
            ok = False
        return None if ok else (mt, "".join(t.text for t in tokens(tr)) + "." + mt.text)

    def nonconstant_call(self, node) -> Optional[Token]:
        """The first call in an expression outside its `fn` literals, or None: aspect arguments are compile-time
        constants (11.8), so `@caps(Host.capabilities())` runs nothing (D-149)."""
        if node is None or isinstance(node, Token):
            return None
        n = name(node)
        if n == "lambda":
            return None
        if n == "methodCall":
            m = tree_mod.child(node, "memberName")
            return tokens(m)[0] if m is not None else tokens(node)[0]
        if n == "postfix" and direct_tokens(node) and direct_tokens(node)[0].text == "(":
            return direct_tokens(node)[0]
        if n == "primaryExpr" and tree_mod.child(node, "memberName") is not None and any(t.text == "(" for t in direct_tokens(node)):
            return tokens(tree_mod.child(node, "memberName"))[0]
        for c in children(node):
            r = self.nonconstant_call(c)
            if r is not None:
                return r
        return None

    def decl_context(self, label: str):
        """A constant, a default or a class constant is typed outside every body: a fresh activation under a label of
        its own, whose effects belong to no function's summary (D-149)."""
        self.facts, self.own, self.depth, self.eff = Facts(), Owners(), 0, Summary()
        self.reported, self.schema, self.caps_calls, self.discharges = set(), None, 0, []
        self.self_type, self.self_mode, self.ret_type, self.in_try, self.ctx, self.loop_labels = None, "self", None, False, [], []
        self.current_label = label

    def has_default(self, sig: Sig, pname: str) -> bool:
        if sig.node is None:
            return False
        for p in find_all(sig.node, "param"):
            ts = [t for t in direct_tokens(p) if t.text != "..."]
            if ts and ts[0].text == pname:
                return find(p, "constExpr") is not None
        return False

    def is_lambda(self, e) -> bool:
        """A lambda value, including one wrapped in parentheses (8.4).

        `(fn ...)(1)` is a call: a postfix on the group is not a lambda value. A bare lambda's
        first token is a lambda keyword; parentheses hide that token, so a group with an empty
        postfix is peeled and the inner expression is judged again."""
        node = e
        while node is not None:
            if name(node) == "lambda":
                return True
            if name(node) == "postfixExpr":
                if any(name(c) == "postfix" for c in children(node)):
                    return False
                node = tree_mod.child(node, "primaryExpr")
                continue
            if name(node) == "primaryExpr":
                dts = direct_tokens(node)
                if dts and dts[0].text == "(":
                    node = tree_mod.child(node, "expr")
                    continue
                return tree_mod.child(node, "lambda") is not None
            kids = [c for c in children(node) if name(c) is not None]
            ops = [c for c in children(node) if isinstance(c, Token) and c.kind not in ("NL", "NEWLINE")]
            if len(kids) == 1 and not ops:
                node = kids[0]
                continue
            ts = tokens(node)
            return bool(ts) and ts[0].text in ("fn", "pure", "noalloc", "async", "try") and find(node, "lambda") is not None
        return False

    def seed_expected_result(self, ret: Type, expected: Type, m: Dict[str, Type], drop_functions: bool):
        """Bind type parameters from the expected result (LR 6.10), then drop what must not stick.

        A binding that still names an unsolved type parameter (the caller's, not this body's) is dropped:
        the arguments infer it. A binding to a function type is dropped only when a lambda argument is
        pending (8.4), so the first lambda pins the closure and a second conflicts. With no lambda the
        expected function type stays: `List.of(move id)` annotated `List[fn(Int) Int]` keeps that `fn`.

        A result that is itself a type parameter is left unbound. The arguments fix it, and the caller
        checks that result against the expected type. Binding `T` to `print`'s `StringView` would reject
        `same(text)` for a Copyable `String` (6.6, 6.10). `List[T]` against `List[Int]` still binds `T`:
        the parameter is inside the result, not the result itself."""
        before = set(m)
        pat = self.subst(ret, m)
        if isinstance(strip(pat), TypeParam):
            return
        self.unify(pat, strip(expected), m)
        for k in list(m):
            if k in before:
                continue
            if any(p not in self.tparams_in_scope for p in self.tparams_of(m[k])):
                del m[k]
                continue
            if drop_functions:
                bound = m[k]
                while isinstance(bound, (OptionalT, LoanT)):
                    bound = bound.inner
                if isinstance(bound, FnT):
                    del m[k]

    def seed_dynamic_fn(self, pattern: Type, actual: Type, m: Dict[str, Type]):
        """Bind an unbound type parameter where the expected type already says `dynamic fn` (8.4).

        A non-dynamic expected type is left alone, so the first lambda still pins the parameter and a
        second, different lambda conflicts with it. An expected stack is not used: `List.of(1, 2).count()`
        must not take its element type from the chain's result. A bare type parameter is bound to the
        complete expected type, optional wrappers included, so `List[(dynamic fn)?]` keeps the `?`.
        Optional injection is then peeled one layer at a time (6.8): `List[dynamic fn]?` and
        `(List[dynamic fn]?)?` still seed the list."""
        pattern, actual = strip(pattern), strip(actual)
        if isinstance(pattern, TypeParam):
            if pattern.name not in m and contains_dynamic_fn(actual):
                a = concrete(strip(actual))
                if isinstance(a, Refined) and a.kind == "exact":
                    a = a.base
                m[pattern.name] = a
            return
        # present injection, one layer at a time, after the type parameter has kept its own optional
        if isinstance(pattern, OptionalT) and not isinstance(actual, OptionalT):
            self.seed_dynamic_fn(pattern.inner, actual, m)
            return
        if isinstance(actual, OptionalT) and not isinstance(pattern, OptionalT):
            self.seed_dynamic_fn(pattern, actual.inner, m)
            return
        if isinstance(pattern, Named) and isinstance(actual, Named) and pattern.name == actual.name:
            for pa, aa in zip(pattern.args, actual.args):
                self.seed_dynamic_fn(pa, aa, m)
        elif isinstance(pattern, OptionalT) and isinstance(actual, OptionalT):
            self.seed_dynamic_fn(pattern.inner, actual.inner, m)
        elif isinstance(pattern, LoanT):
            self.seed_dynamic_fn(pattern.inner, actual, m)
        elif isinstance(pattern, FnT) and isinstance(actual, FnT):
            for pp, ap in zip(pattern.params, actual.params):
                self.seed_dynamic_fn(pp, ap, m)
            self.seed_dynamic_fn(pattern.ret, actual.ret, m)
        elif isinstance(pattern, TupleT) and isinstance(actual, TupleT):
            for pp, ap in zip(pattern.items, actual.items):
                self.seed_dynamic_fn(pp, ap, m)
        elif isinstance(pattern, RecordT) and isinstance(actual, RecordT):
            am = dict(actual.fields)
            for k, ft in pattern.fields:
                if k in am:
                    self.seed_dynamic_fn(ft, am[k], m)

    def with_closure(self, want: Type, have: Type) -> Type:
        """Narrow a declared function type to the one lambda that initialises it (8.4).

        The declared signature stays; only an empty identity is filled in. `dynamic fn` is the boxed
        position and is not narrowed, so a later lambda is still assignable there."""
        if isinstance(want, LoanT):
            return LoanT(self.with_closure(want.inner, strip(have)), want.mode, want.origin)
        ws, hs = strip(want), strip(have)
        if isinstance(ws, OptionalT):
            return OptionalT(self.with_closure(ws.inner, hs.inner if isinstance(hs, OptionalT) else hs))
        if isinstance(ws, FnT) and not ws.dynamic and not ws.identity and isinstance(hs, FnT) and hs.identity and not hs.dynamic:
            return FnT(ws.params, ws.ret, ws.kind, ws.modifiers, ws.origin, False, hs.identity)
        if isinstance(ws, Named) and isinstance(hs, Named) and ws.name == hs.name and len(ws.args) == len(hs.args):
            return Named(ws.name, tuple(self.with_closure(a, b) for a, b in zip(ws.args, hs.args)), ws.package, ws.origin)
        if isinstance(ws, TupleT) and isinstance(hs, TupleT) and len(ws.items) == len(hs.items):
            return TupleT(tuple(self.with_closure(a, b) for a, b in zip(ws.items, hs.items)))
        if isinstance(ws, RecordT) and isinstance(hs, RecordT):
            hm = dict(hs.fields)
            return RecordT(tuple((k, self.with_closure(ft, hm[k]) if k in hm else ft) for k, ft in ws.fields))
        return want

    def _field_pin_key(self, decl: Decl, field: str, path: str):
        """The pin of one closure position: declaring package and module, the class, the field and the path."""
        return (decl.type_identity or "", decl.name, field, path)

    def _overlay_pins(self, decl: Decl, field: str, slot: Type, path: str) -> Type:
        """`slot` with each empty closure identity replaced by the lambda this field already holds (8.4)."""
        slot = strip(slot)
        if isinstance(slot, OptionalT):
            return OptionalT(self._overlay_pins(decl, field, slot.inner, path))
        if isinstance(slot, LoanT):
            return LoanT(self._overlay_pins(decl, field, slot.inner, path), slot.mode, slot.origin)
        if isinstance(slot, FnT):
            if slot.dynamic or slot.identity:
                return slot
            ident = self.field_closures.get(self._field_pin_key(decl, field, path))
            if not ident:
                return slot
            return FnT(slot.params, slot.ret, slot.kind, slot.modifiers, slot.origin, False, ident)
        if isinstance(slot, Named):
            return Named(slot.name, tuple(self._overlay_pins(decl, field, a, f"{path}/{i}") for i, a in enumerate(slot.args)), slot.package, slot.origin)
        if isinstance(slot, TupleT):
            return TupleT(tuple(self._overlay_pins(decl, field, a, f"{path}/{i}") for i, a in enumerate(slot.items)))
        if isinstance(slot, RecordT):
            return RecordT(tuple((k, self._overlay_pins(decl, field, ft, f"{path}/{k}")) for k, ft in slot.fields))
        return slot

    def _peel_expected(self, expected):
        """The expected type with every present-injection optional removed (6.8)."""
        exp = strip(expected) if expected is not None else None
        while isinstance(exp, OptionalT):
            exp = strip(exp.inner)
        return exp

    def note_field_closure(self, decl: Decl, field: str, field_ty: Type, value_ty: Type, tok: Token):
        """A field whose type contains a closure holds one lambda at that position for the whole check (8.4)."""
        self._pin_field_closure(decl, field, field_ty, value_ty, tok, "")

    def _pin_field_closure(self, decl: Decl, field: str, slot: Type, value: Type, tok: Token, path: str):
        """Pin each non-dynamic closure inside `slot` to the lambda in `value` (8.4).

        Optional injection, a loan, and the elements of a collection, tuple or record are the same
        position as their inner closure. The first identity wins. A second, different lambda is rejected.
        `dynamic fn` is not pinned: it is the boxed heterogeneous position."""
        slot, value = strip(slot), strip(value)
        if isinstance(slot, OptionalT):
            inner = value.inner if isinstance(value, OptionalT) else value
            self._pin_field_closure(decl, field, slot.inner, inner, tok, path)
            return
        if isinstance(slot, LoanT):
            inner = value.inner if isinstance(value, LoanT) else value
            self._pin_field_closure(decl, field, slot.inner, inner, tok, path)
            return
        if isinstance(slot, FnT):
            if slot.dynamic or not isinstance(value, FnT) or not value.identity or value.dynamic:
                return
            key = self._field_pin_key(decl, field, path)
            prev = self.field_closures.get(key)
            if prev is None:
                self.field_closures[key] = value.identity
            elif prev != value.identity:
                shown = value.show()
                self.typ("E-TYP-040", f"field `{field}` has type {shown}, expected {slot.show()}: {CLOSURE_WHY}", tok, slot.show(), shown, field, "LANG-08-04")
            return
        if isinstance(slot, Named) and isinstance(value, Named) and slot.name == value.name:
            for i, (a, b) in enumerate(zip(slot.args, value.args)):
                self._pin_field_closure(decl, field, a, b, tok, f"{path}/{i}")
            return
        if isinstance(slot, TupleT) and isinstance(value, TupleT):
            for i, (a, b) in enumerate(zip(slot.items, value.items)):
                self._pin_field_closure(decl, field, a, b, tok, f"{path}/{i}")
            return
        if isinstance(slot, RecordT) and isinstance(value, RecordT):
            vm = dict(value.fields)
            for k, ft in slot.fields:
                if k in vm:
                    self._pin_field_closure(decl, field, ft, vm[k], tok, f"{path}/{k}")

    def note_field_assign(self, pl, value_ty: Type, tok: Token, env: Env):
        """`p.field = fn …`: the field's class, not the binding, owns the one closure type (8.4)."""
        members = [t for t in tokens(pl) if t.kind == "LOWER"]
        if len(members) < 2:
            return
        field = members[-1].text
        if members[0].text == "self":
            owner: Type = self.self_type or Unknown("self")
        else:
            owner = env.get(members[0].text) or Unknown("field owner")
        owner = strip(owner)
        d = self.decl_of(owner)
        for mid in members[1:-1]:
            if d is None or mid.text not in d.fields:
                return
            mm = self.recv_subst(owner, d) if isinstance(owner, Named) else {}
            owner = strip(self.subst(d.fields[mid.text][0], mm))
            d = self.decl_of(owner)
        if d is None or field not in d.fields:
            return
        mm = self.recv_subst(owner, d) if isinstance(owner, Named) else {}
        self.note_field_closure(d, field, self.subst(d.fields[field][0], mm), value_ty, tok)

    # ---- assignability ------------------------------------------------------------------
    def enum_operand(self, t: Type) -> Optional[Type]:
        """The enum a comparison's left operand has (an optional one's present type), which the right operand is expected
        to have, so a bare variant there is read as that enum's (9.7, X-345); None for any other type."""
        s = strip(t)
        if isinstance(s, OptionalT):
            s = strip(s.inner)
        d = self.decl_of(s) if known(s) else None
        return s if d is not None and d.kind == "enum" else None

    def widened(self, t: Type) -> Type:
        """The type an unannotated binding takes from its initialiser (5.1): a computed integer interval or a literal's exact
        value widens to its base type, which stays a fact; an inferred lambda result takes the same (8.2, X-345)."""
        t = concrete(t)
        if isinstance(t, Refined) and t.kind in ("exact", "range") and self.is_int(t):
            return t.base
        return t

    def stamp_literal(self, t: Type) -> Type:
        """A core literal names the standard type. `10ms` is `Duration` of `fortis.time`, the origin a signature
        parsed against the prelude's copy carries. An unstamped name would be a different type."""
        if isinstance(t, Named) and not t.origin and not t.package:
            origin = self.dc.core_module(t.name)
            if origin:
                return Named(t.name, t.args, "", origin)
        return t

    def expect(self, have: Type, want: Type, tok: Token, what: str):
        if isinstance(want, DynamicT) and isinstance(strip(have), (Named, Refined)):
            self.eff.add("alloc", "proved", tok)          # a class value made an owned `dynamic I` is boxed (Appendix C.3; X-117)
        ok, why = assignable(self.dc, have, want)
        hs, ws = strip(have), strip(want)
        if ok is None and isinstance(ws, Refined) and self.is_int(ws) and self.is_int(hs):
            hi_, wi = self.interval_of(hs), self.interval_of(ws)
            if hi_ is not None and wi is not None:
                if wi[0] <= hi_[0] and hi_[1] <= wi[1]:
                    return
                status = iv.classify(hi_, wi)
                self.ref("E-REF-001", f"{what} has interval {self.show_iv(hi_)}, which is not within {ws.show()} ({status}); validate it or widen the declared type", tok, status, ws.show(), self.show_iv(hi_))
                return
        if ok is None and isinstance(ws, Refined) and ws.kind in ("length", "scalars"):
            if isinstance(hs, Refined) and hs.kind == ws.kind and (hs.base == ws.base or is_strlit(hs)) and (ws.lo is None or (hs.lo is not None and hs.lo >= ws.lo)) and (ws.hi is None or (hs.hi is not None and hs.hi <= ws.hi)):
                return
            self.ref("E-REF-001", f"{what} is not proved to satisfy {ws.show()}: the domain has no {ws.kind} facts (unknown); validate it", tok, "unknown", ws.show(), "?")
            return
        if ok is False:
            shown = "a string literal" if is_strlit(have) else have.show()
            if isinstance(have, Refined) and have.kind == "exact" and "interval" in why:
                self.ref("E-REF-001", f"literal {have.lo} does not fit {want.show()}: {why}", tok, "violated", want.show(), f"[{have.lo}, {have.hi}]")
                return
            rule = "LANG-08-04" if closure_conflict(have, want) else ("LANG-06-08" if isinstance(strip(have), OptionalT) or isinstance(strip(want), OptionalT) else "LANG-06-01")
            self.typ("E-TYP-040", f"{what} has type {shown}, expected {want.show()}: {why}", tok, want.show(), shown, what, rule)

    def unify_branches(self, types: List[Tuple[Type, Token]], what: str, expected: Optional[Type] = None) -> Type:
        result: Optional[Type] = None
        for t, tok in types:
            if not known(t) or t == NEVER:
                continue
            if t == INTERP:
                t = STRING
            if result is None:
                result = t                  # STRLIT stays literal: the context decides view or String (6.6)
                continue
            if t == result:
                continue
            if is_strlit(result) and (t in (STRING, STRING_VIEW) or is_strlit(t)):
                result = t if not is_strlit(t) else result
                continue
            if is_strlit(t) and result in (STRING, STRING_VIEW):
                continue
            # present injection at a join (6.8): T and null unify to T?, T and T? to T?
            if t == OptionalT(NEVER) and not isinstance(strip(result), OptionalT):
                result = OptionalT(result)
                continue
            if result == OptionalT(NEVER) and not isinstance(strip(t), OptionalT):
                result = OptionalT(t)
                continue
            if isinstance(strip(t), OptionalT) and not isinstance(strip(result), OptionalT) and assignable(self.dc, result, strip(t).inner)[0]:
                result = t
                continue
            ok, _ = assignable(self.dc, t, result)
            if ok:
                continue
            ok2, _ = assignable(self.dc, result, t)
            if ok2:
                result = t
                continue
            if ok is False:
                if closure_conflict(t, result) and dynamic_covers(expected, t) and dynamic_covers(expected, result):
                    continue                  # `dynamic fn` is the one position two lambdas may join (8.4)
                rule = "LANG-08-04" if closure_conflict(t, result) else "LANG-06-01"
                self.typ("E-TYP-040", f"{what} yields {t.show()} where the other branches yield {result.show()}", tok, result.show(), t.show(), what, rule)
        return result if result is not None else Unknown("branches")

    # ---- expressions ------------------------------------------------------------------
    def infer(self, node, env: Env, expected=None):
        ops = self.test_region(node)
        if ops:
            # unproved `Int` arithmetic in a test body is a checked region with an implied `or fail` (14, 6.4; X-345)
            self.regions.append({"ops": ops, "checked": []})
            try:
                t = self._infer(node, env, expected)
            finally:
                region = self.regions.pop()
            if region["checked"]:
                first = region["checked"][0]
                self.eff.add("fail", "proved", first)
                self.unwind_point(first, "or fail", env)
                if self.defer_depth:
                    self.def_err("E-DEF-001", "failing arithmetic inside a deferred block: there is no outcome to propagate to (12.1)", first, "or fail")
                self.implied_regions.add(min(ops))
        else:
            t = self._infer(node, env, expected)
        if self.test_body and not isinstance(node, Token) and name(node) in ("postfixExpr", "markerExpr") and id(node) not in self.consumed_ops:
            t = self.implied_or_fail(node, t, env)
        if isinstance(t, Unknown) and t.why == "generic result" and expected is None and not self.branch_target and not isinstance(node, Token):
            # a call whose arguments do not fix its type arguments, with no expected type to fix them (6.10): a
            # proved inference failure, repaired by annotating the receiving binding
            # (each enclosing expression node sees the same result; the record is kept once per line and message)
            tok = tokens(node)[0]
            self.typ("E-TYP-160", "the type arguments of this call are not fixed by its arguments; annotate the receiving binding (6.10)", tok, "an annotated binding", "uninferred type arguments", tok.text, "LANG-06-11")
        if self.node_types is not None and not isinstance(node, Token):
            self.node_types[id(node)] = t          # per-expression types for the experimental lowering (DEL-25)
        return t

    def _infer(self, node, env: Env, expected: Optional[Type] = None) -> Type:
        if node is None or isinstance(node, Token):
            return Unknown()
        n = name(node)
        kids = children(node)
        if n == "constExpr" and len(kids) > 1:
            return self.const_arith(kids, env)
        if n in ("constExpr", "expr", "exprStmt"):
            return self.infer(kids[0], env, expected)
        if n == "constTerm":
            return self.const_term(node, env, expected)
        if n == "outcomeExpr":
            return self.outcome(node, env, expected)
        if n == "coalesceExpr":
            if len(kids) == 1:
                return self.infer(kids[0], env, expected)
            left = self.infer(kids[0], env)
            after_left = self.facts.copy()
            skip_own = self.own.copy()                     # the fallback runs only when the left is null (5.7)
            right = self.infer(kids[2], env, expected=left.inner if isinstance(strip(left), OptionalT) else None)
            self.facts = after_left.join(self.facts)       # D-76: the fallback runs only when the left is null
            self.join_conditional(skip_own)
            ls = strip(left)
            if known(ls) and not isinstance(ls, OptionalT):
                self.typ("E-TYP-040", f"`??` needs an optional left operand, got {ls.show()}", tokens(kids[0])[0], "T?", ls.show(), "??", "LANG-06-08")
                return Unknown()
            if isinstance(ls, OptionalT):
                self.expect(right, ls.inner, tokens(kids[2])[0], "`??` fallback")
                return self.coalesce_result(ls.inner, right, kids[0], kids[2], env)
            return Unknown()
        if n in ("orExpr", "andExpr"):
            if len(kids) == 1:
                return self.infer(kids[0], env, expected)
            cur = self.facts.copy()
            exits: List[Facts] = []                  # the state after each operand: the expression may end there
            first = True
            for k in kids[::2]:
                pre = cur.copy()
                self.facts = cur
                # the first operand always runs; each later one is skipped when an earlier operand decides (5.7)
                skip_own = None if first else self.own.copy()
                t = self.infer(k, env)
                cur = self.facts
                exits.append(cur.copy())
                if known(strip(t)) and base_name(strip(t)) != "Bool":
                    self.typ("E-TYP-040", f"`{kids[1].text}` operand has type {t.show()}, expected Bool", tokens(k)[0], "Bool", t.show(), kids[1].text)
                if n == "andExpr":
                    before = len(self.records)
                    self.facts = pre                        # narrowing re-reads the operand from its own start (D-73)
                    cur, _, _ = self.narrow(k, env)         # `a != null && e`: a is present inside e (6.8)
                    del self.records[before:]               # narrowing re-reads the operand; its diagnostics were reported above
                if skip_own is not None:
                    self.join_conditional(skip_own)         # a consume on the skipped operand is one path only (7.4, 10.1)
                first = False
            # D-73: the operands' effects stay (an `editable` argument in the right operand invalidates its place); the
            # state after the expression is the join of the states where it can end
            self.facts = Facts.join_all(exits)
            return BOOL
        if n in ("bitOrExpr", "bitXorExpr", "bitAndExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr"):
            if len(kids) == 1:
                return self.infer(kids[0], env, expected)
            t = self.infer(kids[0], env)
            for i in range(1, len(kids), 2):
                r = self.infer(kids[i + 1], env)
                t = self.arith(kids[i], t, r)
            return t
        if n in ("equalityExpr", "relationalExpr"):
            if len(kids) == 1:
                return self.infer(kids[0], env, expected)
            l = self.infer(kids[0], env)
            r = self.infer(kids[2], env, expected=self.enum_operand(l))   # `s == Error`: the left operand's enum (9.7, X-345)
            self.compare(kids[1], l, r)
            return BOOL
        if n == "rangeExpr":
            for k in kids[::2]:
                t = self.infer(k, env)
                if known(strip(t)) and int_bounds(strip(t)) is None:
                    self.typ("E-TYP-040", f"range bound has type {t.show()}, expected an integer", tokens(k)[0], "Int", t.show(), "range")
            return Unknown("range")
        if n == "unaryExpr":
            if isinstance(kids[0], Token):
                op = kids[0].text
                t = self.infer(kids[1], env, expected if op == "-" else None)
                s = strip(t)
                if op == "!":
                    if known(s) and base_name(s) != "Bool":
                        self.typ("E-TYP-040", f"`!` operand has type {t.show()}, expected Bool", tokens(kids[1])[0], "Bool", t.show(), "!")
                    return BOOL
                if op == "-":
                    return self.negated_type(kids[0], kids[1], t)
                if op == "await":
                    if self.defer_depth:
                        self.def_err("E-DEF-001", "`await` inside a deferred block: `defer await` is not in v1 (12.1)", kids[0], "await")
                    for gname, gdepth, gtok in self.guards:
                        if gdepth <= self.depth and env.lookup(gname) is not None and not self.own.is_moved(gname):
                            self.rec("E-ASY-002", f"the MutexGuard `{gname}` (line {gtok.line}) is live across this await: an exclusive guard may not be held across a suspension (12.3)", "LANG-12-06", kids[0], {"offending_statement": "await", "rule_reference": "12.3"})
                    self.obs_exit("cancellation", kids[0], f"the await at line {kids[0].line} (a cancellation point)")
                    self.unwind_point(kids[0], "await", env, keep=self.path_of_expr(kids[1]))   # the awaited value is consumed here (X-123)
                    self.eff.add("suspend", "proved", kids[0])
                    # the future awaited here lends its temporaries no further, unless what it gives can still carry
                    # them (a future, a task, a loan); another future's stay (D-31)
                    st = strip(t)
                    res = st.args[0] if isinstance(st, Named) and st.name in ("Future", "Task") and st.args else None
                    onward = res is not None and self.carries_onward(res)
                    if not onward:
                        ots = tokens(kids[1])
                        lo, hi = (ots[0].line, ots[0].col), (ots[-1].line, ots[-1].col)
                        self.carried_temps = [c for c in self.carried_temps if not lo <= (c[0].line, c[0].col) <= hi]
                    for l in self.carried:               # the loans a future carried end when it is awaited (12.1)
                        if l in self.own.loans and not (isinstance(strip(t), Named) and strip(t).name == "Future" and strip(t).args and self.loan_carrying(strip(t).args[0])):
                            self.own.loans.remove(l)
                    self.carried = [l for l in self.carried if l in self.own.loans]
                    if isinstance(s, Named) and s.name in ("Future", "Task") and s.args:
                        ap = self.path_of_expr(kids[1])
                        if ap is not None:
                            # awaiting consumes the future (12.3) or the task handle (12.5): the place must own it, and is
                            # moved (a loan or a Shared owns nothing to consume; a second await is a use after move; D-33)
                            h = self.own.holders.get(root_of(ap))
                            if h is not None and root_of(h.owner) != root_of(ap):
                                self.own_err("E-OWN-001", f"`await {ap}`: `{root_of(ap)}` holds a loan of `{h.owner}` and owns no future to consume (12.3)",
                                             tokens(kids[1])[0], ap, "await of a loan", "LANG-07-01")
                            else:
                                self.use_place(ap, tokens(kids[1])[0], "move", env)
                            self.own.move(ap)
                        self.consume(ap)
                        return s.args[0]
                    if known(s):
                        self.typ("E-TYP-040", f"`await` on {t.show()}, which is not a Future or a Task", tokens(kids[1])[0], "Future[T] or Task[T]", t.show(), "await")
                    return Unknown("await")
            return self.infer(kids[0], env, expected)
        if n == "markerExpr":
            return self.marker(node, env)
        if n == "postfixExpr":
            return self.postfix_chain(node, env, expected)
        if n == "primaryExpr":
            return self.primary(node, env, expected)
        if n == "literal":
            tk = tokens(node)[0]
            if tk.kind == "STR" and "${" in tk.text:
                self.check_interpolation(tk, env)
                self.eff.add("alloc", "proved", tk)      # interpolation allocates a String (6.6)
            return self.stamp_literal(literal_type(tk))
        if n == "place":
            return self.place_type(node, env)
        if n == "lambda":
            return self.lambda_type(node, env, expected)
        if n == "matchExpr":
            return self.match(node, env, expected, as_statement=False)
        if n == "ifExpr":
            return self.if_expr(node, env, expected)
        if n == "block":
            self.walk_block(node, env)
            return VOID
        if n == "tupleLit":
            es = [c for c in kids if name(c) == "expr"]
            exp = self._peel_expected(expected)
            items = []
            for i, e in enumerate(es):
                child = exp.items[i] if isinstance(exp, TupleT) and i < len(exp.items) else None
                items.append(self.infer(e, env, expected=child))
                self.settle_element()          # a move in this element is consumed before the next one (D-159)
            return TupleT(tuple(items))
        if n == "recordLit":
            fields = []
            ks = [tokens(k)[0] for k in kids if name(k) == "ident"] or [k for k in kids if isinstance(k, Token) and k.kind in ("LOWER", "UPPER", "KW")]
            es = [c for c in kids if name(c) == "expr"]
            if any(k.kind == "UPPER" for k in ks):
                for e in es:
                    self.infer(e, env)
                    self.settle_element()
                return Unknown("variant map")
            exp = self._peel_expected(expected)
            em = dict(exp.fields) if isinstance(exp, RecordT) else {}
            for k, e in zip(ks, es):
                fields.append((k.text, self.infer(e, env, expected=em.get(k.text))))
                self.settle_element()
            return RecordT(tuple(fields))
        if n == "argList":
            # the arguments of a call the checker could not resolve: every expression in them is typed with no context,
            # which proves nothing about what it expects (D-304)
            self.unresolved_args += 1
            for a in find_all(node, "expr"):
                self.infer(a, env)
            self.unresolved_args -= 1
            return VOID
        for c in kids:
            if not isinstance(c, Token):
                self.infer(c, env)
        return Unknown(n or "token")

    def negated(self, op: Token, operand, env: Env, expected) -> Type:
        return self.negated_type(op, operand, self.infer(operand, env, expected))

    def negated_type(self, op: Token, operand, t: Type) -> Type:
        """`-e` (5.6): a numeric operand, its exact value or interval negated."""
        s = strip(t)
        if known(s) and int_bounds(s) is None and base_name(s) not in ("Float", "Float32", "Decimal", "Duration"):
            self.typ("E-TYP-040", f"unary `-` on {t.show()}", tokens(operand)[0], "a numeric type", t.show(), "-")
        if isinstance(s, Refined) and s.kind == "exact":
            return negate(s)
        i = self.interval_of(s)
        if i is not None:
            base = s.base if isinstance(s, Refined) else s
            return self.int_arith(Token("PUNCT", "-", op.line, op.col), base, (0, 0), i)
        return t

    def arith(self, op: Token, l: Type, r: Type) -> Type:
        ls, rs = strip(l), strip(r)
        if not known(ls) or not known(rs):
            return Unknown("arithmetic")
        lb, rb = base_name(ls), base_name(rs)
        if lb in INT_TYPES and rb in INT_TYPES:
            if lb != rb:
                self.typ("E-TYP-040", f"`{op.text}` on {lb} and {rb}: distinct integer types do not mix", op, lb, rb, op.text)
                return Unknown()
            return self.int_arith(op, Named(lb), self.interval_of(ls), self.interval_of(rs))
        if op.text in ("+", "-", "*", "/") and lb == rb and lb in ("Float", "Float32"):
            return Named(lb)
        self.typ("E-TYP-040", f"`{op.text}` is not defined on {ls.show()} and {rs.show()}" + (" (use Decimal.add and friends)" if "Decimal" in (lb, rb) else " (use Duration.add and friends)" if "Duration" in (lb, rb) else " (text is built with text.Builder)" if lb in ("String", "StringView") else ""),
                 op, "integer or float operands", f"{ls.show()}, {rs.show()}", op.text)
        return Unknown()

    def int_arith(self, op: Token, base: Named, a: Optional[iv.Interval], b: Optional[iv.Interval]) -> Type:
        """Proof-required integer operators (6.4): the result is the base refined to the computed interval, or E-REF-002."""
        if a is None or b is None:
            return base
        full = INT_TYPES[base.name]
        width = {"Int8": 8, "UInt8": 8, "Byte": 8, "Int16": 16, "UInt16": 16, "Int32": 32, "UInt32": 32}.get(base.name, 64)
        if op.text in ("&", "|", "^"):
            return base                    # bitwise operators are total (6.4 lists only + - * / % << >> as proof-required)
        checked = {"+": "Int.add", "-": "Int.sub", "*": "Int.mul", "/": "Int.div", "%": "Int.rem", "<<": "Int.shiftLeft"}.get(op.text, "a checked operation")
        r = iv.arith(op.text, a, b, width)
        if self.regions and (op.line, op.col) in self.regions[-1]["ops"] and base.name in ("Int", "Int64") and op.text in CHECKED_OPS and not self.proved(op, base, a, b, r):
            # the checked operation (6.4, X-345): it fails as `Int.add` and its family fail, and its success value has the
            # operation's interval clipped to the type's range, as theirs does (6.3)
            self.regions[-1]["checked"].append(op)
            self.checked_ops.add((op.line, op.col))
            clipped = iv.meet(r, full) if r is not None else None
            return Refined(base, clipped[0], clipped[1]) if clipped is not None else base
        if base.name in ("Int", "Int64") and op.text in CHECKED_OPS:
            form = "-x" if op.text == "-" and a == (0, 0) else f"a {op.text} b"
            checked = f"{checked}, or write the operator as the checked operation: `{form} or fail` (or `or recover`, `catch`)"
        elif base.name not in ("Int", "Int64"):
            # the sized integer types have no checked family (6.4): only a proof makes the operator safe
            checked = f"a refined type or a test that proves it ({base.name} has no checked operation)"
        else:
            checked = f"a refined type or a test that proves it (`{op.text}` has no checked operation)"
        if self.regions and (op.line, op.col) in self.regions[-1]["ops"] and not self.proved(op, base, a, b, r):
            self.regions[-1]["refused"] = True     # rejected below: the region's handler adds no record of its own (6.4)
        if r is None:
            why = "the divisor may be zero" if op.text in ("/", "%") else f"the shift amount is not proved within [0, {width - 1}]"
            status = "violated" if (op.text in ("/", "%") and b == (0, 0)) else "unproved"
            self.ref("E-REF-002", f"`{op.text}` is not proved safe: {why} (operands {self.show_iv(a)}, {self.show_iv(b)}); use {checked}", op, status, f"divisor excluding 0" if op.text in ("/", "%") else f"[0, {width - 1}]", self.show_iv(b))
            return base
        if op.text == "/" and a[0] <= full[0] and b[0] <= -1 <= b[1]:
            self.ref("E-REF-002", f"`/` is not proved safe: MIN / -1 cannot be excluded (operands {self.show_iv(a)}, {self.show_iv(b)}); use {checked}", op, "unproved", self.show_iv(full), self.show_iv(a))
            return base
        if r[0] < full[0] or r[1] > full[1]:
            status = "violated" if (r[1] < full[0] or r[0] > full[1]) else "unproved"
            self.ref("E-REF-002", f"`{op.text}` may overflow {base.name}: operands {self.show_iv(a)} and {self.show_iv(b)} give {self.show_iv(r)}; use {checked}", op, status, self.show_iv(full), self.show_iv(r))
            return base
        return Refined(base, r[0], r[1])

    @staticmethod
    def proved(op: Token, base: Named, a: iv.Interval, b: iv.Interval, r: Optional[iv.Interval]) -> bool:
        """Whether the interval rules prove an integer operation safe (6.4): no division by zero, no out-of-range shift, no
        `MIN / -1` and no overflow."""
        full = INT_TYPES[base.name]
        if r is None:
            return False
        if op.text == "/" and a[0] <= full[0] and b[0] <= -1 <= b[1]:
            return False
        return full[0] <= r[0] and r[1] <= full[1]

    def compare(self, op: Token, l: Type, r: Type):
        ls, rs = concrete(strip(l)), concrete(strip(r))
        if not known(ls) or not known(rs):
            return
        if op.text in ("==", "!="):
            if isinstance(ls, OptionalT) and rs == OptionalT(NEVER) or isinstance(rs, OptionalT) and ls == OptionalT(NEVER):
                return              # null test (6.8)
            a = ls.inner if isinstance(ls, OptionalT) else ls
            b = rs.inner if isinstance(rs, OptionalT) else rs
            if base_name(a) in ("Float", "Float32") and base_name(b) == base_name(a):
                return              # IEEE equality (6.5)
            ok, _ = assignable(self.dc, a, b)
            ok2, _ = assignable(self.dc, b, a)
            if ok is False and ok2 is False:
                self.typ("E-TYP-040", f"`{op.text}` compares {ls.show()} with {rs.show()}", op, ls.show(), rs.show(), op.text, "LANG-06-08" if isinstance(ls, OptionalT) != isinstance(rs, OptionalT) else "LANG-06-01")
                return
            if not self.dc.implements(a, "Equatable"):
                self.typ("E-TYP-040", f"`{op.text}` needs Equatable; {a.show()} does not implement it", op, "Equatable", a.show(), op.text)
            return
        a, b = ls, rs
        if base_name(a) in ("Float", "Float32") and base_name(b) == base_name(a):
            return
        ok, _ = assignable(self.dc, a, b)
        ok2, _ = assignable(self.dc, b, a)
        if ok is False and ok2 is False:
            self.typ("E-TYP-040", f"`{op.text}` compares {a.show()} with {b.show()}", op, a.show(), b.show(), op.text)
            return
        if not self.dc.implements(a, "Comparable"):
            self.typ("E-TYP-040", f"`{op.text}` needs Comparable; {a.show()} does not implement it", op, "Comparable", a.show(), op.text)

    def outcome(self, node, env: Env, expected) -> Type:
        kids = children(node)
        words = [k.text for k in kids if isinstance(k, Token)]
        # the left operand of an outcome operator, when it is integer arithmetic, is a checked region: an `Int` operator
        # in it that the interval rules cannot prove safe is the checked operation, and the region fails as it fails
        # (6.4, X-345)
        ops = region_operators(kids[0]) if len(kids) > 1 else set()
        if ops:
            self.regions.append({"ops": ops, "checked": []})
        if len(kids) > 1:
            self.mark_consumed(kids[0])             # `or` and `catch` consume the outcome themselves (14)
        left = self.infer(kids[0], env, expected if len(kids) == 1 else None)
        region = self.regions.pop() if ops else None
        if len(kids) == 1:
            return left
        if region is not None and region["checked"]:
            left = Named("Outcome", (left,))
        ls = strip(left)
        inner: Type = Unknown("outcome")
        if isinstance(ls, Named) and ls.name == "Outcome" and ls.args:
            inner = ls.args[0]
        elif known(ls):
            if region is not None and region.get("refused"):
                pass                                # an operator the region could not check is E-REF-002 already (6.4)
            elif region is not None:
                self.typ("E-TYP-040", f"`{' '.join(words[:2])}` needs an Outcome left operand: this arithmetic is proved safe ({ls.show()}) and cannot fail, so drop `{' '.join(words[:2])}`", tokens(kids[0])[0], "Outcome[T]", ls.show(), words[0], "LANG-06-08")
            else:
                self.typ("E-TYP-040", f"`{' '.join(words[:2])}` needs an Outcome left operand, got {ls.show()}", tokens(kids[0])[0], "Outcome[T]", ls.show(), words[0], "LANG-06-08")
            return Unknown()
        self.consume(self.path_of_expr(kids[0]))
        if "fail" in words:
            if self.defer_depth:
                self.def_err("E-DEF-001", "`or fail` inside a deferred block: there is no outcome to propagate to (12.1)", next(k for k in kids if isinstance(k, Token) and k.text == "or"), "or fail")
            self.eff.add("fail", "proved", kids[0] if isinstance(kids[0], Token) else tokens(kids[0])[0])
            ortok = next(k for k in kids if isinstance(k, Token) and k.text == "or")
            self.unwind_point(ortok, "or fail", env)
            self.obs_exit("failure", ortok, f"the `or fail` at line {ortok.line}")
            if not self.in_try:
                self.rec("E-ERR-001", "`or fail` outside a `try` function", "LANG-10-01", next(k for k in kids if isinstance(k, Token) and k.text == "or"), {"obligation_origin": f"{tokens(kids[0])[0].line}:{tokens(kids[0])[0].col}", "unconsumed_path": "or fail"})
            return inner
        # the replacement value unifies with the expected type when the success value injects into it (Money into Money?)
        target = inner
        es = strip(expected) if expected is not None else None
        if es is not None and known(es) and known(inner) and assignable(self.dc, inner, es)[0]:
            target = es
        after_left = self.facts.copy()          # the success path's state (D-76)
        skip_own = self.own.copy()              # the handler runs only on failure (10.2); the outcome itself is already consumed
        if "recover" in words:
            self.eff.add("recovers", "proved", tokens(kids[-1])[0])   # `or recover` discharges without a report (10.2)
            r = self.infer(kids[-1], env, expected=target)
            self.facts = after_left.join(self.facts)       # D-76: the replacement runs only on failure
            self.join_conditional(skip_own)
            if known(target):
                if r == OptionalT(NEVER) and not isinstance(strip(target), OptionalT):
                    return OptionalT(target)           # `e or recover null`: the expression is T? (6.8)
                self.expect(r, target, tokens(kids[-1])[0], "`or recover` value")
                self.loan_into_owned(r, target, tokens(kids[-1])[0], "the `or recover` value")
            return target
        # catch e { ... }
        ident = next(k for k in kids if isinstance(k, Token) and k.kind == "LOWER")
        e = Env(env)
        e.set(ident.text, FAILURE, kind="catch")
        self.ctx.append("catch")
        self.arm_types.append([])
        self.exit_frames.append((self.depth, []))
        self.branch_expected.append(target)
        blk = find(node, "block")
        self.own.obligations[ident.text] = (ident, self.depth + 1)       # the bound failure is owed inside the block
        self.catch_bound.append(ident.text)
        if not self.terminates(blk):
            self.obl_err("E-OBL-002", "a catch block must end every path in `recover`, `return`, `fail` or a Never call", next(k for k in kids if isinstance(k, Token) and k.text == "catch"), f"{ident.line}:{ident.col}", ident.text, "LANG-10-01")
        self.walk_block(blk, e)
        _, recovered = self.exit_frames.pop()
        # D-76: the handler runs only on failure, and its value leaves at each `recover`
        self.facts = Facts.join_all([after_left] + recovered + ([] if self.terminates(blk) else [self.facts]))
        self.branch_expected.pop()
        self.catch_bound.pop()
        self.reported.discard(ident.text)
        self.own.obligations.pop(ident.text, None)
        self.own.reinit(ident.text)
        # A handler that leaves the function (`return`, `fail`) does not continue into this expression,
        # so its consume is not a path of the value (10.2). A `recover` does continue, and so does a
        # handler that fails to terminate. The catch binding itself was popped above.
        if self.terminates(blk) and not recovered:
            self.own = skip_own
        else:
            self.join_conditional(skip_own)
        rec = self.arm_types.pop()
        self.ctx.pop()
        if known(target) and any(t == OptionalT(NEVER) for t, _ in rec) and not isinstance(strip(target), OptionalT):
            target = OptionalT(target)      # `recover null`: the expression is T? (6.8)
        for t, tok in rec:
            if known(target):
                self.expect(t, target, tok, "`recover` value")
                self.loan_into_owned(t, target, tok, "the `recover` value")
        return target                       # plug-in: every path ends in recover/return/fail (E-OBL-002, DEL-17)

    def marker(self, node, env: Env) -> Type:
        kids = children(node)
        pl = find(node, "place")
        self.in_marker = True
        t = self.place_type(pl, env)
        self.in_marker = False
        mk = tokens(kids[0])[0].text
        mtok = tokens(kids[0])[0]
        members = [k for k in children(pl) if name(k) == "memberName"]
        call_follows = len(kids) > 2 and isinstance(kids[2], Token) and kids[2].text == "(" and members
        path = self.path_of_place(pl) if not call_follows else ".".join([direct_tokens(pl)[0].text] + [tokens(k)[0].text for k in members[:-1]])
        self.use_place(path, mtok, "copy" if mk == "copy" else mk, env)
        if mk == "copy":
            # `copy v` of a binding that holds a loan copies the value it lends, narrowed or not (7.5; D-341)
            declared = self.declared_type(pl, env) if not call_follows else Unknown()
            lent_decl = self.lent(declared)
            base = lent_decl if lent_decl is not None else strip(declared)
            if known(base) and not self.dc.is_trivial(base):
                self.eff.add("alloc", "proved", mtok)          # copy of a heap-owning value allocates (6.11)
            if known(base) and not self.dc.is_copyable(base):
                self.copy_refused.add((mtok.line, mtok.col))
                self.own_err("E-OWN-002", f"`copy {path}`: {base.show()} is not Copyable (6.11)", mtok, path, "copy of a non-copyable place", "LANG-07-08" if isinstance(base, Named) and base.name in ("Shared", "Weak") else "LANG-07-01")
        if len(kids) > 2:
            self.receiver = (path, mk)               # the marker applies to the receiver of the call that follows (7.6)
        elif self.arg_mode is not None:
            mode = self.arg_mode
            ok = self.use_ok
            if mk == "move" or mk == "copy":
                if mode == "editable":
                    self.own_err("E-OWN-001", f"an `editable` parameter takes `editable {path}`, not `{mk}`", mtok, path, "wrong marker", "LANG-07-01"); ok = False
                elif mode == "loan":
                    self.own_err("E-OWN-001", f"a stored loan takes `alias {path}` or `editable {path}`, not `{mk}`", mtok, path, "wrong marker", "LANG-07-06"); ok = False
                if mk == "move" and ok:
                    self.reserve(path, "move", mtok)
            elif mk == "editable":
                if mode in ("owned",):
                    self.own_err("E-OWN-001", f"an `owned` parameter takes `move {path}` or `copy {path}`, not `editable`", mtok, path, "wrong marker", "LANG-07-01"); ok = False
                if ok:
                    self.reserve(path, "editable", mtok)
            else:
                if mode == "owned":
                    self.own_err("E-OWN-001", f"an `owned` parameter takes `move {path}` or `copy {path}`, not `alias`", mtok, path, "wrong marker", "LANG-07-01"); ok = False
                if ok:
                    self.reserve(path, "alias", mtok)
        else:
            self.pending = (path, mk, mtok)          # consumed by let / return / field initialiser / statement end
        # the call's receiver, narrowed where the marker stands, captured before the loan drops that fact (6.8)
        narrowed = None
        if mk == "editable":
            members = [k for k in children(pl) if name(k) == "memberName"]
            call_follows = len(kids) > 2 and isinstance(kids[2], Token) and kids[2].text == "(" and members
            saved, self.facts = self.facts, Facts()
            decl_t = self.root_type(direct_tokens(pl)[0], env)
            for k in (members[:-1] if call_follows else members):
                decl_t = self.member_type(decl_t, tokens(k)[0])
            self.facts = saved
            decl_t = strip(decl_t)
            loaned = self.path_of_place(pl) if not call_follows else ".".join([direct_tokens(pl)[0].text] + [tokens(k)[0].text for k in members[:-1]])
            if isinstance(decl_t, Refined) and decl_t.kind == "length":
                self.ref("E-REF-003", f"`{loaned}` has a length refinement and may not be loaned editable", tokens(kids[0])[0], "violated", decl_t.show(), "")
            if call_follows:
                narrowed = self.fact_type(loaned, decl_t)
            self.facts.invalidate(loaned)
        if len(kids) == 2:
            if mk in ("alias", "editable") and known(t) and not isinstance(t, LoanT):
                return LoanT(t, mk, direct_tokens(pl)[0].text)
            if mk == "copy" and (owned := self.lent(t)) is not None:
                return owned                         # a copy is owned, whatever the place holds (7.5; D-341)
            return t
        rest = kids[2:]
        if name(rest[0]) == "methodCall":
            t = self.method_call(t, rest[0], env)
            rest = rest[1:]
        elif isinstance(rest[0], Token) and rest[0].text == "(":
            al = next((k for k in kids if name(k) == "argList"), None)
            members = [k for k in children(pl) if name(k) == "memberName"]
            if members:                     # `editable p.m(...)`: the greedy place took the method name
                if narrowed is not None:
                    prefix_t = narrowed       # captured before `editable` invalidated the receiver (6.8)
                else:
                    prefix_t = self.root_type(direct_tokens(pl)[0], env)
                    for k in members[:-1]:
                        prefix_t = self.member_type(prefix_t, tokens(k)[0])
                    prefix_t = self.fact_type(path, prefix_t)   # a receiver narrowed where the marker stands keeps its narrowing (6.8)
                last = tokens(members[-1])[0]
                self.records[:] = [r for r in self.records if not (r["code"] == "E-NAM-001" and r["line"] == last.line and f"`{last.text}`" in r["message"])]
                t = self.method_call_named(prefix_t, last, al, env)
            elif isinstance(strip(t), FnT):
                ft = strip(t)
                if ft.kind == "fn mut" and mk != "editable":
                    self.own_err("E-OWN-001", f"a `fn mut` closure is called `editable {path}(…)`, not `{mk}`", mtok, path, "wrong call marker", "LANG-08-02")
                elif ft.kind == "fn once" and mk != "move":
                    self.own_err("E-OWN-001", f"a `fn once` closure is called `move {path}(…)`: it is consumed by the call", mtok, path, "wrong call marker", "LANG-08-02")
                elif ft.kind == "fn" and mk == "move":
                    pass
                for cp, cm in self.closures.get(root_of(path), {}).get("captures", []):
                    if cm == "editable":
                        self.facts.invalidate(cp)        # a reachable editable capture of a called closure (6.3)
                t = self.call_fn(ft, al, env, "closure", mtok, self.closures.get(root_of(path), {}).get("eff"))
            else:
                self.receiver = None
                if al is not None:
                    self.infer(al, env)
                t = Unknown("call")
            rest = [r for r in rest if name(r) == "postfix"]
        self.receiver = None
        self.marker_call_field(pl, mk, mtok, rest, env)
        for pf in rest:
            if name(pf) == "postfix":
                self.temp_field_siblings(t, pf)
                t = self.apply_postfix(t, pf, env)
        return t

    def call_fn(self, ft: FnT, al, env: Env, where: str, tok: Optional[Token] = None, known_eff: Optional[Summary] = None) -> Type:
        sig = Sig(where, [(f"p{i}", p, "") for i, p in enumerate(ft.params)], ft.ret, None, ft.modifiers, (), True)
        self.call_decl, self.call_label = None, "closure"
        # a callback parameter contributes what its type permits; a local closure its computed summary (11.3)
        self.summaries["closure"] = known_eff if known_eff is not None else self.summary_from_modifiers(sig)
        return self.check_call(sig, al, env, {}, where, tok or (tokens(al)[0] if al is not None and tokens(al) else Token("PUNCT", "(", 1, 1)))

    def place_type(self, pl, env: Env) -> Type:
        ts = direct_tokens(pl)
        t = self.root_type(ts[0], env)
        for k in children(pl)[1:]:
            if name(k) == "memberName":
                t = self.member_type(t, tokens(k)[0])
            elif isinstance(k, Token) and k.kind == "INT":
                s = strip(t)
                t = s.args[0] if isinstance(s, Named) and s.name in ("Array", "FixedList") and s.args else Unknown("element")
        return self.fact_type(self.path_of_place(pl), t)

    def root_type(self, tok: Token, env: Env) -> Type:
        if tok.text == "self":
            return self.self_type or Unknown("self")
        b = env.lookup(tok.text)
        if b is not None:
            return b.type
        d = self.table.get(tok.text)
        if d is not None and d.kind == "const":
            used = self.const_use_type(d)
            if used is not None:
                return used
        if d is not None and d.kind == "func" and d.sig is not None:
            self.entry_main_ref(d, tok)
            if d.sig.where:
                self.typ("E-TYP-040", f"`{d.name}` has `where` bounds, which hold at each call (6.10): it is called, never taken as a value; to pass it, wrap the call in a `fn` literal", tok, f"a call of {d.name}", "a function value", d.name)
            return FnT(tuple(p[1] for p in d.sig.params), self.result_of(d.sig, {}), "fn", d.sig.modifiers)
        return Unknown("untyped binding")

    def member_type(self, t: Type, mtok: Token, as_value: bool = False) -> Type:
        s = strip(t)
        if isinstance(s, Named) and s.name == "Shared" and s.args:
            s = strip(s.args[0])                  # reading through a Shared is a read loan (X-85)
        if isinstance(s, TupleT):
            return Unknown("tuple member")
        if isinstance(s, RecordT):
            for k, ft in s.fields:
                if k == mtok.text:
                    return ft
            self.rec("E-NAM-001", f"record has no field `{mtok.text}`", "LANG-04-05", mtok, {"name": mtok.text, "candidates": [k for k, _ in s.fields]})
            return Unknown()
        d = self.decl_of(s)
        if d is None:
            return Unknown("member of unknown")
        m = mtok.text
        if m in d.fields:
            self.check_member(d, m, mtok, "field")
            return self.subst(d.fields[m][0], self.recv_subst(s, d))
        if m in d.methods:
            if as_value:
                # a method is called, never taken as a value (4.5: `value.field` or `value.method(...)`): a value would
                # skip its signature and its `where` bounds (D-171)
                self.typ("E-TYP-040", f"`{m}` is a method of {d.name}: it is called (`.{m}(…)`), never taken as a value; to pass it, wrap the call in a `fn` literal (4.5)",
                         mtok, f"a call of {d.name}.{m}", "a method value", m)
            return Unknown("method value")
        if d.kind in ("class", "enum") and m not in d.constants:
            self.rec("E-NAM-001", f"{d.name} has no member `{m}`", "LANG-04-05", mtok, {"name": m, "candidates": sorted(d.fields) + sorted(d.methods)})
        return Unknown("member")

    def method_call(self, recv: Type, mc, env: Env) -> Type:
        return self.method_call_named(recv, tokens(find(mc, "memberName"))[0], find(mc, "argList"), env)

    def method_call_named(self, recv: Type, mtok: Token, al, env: Env) -> Type:
        # A marked receiver (`editable p.m(...)`) belongs to this call only: it is handed back on the path that
        # reaches check_call and dropped on every other exit, so it cannot attach to a later call (7.6; X-114).
        marked, self.receiver = self.receiver, None
        s = concrete(strip(recv))
        if isinstance(s, Refined):
            s = s.base
        if isinstance(s, Named) and s.name == "Shared" and s.args and mtok.text not in ("retain", "downgrade"):
            s = strip(s.args[0])
        # a method call reads its receiver as T only where a null test, an `&&` conjunct or a diverging guard narrowed
        # the place (6.8); `||` and function calls do not narrow (6.3), so an unnarrowed T? receiver is rejected (D-51.6)
        if isinstance(s, OptionalT) and not isinstance(s.inner, Unknown):
            self.typ("E-TYP-040", f"method `{mtok.text}` of an optional {s.show()}: narrow it with a null test, or match on it", mtok, s.inner.show(), s.show(), mtok.text, "LANG-06-08")
            self.infer(al, env) if al is not None else None
            return Unknown()
        # a `dynamic I` receiver's methods are I's (X-124: they were looked up after an early unknown return)
        d = self.dc.resolve_type(s.iface) if isinstance(s, DynamicT) else self.decl_of(s)
        if d is None or not known(s):
            self.infer(al, env) if al is not None else None
            return Unknown("method on unknown")
        m = mtok.text
        sig = d.methods.get(m)
        if sig is None and d.kind == "mixin":
            sig = d.methods.get("requires:" + m)
        if sig is None:
            for mx in d.mixins:
                md = self.dc.resolve_type(mx)
                if md is not None and m in md.methods:
                    sig = md.methods[m]
            for iface in d.implements:
                idecl = self.dc.resolve_type(iface)
                if sig is None and idecl is not None and m in idecl.methods:
                    sig = idecl.methods[m]
            if sig is None and m == "to":
                self.infer(al, env) if al is not None else None
                return Unknown("to[T]")
            if sig is None and m in ("toText", "toBytes"):
                # the canonical pair (6.10): every Encodable type has them; both allocate
                if not self.dc.implements(s, "Encodable"):
                    self.typ("E-TYP-090", f"`{m}` needs {s.show()} to be Encodable", mtok, "Encodable", s.show(), m, "LANG-06-10")
                if al is not None:
                    self.infer(al, env)
                self.eff.add("alloc", "proved", mtok)
                return STRING if m == "toText" else Named("Bytes")
            if sig is None and m in ("equals", "compare", "hash", "display", "encode") and (self.dc.implements(s, "Equatable") or self.dc.implements(s, "Encodable") or self.dc.implements(s, "Displayable")):
                self.infer(al, env) if al is not None else None
                return {"equals": BOOL, "compare": Named("Ordering")}.get(m, VOID)
        if sig is None:
            if m == "cleanup" and d.has_cleanup:
                self.typ("E-TYP-040", f"`cleanup` of {d.name} runs at destruction and cannot be called directly (9.4)", mtok, "destruction", "direct call", m)
            elif m in d.fields:
                self.typ("E-TYP-040", f"`{m}` is a field of {d.name}, not a method", mtok, "a method", "field", m)
            else:
                self.rec("E-NAM-001", f"{d.name} has no method `{m}`", "LANG-04-05", mtok, {"name": m, "candidates": sorted(d.methods)})
            self.infer(al, env) if al is not None else None
            return Unknown()
        if sig.receiver is None:
            self.typ("E-TYP-040", f"`{m}` is an associated function of {d.name}: call it as {d.name}.{m}(…)", mtok, f"{d.name}.{m}", "method call", m)
        self.check_member(d, m, mtok, "method")
        if isinstance(strip(recv), DynamicT) and "async" in sig.modifiers:
            self.eff.add("alloc", "proved", mtok)        # an async call through `dynamic` boxes its future (C.3; X-122)
        mm = self.recv_subst(s, d)
        rp = self.coll_path
        self.call_decl, self.call_label = d, self.method_label(d, m)
        if d.name == "Scope" and m in ("launchParallel", "launchBlocking"):
            self.check_pool_launch(m, al, env, mtok)
        self.receiver = marked
        if self.receiver is None and rp is not None and sig.receiver == "editable self":
            # reached only on a root `editable` could not be written on (X-345; D-303: whatever the receiver's type, a
            # trivial value or a read loan included): a `let` binding, a read parameter, capture or loan, a read `self`
            self.own_err("E-OWN-001", f"`{m}` takes `editable self`, and `{rp}` cannot change: declare it `let mut` or take it `editable`", mtok, rp, "editable receiver on an immutable root", "LANG-07-01")
        elif self.receiver is None and rp is not None and self.is_bearing(recv) and sig.receiver is not None:
            if sig.receiver == "owned self":
                self.own_err("E-OWN-001", f"`{m}` takes `owned self`: write `move {rp}.{m}(…)`", mtok, rp, "move receiver marker required", "LANG-07-01")
            else:
                self.receiver = (rp, "alias")        # implicit read loan of the receiver for the call
        if "async" in sig.modifiers and sig.receiver == "self" and self.is_bearing(recv) and (
                (self.receiver is None and rp is None) or (self.receiver is not None and self.receiver[1] == "copy")):
            self.carried_temps.append((mtok, self.call_depth + 1))   # a temporary receiver is lent to the future (12.3; D-31)
        elif self.receiver is not None and sig.receiver is not None:
            rmk = self.receiver[1]
            if sig.receiver == "editable self" and rmk != "editable":
                self.own_err("E-OWN-001", f"`{m}` takes `editable self`: the receiver needs `editable`, not `{rmk}`", mtok, self.receiver[0], "wrong receiver marker", "LANG-07-01")
            elif sig.receiver == "owned self" and rmk != "move":
                self.own_err("E-OWN-001", f"`{m}` takes `owned self`: the receiver needs `move`, not `{rmk}`", mtok, self.receiver[0], "wrong receiver marker", "LANG-07-01")
            elif sig.receiver == "self" and rmk == "move":
                self.own_err("E-OWN-001", f"`{m}` takes a read loan of self: `move` is not permitted on the receiver", mtok, self.receiver[0], "wrong receiver marker", "LANG-07-01")
        self.pending_owner_where = tuple(getattr(d, "where", ()) or ())
        if self.result_expected is not None:
            self.pending_call_expected = self.result_expected   # copied here, after every early return (8.4)
        if d.name == "Map" and d.module == "fortis.prelude" and m == "get" and mm.get("K") == STRING and sig.params:
            # `get` only reads its key: on a Map[String, V] the key is a StringView, so no copy makes one (6.7, X-345)
            sig = dataclasses.replace(sig, params=[(sig.params[0][0], STRING_VIEW, sig.params[0][2])] + list(sig.params[1:]))
        t = self.check_call(sig, al, env, mm, f"{d.name}.{m}", mtok)
        if d.name == "Scope" and m in ("launch", "launchParallel") and al is not None:
            # a future launched directly owns its call's own temporaries (its arguments and receiver): they live as
            # long as the task; a nested future's are not the launched call's (12.3, X-251)
            ats = tokens(al)
            lo, hi = (ats[0].line, ats[0].col), (ats[-1].line, ats[-1].col)
            launched = self.call_depth + 2
            self.carried_temps = [c for c in self.carried_temps if not (c[1] == launched and lo <= (c[0].line, c[0].col) <= hi)]
        return t

    def marker_call_field(self, pl, mk: str, mtok: Token, rest, env: Env):
        """`move make().a` marks the function name. A function is not a place, and neither is its result (7.2)."""
        if not any(self.field_postfix(pf) is not None for pf in rest):
            return
        root = direct_tokens(pl)[0].text
        if root == "self" or self.binding_kind(root, env) is not None:
            return
        path = self.path_of_place(pl)
        d = self.table.get(path) or self.table.get(root)
        if d is None or d.kind != "func":
            return
        self.own_err("E-OWN-001", f"`{mk} {root}` applies a marker to `{root}`, which is not a place; a call result is not a place either (7.2)", mtok, root, "marker on a non-place", "LANG-07-01")

    def field_postfix(self, pf):
        """The field a `.` or `?.` postfix projects, and the token that names it. None for a call or an index."""
        if name(pf) != "postfix":
            return None
        kids = children(pf)
        if not kids or not isinstance(kids[0], Token) or kids[0].text not in (".", "?.") or len(kids) < 2:
            return None
        second = kids[1]
        if name(second) == "memberName":
            tok = tokens(second)[0]
            return tok.text, tok
        if isinstance(second, Token) and second.kind == "INT":
            return second.text, second
        return None

    def aggregate_fields(self, recv: Type):
        """Fields of a class, a record or a tuple, in declaration order. A field type is substituted."""
        s = strip(recv)
        if isinstance(s, TupleT):
            return [(str(i), t) for i, t in enumerate(s.items)]
        if isinstance(s, RecordT):
            return list(s.fields)
        d = self.decl_of(s)
        if d is None or d.kind != "class":
            return []
        mm = self.recv_subst(s, d)
        return [(f, self.subst(ft, mm)) for f, (ft, _, _) in d.fields.items()]

    def temp_field_siblings(self, recv: Type, pf):
        """Projecting a field from a value that is not a place drops the other fields at the end of the
        statement (7.9). A linear one has no consumer (10.2). The projected field itself is the expression's
        result; a statement that drops that result is E-OBL-001."""
        got = self.field_postfix(pf)
        if got is None:
            return
        taken, tok = got
        # `?.` projects a field of the value inside the optional. One layer, as the member lookup does.
        base = strip(recv)
        if children(pf)[0].text == "?.":
            if not isinstance(base, OptionalT):
                return
            base = base.inner
        for fname, ft in self.aggregate_fields(base):
            if fname == taken or not self.is_linear(ft):
                continue
            self.obl_err("E-OBL-007", f"projecting `{taken}` from this temporary drops field `{fname}` ({ft.show()}), which is linear", tok, fname, fname, "LANG-10-02")

    def apply_postfix(self, t: Type, pf, env: Env) -> Type:
        kids = children(pf)
        if name(pf) == "methodCall":
            return self.method_call(t, pf, env)
        if kids and name(kids[0]) == "methodCall":
            return self.method_call(t, kids[0], env)
        first = kids[0]
        if isinstance(first, Token) and first.text in (".", "?."):
            second = kids[1]
            if name(second) == "memberName":
                mt = tokens(second)[0]
                s = strip(t)
                if first.text == "?.":
                    if known(s) and not isinstance(s, OptionalT):
                        self.typ("E-TYP-040", f"`?.` on {s.show()}, which is not optional", mt, "T?", s.show(), "?.", "LANG-06-08")
                        return Unknown()
                    base = s.inner if isinstance(s, OptionalT) else s
                    r = self.member_type(base, mt, as_value=True)
                    return r if isinstance(r, OptionalT) or not known(r) else OptionalT(r)   # `a?.f` is T? even when f is itself T? (6.8)
                if isinstance(s, OptionalT):
                    self.typ("E-TYP-040", f"member `{mt.text}` of an optional {s.show()}: use `?.` or match on it", mt, s.inner.show(), s.show(), mt.text, "LANG-06-08")
                    return Unknown()
                return self.member_type(s, mt, as_value=True)
            if isinstance(second, Token) and second.kind == "INT":
                s = strip(t)
                if isinstance(s, TupleT):
                    i = int(second.text)
                    if i < len(s.items):
                        return s.items[i]
                    self.typ("E-TYP-040", f"tuple {s.show()} has no field .{i}", second, f".0 to .{len(s.items) - 1}", f".{i}", f".{i}")
                    return Unknown()
                if known(s):
                    self.typ("E-TYP-040", f".{second.text} on {s.show()}, which is not a tuple", second, "a tuple", s.show(), f".{second.text}")
                return Unknown("tuple field")
        if isinstance(first, Token) and first.text == "(":
            al = find(pf, "argList")
            s = strip(t)
            if isinstance(s, FnT):
                cp = self.coll_path
                if cp is not None and s.kind == "fn mut" and self.unmarked_editable_root(cp, env):
                    # X-345: a `fn mut` held by a binding that shows it may change is called as `editable f(…)` (8.2)
                    self.unmarked_editable(cp, first, env)
                    info = self.closures.get(root_of(cp), {})
                    for cap, cm in info.get("captures", []):
                        if cm == "editable":
                            self.facts.invalidate(cap)        # a reachable editable capture of a called closure (6.3)
                    return self.call_fn(s, al, env, "closure", first, info.get("eff"))
                if cp is not None and s.kind == "fn mut":
                    self.own_err("E-OWN-001", f"a `fn mut` closure is called `editable {cp}(…)`", first, cp, "editable call marker required", "LANG-08-02")
                elif cp is not None and s.kind == "fn once":
                    self.own_err("E-OWN-001", f"a `fn once` closure is called `move {cp}(…)` and consumed", first, cp, "move call marker required", "LANG-08-02")
                return self.call_fn(s, al, env, "closure", first)
            if al is not None:
                self.infer(al, env)
            if known(s):
                self.typ("E-TYP-040", f"call of a value of type {s.show()}", first, "a function type", s.show(), "call")
            return Unknown("call")
        if isinstance(first, Token) and first.text == "[":
            e = find(pf, "expr")
            cp = self.coll_path                 # captured before the index expression is typed (it resets the chain state)
            it = self.infer(e, env)
            self.coll_path = cp
            s = strip(t)
            if known(strip(it)) and int_bounds(strip(it)) is None:
                self.typ("E-TYP-040", f"index has type {it.show()}, expected an integer", tokens(e)[0], "Int", it.show(), "index")
            if isinstance(s, Named) and s.name in ("List", "ListView", "Array", "FixedList") and s.args:
                self.index_proof(t, s, it, e, first)
                return s.args[0]
            if isinstance(s, Named) and s.name in ("Map", "OrderedMap", "FixedMap") and len(s.args) >= 2:
                return OptionalT(s.args[1])
            if known(s):
                self.typ("E-TYP-040", f"indexing a value of type {s.show()}", first, "a list, array or map", s.show(), "index")
            return Unknown("index")
        return Unknown("postfix")

    def index_proof(self, coll_t: Type, s: Named, it: Type, e, tok: Token):
        """`list[i]` needs i >= 0 and i below the length (6.7): from the type, a `i < p.count()` fact, or Array[T, N]."""
        ii = self.interval_of(it)
        ip = self.path_of_expr(e)
        if ii is None:
            return
        if ii[0] < 0:
            self.ref("E-REF-002", f"index interval {self.show_iv(ii)} is not proved non-negative; test `i >= 0` or use get/at", tok, "unproved" if ii[1] >= 0 else "violated", "[0, length)", self.show_iv(ii))
            return
        upper = None
        if s.name in ("Array", "FixedList") and len(s.args) > 1 and isinstance(s.args[1], Named) and s.args[1].name.startswith("#"):
            upper = int(s.args[1].name[1:])
        if isinstance(coll_t, Refined) and coll_t.kind == "length" and coll_t.hi is not None:
            upper = coll_t.hi
        if upper is not None:
            if ii[1] < upper:
                return
            self.ref("E-REF-002", f"index interval {self.show_iv(ii)} is not below the length {upper}", tok, "unproved" if ii[0] < upper else "violated", f"[0, {upper})", self.show_iv(ii))
            return
        cp = self.coll_path
        if ip is not None and cp is not None and self.facts.below.get(ip) == cp:
            return
        self.ref("E-REF-002", f"index is not proved below the length: no `{ip or 'i'} < {cp or 'list'}.count()` fact is in force; use get/at or test the bound", tok, "unproved", "[0, length)", self.show_iv(ii))

    def postfix_chain(self, node, env: Env, expected) -> Type:
        saved_result = self.result_expected
        try:
            return self.postfix_chain_at(node, env, expected)
        finally:
            self.result_expected = saved_result

    def postfix_chain_at(self, node, env: Env, expected) -> Type:
        kids = children(node)
        prim = kids[0]
        pk = children(prim)
        # module alias member: alias.member or alias.member(args)
        if len(pk) == 1 and isinstance(pk[0], Token) and pk[0].kind == "LOWER" and pk[0].text in self.aliases and env.lookup(pk[0].text) is None and len(kids) > 1:
            mn = find(kids[1], "memberName")
            if mn is not None:
                mtok = tokens(mn)[0]
                d = self.table.get(pk[0].text + "." + mtok.text)
                exported = sorted(k.split('.', 1)[1] for k, v in self.table.decls.items() if k.startswith(pk[0].text + '.') and v.exported)
                if d is not None and not d.exported and d.kind != "unknown":
                    self.rec("E-NAM-004", f"{self.aliases[pk[0].text]} does not export `{mtok.text}`", "LANG-04-05", mtok, {"name": mtok.text, "candidates": exported})
                elif d is None and self.dc.lib.modules.get(self.aliases[pk[0].text]) is not None:
                    # a member the module does not declare (D-36), reported as the same name in a type position is
                    # (names.py); a module the corpus does not contain has no declarations to judge against
                    self.rec("E-NAM-001", f"{self.aliases[pk[0].text]} declares no {mtok.text}", "LANG-04-03", mtok, {"name": mtok.text, "candidates": exported})
                t: Type = Unknown("module member")
                al = find(kids[1], "argList")
                self.entry_main_ref(d, mtok)
                if d is not None and d.kind == "func" and d.sig is not None and name(kids[1]) == "postfix" and name(children(kids[1])[0]) == "methodCall":
                    # every call names its callee (D-37): an alias call never inherits an earlier call's
                    self.call_decl, self.call_label = d, self.function_label(d)
                    if len(kids) == 2:
                        self.pending_call_expected = expected   # the call is the chain's result
                    t = self.check_call(d.sig, al, env, {}, f"{pk[0].text}.{mtok.text}", mtok)
                elif d is not None and d.kind == "const":
                    used = self.const_use_type(d)
                    if used is not None:
                        t = used
                elif al is not None:
                    self.infer(al, env)
                for pf in kids[2:]:
                    self.temp_field_siblings(t, pf)
                    t = self.apply_postfix(t, pf, env)
                return t
        # a module function called directly
        if len(pk) == 1 and isinstance(pk[0], Token) and pk[0].kind == "LOWER" and env.lookup(pk[0].text) is None and len(kids) > 1:
            fd = self.table.get(pk[0].text)
            first_pf = children(kids[1])
            if fd is not None and fd.kind == "func" and fd.sig is not None and first_pf and isinstance(first_pf[0], Token) and first_pf[0].text == "(":
                self.entry_main_ref(fd, pk[0])
                self.call_decl, self.call_label = None, self.function_label(fd)
                if len(kids) == 2:
                    self.pending_call_expected = expected   # the call is the chain's result
                t = self.check_call(fd.sig, find(kids[1], "argList"), env, {}, pk[0].text, pk[0])
                if len(kids) > 2:
                    t = self.unwrap_call(pk[0], t, env)        # a receiver in a test body (14)
                for i, pf in enumerate(kids[2:]):
                    self.temp_field_siblings(t, pf)
                    t = self.apply_postfix(t, pf, env)
                    ct = postfix_call_token(kids[1:], i + 1, prim)
                    if ct is not None and i + 3 < len(kids):
                        t = self.unwrap_call(ct, t, env)
                return t
        t = self.primary(prim, env, expected if len(kids) == 1 else None)
        path = self.path_of_expr(prim)
        member_toks: List[Token] = []
        pfs = kids[1:]
        if pfs and self.test_body:
            pt0 = call_name_token(("postfixExpr", [prim]))
            if pt0 is not None:
                t = self.unwrap_call(pt0, t, env)          # `Type.m(a).n()` in a test body (14)
        for i, pf in enumerate(pfs):
            if i > 0 and self.test_body:
                ct = postfix_call_token(pfs, i - 1, prim)
                if ct is not None:
                    t = self.unwrap_call(ct, t, env)       # the receiver of the next postfix, in a test body (14)
            # only the last call's result is the chain's result; an earlier `List.of` must not see it (8.4)
            self.result_expected = expected if i == len(pfs) - 1 else None
            self.coll_path = path
            is_mc = name(pf) == "methodCall" or (children(pf) and name(children(pf)[0]) == "methodCall")
            if path is not None and not self.in_marker and is_mc:
                mc = pf if name(pf) == "methodCall" else children(pf)[0]
                if self.receiver_mode(t, tokens(find(mc, "memberName"))[0].text) == "editable self" and self.unmarked_editable_root(path, env):
                    # X-345: the declaration shows the receiver is editable; checked as `editable p.m(…)` (7.5), whatever
                    # the receiver's type: a trivial value or a binding holding a loan takes the same exclusive loan (D-303)
                    saved_f, self.facts = self.facts, Facts()
                    decl_t = self.root_type(tokens(prim)[0], env)
                    for mt in member_toks:
                        decl_t = self.member_type(decl_t, mt)
                    self.facts = saved_f
                    decl_t = strip(decl_t)
                    if isinstance(decl_t, Refined) and decl_t.kind == "length":
                        self.ref("E-REF-003", f"`{path}` has a length refinement and may not be loaned editable", tokens(prim)[0], "violated", decl_t.show(), "")
                    self.unmarked_editable(path, tokens(prim)[0], env)
                else:
                    self.use_place(path, tokens(prim)[0], "read", env)
            if path is None:
                self.temp_field_siblings(t, pf)
            t = self.apply_postfix(t, pf, env)
            k = children(pf)
            if path is not None and k and isinstance(k[0], Token) and k[0].text == "." and len(k) > 1 and name(k[1]) == "memberName":
                member_toks.append(tokens(k[1])[0])
                path = path + "." + tokens(k[1])[0].text
                if not is_mc:
                    t = self.fact_type(path, t)       # `d.p.read()` sees `d.p` as the null test narrowed it (6.8)
            else:
                path = None
        self.coll_path = None
        full = self.path_of_expr(node)
        if full is not None and not self.in_marker:
            self.use_place(full, tokens(node)[0], "read", env)
        return self.fact_type(full, t)

    def primary(self, node, env: Env, expected) -> Type:
        kids = children(node)
        first = kids[0]
        if isinstance(first, Token):
            if first.text == "self":
                return self.self_type or Unknown("self")
            if first.kind == "LOWER":
                if first.text in self.aliases and env.lookup(first.text) is None:
                    # a module is not a value: its members are named as `alias.member`, where they are checked (4.5;
                    # D-36: a grouped or bound alias hid its members from that check)
                    self.rec("E-NAM-001", f"`{first.text}` names the module {self.aliases[first.text]}, which is not a value: name a member as `{first.text}.member`",
                             "LANG-04-03", first, {"name": first.text, "candidates": sorted(k.split('.', 1)[1] for k, v in self.table.decls.items() if k.startswith(first.text + '.') and v.exported)})
                    return Unknown("module")
                return self.fact_type(first.text, self.root_type(first, env))
            if first.text == "null":
                return OptionalT(NEVER)
            if first.text == "present":
                return OptionalT(self.infer(find(node, "expr"), env))
            if first.text == "fail":
                if self.defer_depth:
                    self.def_err("E-DEF-001", "`fail` inside a deferred block: a defer consumes its own failures (or recover, catch … recover, report) (12.1)", first, "fail")
                ft = self.infer(find(node, "expr"), env)
                fp = self.path_of_expr(find(node, "expr"))
                if fp is not None:
                    self.use_place(fp, first, "move", env)
                    self.own.move(fp)
                self.consume(fp)
                if not self.in_try:
                    self.rec("E-ERR-001", "`fail` outside a `try` function", "LANG-10-01", first, {"obligation_origin": f"{first.line}:{first.col}", "unconsumed_path": "fail"})
                self.eff.add("fail", "proved", first)
                self.unwind_point(first, "fail", env)
                self.obs_exit("failure", first, f"the fail at line {first.line}")
                return NEVER
            if first.text == "(":
                return self.infer(find(node, "expr"), env, expected)
            return Unknown("primary token")
        n = name(first)
        if n == "literal":
            return self.infer(first, env)
        if n == "typeRef":
            return self.type_primary(node, env, expected)
        if n in ("lambda", "matchExpr", "ifExpr", "tupleLit", "recordLit"):
            return self.infer(first, env, expected)
        return Unknown("primary")

    def variant_owners(self, vname: str) -> List[str]:
        """The enums in scope that declare `vname` as a variant, reachable bare: the table's entries under their
        own name (the module's, braced imports', the prelude's). An alias import's enum sits under its
        `alias.name` key and does not make the bare name ambiguous (the resolver's `enum_variants` agrees)."""
        return sorted(dd.name for k, dd in self.dc.table.decls.items() if k == dd.name and dd.kind == "enum" and vname in dd.variants)

    def variant_constructor(self, d: Decl, vname: str, ret: Type) -> FnT:
        """The constructor a variant name denotes where a function type is expected (LR 9.7): the payload fields
        are positional parameters and the result is that enum. `pure`, so it meets a `pure fn` position."""
        payload = d.variants.get(vname) or []
        mm = self.recv_subst(ret, d)
        return FnT(tuple(self.subst(ft, mm) for _, ft in payload), ret, "fn", ("pure",))

    def note_ctor_mismatch(self, have: Type, want: Type, tok: Token) -> None:
        """Explicit type arguments that are a different concrete instantiation from the expected result (LR 9.7)."""
        hs, ws = strip(have), strip(want)
        if not isinstance(hs, Named) or not isinstance(ws, Named) or hs.name != ws.name or len(hs.args) != len(ws.args) or not hs.args:
            return
        hk = tuple(self.nominal_key(a) for a in hs.args)
        wk = tuple(self.nominal_key(a) for a in ws.args)
        if None in hk or None in wk or hk == wk:
            return
        self.typ("E-TYP-040", f"variant constructor has type {hs.show()}, expected {ws.show()}: {hs.show()} is not {ws.show()}", tok, ws.show(), hs.show(), "variant constructor", "LANG-06-01")

    def nominal_key(self, t: Type):
        t = strip(t)
        if isinstance(t, Refined):
            t = t.base
        if isinstance(t, Named):
            args = tuple(self.nominal_key(a) for a in t.args)
            if None in args:
                return None
            return (t.name, args)
        return None

    def variant_ctor_result(self, d: Decl, tref: Type, es: FnT) -> Type:
        """The result of `Enum.Variant` where a function type is expected (LR 9.7). A reference that already carries
        type arguments keeps them. When the expected result is this enum, that result is used so its arguments flow
        (`Level.Low` where `fn() Level[Int]` is expected). Otherwise the enum itself, and the expected function is
        checked against this constructor. The result is `as_named`, so a core enum keeps its module: `money.Currency`
        is not this package's `Currency` (D-142)."""
        if isinstance(tref, Named) and (tref.args or not d.type_params):
            return d.as_named(tref.args)
        ret_d = self.decl_of(es.ret)
        if ret_d is not None and ret_d.kind == "enum" and ret_d.name == d.name and ret_d.type_identity == d.type_identity and ret_d.as_named().origin == d.as_named().origin:
            return es.ret
        if not d.type_params:
            return d.as_named()
        return d.as_named(tuple(TypeParam(p) for p in d.type_params))

    def type_primary(self, node, env: Env, expected) -> Type:
        """typeRef [ "." (upperIdent | memberName) ] [ "(" argList ")" ]."""
        kids = children(node)
        tr = kids[0]
        al = find(node, "argList") if name(node) != "constTerm" else tree_mod.child(node, "constArgList")
        called = any(isinstance(k, Token) and k.text == "(" for k in kids)
        member = None
        for k in kids[1:]:
            if name(k) == "memberName":
                member = tokens(k)[0]
            elif isinstance(k, Token) and k.kind == "UPPER":
                member = k
        t = self.dc.type_of_ref(tr)
        d = self.decl_of(t)
        rts = direct_tokens(tr)
        if rts[0].kind == "LOWER":
            # `alias.Type` names the declaration its module alias reaches (4.3), whatever a type of that name elsewhere is:
            # a core type and a single-file program's own type share the empty identity (D-140)
            dq = self.table.get(rts[0].text + "." + rts[2].text)
            if dq is not None and dq.kind != "unknown":
                d = dq
        tr_tok = next(x for x in tokens(tr) if x.kind == "UPPER")
        # a bare variant where the expected type is that enum (9.7) whose name also reaches a declaration in scope is
        # ambiguous: the qualified form says which is meant, so a name never silently changes meaning (X-345)
        if d is not None and member is None and isinstance(t, Named) and rts[0].kind == "UPPER" and expected is not None:
            es = strip(expected)
            ed = self.decl_of(es)
            if ed is not None and ed.kind == "enum" and t.name in ed.variants and not (d.kind == "enum" and d.name == ed.name):
                self.typ("E-TYP-040", f"`{t.name}` is ambiguous: the {d.kind} `{d.name}` is in scope and {ed.name}, the expected type, declares `{t.name}` as a variant; write `{ed.name}.{t.name}`",
                         tr_tok, f"{ed.name}.{t.name}", f"{d.kind} {d.name}", t.name)
                if al is not None:
                    self.infer(al, env)
                return es
        # a bare variant where the expected type is that enum (9.7)
        if d is None and member is None and isinstance(t, Named):
            es = strip(expected) if expected is not None else None
            ed = self.decl_of(es) if es is not None else None
            if ed is not None and ed.kind == "enum" and t.name in ed.variants:
                self.variant_args(ed, t.name, al, env, tr_tok)
                return es
            # a variant name where a function type is expected denotes that enum's constructor (LR 9.7). The
            # result type picks the enum, as an expected enum picks the value form. One owner is unambiguous, so
            # the name denotes that enum's constructor and the expected function is checked against it. Several
            # owners otherwise stay ambiguous (D-253)
            if isinstance(es, FnT) and not called:
                ret_d = self.decl_of(es.ret)
                if ret_d is not None and ret_d.kind == "enum" and t.name in ret_d.variants:
                    return self.variant_constructor(ret_d, t.name, es.ret)
                owners = self.variant_owners(t.name)
                if len(owners) == 1:
                    only = self.table.get(owners[0])
                    if only is not None and only.kind == "enum" and t.name in only.variants:
                        own = only.as_named(tuple(TypeParam(p) for p in only.type_params))
                        return self.variant_constructor(only, t.name, own)
            owners = self.variant_owners(t.name)
            if len(owners) > 1:
                # no expected type picks the enum and several in scope declare the name: the unprefixed form is
                # ambiguous (LR 9.7 lets it out only where the expected type is that enum, or the name is
                # unambiguous; D-253)
                listed = " and ".join(owners) if len(owners) == 2 else ", ".join(owners[:-1]) + " and " + owners[-1]
                forms = " or ".join(f"`{o}.{t.name}`" for o in owners)
                self.rec("E-NAM-001", f"`{t.name}` is ambiguous: {listed} declare it as a variant; write {forms}", "LANG-04-05", tr_tok,
                         {"name": t.name, "candidates": [f"{o}.{t.name}" for o in owners]})
            elif len(owners) == 1 and not self.unresolved_args and not isinstance(es, Unknown) and not isinstance(es.inner if isinstance(es, OptionalT) else es, FnT):
                # the bare form is read only where the expected type is the enum (9.7): with no such type the value has no
                # type, and no backend could build it (D-304, X-345). An unknown expected type is not a proof, and where a
                # function (or an optional one: an aspect's `fallback`) is expected the name is the variant's constructor.
                self.typ("E-TYP-040", f"`{t.name}` is a variant of {owners[0]}, but nothing here expects a {owners[0]}: the bare form is read only where the expected type is that enum; write `{owners[0]}.{t.name}`",
                         tr_tok, f"{owners[0]}.{t.name}", "a bare variant with no expected enum", t.name)
        if d is None:
            if al is not None:
                self.infer(al, env)
            return Unknown("type primary")
        if member is not None and member.text in ("fromText", "fromBytes") and called:
            # the canonical decoders (6.10): associated on every Decodable type; fallible, allocating
            if not self.dc.implements(t, "Decodable"):
                self.typ("E-TYP-090", f"`{member.text}` needs {t.show()} to be Decodable", member, "Decodable", t.show(), member.text, "LANG-06-10")
            inp = STRING_VIEW if member.text == "fromText" else Named("BytesView")
            sig = Sig(member.text, [("input", inp, "")], t, None, ("try",), (), True)
            self.call_decl, self.call_label = None, f"codec {member.text}"
            self.eff.add("alloc", "proved", member)
            self.pending_call_expected = expected
            return self.check_call(sig, al, env, {}, f"{t.show()}.{member.text}", member)
        if member is not None and d.kind == "type" and member.text == "validate":
            repr_t = d.alias.base if isinstance(d.alias, Refined) else d.alias
            sig = Sig("validate", [("value", repr_t, "")], d.as_named(), None, ("try",), (), True)
            self.call_decl, self.call_label = d, f"validate {d.name}"       # every call names its callee (D-37, D-133)
            self.pending_call_expected = expected
            return self.check_call(sig, al, env, {}, f"{d.name}.validate", member)
        if member is None:
            if d.kind == "class" and called:
                return self.construct(d, t, al, env, tr_tok)
            if d.kind == "type" and called:
                if al is not None:
                    self.infer(al, env)
                return Unknown("newtype call")
            if d.kind == "error" and called:
                payload = getattr(d, "payload", None) or {}
                seen_named = False
                for a in self.arg_nodes(al):
                    ats = tokens(a)
                    lbl = ats[0].text if len(ats) > 1 and ats[1].text == ":" else None
                    if lbl is None and seen_named and pun_name(a) is not None:
                        lbl = pun_name(a).text                    # `wanted` after a named argument is `wanted: wanted` (5.8, X-345)
                    seen_named = seen_named or lbl is not None
                    e = self.arg_expr(a)
                    want = payload.get(lbl) if lbl is not None else None
                    at = self.infer(e, env, expected=want) if e is not None else Unknown("arg")
                    if want is not None and known(at) and known(want):
                        self.expect(at, want, ats[0], f"payload field `{lbl}` of {d.name}")
                        self.loan_into_owned(at, want, ats[0], f"payload field `{lbl}` of {d.name}")
                    ft_ = want if want is not None else at
                    if known(strip(ft_)) and isinstance(base_type(ft_), Named) and base_type(ft_).name in ("String", "Bytes", "List", "Map", "Set"):
                        self.eff.add("alloc", "proved", ats[0])      # an unbounded payload allocates (10.3, noalloc try)
                return d.as_named()
            if not called and not (d.name == "Void" and d.module == "fortis.prelude"):
                # a type is not a value: there is no first-class type, so `let h = Host` names nothing a program can use
                # (4.5's qualified forms are `Type.associated` and `Enum.Variant`; D-141). `Void` alone is also its one value (6.2)
                self.typ("E-TYP-040", f"type `{d.name}` is used as a value: name a value, a constructor call, `{d.name}.associated(…)` or a variant", tr_tok,
                         "a value", f"type {d.name}", d.name)
            return Unknown("type value")
        m = member.text
        if isinstance(t, Refined) and m == "validate":
            # every refinement type has `validate(value: Base) Outcome[Refined]` (6.3)
            sig = Sig("validate", [("value", t.base, "owned" if not self.is_int(t.base) else "")], t, None, ("try",), (), True)
            if called:
                self.call_decl, self.call_label = None, f"validate {t.show()}"
                self.pending_call_expected = expected
                return self.check_call(sig, al, env, {}, f"{t.show()}.validate", member)
            return FnT((t.base,), Named("Outcome", (t,)), "fn", ("try",))
        if d.kind == "enum" and m in d.variants:
            # `Enum.Variant` where a function type is expected denotes the constructor (LR 9.7), as the bare name
            # does. Called, or where the expected type is not a function, it stays the enum value (D-54.5).
            if not called:
                es = strip(expected) if expected is not None else None
                if isinstance(es, FnT):
                    ret = self.variant_ctor_result(d, t, es)
                    # `Level[Int].High` is not `fn(Int) Level[String]`: explicit arguments that differ are a mismatch.
                    # Assignability leaves two concrete instantiations unknown, so the let would otherwise be accepted.
                    self.note_ctor_mismatch(ret, es.ret, member)
                    return self.variant_constructor(d, m, ret)
            self.variant_args(d, m, al, env, member)
            return d.as_named() if not d.type_params else Unknown("generic enum")
        if m in d.methods:
            sig = d.methods[m]
            self.check_member(d, m, member, "associated function")
            if sig.receiver is not None:
                self.typ("E-TYP-040", f"`{m}` is a method of {d.name}: call it on a value", member, "a value receiver", f"{d.name}.{m}", m)
            mm: Dict[str, Type] = {"Self": d.as_named()}
            if isinstance(t, Refined) and m == "validate":
                if al is not None:
                    self.check_call(sig, al, env, mm, f"{t.show()}.validate", member)
                return Named("Outcome", (t,))
            if iv.CHECKED_RESULT_INTERVAL and d.name in INT_TYPES and m in ("add", "sub", "mul", "div", "rem") and al is not None and called:
                args = self.arg_nodes(al)
                if len(args) == 2:
                    a_t, b_t = self.infer(self.arg_expr(args[0]), env), self.infer(self.arg_expr(args[1]), env)
                    ai, bi = self.interval_of(a_t), self.interval_of(b_t)
                    full = INT_TYPES[d.name]
                    r = iv.arith({"add": "+", "sub": "-", "mul": "*", "div": "/", "rem": "%"}[m], ai, bi) if ai is not None and bi is not None else None
                    if r is not None:
                        clipped = iv.meet(r, full)
                        if clipped is not None:
                            return Named("Outcome", (Refined(d.as_named(), clipped[0], clipped[1]),))
            if d.type_params and isinstance(t, Named) and t.args:
                for p, a in zip(d.type_params, t.args):
                    mm[p] = a
            if self.is_host(d) and m == "capabilities":
                self.capabilities_use(member, called)
            if not called:
                if sig.where:
                    self.typ("E-TYP-040", f"`{d.name}.{m}` has `where` bounds, which hold at each call (6.10): it is called, never taken as a value; to pass it, wrap the call in a `fn` literal", member, f"a call of {d.name}.{m}", "a function value", m)
                return FnT(tuple(p[1] for p in sig.params), self.result_of(sig, mm), "fn", sig.modifiers)
            self.call_decl, self.call_label = d, self.method_label(d, m)
            self.pending_owner_where = tuple(getattr(d, "where", ()) or ())
            # the expected result of a generic associated call fills its type parameters before the arguments are
            # typed (LR 6.10). `List.of(Red)`, where the call is expected `List[Light]`, then sees `Red` as `Light`,
            # so 9.7's unprefixed form is the variant that type picks (D-253). A binding that still names an unsolved
            # type parameter (the caller's, not this body's) is dropped: the arguments infer it. `List.of` passed to
            # `Outcome.collect` is expected `List[Outcome[T]]` while T is collect's parameter. Several owners and no
            # expected result stay ambiguous.
            #
            # A binding to a function type is dropped as well, dynamic or not, with optional and loan wrappers peeled
            # (8.4). `List[fn(Int) Int]` would otherwise fix the element before either argument, so
            # `List.of(lamA, lamB)` would accept the second lambda; left unbound, the first lambda pins the parameter
            # and the second conflicts. `dynamic fn` is not installed here either. `seed_dynamic_fn` binds that one,
            # and it peels present-injection optionals this unification does not (`(List[dynamic fn]?)?`,
            # `List[(dynamic fn)?]`). An enum, or any other non-function expected type, still binds.
            if expected is not None:
                pat = self.subst(sig.ret, mm)
                # A bare result parameter is fixed by the arguments (see seed_expected_result).
                if not isinstance(strip(pat), TypeParam):
                    before = set(mm)
                    self.unify(pat, strip(expected), mm)
                    for k in list(mm):
                        if k in before:
                            continue
                        bound = mm[k]
                        while isinstance(bound, (OptionalT, LoanT)):
                            bound = bound.inner
                        if isinstance(bound, FnT) or any(p not in self.tparams_in_scope for p in self.tparams_of(mm[k])):
                            del mm[k]
            self.pending_call_expected = expected
            return self.check_call(sig, al, env, mm, f"{d.name}.{m}", member)
        if m == "default" and any(n == "Default" for n, _ in d.derives):
            # `derive Default` generates the associated `default()` from each field's initialiser (6.10, 9.2; D-170)
            self_t = t if isinstance(t, Named) else d.as_named()
            if not called:
                return FnT((), self_t, "fn", ("pure",))
            if any(not self.dc.is_trivial(ft) for ft, _, _ in d.fields.values()):
                self.eff.add("alloc", "proved", member)          # a field that owns heap storage is created (C.3)
            self.call_decl, self.call_label = None, f"derived {d.name}.default"
            self.pending_call_expected = expected
            return self.check_call(Sig("default", [], self_t, None, ("pure",), (), True), al, env, {}, f"{d.name}.default", member)
        if m in d.constants:
            return d.constants[m]
        if d.kind == "enum":
            self.rec("E-NAM-001", f"{d.name} has no variant `{m}`", "LANG-04-05", member, {"name": m, "candidates": sorted(d.variants)})
        else:
            self.rec("E-NAM-001", f"{d.name} has no associated member `{m}`", "LANG-04-05", member, {"name": m, "candidates": sorted(k for k, s in d.methods.items() if s.receiver is None) + sorted(d.constants)})
        if al is not None:
            self.infer(al, env)
        return Unknown("associated")

    def variant_args(self, d: Decl, vname: str, al, env: Env, tok: Token):
        payload = d.variants[vname]
        args = self.arg_nodes(al)
        if not payload and args:
            self.typ("E-TYP-130", f"variant {d.name}.{vname} has no payload", tok, "no arguments", f"{len(args)}", vname, "LANG-05-09")
        # a payload is passed as a read parameter is, but the variant owns it: a loan there needs `copy` (D-341)
        sig = Sig(vname, [(f, t, "payload") for f, t in payload], d.as_named(), None, (), (), True)
        if payload and al is not None:
            self.call_decl, self.call_label = d, f"variant {d.name}.{vname}"
            self.check_call(sig, al, env, {}, f"{d.name}.{vname}", tok)

    def construct(self, d: Decl, t: Type, al, env: Env, tok: Token) -> Type:
        if self.foreign(d):
            hidden = [f for f, (ft, exp, _) in d.fields.items() if not exp]
            if hidden:
                self.rec("E-NAM-004", f"constructor of {d.name} is not callable here: field{'s' if len(hidden) > 1 else ''} {', '.join('`' + h + '`' for h in hidden)} not exported ({d.name} is declared in {d.module})",
                         "LANG-09-02", tok, {"name": d.name, "candidates": hidden})
        given: Dict[str, Token] = {}
        mm = self.recv_subst(t, d) if isinstance(t, Named) and t.args else {"Self": t}
        for a in self.arg_nodes(al):
            ts = direct_tokens(a)
            e = self.arg_expr(a)
            if ts and ts[0].text == "...":
                pl = find(a, "place")
                if pl is not None:
                    self.use_place(self.path_of_place(pl), tokens(a)[0], "move" if ts[1].text == "move" else "copy", env)
                    if ts[1].text == "move":
                        self.own.move(self.path_of_place(pl))
                st = strip(self.place_type(pl, env)) if pl is not None else Unknown()
                if ts[1].text == "copy" and known(st) and not self.dc.is_trivial(st):
                    self.eff.add("alloc", "proved", tokens(a)[0])     # `...copy` of a heap-owning value deep-copies it (6.11; X-116)
                sd = self.decl_of(st)
                if isinstance(st, RecordT):
                    src_fields = dict(st.fields)
                    src_decl = None
                elif sd is not None:
                    src_fields = {k: v[0] for k, v in sd.fields.items()}
                    src_decl = sd
                else:
                    src_fields = None
                    src_decl = None
                if src_fields is None:
                    return d.as_named()       # unknown source: nothing provable
                src_mm = self.recv_subst(st, src_decl) if src_decl is not None and isinstance(st, Named) else {}
                for f, ft in src_fields.items():
                    if f in d.fields:
                        if f in given:
                            self.typ("E-TYP-130", f"field `{f}` of {d.name} is given by the spread and explicitly", tokens(a)[0], "each field once", f, f, "LANG-09-02")
                        given[f] = tokens(a)[0]
                        declared = self.subst(d.fields[f][0], mm)
                        # a class field's declared type has no identity; the session pin is the lambda it holds (8.4)
                        value_ty = self._overlay_pins(src_decl, f, self.subst(ft, src_mm), "") if src_decl is not None else ft
                        if not self.has_free(declared):
                            self.expect(value_ty, declared, tokens(a)[0], f"field `{f}`")
                        self.note_field_closure(d, f, declared, value_ty, tokens(a)[0])
                    elif ts[1].text == "move" and self.is_linear(ft):
                        self.obl_err("E-OBL-007", f"`...move` drops field `{f}` ({ft.show()}), which is linear and has no counterpart in {d.name}", tokens(a)[0], f"{tokens(a)[0].line}:{tokens(a)[0].col}", f, "LANG-10-02")
                continue
            pun = pun_name(a)                                # `sku` is `sku: sku`: constructor arguments are named (9.2, X-345)
            if pun is not None:
                ts = [pun, Token("PUNCT", ":", pun.line, pun.col)]
            if not (ts and ts[0].kind == "LOWER" and len(ts) > 1 and ts[1].text == ":"):
                self.typ("E-TYP-130", f"constructor arguments of {d.name} are named (field: value)", tokens(a)[0], "field: value", "positional", d.name, "LANG-09-02")
                self.infer(e, env)
                continue
            f = ts[0].text
            if f not in d.fields:
                why = f" (`{f}` stands for `{f}: {f}`)" if pun is not None else ""
                self.typ("E-TYP-130", f"{d.name} has no field `{f}`{why}", ts[0], ", ".join(d.fields), f, f, "LANG-09-02")
                self.infer(e, env)
                continue
            if f in given:
                self.typ("E-TYP-130", f"field `{f}` of {d.name} is given twice", ts[0], "each field once", f, f, "LANG-09-02")
            given[f] = ts[0]
            ft = self.subst(d.fields[f][0], mm)
            fmode = "loan" if isinstance(ft, LoanT) else "owned"       # a stored loan takes alias/editable (7.7); a field takes ownership (9.2)
            self.arg_mode = fmode
            et = self.infer(e, env, expected=ft)
            self.arg_mode = None
            self.check_arg_marker(e, f, fmode, et, tokens(e)[0], env)
            if not self.has_free(ft):
                self.expect(et, ft, tokens(e)[0], f"field `{f}`")
            self.note_field_closure(d, f, ft, et, tokens(e)[0])
        missing = [f for f, (ft, _, has_default) in d.fields.items() if f not in given and not has_default]
        if missing:
            self.typ("E-TYP-050", f"constructor of {d.name} lacks field{'s' if len(missing) > 1 else ''} {', '.join('`' + f + '`' for f in missing)}", tok, ", ".join(missing), "absent", d.name, "LANG-09-02")
        self.activate(tok, d.lifetime is not None, env)   # the constructor takes ownership of every argument (9.2); a lifetime class keeps the loans it stores (7.7)
        return t if isinstance(t, Named) and t.args else d.as_named()

    def lambda_type(self, node, env: Env, expected) -> Type:
        e = Env(env)
        exp = strip(expected) if expected is not None else None
        exp_fn = exp if isinstance(exp, FnT) else None
        params: List[Type] = []
        lp = tree_mod.child(node, "lambdaParams")       # the lambda's own, not a nested lambda's (D-34)
        if lp is not None:
            for i, p in enumerate(find_all(lp, "lambdaParam")):
                ts = direct_tokens(p)
                pt = find(p, "paramType")
                if pt is not None:
                    t = self.dc.type_of(find(pt, "type"))
                elif exp_fn is not None and i < len(exp_fn.params):
                    t = exp_fn.params[i]
                else:
                    t = Unknown("inferred lambda parameter")
                params.append(t)
                e.set(ts[0].text, t, kind="param")
        kinds = [k.text for k in children(node) if isinstance(k, Token)]
        kind = "fn once" if "once" in kinds else ("fn mut" if "mut" in kinds else "fn")
        cl = tree_mod.child(node, "captureList")
        captures: List[Tuple[str, str]] = []
        cap_loans = []
        linear_capture = False
        if cl is not None:
            for cap in find_all(cl, "capture"):
                pl = find(cap, "place")
                ctok = tokens(cap)[0]
                root_name = direct_tokens(pl)[0].text if pl is not None else ctok.text
                if root_name != "self" and env.lookup(root_name) is None:
                    continue        # not a local or parameter: the resolver rejects the entry (E-FUN-011), and the body names it directly
                if pl is not None:
                    mk = ctok.text
                    path = self.path_of_place(pl)
                    self.in_marker = True
                    ct = self.place_type(pl, env)
                    self.in_marker = False
                    self.use_place(path, ctok, "copy" if mk == "copy" else mk, env)
                    if mk == "copy" and known(strip(self.declared_type(pl, env))) and not self.dc.is_copyable(strip(self.declared_type(pl, env))):
                        self.own_err("E-OWN-002", f"`copy {path}` in a capture list: {strip(ct).show()} is not Copyable", ctok, path, "copy of a non-copyable place", "LANG-07-01")
                    captures.append((path, mk))
                    root_tok = direct_tokens(pl)[0]
                    saved_f, self.facts = self.facts, Facts()
                    root_t = self.root_type(root_tok, env)          # the body sees the captured root by its declared type (facts stay outside)
                    self.facts = saved_f
                    prev = e.vars.get(root_tok.text)
                    e.set(root_tok.text, root_t, mutable=(mk in ("editable", "move", "copy")) or (prev is not None and prev.mutable), kind="capture-" + mk if prev is None or prev.kind == "capture-alias" else prev.kind)
                else:
                    ct_t = env.get(ctok.text) or Unknown()
                    if known(strip(ct_t)) and not self.dc.is_trivial(strip(ct_t)):
                        self.own_err("E-OWN-001", f"bare capture `{ctok.text}`: only a trivial value may be listed without a marker (8.3)", ctok, ctok.text, "marker required in a capture list", "LANG-08-02")
                    captures.append((ctok.text, "copy"))
                    e.set(ctok.text, ct_t, mutable=False, kind="capture-copy")
            # an editable capture and any other capture of the same owner conflict (8.3)
            for i, (pa, ma) in enumerate(captures):
                for pb, mb in captures[i + 1:]:
                    if (ma == "editable" or mb == "editable") and root_of(self.own.owner_path(pa)) == root_of(self.own.owner_path(pb)):
                        self.own_err("E-OWN-010", f"captures `{ma} {pa}` and `{mb} {pb}` loan one owner and one of them is exclusive", tokens(node)[0], pa, f"{mb} {pb}", "LANG-08-02")
            linear_capture = False
            for path, mk in captures:
                if mk in ("alias", "editable") and self.use_ok:
                    cap_loans.append(self.own.add_loan(path, mk, None, self.depth, tokens(node)[0].line))
                elif mk == "move":
                    if path in self.own.obligations:
                        linear_capture = True
                    self.own.move(path)
            if linear_capture and kind != "fn once":
                self.obl_err("E-OBL-006", f"this `{kind}` lambda captures a linear value by move: a closure owning a linear value must be `fn once`", tokens(node)[0], f"{tokens(node)[0].line}:{tokens(node)[0].col}", "closure", "LANG-10-02")
        rt_node = next((c for c in children(node) if name(c) == "type"), None)
        rt = self.dc.type_of(rt_node) if rt_node is not None else Unknown("lambda result")
        # an expression body may leave out its return type (8.2, X-345): the callee's parameter type decides it when
        # that type names one, otherwise the body's type does, widened as an unannotated `let` widens its initialiser
        inferred_rt = rt_node is None
        if inferred_rt and exp_fn is not None and known(strip(exp_fn.ret)) and not self.has_free(exp_fn.ret):
            rt = exp_fn.ret
        mods = tree_mod.child(node, "modifiers")
        written = tuple(t.text for t in tokens(mods)) if mods else ()
        lam_contracts = tuple(m for m in written if m[:1].isupper())
        modifiers = tuple(m for m in written if not m[:1].isupper())
        # a contract on a lambda binds it as its modifiers do (11.5): `P fn` with `contract P { pure }` is pure (D-145)
        modifiers += tuple(sorted(self.expanded_mods(Sig("lambda", [], VOID, None, modifiers, lam_contracts, True)) - set(modifiers)))
        for part in (tree_mod.child(node, "lambdaParams"), tree_mod.child(node, "type")):
            for q, n, qtok in (self.dc.shadowed_qualified_interfaces(part) if part is not None else []):
                self.typ("E-TYP-040", f"`{q}` is the interface {n}, which no value has: write `dynamic {q}`, `Shared[dynamic {q}]` or a type parameter bounded by it (9.5)", qtok, f"dynamic {q}", n, "lambda")
        for t in list(params) + [rt]:
            for i in self.dc.bare_interfaces(t):
                self.typ("E-TYP-040", f"a parameter or the result of this lambda has the interface type {i}, which no value has: write `dynamic {i}`, `Shared[dynamic {i}]` or a type parameter bounded by {i} (9.5)",
                         tokens(node)[0], f"dynamic {i}", i, "lambda")
        if "pure" in modifiers:
            for t in params:
                if self.is_authority(t):
                    self.eff_err("E-EFF-001", f"lambda is declared `pure` but takes the capability {base_name(self.unshared(t))}: a pure function receives no authority (11.5)",
                                 tokens(node)[0], "pure", "io", "lambda", "LANG-11-06")
        body = lambda_body(node)
        if body is not None:
            saved = (self.ret_type, self.in_try, self.ctx, self.loop_labels, self.facts, self.own, self.depth, self.arg_mode, self.pending, self.carried, self.res_stack, self.carried_temps)
            self.ret_type, self.in_try, self.ctx, self.loop_labels, self.facts, self.own, self.depth, self.arg_mode, self.pending, self.carried, self.res_stack, self.carried_temps = rt, "try" in modifiers, ["lambda"], [], Facts(), Owners(), 0, None, None, [], [], []
            self.lambda_kind.append(kind)
            # the lambda body is a separate activation: its exits and defers are its own, and the places it writes are its
            # own (its invalidations are dropped from the log when it ends, D-74)
            outer_flow = (self.exit_frames, self.break_frames, self.defer_bodies, self.block_rest, len(iv.LOG))
            self.exit_frames, self.break_frames, self.defer_bodies, self.block_rest = [], [], [], []
            outer_eff, self.eff = self.eff, Summary()
            outer_test, self.test_body = self.test_body, False      # a lambda's body is its own function, not the test's (14)
            body_t = self.body(body, e, rt, tokens(node)[0], "lambda")
            self.test_body = outer_test
            if inferred_rt and not known(strip(rt)) and body_t is not None:
                rt = self.widened(body_t)
            self.exit_frames, self.break_frames, self.defer_bodies, self.block_rest, log_mark = outer_flow
            del iv.LOG[log_mark:]
            if isinstance(strip(rt), FnT):
                # natively, a closure's function result is boxed: a Rust closure cannot name an `impl Fn` result
                # (Appendix C.1), so returning one allocates (C.3; D-34)
                self.eff.add("alloc", "proved", tokens(node)[0])
            lam_eff = self.eff
            self.eff = outer_eff
            self.last_closure_eff = lam_eff
            self.lambda_kind.pop()
            if self.final_pass:
                self.eff = lam_eff                       # a lambda's own claims are checked against its own summary
                lam_eff.authorities = set(outer_eff.authorities) | {"enclosing"}   # it acts under the enclosing function's authorities (11.6)
                lam_names = [direct_tokens(p)[0].text for p in find_all(lp, "lambdaParam")] if lp is not None else []
                self.check_claims(Sig("lambda", [(n, t, "") for n, t in zip(lam_names, params)], rt, None, modifiers, lam_contracts, True), tokens(node)[0], "lambda")
                self.eff = outer_eff
            self.ret_type, self.in_try, self.ctx, self.loop_labels, self.facts, self.own, self.depth, self.arg_mode, self.pending, self.carried, self.res_stack, self.carried_temps = saved
        self.carried += cap_loans                    # the closure holds its capture loans; its binding owns the closure (8.3)
        self.last_closure = {"kind": kind, "captures": captures, "linear": linear_capture, "eff": getattr(self, "last_closure_eff", None)}
        self.closure_info[id(node)] = self.last_closure
        le = getattr(self, "last_closure_eff", None)
        if body is not None and le is not None and "pure" not in modifiers and not (set(le.effects) & set(fx.PURE_FORBIDS)):
            modifiers = tuple(modifiers) + ("pure",)   # a lambda literal's type carries its inferred purity (11.3: local summaries are inferred)
        fn_tok = next((t for t in direct_tokens(node) if t.text == "fn"), tokens(node)[0])
        return FnT(tuple(params), rt, kind, modifiers, "", False, f"{self.module}:{fn_tok.line}:{fn_tok.col}")

    @staticmethod
    def hole_tokens(tk: Token, k: int, src: str) -> List[Token]:
        """A hole's tokens moved to the hole's place in the file (its position in `tk.interps`, else the token's own), as
        fortis-types `parse_hole` moves them, so an effect site inside a hole names the file's line and column (D-143)."""
        line, col = (tk.interps[k][1], tk.interps[k][2]) if k < len(tk.interps) and tk.interps[k][0] == src else (tk.line, tk.col)
        toks = lexer_mod.lex(src)
        for t in toks:
            if t.line == 1:
                t.col += col - 1
                t.interps = [(x, l + line - 1, c + (col - 1 if l == 1 else 0)) for x, l, c in t.interps]
            t.line += line - 1
        return toks

    def check_interpolation(self, tk: Token, env: Env):
        for k, src in enumerate(hole_sources(tk)):
            try:
                et = self.dc.lib.rec.parse(self.hole_tokens(tk, k, src), "expr")   # the hole's own positions (D-143)
            except Exception:  # noqa: BLE001
                continue
            before = len(self.records)
            effects_before = set(self.eff.effects)
            t = concrete(strip(self.infer(et, env)))
            for r in self.records[before:]:
                r["line"], r["column"] = tk.line, tk.col
            bad = sorted((set(self.eff.effects) - effects_before) & {"fail", "suspend"})
            if bad:
                self.rec("E-TYP-040", f"interpolated `{src}` may {' and '.join(bad)}: an interpolated expression is evaluated once, in order, and is neither fallible nor suspending (3.6)", "LANG-03-06", tk,
                         {"expected_type": "an infallible, non-suspending expression", "actual_type": ", ".join(bad), "primary_span": "${" + src + "}"})
            if known(t) and not self.dc.implements(t, "Displayable"):
                self.rec("E-TYP-040", f"interpolated `{src}` has type {t.show()}, which is not Displayable", "LANG-03-05", tk,
                         {"expected_type": "Displayable", "actual_type": t.show(), "primary_span": "${" + src + "}"})

    # ---- match and patterns (9.8) -------------------------------------------------------
    def _scrub_arm_breaks(self, marks, names, outer) -> None:
        """`break` states recorded during this arm name its bindings. Put back the facts those names had on entry, so a
        loop outside the match does not keep this arm's guard facts (the same rule as the arm's residual and exits)."""
        for frame, n in zip(self.break_frames, marks):
            for st in frame[2][n:]:
                st.restore_names(names, outer)

    def match(self, node, env: Env, expected, as_statement: bool) -> Type:
        kids = children(node)
        moving = any(isinstance(k, Token) and k.text == "move" for k in kids)
        scrut = next((c for c in kids if name(c) in ("expr", "place")), None)
        self.mark_consumed(scrut)                   # a `match` consumes its outcome itself (14)
        st = strip(self.infer(scrut, env))
        if is_strlit(st): st = STRING_VIEW
        sp = self.path_of_place(scrut) if name(scrut) == "place" else self.path_of_expr(scrut)
        if sp is not None and moving:
            self.use_place(sp, tokens(scrut)[0], "move", env)
            self.own.move(sp)
        owning = moving or sp is None
        kw = kids[0]
        arms = [_a for _a in children(node) if name(_a) == "matchArm"]
        covered: Set[str] = set()
        catch_all: Optional[Token] = None
        arm_results: List[Tuple[Type, Token]] = []
        closed = self.closed_variants(st)
        # D-76: each arm runs from the state left when the arms before it did not match (a failed `when` guard keeps its
        # effects); the state after the match joins the arms that complete
        entry = self.facts
        residual = entry.copy()
        ends: List[Facts] = []
        # each arm starts from the ownership state of the paths that have not matched yet, and the state after the
        # match joins the arms that continue (a move in one arm is not a move in another; D-159 review)
        own_entry = self.snapshot_own(self.own)
        own_residual = self.snapshot_own(own_entry)
        own_ends: List[Owners] = []
        for arm in arms:
            self.facts = residual.copy()
            # the live arm does not carry the residual's half: an inner join would report it early or drop it
            self.own = own_residual.copy()
            e = Env(env)
            pats = self.arm_patterns(arm)
            pat = pats[0]
            guard = next((c for c in children(arm) if name(c) == "expr" and children(arm).index(c) < len(children(arm)) - 1 and any(isinstance(k, Token) and k.text == "when" for k in children(arm))), None)
            # A fact about a name this arm binds belongs to this binding only (6.8, 9.8). Drop what an earlier arm
            # recorded under that name before this pattern and its guard establish their own facts. `outer` is what the
            # name meant on entry; residual and exit states put it back, so a later arm or the code after the match
            # does not inherit this binding's guard (a field path of the name included).
            bound = [b for p in pats for b in self.names_in(p)]
            outer = self.facts.split_off_names(bound)
            if len(pats) > 1:
                self.check_alternatives(pats, next((k for k in children(arm) if isinstance(k, Token) and k.text == "when"), None))
            cov, is_catch = set(), False
            for p in pats:
                c, k = self.pattern(p, st, e, moving)
                cov |= c
                is_catch = is_catch or k
            arm_holders = []
            arm_obligations = []
            if owning:
                for b in bound:
                    if b in e.vars and self.is_linear(e.vars[b].type):
                        self.own.obligations[b] = (tokens(pat)[0], self.depth + 1)
                        arm_obligations.append(b)
                for p in pats:
                    pt = direct_tokens(p)
                    if pt and pt[0].text == "is" and "as" not in [t.text for t in pt] and isinstance(st, Named) and st.name == "Failure":
                        self.discharges.append({"line": pt[0].line, "kind": "is-arm without as"})   # discharged: recovers effect (DEL-19)
                        self.eff.add("recovers", "proved", pt[0])
            if sp is not None and not moving:
                for b in bound:
                    if b in e.vars:
                        e.vars[b].kind = "pattern-loan"
                        self.own.holders[b] = self.own.add_loan(sp, "alias", b, self.depth + 1, tokens(pat)[0].line)
                        arm_holders.append(b)
            if guard is not None:
                # the arm sees the guard's true state, as an `if` body sees its condition's (6.8, 9.8); a later arm
                # sees the false state, effects included (D-76)
                before = len(self.records)
                true_f, false_f, _ = self.condition_states(guard, e)
                for r in self.records[before:]:
                    if r["code"] == "E-TYP-010" and r["message"].startswith("condition has type"):
                        r["message"] = "guard" + r["message"][len("condition"):]
                        r["fields"]["primary_span"] = "when"
                residual = false_f.copy() if (is_catch or any(self.irrefutable(p) for p in pats)) else residual.join(false_f)
                # own_outside_arm and the own_residual update read self.own, not self.facts. condition_states has
                # typed the guard, so self.own is the post-guard state master snapshotted before the arm body. Take
                # that before the body, and before self.facts becomes the true state the body sees.
                guarded = self.own_outside_arm(arm_holders, arm_obligations)
                if is_catch or any(self.irrefutable(p) for p in pats):
                    own_residual = self.retain_half(own_residual, guarded)   # replacing the residual must keep a half already found
                else:
                    own_residual = self.join_own(own_residual, guarded)
                self.facts = true_f
            else:
                covered |= cov
                if is_catch and catch_all is None:
                    catch_all = tokens(pat)[0]
            residual.restore_names(bound, outer)
            body_nodes = [c for c in children(arm) if name(c) in ("expr", "block")]
            body_node = body_nodes[-1] if body_nodes else None
            if body_node is None:
                continue
            completes = False
            break_at = [len(f[2]) for f in self.break_frames]
            if name(body_node) == "block":
                self.ctx.append("arm")
                self.arm_types.append([])
                self.exit_frames.append((self.depth, []))
                self.give_seen.append(False)
                self.branch_expected.append(expected)
                self.walk_block(body_node, e)
                self.branch_expected.pop()
                gives = self.arm_types.pop()
                _, gave = self.exit_frames.pop()
                self.ctx.pop()
                self.give_seen.pop()
                falls = not self.terminates(body_node)     # D-76: an early `give` does not stop the arm falling through
                if not as_statement:
                    if falls:
                        self.typ("E-TYP-110", "match arm block in expression position does not end in `give`", tokens(body_node)[0], "give value", "no give", "match arm", "LANG-05-05")
                    arm_results += gives
                ends += [f.sealed(bound, outer) for f in gave]   # D-76: the state at every `give`, an early one included
                if falls:
                    ends.append(self.facts.sealed(bound, outer))
                completes = bool(gave) or falls
            else:
                # the arm's own marker is not the enclosing expression's: a `move` already evaluated beside this
                # match stays pending for its consumer (D-159 review)
                outer_pending = self.pending
                self.pending = None
                t = self.infer(body_node, e, expected)
                self.branch_value(body_node, t, "=>")   # an expression arm is its `give` (9.8): the value is handed on (D-159)
                if self.pending is None:
                    self.pending = outer_pending
                arm_results.append((t, tokens(body_node)[0]))
                if strip(t) != NEVER:
                    ends.append(self.facts.sealed(bound, outer))
                    completes = True
            self._scrub_arm_breaks(break_at, bound, outer)
            for b in arm_holders:                    # payload loans last for the arm (9.8)
                self.own.loans = [l for l in self.own.loans if l.holder != b]
                self.own.holders.pop(b, None)
            for b in arm_obligations:
                if b in self.own.obligations:
                    tok0 = self.own.obligations.pop(b)[0]
                    self.obl_err("E-OBL-001", f"the failure `{b}` bound by this arm is not consumed (report, fail, move or match it)", tok0, f"{tok0.line}:{tok0.col}", b)
            if completes:
                own_ends.append(self.snapshot_own(self.own))
        # exhaustiveness
        exhaustive = True
        if closed is not None:
            if catch_all is not None:
                self.rec("E-MAT-002", f"catch-all arm over the closed type {st.show()}: name every case instead", "LANG-09-05", catch_all, {"missing_variants": [], "arm_set": st.show()})
            else:
                missing = [v for v in closed if not self.covers(v, covered, st)]
                exhaustive = not missing
                for v in missing:
                    self.rec("E-MAT-001", f"match over {st.show()} does not cover `{v}`", "LANG-09-05", kw, {"missing_variants": missing, "arm_set": st.show()})
        elif known(st) and catch_all is None and not self.irrefutable_present(arms):
            exhaustive = False
            self.rec("E-MAT-001", f"match over the open type {st.show()} has no catch-all arm", "LANG-09-05", kw, {"missing_variants": ["otherwise"], "arm_set": st.show()})
        elif not known(st) and catch_all is None and not self.irrefutable_present(arms):
            exhaustive = False
        if not exhaustive:
            ends.append(residual)              # D-79: no arm may run; the state is the residual
            own_ends.append(self.snapshot_own(own_residual))
        self.facts = Facts.join_all(ends) if ends else entry.copy()
        if own_ends:
            joined_own = own_ends[0]
            for o in own_ends[1:]:
                joined_own = self.join_own(joined_own, o)
            self.own = joined_own
        else:
            self.own = self.snapshot_own(own_entry)
        self.retain_half(own_residual, self.own)   # a guard's half lives on the residual, not on the arms that continue
        self.report_half()
        if as_statement:
            return VOID
        return self.unify_branches(arm_results, "match arm", expected)

    def covers(self, v: str, covered: Set[str], st: Type) -> bool:
        """Whether the arms cover case `v` of the scrutinee type: an arm covers it whole, or the arms' nested patterns cover
        every case of its payload together (D-79)."""
        if v in covered:
            return True
        inner: Optional[Type] = None
        if isinstance(st, OptionalT) and v == "present":
            inner = st.inner
        elif isinstance(st, Named) and st.name == "Outcome" and st.args and v == "success":
            inner = st.args[0]
        else:
            d = self.decl_of(st)
            if d is not None and d.kind == "enum" and v in d.variants and len(d.variants[v]) == 1:
                inner = self.subst(list(dict(d.variants[v]).values())[0], self.recv_subst(st, d))
        if inner is None:
            return False
        sub = {c[len(v) + 1:] for c in covered if c.startswith(v + "/")}
        cases = self.closed_variants(strip(inner)) if sub else None
        return cases is not None and all(self.covers(x, sub, strip(inner)) for x in cases)

    def irrefutable_present(self, arms) -> bool:
        return any(any(self.irrefutable(p) for p in self.arm_patterns(arm)) and not any(isinstance(k, Token) and k.text == "when" for k in children(arm)) for arm in arms)

    @staticmethod
    def arm_patterns(arm) -> list:
        """An arm's patterns: one, or the alternatives of an or-pattern `A | B` (9.8, X-345)."""
        return [c for c in children(arm) if name(c) == "pattern"]

    def check_alternatives(self, pats, when_tok):
        """The alternatives of an or-pattern bind no names and are not `otherwise`, and the arm takes no guard (9.8,
        X-345): the arm body sees the same bindings, none, whichever alternative matched, and nothing runs twice."""
        if when_tok is not None:
            self.rec("E-MAT-003", "an or-pattern arm takes no `when` guard: match the guarded cases in arms of their own", "LANG-09-05", when_tok,
                     {"missing_variants": [], "arm_set": " | ".join("".join(t.text for t in tokens(q)) for q in pats)})
        for p in pats:
            ts = direct_tokens(p)
            if ts and ts[0].text == "otherwise":
                self.rec("E-MAT-003", "`otherwise` cannot be an alternative of an or-pattern: write it as its own last arm", "LANG-09-05", ts[0],
                         {"missing_variants": [], "arm_set": "otherwise"})
                continue
            for b in dict.fromkeys(self.names_in(p)):
                bt = next((t for t in tokens(p) if t.text == b), ts[0] if ts else tokens(p)[0])
                self.rec("E-MAT-003", f"an alternative of an or-pattern binds `{b}`: alternatives bind no names; match the cases apart, or use `_`", "LANG-09-05", bt,
                         {"missing_variants": [], "arm_set": " | ".join("".join(t.text for t in tokens(q)) for q in pats)})

    def irrefutable(self, pat) -> bool:
        ts = direct_tokens(pat)
        words = [t.text for t in ts]
        if not words:
            return False
        if words[0] in ("_", "otherwise") or (len(ts) == 1 and ts[0].kind == "LOWER"):
            return True
        if words[0] in ("(", "{"):
            return all(self.irrefutable(p) for p in [c for c in children(pat) if name(c) == "pattern"])
        return False

    def closed_variants(self, st: Type) -> Optional[List[str]]:
        if isinstance(st, OptionalT):
            return ["present", "null"]
        if isinstance(st, Refined):
            st = st.base
        if isinstance(st, Named):
            if st.name == "Outcome": return ["success", "failure"]
            if st.name == "Bool": return ["true", "false"]
            d = self.decl_of(st)
            if d is not None and d.kind == "enum":
                return list(d.variants)
        return None

    def pattern(self, pat, st: Type, env: Env, moving: bool) -> Tuple[Set[str], bool]:
        """Bind the pattern's names in env against the scrutinee type; return (covered cases, is catch-all)."""
        for b in self.names_in(pat):
            self.own.reinit(b)                        # a fresh binding each time the pattern runs (a loop re-entry included)
            self.own.obligations.pop(b, None)
        ts = direct_tokens(pat)
        words = [t.text for t in ts]
        subs = [c for c in children(pat) if name(c) == "pattern"]
        if not ts and children(pat) and name(children(pat)[0]) == "literal":
            ts = tokens(children(pat)[0])
        if not ts:
            return set(), False
        head = ts[0]
        if head.text == "_":
            return set(), True
        if head.text == "otherwise":
            if len(ts) >= 3:
                env.set(ts[2].text, st, kind="pattern")
            return set(), True
        if len(ts) == 1 and head.kind == "LOWER":
            env.set(head.text, st, kind="pattern")
            return set(), True
        if head.text in ("true", "false"):
            if known(st) and base_name(st) != "Bool":
                self.typ("E-TYP-040", f"literal pattern Bool against {st.show()}", head, st.show(), "Bool", "pattern")
            return {head.text}, False
        if name(children(pat)[0]) == "literal":
            lt = self.stamp_literal(literal_type(tokens(pat)[0]))
            if known(st):
                a = STRING_VIEW if is_strlit(lt) else lt
                ok, _ = assignable(self.dc, a, st)
                if ok is False and st not in (STRING,):
                    self.typ("E-TYP-040", f"literal pattern {a.show()} against {st.show()}", head, st.show(), a.show(), "pattern")
            if base_name(st) == "Bool":
                return {head.text}, False
            return set(), False
        if head.text == "null":
            if known(st) and not isinstance(st, OptionalT):
                self.typ("E-TYP-040", f"`null` pattern against {st.show()}, which is not optional", head, "T?", st.show(), "null", "LANG-06-08")
            return {"null"}, False
        if head.text == "present":
            if known(st) and not isinstance(st, OptionalT):
                self.typ("E-TYP-040", f"`present` pattern against {st.show()}, which is not optional", head, "T?", st.show(), "present", "LANG-06-08")
            inner = st.inner if isinstance(st, OptionalT) else Unknown()
            if not subs:
                return {"present"}, False
            # D-79: `present(0)` covers one present value, not every one; an irrefutable inner pattern covers the case, and
            # refutable ones cover it only together (`present(true)` and `present(false)` over Bool?)
            icov, icatch = self.pattern(subs[0], inner, env, moving)
            if icatch or self.irrefutable(subs[0]):
                return {"present"}, False
            return {"present/" + c for c in icov}, False
        if head.text in ("success", "failure"):
            inner: Type = Unknown()
            if isinstance(st, Named) and st.name == "Outcome" and st.args:
                inner = st.args[0] if head.text == "success" else FAILURE
            elif known(st):
                self.typ("E-TYP-040", f"`{head.text}` pattern against {st.show()}, which is not an Outcome", head, "Outcome[T]", st.show(), head.text)
            if not subs:
                return {head.text}, False
            icov, icatch = self.pattern(subs[0], inner, env, moving)      # D-79, as for `present`
            if icatch or self.irrefutable(subs[0]):
                return {head.text}, False
            return {head.text + "/" + c for c in icov}, False
        if head.text == "is":
            if known(st) and base_name(st) != "Failure":
                self.typ("E-TYP-040", f"`is` pattern against {st.show()}, which is not a Failure", head, "Failure", st.show(), "is")
            ename = ts[1].text
            ed = self.table.get(ename)
            if ed is None or ed.kind != "error":
                self.rec("E-NAM-001", f"`{ename}` is not a declared error", "LANG-04-05", ts[1], {"name": ename, "candidates": sorted(k for k, v in self.table.decls.items() if v.kind == "error")})
            self.pattern_fields(pat, ed.payload if ed is not None else {}, env, moving, ts[1], ename)
            if "as" in words:
                env.set(ts[words.index("as") + 1].text, FAILURE, kind="pattern")
            return set(), False
        if head.text == "(":
            items = st.items if isinstance(st, TupleT) else [Unknown()] * len(subs)
            if isinstance(st, TupleT) and len(items) != len(subs):
                self.typ("E-TYP-040", f"tuple pattern of {len(subs)} against {st.show()}", head, st.show(), f"{len(subs)} elements", "pattern")
            elif known(st) and not isinstance(st, TupleT):
                self.typ("E-TYP-040", f"tuple pattern against {st.show()}", head, "a tuple", st.show(), "pattern")
            for p, t in zip(subs, items):
                self.pattern(p, t, env, moving)
            return set(), all(self.irrefutable(p) for p in subs)
        if head.text == "{":
            fields = dict(st.fields) if isinstance(st, RecordT) else {}
            if known(st) and not isinstance(st, RecordT):
                self.typ("E-TYP-040", f"record pattern against {st.show()}", head, "an anonymous record", st.show(), "pattern")
            keys = [t for t in ts if t.kind == "LOWER"]
            for k, p in zip(keys, subs):
                self.pattern(p, fields.get(k.text, Unknown()), env, moving)
            return set(), all(self.irrefutable(p) for p in subs)
        if head.kind == "UPPER":
            d = self.decl_of(st)
            if d is not None and d.kind == "enum":
                if head.text not in d.variants:
                    self.rec("E-NAM-001", f"{d.name} has no variant `{head.text}`", "LANG-09-05", head, {"name": head.text, "candidates": sorted(d.variants)})
                    return set(), False
                payload = dict(d.variants[head.text])
                mm = self.recv_subst(st, d)
                covs = self.pattern_fields(pat, {k: self.subst(v, mm) for k, v in payload.items()}, env, moving, head, f"{d.name}.{head.text}")
                # D-79: a variant is covered by irrefutable payload patterns, or, for a one-field payload, by refutable
                # ones that cover the field's cases together (`Flag(true)` and `Flag(false)`)
                field_pats = self.field_patterns(pat)
                if all(fp is None or self.irrefutable(fp) for fp in field_pats):
                    return {head.text}, False
                if len(payload) == 1 and len(covs) == 1:
                    cov, catch = covs[0]
                    return ({head.text} if catch else {head.text + "/" + c for c in cov}), False
                return set(), False
            if d is not None and d.kind == "class":
                if d.name != head.text:
                    self.typ("E-TYP-040", f"pattern {head.text} against {st.show()}", head, st.show(), head.text, "pattern")
                    return set(), False
                if not moving:
                    self.rec("E-MAT-002", f"destructuring class {d.name} needs `match move` / `let … = move` (9.8)", "LANG-09-06", head, {"missing_variants": [], "arm_set": d.name})
                if d.has_cleanup:
                    self.rec("E-MAT-002", f"class {d.name} has a cleanup and cannot be destructured", "LANG-09-06", head, {"missing_variants": [], "arm_set": d.name})
                mm = self.recv_subst(st, d)
                self.pattern_fields(pat, {k: self.subst(v[0], mm) for k, v in d.fields.items()}, env, moving, head, d.name, require_all=True)
                # D-79: a class destructure always matches only when every field pattern is irrefutable
                return set(), all(fp is None or self.irrefutable(fp) for fp in self.field_patterns(pat))
            if known(st):
                self.typ("E-TYP-040", f"pattern {head.text} against {st.show()}", head, st.show(), head.text, "pattern")
            else:
                for p in find_all(pat, "pattern"):
                    if p is not pat:
                        for b in self.names_in(p):
                            env.set(b, Unknown("pattern binding"), kind="pattern")
            return set(), False
        return set(), False

    def let_refutable(self, pat, st: Type):
        """The first part of a `let` pattern that can fail to match a value of `st`, or None (5.1; D-243). A `let`
        destructures a class, tuple or record, so a variant, a literal, `null`, `present(…)`, `success(…)`,
        `failure(…)` or `is …` pattern is refutable at any depth; `_` and a name always match. A pattern the typer
        has already rejected against its type is left to that record."""
        ts = direct_tokens(pat)
        subs = [c for c in children(pat) if name(c) == "pattern"]
        if not ts:
            return pat if children(pat) and name(children(pat)[0]) == "literal" else None
        head = ts[0]
        if head.text in ("_", "otherwise") or (len(ts) == 1 and head.kind == "LOWER"):
            return None
        if head.text == "(":
            items = list(st.items) if isinstance(st, TupleT) else [Unknown()] * len(subs)
            return next((r for r in (self.let_refutable(p, t) for p, t in zip(subs, items)) if r is not None), None)
        if head.text == "{":
            fields = dict(st.fields) if isinstance(st, RecordT) else {}
            keys = [t for t in ts if t.kind == "LOWER"]
            return next((r for r in (self.let_refutable(p, fields.get(k.text, Unknown())) for k, p in zip(keys, subs)) if r is not None), None)
        if head.kind == "UPPER":
            d = self.decl_of(st)
            if d is None or (d.kind == "class" and d.name != head.text):
                return None
            if d.kind != "class":
                return pat if d.kind == "enum" else None
            mm = self.recv_subst(st, d)
            pfn = find(pat, "patternFields")
            for pf in [c for c in children(pfn) if name(c) == "patternField"] if pfn is not None else []:
                pts = direct_tokens(pf)
                sub = find(pf, "pattern")
                if sub is None:
                    continue
                named = len(pts) > 1 and pts[0].kind == "LOWER" and pts[1].text == ":" and pts[0].text in d.fields
                r = self.let_refutable(sub, self.subst(d.fields[pts[0].text][0], mm) if named else Unknown())
                if r is not None:
                    return r
            return None
        return pat

    def field_patterns(self, pat) -> List:
        """The pattern of each field of a variant or class pattern (None for a field written without one)."""
        pfs = find(pat, "patternFields")
        return [find(pf, "pattern") for pf in children(pfs) if name(pf) == "patternField"] if pfs is not None else []

    def pattern_fields(self, pat, fields: Dict[str, Type], env: Env, moving: bool, tok: Token, label: str, require_all=False) -> List[Tuple[Set[str], bool]]:
        """Bind each field pattern; return each typed field pattern's (covered cases, is catch-all) in order (D-79)."""
        covs: List[Tuple[Set[str], bool]] = []
        pfn = find(pat, "patternFields")
        pfs = [c for c in children(pfn) if name(c) == "patternField"] if pfn is not None else []    # its own, not a nested pattern's
        names_ = list(fields)
        seen = set()
        for i, pf in enumerate(pfs):
            ts = direct_tokens(pf)
            sub = find(pf, "pattern")
            if ts and ts[0].kind == "LOWER" and len(ts) > 1 and ts[1].text == ":":
                fname = ts[0].text
                if fname not in fields:
                    self.rec("E-NAM-001", f"{label} has no field `{fname}`", "LANG-09-05", ts[0], {"name": fname, "candidates": names_})
                    continue
            else:
                if require_all:
                    self.typ("E-TYP-130", f"class destructure of {label} names every field", tokens(pf)[0], "field: pattern", "positional", label, "LANG-09-06")
                if i >= len(names_):
                    self.typ("E-TYP-130", f"{label} has {len(names_)} payload field{'s' if len(names_) != 1 else ''}", tokens(pf)[0], f"{len(names_)}", f"{i + 1}", label, "LANG-09-05")
                    continue
                fname = names_[i]
            seen.add(fname)
            covs.append(self.pattern(sub, fields[fname], env, moving))
        if require_all:
            missing = [f for f in names_ if f not in seen]
            if missing:
                self.typ("E-TYP-050", f"destructure of {label} lacks field{'s' if len(missing) > 1 else ''} {', '.join(missing)}", tok, ", ".join(missing), "absent", label, "LANG-09-06")
        return covs

    def names_in(self, pat) -> List[str]:
        out = []
        ts = direct_tokens(pat)
        words = [t.text for t in ts]
        if "as" in words:
            out.append(ts[words.index("as") + 1].text)
        if words and words[0] == "otherwise" and len(ts) >= 3:
            out.append(ts[2].text)
        if len(ts) == 1 and ts[0].kind == "LOWER" and ts[0].text != "_":
            out.append(ts[0].text)
        for sub in find_all(pat, "pattern"):
            if sub is not pat:
                out += self.names_in(sub)
        return out

    def if_expr(self, node, env: Env, expected) -> Type:
        results: List[Tuple[Type, Token]] = []
        entry = self.facts.copy()               # before the conditions' effects (as both checkers keep it)
        ends = self.if_expr_paths(node, env, expected, results)
        # D-76: each branch runs from its condition's state, and the expression's value comes from one of them; the state
        # after is the join of the branches that complete, never the last branch walked
        self.facts = Facts.join_all(ends) if ends else entry.copy()
        return self.unify_branches(results, "if-expression branch", expected)

    def if_expr_paths(self, node, env: Env, expected, results) -> List[Facts]:
        """Walk an if-expression's branches, each from the state its condition gives; return the end states of those
        that complete (D-76)."""
        ends: List[Facts] = []
        nxt = self.facts                       # the state the next block starts from
        other = self.facts                     # the state when every condition so far was false
        has_else = False
        for c in children(node):
            if isinstance(c, Token):
                if c.text == "else":
                    has_else = True
                continue
            if name(c) == "expr":
                self.facts = other
                nxt, other, _ = self.condition_states(c, env)
            elif name(c) == "block":
                self.facts = nxt
                self.ctx.append("ifexpr")
                self.arm_types.append([])
                self.exit_frames.append((self.depth, []))
                self.branch_expected.append(expected)
                self.walk_block(c, env)
                self.branch_expected.pop()
                gives = self.arm_types.pop()
                _, gave = self.exit_frames.pop()
                self.ctx.pop()
                falls = not self.terminates(c)             # D-76: an early `give` does not stop the branch falling through
                if falls:
                    self.typ("E-TYP-110", "if-expression branch does not end in `give`", tokens(c)[0], "give value", "no give", "if expression", "LANG-05-05")
                results += gives
                ends += gave                               # D-76: the state at every `give`, an early one included
                if falls:
                    ends.append(self.facts)
                nxt = other
            elif name(c) == "ifExpr":
                self.facts = other
                sub: List[Tuple[Type, Token]] = []
                ends += self.if_expr_paths(c, env, expected, sub)
                results.append((self.unify_branches(sub, "if-expression branch", expected), tokens(c)[0]))
        if not has_else:
            ends.append(other)
        return ends

    def condition(self, expr, env: Env):
        t = strip(self.infer(expr, env))
        if not known(t):
            return
        if base_name(t) != "Bool" or isinstance(t, OptionalT):
            tk = tokens(expr)[0]
            self.typ("E-TYP-010", f"condition has type {t.show()}, not Bool", tk, "Bool", t.show(), " ".join(x.text for x in tokens(expr)[:6]), "LANG-05-03")

    # ---- statements (5) -----------------------------------------------------------------
    def walk_block(self, block, env: Env):
        e = Env(env)
        self.depth += 1
        n_deferred = len(self.deferred)
        n_bodies = len(self.defer_bodies)
        stmts = [c for c in children(block) if name(c) == "statement"]
        for i, st in enumerate(stmts):
            self.block_rest = stmts[i + 1:]          # what runs after a `defer` registered here, before it (D-75)
            self.statement(st, e)
            self.temporaries_escape(st)
            self.own.end_statement()
            self.pending = None
            self.carried = []
        # D-75: the block's deferred bodies run at its exit, last registered first
        for _, blk in reversed(self.defer_bodies[n_bodies:]):
            self.deferred_effects(self.facts, blk)
        del self.defer_bodies[n_bodies:]
        if not self.terminates(block):
            self.report_open(self.depth, "at the end of the block")
        else:
            for p, (tok, d) in list(self.own.obligations.items()):
                if d >= self.depth:
                    del self.own.obligations[p]
        self.own.end_block(self.depth)
        del self.deferred[n_deferred:]           # the block's defers have run at its exit (12.1)
        self.guards = [g for g in self.guards if g[1] < self.depth]
        self.depth -= 1

    def statement(self, st, env: Env):
        inner = children(st)[0]
        n = name(inner)
        if n is None:
            self.bare_statement(st, env)
            return
        if n == "letStmt":
            self.let(inner, env)
        elif n == "assignStmt":
            self.assign(inner, env)
        elif n == "ifStmt":
            self.if_statement(inner, env)
        elif n == "whileStmt":
            blk = find(inner, "block")
            self.widen(blk)
            cond = find(inner, "expr")
            self.ctx.append("whilecond")              # the condition is evaluated on every iteration (11.6 counts it as the loop)
            yes, no, _ = self.condition_states(cond, env)
            self.ctx.pop()
            self.facts = yes
            breaks = self.loop_body(inner, blk, env)
            # D-72: the loop ends when the condition is false or at a `break` that targets it, with the state there
            self.facts = Facts.join_all([no] + breaks)
        elif n == "loopStmt":
            self.widen(find(inner, "block"))
            self.loop_body(inner, find(inner, "block"), env)
        elif n == "forStmt":
            self.for_stmt(inner, env)
        elif n == "deferStmt":
            self.defer_stmt(inner, env)
        elif n == "scopeStmt":
            e = Env(env)
            sname = direct_tokens(inner)[1].text
            e.set(sname, Named("Scope"), mutable=True, kind="mut")   # a scope handle (12.5)
            self.scope_depths.append((sname, self.depth + 1))
            self.walk_block(find(inner, "block"), e)
            self.scope_depths.pop()
        elif n == "captureStmt":
            self.capture_stmt(inner, env)
        elif n == "recordStmt":
            self.record_stmt(inner, env)
        elif n == "recordStmt_old":
            al = find(inner, "argList")
            if al is not None:
                self.infer(al, env)
        elif n == "expectStmt":
            self.condition(find(inner, "expr"), env)
        elif n == "exprStmt":
            self.expr_statement(inner, env)
        elif n == "block":
            self.walk_block(inner, env)
        else:
            for c in children(inner):
                if name(c) == "expr":
                    self.infer(c, env)

    def temporaries_escape(self, st):
        """A future that keeps a loan of a temporary argument may not outlive it (12.3): the temporary ends with its
        statement (5.7), so a statement that binds, assigns, returns or gives such a future is rejected (D-31)."""
        temps, self.carried_temps = self.carried_temps, []
        if not temps:
            return
        first = children(st)[0]
        n = name(first)
        words = [t.text for t in direct_tokens(st)]
        if n in ("letStmt", "assignStmt") or (n is None and words and words[0] in ("return", "give")):
            self.temporaries_kept(temps)

    def temporaries_kept(self, temps):
        for tok, _ in temps:
            self.own_err("E-OWN-051", "this argument is a temporary that ends with the statement, but the future that borrows it outlives the statement: bind it first, or take the parameter `owned` (12.3, 5.7)", tok, tok.text, "loan of a temporary kept by a future", "LANG-07-04")

    def widen(self, block):
        """At a loop header every place assigned or exclusively accessed in the body has its declared interval (6.3)."""
        for p in self.assigned_places(block):
            self.facts.invalidate(p)

    def if_statement(self, inner, env: Env):
        conds = [c for c in children(inner) if name(c) == "expr"]
        blocks = [c for c in children(inner) if name(c) == "block"]
        has_else = len(blocks) > len(conds)
        entry = self.facts.copy()               # before the conditions' effects (as both checkers keep it)
        ends: List[Facts] = []
        own_ends: List[Owners] = []
        own_entry = self.own
        fall = self.facts
        own_fall = own_entry
        for i, (cond, blk) in enumerate(zip(conds, blocks)):
            self.facts = fall
            self.own = own_fall.copy()
            pre = self.facts.copy()
            self.condition(cond, env)
            own_after_cond = self.own.copy()
            self.facts = pre
            yes, no, exact = self.narrow(cond, env)        # the condition's effects, applied once, are in both states
            self.facts = yes
            self.walk_block(blk, env)
            diverges = self.terminates(blk)
            if not diverges:
                ends.append(self.facts)
                own_ends.append(self.own)
            fall = no                                      # D-73: the false path keeps the condition's effects
            own_fall = own_after_cond
        if has_else:
            self.facts = fall
            self.own = own_fall.copy()
            self.walk_block(blocks[-1], env)
            if not self.terminates(blocks[-1]):
                ends.append(self.facts)
                own_ends.append(self.own)
        else:
            ends.append(fall)              # the diverging-branch rule: the rest of the block sees the negation
            own_ends.append(own_fall)
        if not ends:
            self.facts = entry.copy()
            self.own = own_fall
            return
        joined = ends[0]
        for f in ends[1:]:
            joined = joined.join(f)
        self.facts = joined
        oj = own_ends[0]
        for o in own_ends[1:]:
            oj = oj.join(o)
        self.own = oj
        self.report_half()

    def expr_statement(self, inner, env: Env):
        e = children(inner)[0]
        m = find(e, "matchExpr")
        # a match used as a statement: arms are blocks or Void expressions
        if m is not None and name(children(e)[0]) == "outcomeExpr" and len(tokens(e)) and tokens(e)[0].text == "match":
            self.match(m, env, None, as_statement=True)
            return
        if self.log_statement(inner, env):
            return
        t = self.infer(e, env)
        if self.is_linear(t) and self.path_of_expr(e) is None:
            self.obl_err("E-OBL-001", f"the {strip(t).show()} produced here is dropped at the end of the statement; consume it (or fail, or recover, catch, match, bind it)", tokens(e)[0], f"{tokens(e)[0].line}:{tokens(e)[0].col}", "statement result")
        elif known(strip(t)) and strip(t) not in (VOID, NEVER) and not self.is_linear(t) and find(e, "matchExpr") is None and not self.ignorable_call(e):
            self.typ("E-TYP-140", f"the value of this statement ({strip(t).show()}) is unused: write `discard` to drop it (5.5)", tokens(e)[0], "Void or `discard`", strip(t).show(), "expression statement", "LANG-05-08")

    def test_region(self, node) -> set:
        """The operators of a checked region that a test body's arithmetic starts here (14, 6.4; X-345): integer
        arithmetic not already inside a region (the left operand of `or` or `catch` is C's own), outside lambdas."""
        if not self.test_body or isinstance(node, Token):
            return set()
        n, kids = name(node), children(node)
        root = (n in ARITH_RULES and len(kids) > 1) or (n == "unaryExpr" and len(kids) == 2 and isinstance(kids[0], Token) and kids[0].text == "-")
        if not root:
            return set()
        ops = region_operators(node)
        if not ops or (self.regions and ops & self.regions[-1]["ops"]):
            return set()
        return ops

    def mark_consumed(self, node):
        """The single-child chain under a consuming operand: a call there keeps its outcome in a test body (14)."""
        n = node
        while n is not None and not isinstance(n, Token):
            self.consumed_ops.add(id(n))
            kids = children(n)
            if name(n) == "primaryExpr" and len(kids) == 3 and isinstance(kids[0], Token) and kids[0].text == "(":
                n = kids[1]                              # through parentheses: `(f()) or fail`
                continue
            n = kids[0] if len(kids) == 1 else None

    def implied_or_fail(self, node, t: Type, env: Env) -> Type:
        """In a `test` or `property` body, a call of a fallible function that nothing consumes is that call followed
        by `or fail` (14, LANG-14-03; X-345): it has its success type, and a failure fails the test."""
        tok = call_name_token(node)
        if tok is None:
            return t
        return self.unwrap_call(tok, t, env, self.path_of_expr(node))

    def unwrap_call(self, tok: Token, t: Type, env: Env, path: Optional[str] = None) -> Type:
        """The implied `or fail` of a test body's call named at `tok` (14): its success type, when the call fails."""
        s = strip(t)
        if not (self.test_body and isinstance(s, Named) and s.name == "Outcome" and s.args):
            return t
        sig = self.call_sigs.get((tok.line, tok.col))
        if sig is not None and "try" not in sig.modifiers:
            return t                                  # a function that returns an Outcome it built is not a failing call
        self.consume(path)
        self.eff.add("fail", "proved", tok)
        self.unwind_point(tok, "or fail", env)
        if self.defer_depth:
            self.def_err("E-DEF-001", "a failing call inside a deferred block: there is no outcome to propagate to (12.1)", tok, "or fail")
        self.implied_fails.append((tok.line, tok.col))
        return s.args[0]

    def log_statement(self, inner, env: Env) -> bool:
        """`log.info(m)`, `log.warn(m)`, `log.error(m)` (13, LANG-13-04; X-345): one `StringView` message, the `io`
        effect like `report` and no authority. `log` is the statement only where it names nothing in scope."""
        lc = log_call(inner)
        if lc is None or env.lookup("log") is not None or self.table.get("log") is not None:
            return False
        _, level, al = lc
        if level.text not in LOG_LEVELS:
            return True                             # the names layer reports the level (E-NAM-001)
        self.eff.add("io", "proved", level)           # the diagnostic plane, as `report` (10.6, 11.6)
        args = self.arg_nodes(al)
        if len(args) != 1 or any(t.text in (":", "...") for t in direct_tokens(args[0])):
            self.typ("E-TYP-040", f"`log.{level.text}` takes one message", level, "one StringView argument", f"{len(args)} arguments", f"log.{level.text}")
            for a in args:
                self.infer(self.arg_expr(a), env)
            return True
        ae = self.arg_expr(args[0])
        t = self.infer(ae, env, expected=STRING_VIEW)
        self.expect(t, STRING_VIEW, tokens(ae)[0], f"the message of `log.{level.text}`")
        return True

    def ignorable_call(self, e) -> bool:
        """The statement is a direct call of a function declared `@ignorable` (5.5, the prelude's aspect): the callee's
        declaring module resolved the name to the core aspect (`Sig.ignorable`; D-314), whatever it names here."""
        tok = call_name_token(e)
        if tok is None:
            return False
        sig = self.call_sigs.get((tok.line, tok.col))
        return sig is not None and sig.ignorable is not None

    def check_ignorable(self, sig: Sig, label: str):
        """`@ignorable` on a function whose result is linear is rejected (5.5, 11.8): a linear value is never dropped."""
        if sig.ignorable is None:
            return
        mods = self.expanded_mods(sig)
        if "try" in mods or "async" in mods or self.is_linear(sig.ret):
            what = "a `try` function's result is an Outcome" if "try" in mods else "an `async` function's result is a Future" if "async" in mods else f"{strip(sig.ret).show()} is linear"
            self.asp_err(f"@ignorable on {label}: {what}, and a linear result can never be dropped (5.5)", sig.ignorable, "a non-linear result", strip(sig.ret).show(), "ignorable")

    def bare_statement(self, st, env: Env):
        ts = direct_tokens(st)
        word = ts[0].text
        e = next((c for c in children(st) if name(c) == "expr"), None)
        if self.defer_depth and word in ("return", "give", "break", "continue"):
            self.def_err("E-DEF-001", f"`{word}` inside a deferred block: a defer cannot alter the enclosing function's outcome or leave the block (12.1)", ts[0], word)
            if e is not None:
                self.infer(e, env)
            return
        if word == "return":
            if "lambda" not in self.ctx:
                self.obs_exit("success", ts[0], f"the return at line {ts[0].line}")
            if e is not None and self.scope_depths and self.task_typed(e, env):
                self.rec("E-ASY-001", f"a Task leaves scope `{self.scope_depths[-1][0]}` through this return: a task is awaited inside its scope or detached (12.5)", "LANG-12-04", ts[0], {"offending_statement": "return", "rule_reference": "12.5"})
            if e is not None:
                self.pending = None
                local = self.returned_local(e, env)
                if local is not None:
                    self.in_marker = True              # its read is the move below, as in `return move x`
                recording = self.lends_result(self.ret_type)   # the parts of a `from` result are judged by their types (D-381)
                try:
                    t = self.infer(e, env, expected=self.ret_type)
                finally:
                    if local is not None:
                        self.in_marker = False
                if local is not None:
                    if self.is_bearing(t):
                        # X-345: `return x` of a local `let` binding is `return move x` (7.4, 7.5)
                        self.use_place(local, tokens(e)[0], "move", env)
                        self.pending = (local, "move", tokens(e)[0])
                    else:
                        self.use_place(local, tokens(e)[0], "read", env)
                self.consume(self.path_of_expr(e))
                if self.ret_type is not None:
                    self.expect(t, self.ret_type, tokens(e)[0], "returned value")
                    self.loan_into_owned(t, self.ret_type, tokens(e)[0], "the returned value")
                    self.owned_into_lent(t, self.ret_type, e, env)
                if recording:
                    self.node_types = None
                rp = self.path_of_expr(e)
                rt = self.ret_type
                while isinstance(rt, OptionalT) and isinstance(rt.inner, (LoanT, OptionalT)):
                    rt = rt.inner                         # `(alias T from self)?` names its origin inside the optional (7.6)
                origin = rt.origin if isinstance(rt, LoanT) else (rt.origin or None if isinstance(rt, FnT) else None)
                if origin is None and self.ret_type is not None:
                    nested = self.nested_origins(self.ret_type)
                    origin = next(iter(nested)) if len(nested) == 1 else None   # `((alias T from xs)?, Int)`
                # a loan held inside an optional, a tuple or a record is returned only when the result lends one; an
                # owned result was reported by loan_into_owned (D-341, D-348)
                lends = self.ret_type is not None and self.lent(self.ret_type) is not None
                loan_roots: List[str] = []
                if self.pending is not None:
                    ppath, mk, mtok = self.pending
                    self.pending = None
                    if mk == "move":
                        for l in self.own.loans:
                            if l.holder == root_of(ppath) and (lends or not l.nested):
                                loan_roots.append(root_of(l.owner))   # a moved closure or lifetime value keeps its loans
                        self.own.move(ppath)
                    elif mk in ("alias", "editable"):
                        loan_roots.append(root_of(self.own.owner_path(ppath)))
                elif rp is not None and self.is_bearing(t) and find(e, "markerExpr") is None and not isinstance(t, LoanT):
                    self.own_err("E-OWN-001", f"`return {rp}`: an existing ownership-bearing place needs `move {rp}`, `copy {rp}` or a `from` signature with `alias {rp}`", tokens(e)[0], rp, "marker required", "LANG-07-01")
                elif rp is not None and root_of(rp) in self.own.holders and self.loan_carrying(t):
                    loan_roots.append(root_of(self.own.holders[root_of(rp)].owner))
                for l in self.carried:
                    if lends or not l.nested:
                        loan_roots.append(root_of(l.owner))
                self.carried = []
                if origin is None and loan_roots:
                    for r in loan_roots:
                        b = env.lookup(r)
                        if b is not None and b.kind in ("param", "editable", "owned") or r == "self":
                            self.own_err("E-OWN-053", f"the returned value keeps a loan of `{r}` but the result type names no origin: write `… from {r}`", tokens(e)[0], r, "loan-carrying result without an origin", "LANG-07-06")
                        else:
                            self.own_err("E-OWN-051", f"the returned value keeps a loan of `{r}`, a local or temporary that ends with the function", tokens(e)[0], r, "loan of a local returned", "LANG-07-06")
                if origin is not None and (origin in ("static", "self") or env.lookup(origin) is not None):
                    for r in loan_roots:
                        if r == origin:
                            continue
                        b = env.lookup(r)
                        if b is not None and b.kind in ("param", "editable", "owned") or r == "self":
                            self.own_err("E-OWN-050", f"the result is declared `from {origin}` but this return keeps a loan of `{r}`: a result may name one origin", tokens(e)[0], r, f"second origin {r}", "LANG-07-06")
                        else:
                            self.own_err("E-OWN-051", f"the result keeps a loan of `{r}`, a local or temporary that ends with the function", tokens(e)[0], r, "loan of a local returned", "LANG-07-06")
            elif self.ret_type is not None and known(self.ret_type) and self.ret_type != VOID:
                self.typ("E-TYP-040", f"bare `return` in a function returning {self.ret_type.show()}", ts[0], self.ret_type.show(), "Void", "return", "LANG-05-05")
            self.report_open(0, "at this return")
            return
        if word == "give":
            self.pending = None
            t = self.branch_infer(e, env)
            self.branch_value(e, t, "give")
            if not self.ctx or self.ctx[-1] not in ("arm", "ifexpr"):
                self.typ("E-TYP-120", "`give` outside a match arm or if-expression branch" + (" (a catch block ends with `recover`)" if self.ctx and self.ctx[-1] == "catch" else ""), ts[0], "a match arm or if-expression branch", self.ctx[-1] if self.ctx else "function body", "give", "LANG-05-05")
                return
            self.arm_types[-1].append((t, tokens(e)[0]))
            if self.exit_frames:
                d, states = self.exit_frames[-1]
                states.append(self.cleanup_state(d))      # D-76: the expression's value leaves from here
            return
        if word == "recover":
            self.pending = None
            t = self.branch_infer(e, env)
            self.branch_value(e, t, "recover")
            if not self.ctx or self.ctx[-1] != "catch":
                self.typ("E-TYP-120", "`recover` outside a catch block", ts[0], "a catch block", self.ctx[-1] if self.ctx else "function body", "recover", "LANG-05-05")
                return
            if self.catch_bound and self.catch_bound[-1] not in self.reported:
                self.discharges.append({"line": ts[0].line, "kind": "recover without report"})   # 10.4: the recovers effect
                self.eff.add("recovers", "proved", ts[0])
            self.arm_types[-1].append((t, tokens(e)[0]))
            if self.exit_frames:
                d, states = self.exit_frames[-1]
                states.append(self.cleanup_state(d))      # D-76: the catch expression's value leaves from here
            return
        if word in ("break", "continue"):
            if "loop" not in self.ctx:
                self.typ("E-TYP-120", f"`{word}` outside a loop", ts[0], "a loop body", self.ctx[-1] if self.ctx else "function body", word, "LANG-05-05")
            elif len(ts) > 1 and ts[1].text not in self.loop_labels:
                self.rec("E-NAM-001", f"no enclosing loop is labelled `{ts[1].text}`", "LANG-04-05", ts[1], {"name": ts[1].text, "candidates": [l for l in self.loop_labels if l]})
            elif word == "break" and self.break_frames:
                target = ts[1].text if len(ts) > 1 else None
                frame = next((f for f in reversed(self.break_frames) if target is None or f[0] == target), None)
                if frame is not None:
                    frame[2].append(self.cleanup_state(frame[1]))   # D-72: the loop is left from here
            return
        if word == "report":
            self.eff.add("io", "proved", ts[0])              # report has the io effect (10.6)
            t = strip(self.infer(e, env))
            rp = self.path_of_expr(e)
            if rp is not None:
                self.use_place(rp, tokens(e)[0], "move", env)
                self.own.move(rp)                    # a reported failure is unreachable afterwards (10.6)
            self.consume(rp)
            for b in self.catch_bound:
                if rp == b or self.own.is_moved(b):
                    self.reported.add(b)             # the bound failure (or a value built from it) was reported
            if known(t) and base_name(t) not in ("Failure", "Report"):
                self.typ("E-TYP-040", f"`report` takes a Failure, got {t.show()}", tokens(e)[0], "Failure", t.show(), "report")
            if "as" in [t.text for t in ts]:
                env.set(ts[[t.text for t in ts].index("as") + 1].text, Named("Report"), kind="let")
            return
        if word == "discard":
            t = self.infer(e, env)
            if isinstance(strip(t), Named) and strip(t).name == "Future" and self.path_of_expr(e) is None and not [m for m in find_all(e, "markerExpr") if tokens(m)[0].text in ("editable", "move")]:
                return                              # a never-polled future: the one linear value that may be discarded (12.3)
            if self.is_linear(t):
                self.obl_err("E-OBL-001", f"`discard` of a linear value ({strip(t).show()}): a linear value is consumed, never discarded (5.5)", ts[0], f"{ts[0].line}:{ts[0].col}", self.path_of_expr(e) or "discarded value", "LANG-05-08")
            return
        if e is not None:
            self.infer(e, env)

    def let(self, inner, env: Env):
        ts = direct_tokens(inner)
        words = [t.text for t in ts]
        pat = next((c for c in children(inner) if name(c) == "pattern"), None)
        if pat is not None:
            pl = next((c for c in children(inner) if name(c) == "place"), None)
            pt = strip(self.place_type(pl, env)) if pl is not None else Unknown()
            if pl is not None:
                pp = self.path_of_place(pl)
                self.use_place(pp, tokens(pl)[0], "move", env)
                if self.use_ok:
                    self.own.move(pp)
            self.pattern(pat, pt, env, moving=True)
            bad = self.let_refutable(pat, pt) if known(pt) else None
            if bad is not None:
                # a `let` destructures a class, tuple or record: its pattern matches every value (5.1; D-243)
                self.rec("E-MAT-004", f"a `let` pattern must match every value of {pt.show()}, but `{tokens(bad)[0].text}` can fail to match: use `match` to handle the other cases",
                         "LANG-05-01", tokens(bad)[0], {"missing_variants": [], "arm_set": pt.show()})
            for b in self.names_in(pat):
                bt = env.get(b)
                if bt is not None and self.is_linear(bt):
                    self.obligate(b, tokens(pat)[0])
            return
        ann = next((c for c in children(inner) if name(c) == "type"), None)
        e = next((c for c in children(inner) if name(c) == "expr"), None)
        mutable = "mut" in words
        nm = next(tk for tk in ts if tk.kind == "LOWER")
        self.pending = None
        if ann is not None:
            want = self.dc.type_of(ann)
            have = self.infer(e, env, expected=want)
            if have == INTERP and strip(want) == STRING_VIEW:
                self.typ("E-TYP-040", "an interpolated literal is a fresh String; a binding annotated StringView cannot hold it (3.5)", tokens(e)[0], "String", "an interpolated literal", nm.text, "LANG-03-05")
            else:
                self.expect(have, want, tokens(e)[0], "initialiser")
                self.loan_into_owned(have, want, tokens(e)[0], f"`{nm.text}`")
            t = self.with_closure(want, have)
            self.set_fact(nm.text, have)
        else:
            have = concrete(self.infer(e, env))
            t = have
            if isinstance(t, Refined) and t.kind in ("exact", "range") and self.is_int(t):
                t = t.base                  # the binding's type is the base; the interval is a fact
            if t == OptionalT(NEVER):
                self.typ("E-TYP-040", f"`let {nm.text} = null` needs a type annotation", nm, "T?", "null", nm.text, "LANG-06-08")
            self.set_fact(nm.text, have)
        self.own.reinit(nm.text)
        self.own.obligations.pop(nm.text, None)
        env.set(nm.text, t, mutable=mutable, kind="mut" if mutable else "let")
        env.lookup(nm.text).depth = self.depth
        cd = self.decl_of(strip(t)) if known(strip(t)) else None
        if cd is not None and cd.has_cleanup:
            self.eff.imports(self.summaries.get(f"cleanup {cd.name}", Summary()), nm)   # dropping the value runs its cleanup (11.3)
        if isinstance(strip(t), Named) and strip(t).name == "MutexGuard":
            self.guards.append((nm.text, self.depth, nm))
        if find(e, "markerExpr") is None and not isinstance(have, LoanT) and not (isinstance(strip(have), Named) and strip(have).name in ("StringView", "BytesView", "ListView", "ListCursor")):
            self.fresh.add(nm.text)                   # a constructor, literal or owned call result is fresh storage (11.3)
        else:
            self.fresh.discard(nm.text)
        self.bind_initialiser(nm.text, e, have if ann is None else self.infer_type_cache(e, have), env)
        if self.is_linear(t):
            self.obligate(nm.text, nm)
        elif self.is_linear(have) and self.path_of_expr(e) is None:
            self.obligate(nm.text, nm)

    def infer_type_cache(self, e, t):
        return t

    def bind_initialiser(self, binding: str, e, t: Type, env: Env):
        """`let x = …`: a bare ownership-bearing place needs a marker (7.5); a marked place binds its loan or move."""
        path = self.path_of_expr(e)
        if self.last_closure is not None and find(e, "lambda") is not None:
            self.closures[binding] = self.last_closure
            if self.last_closure.get("linear"):
                self.own.obligations[binding] = (tokens(e)[0], self.depth)   # a linear closure is called exactly once (8.3)
        self.last_closure = None
        self.keep_nested(binding, self.depth, t, e)   # a loan inside an optional, a tuple or a record (7.3; D-348)
        for l in self.carried:                       # a `from` result or view: the kept loan is held by the binding (7.6)
            l.holder, l.depth = binding, self.depth
            inner = strip(t)
            lifetime_value = isinstance(inner, Named) and (d2 := self.decl_of(inner)) is not None and d2.kind == "class" and d2.lifetime is not None
            if not lifetime_value and (isinstance(t, LoanT) or (isinstance(inner, Named) and inner.name in ("StringView", "BytesView", "ListView", "ListCursor"))):
                self.own.holders.setdefault(binding, l)   # a loan binding is a path to the owner; a loan-carrying value is owned (7.7)
        self.carried = []
        if self.pending is not None:
            ppath, mk, mtok = self.pending
            self.pending = None
            if mk in ("alias", "editable") and self.use_ok:
                self.own.add_loan(ppath, mk, binding, self.depth, mtok.line)
            elif mk == "move" and self.use_ok:
                self.own.move(ppath)
            return
        if path is not None and self.is_bearing(t) and find(e, "markerExpr") is None:
            self.own_err("E-OWN-001", f"`{path}` is an existing ownership-bearing place: write `copy {path}`, `alias {path}` or `move {path}`", tokens(e)[0], path, "marker required", "LANG-07-01")

    def branch_infer(self, e, env: Env) -> Type:
        """A `give` or `recover` value: typed as before; when the branch's target type is known (an expected type, or
        the success type a catch recovers to), a generic call in it takes that type, so it is no inference failure."""
        target = self.branch_expected[-1] if self.branch_expected else None
        was = self.branch_target
        self.branch_target = target is not None and known(strip(target))
        try:
            return self.infer(e, env, expected=target)
        finally:
            self.branch_target = was

    def settle_element(self):
        """An aggregate element has finished. A `move` is consumed now, so the next element cannot drop the marker
        (a match arm used to clear it); `copy` is done. A loan stays pending for the binding that receives the value."""
        if self.pending is None:
            return
        ppath, mk, _mtok = self.pending
        if mk == "move":
            self.pending = None
            if self.use_ok:
                self.own.move(ppath)
        elif mk == "copy":
            self.pending = None

    def own_outside_arm(self, holders, obligations) -> Owners:
        """Ownership after a guard, without this arm's pattern loans, for the paths that do not take the arm."""
        o = self.own.copy()
        o.loans = [l for l in o.loans if l.holder not in holders]
        for b in holders:
            o.holders.pop(b, None)
        for b in obligations:
            o.obligations.pop(b, None)
        return o

    def snapshot_own(self, o: Owners) -> Owners:
        """`Owners.copy` clears a half-obligation. A match snapshot keeps it until `report_half`."""
        s = o.copy()
        s.half = list(o.half)
        return s

    def retain_half(self, previous: Owners, new: Owners) -> Owners:
        """A half-obligation already found on `previous` stays on `new`. `Owners.join` keeps only its own pair."""
        seen = {p for p, _ in new.half}
        for p, tok in previous.half:
            if p not in seen and p not in new.obligations:
                new.half.append((p, tok))
                seen.add(p)
        return new

    def join_own(self, left: Owners, right: Owners) -> Owners:
        """Join two arm states. A half-obligation from either side stays one: `Owners.join` keeps only the latest pair."""
        joined = left.join(right)
        self.retain_half(left, joined)
        self.retain_half(right, joined)
        return joined

    def branch_value(self, e, t, word):
        """`give` and `recover` hand their value to the enclosing binding: an existing ownership-bearing place
        needs a marker, and `move` moves it on this path (7.1; X-120)."""
        rp = self.path_of_expr(e)
        if self.pending is not None:
            ppath, mk, mtok = self.pending
            if mk == "move":
                self.pending = None
                self.own.move(ppath)
        elif rp is not None and self.is_bearing(t) and find(e, "markerExpr") is None and not isinstance(t, LoanT):
            self.own_err("E-OWN-001", f"`{word} {rp}`: an existing ownership-bearing place needs `move {rp}` or `copy {rp}`", tokens(e)[0], rp, "marker required", "LANG-07-01")

    def assign(self, inner, env: Env):
        pl = find(inner, "place")
        e = find(inner, "expr")
        op = next(t for t in direct_tokens(inner) if t.text in ("=", "+=", "-=", "*=", "/=", "%="))
        ts = direct_tokens(pl)
        root = ts[0]
        b = env.lookup(root.text) if root.text != "self" else None
        target_t = self.place_type(pl, env)
        if root.text == "self":
            if self.self_mode != "editable self" and len(ts) > 1:
                self.rec("E-NAM-002", f"assignment to `{''.join(t.text for t in ts)}` through a read `self`: the method needs `editable self`", "LANG-05-01", root, {"name": root.text, "candidates": ["editable self"]})
            elif len(ts) == 1:
                self.rec("E-NAM-002", "`self` cannot be rebound", "LANG-05-01", root, {"name": "self", "candidates": []})
        elif b is None:
            pass                            # the resolver reports E-NAM-001
        elif len(ts) == 1:
            if not b.mutable:
                self.rec("E-NAM-002", f"`{root.text}` is immutable" + (" (a parameter cannot be reassigned)" if b.kind in ("param", "editable", "owned") else ""), "LANG-05-01", root, {"name": root.text, "candidates": ["let mut"]})
            elif b.kind == "capture-copy":
                self.rec("E-NAM-002", f"`{root.text}` is a `copy` capture: read-only in the closure body (8.2); capture it with `move` into a binding the body may edit", "LANG-05-01", root, {"name": root.text, "candidates": ["move"]})
        else:
            if not (b.mutable or b.kind == "editable" or (isinstance(b.type, LoanT) and b.type.mode == "editable")):
                self.rec("E-NAM-002", f"`{root.text}` is not mutable: `{''.join(t.text for t in ts)}` cannot be assigned", "LANG-05-01", root, {"name": root.text, "candidates": ["let mut", "editable"]})
        path = self.path_of_place(pl)
        self.pending = None
        rt = self.infer(e, env, expected=target_t)
        if op.text != "=":
            rt = self.arith(Token("PUNCT", op.text[0], op.line, op.col), target_t, rt)
        if b is not None and b.kind.startswith("capture-") and b.kind != "capture-alias" and not (b.kind == "capture-copy" and len(ts) == 1) and self.lambda_kind and self.lambda_kind[-1] == "fn":
            self.rec("E-FUN-010", f"this `fn` lambda mutates its capture `{root.text}` ({b.kind[8:]}): declare it `fn mut`", "LANG-08-02", root, {"violation": "mutation in a fn", "capture_span": root.text, "use_span": root.text})
        self.use_place(path, root, "write", env)
        self.note_write(path, root, env)
        if self.scope_depths and "." not in path and isinstance(strip(rt), Named) and strip(rt).name == "Task":
            b = env.lookup(path)
            if b is not None and getattr(b, "depth", None) is not None and b.depth < self.scope_depths[-1][1]:
                self.rec("E-ASY-001", f"a Task is stored in `{path}`, declared outside scope `{self.scope_depths[-1][0]}`: a task is awaited inside its scope or detached (12.5)", "LANG-12-04", root, {"offending_statement": "assignment", "rule_reference": "12.5"})
        if path in self.own.obligations and not self.own.is_moved(path):
            tok0 = self.own.obligations[path][0]
            self.obl_err("E-OBL-001", f"the linear value in `{path}` is overwritten without being consumed", root, f"{tok0.line}:{tok0.col}", path)
            self.consume(path)
        elif "." in path and self.is_linear(strip(self.declared_type(pl, env))) and not self.own.is_moved(path):
            self.obl_err("E-OBL-001", f"the linear value in field `{path}` is overwritten without being consumed", root, f"{root.line}:{root.col}", path)
        if "." not in path and self.use_ok:
            old = [l for l in self.own.loans if l.holder == root.text]
            self.own.loans = [l for l in self.own.loans if l.holder != root.text]   # the old value's stored loans end with it (7.9)
            self.assign_nested(root, b, rt, e, old, env)
        if self.pending is not None:
            ppath, mk, mtok = self.pending
            self.pending = None
            if mk == "move":
                self.own.move(ppath)
            elif mk in ("alias", "editable"):
                self.own_err("E-OWN-011", f"a loan (`{mk} {ppath}`) cannot be stored by assignment; bind it with `let`", mtok, ppath, "loan assigned", "LANG-07-05")
        elif self.path_of_expr(e) is not None and self.is_bearing(rt) and find(e, "markerExpr") is None:
            self.own_err("E-OWN-001", f"`{self.path_of_expr(e)}` is an existing ownership-bearing place: write `copy`, `alias` or `move`", tokens(e)[0], self.path_of_expr(e), "marker required", "LANG-07-01")
        self.own.reinit(path)
        if self.is_linear(rt) and "." not in path:
            self.obligate(path, root)         # a linear field is owed through its owner
        decl_t = strip(self.declared_type(pl, env))
        if isinstance(decl_t, Refined) and decl_t.kind == "length" and len(ts) > 1:
            self.ref("E-REF-003", f"`{path}` has a length refinement {decl_t.show()} and may not be mutated; mutate an unrefined collection and validate the result", root, "violated", decl_t.show(), "")
        if known(decl_t):
            self.expect(rt, decl_t, tokens(e)[0], "assigned value")
            self.loan_into_owned(rt, self.declared_type(pl, env), tokens(e)[0], f"`{path}`")
        # the first lambda stored in a closure binding pins it, including after `= null` (8.4)
        if op.text == "=" and "." not in path and b is not None:
            b.type = self.with_closure(b.type, rt)
        if op.text == "=" and "." in path:
            self.note_field_assign(pl, rt, tokens(e)[0], env)
        self.set_fact(path, rt)

    def assign_nested(self, root: Token, b: Optional[Binding], rt: Type, e, old: List[Loan], env: Env):
        """`t = e` where the new value holds a loan inside an optional, a tuple or a record (`t = xs.get(1)`): `t`
        holds its loans until the end of the block that declares `t`, and the old ones too when `e` reads `t` (the new
        value can be the old one: `t = t ?? xs.get(1)`). An owner declared in a deeper block than `t` is dropped while
        `t` still lends it: E-OWN-011 (7.3, 7.9; D-348)."""
        depth = b.depth if b is not None and b.kind in ("let", "mut") else self.depth
        held = self.keep_nested(root.text, depth, rt, e)
        if old and self.holds_loan(rt) and any(tk.text == root.text for tk in tokens(e)):
            self.own.loans += old
        for l in held:
            ob = env.lookup(root_of(l.owner))
            if ob is not None and ob.kind in ("let", "mut") and ob.depth > depth:
                self.own_err("E-OWN-011", f"`{root.text}` keeps a loan of `{l.owner}`, which is dropped at the end of its block while `{root.text}` lives on", root, l.owner,
                             f"{l.mode} loan held by {root.text}", "LANG-07-05", f"until the end of {root.text}'s block")
                break

    def declared_type(self, pl, env: Env) -> Type:
        """A place's declared type, without the current facts."""
        saved = self.facts
        self.facts = Facts()
        try:
            return self.place_type(pl, env)
        finally:
            self.facts = saved

    def loop_body(self, inner, block, env: Env, fresh: Sequence[str] = ()) -> List[Facts]:
        """Walk a loop body; return the states at the `break`s that leave this loop (D-72)."""
        lab = find(inner, "label")
        self.loop_labels.append(tokens(lab)[0].text if lab is not None else None)
        own_lab = next((c for c in children(inner) if name(c) == "label"), None)          # the loop's own label
        self.break_frames.append((tokens(own_lab)[0].text if own_lab is not None else None, self.depth, []))
        self.ctx.append("loop")
        entry = self.own.copy()
        header = self.facts.copy()
        self.walk_block(block, env)
        # a second pass sees the body with the ownership state a later iteration starts from (moves on any path);
        # its facts are the header's again (widening and the condition's narrowing already applied)
        self.own = entry.join(self.own)
        for b in fresh:
            self.own.reinit(b)                        # the `for` binding is a fresh binding on every iteration (5.3)
        self.facts = header.copy()
        self.walk_block(block, env)
        self.facts = header
        self.ctx.pop()
        self.loop_labels.pop()
        return self.break_frames.pop()[2]

    def for_stmt(self, inner, env: Env):
        ts = direct_tokens(inner)
        var = ts[[t.text for t in ts].index("for") + 1]
        moving = "move" in [t.text for t in ts]
        src = next((c for c in children(inner) if name(c) in ("expr", "rangeExpr", "place")), None)
        elem: Type = Unknown("loop variable")
        if name(src) == "rangeExpr":
            self.infer(src, env)
            rk = children(src)
            lo_t, hi_t = self.infer(rk[0], env), self.infer(rk[2], env)
            li, hi_ = self.interval_of(lo_t), self.interval_of(hi_t)
            elem = INT
            if li is not None and hi_ is not None:
                top = hi_[1] - (1 if rk[1].text == "..<" else 0)
                if li[0] <= top:
                    elem = Refined(INT, li[0], top)      # the index has the interval of the range (5.3, 6.3)
        else:
            st = strip(self.infer(src, env) if name(src) != "place" else self.place_type(src, env))
            if isinstance(st, Refined): st = st.base
            if isinstance(st, Named) and st.name in ("List", "ListView", "Array", "Set", "FixedList") and st.args:
                elem = st.args[0]
            elif isinstance(st, Named) and st.name in ("Map", "OrderedMap", "FixedMap") and len(st.args) >= 2:
                elem = TupleT((st.args[0], st.args[1]))
            elif known(st):
                d = self.decl_of(st)
                iterable = d is not None and any(i.name in ("Iterable", "Consumable") for i in d.implements)
                if iterable:
                    it = next(i for i in d.implements if i.name in ("Iterable", "Consumable"))
                    elem = self.subst(it.args[0], self.recv_subst(st, d)) if it.args else Unknown()
                else:
                    self.typ("E-TYP-040", f"`for` over {st.show()}, which is not Iterable", tokens(src)[0], "Iterable[T, I]", st.show(), "for")
        e = Env(env)
        self.widen(find(inner, "block"))
        src_path = self.path_of_place(src) if name(src) == "place" else self.path_of_expr(src)
        if src_path is not None and name(src) != "rangeExpr":
            if moving:
                self.use_place(src_path, tokens(src)[0], "move", env)
                self.own.move(src_path)
            elif self.is_bearing(st if 'st' in dir() else Unknown()):
                self.own.add_loan(src_path, "alias", f"for@{tokens(src)[0].line}", self.depth + 1, tokens(src)[0].line)   # read loan for the whole loop (7.5)
        if var.text != "_":
            e.set(var.text, elem, mutable=False, kind="for")
            self.own.reinit(var.text)                     # a new binding: a moved earlier binding of the name is gone (5.3)
            self.set_fact(var.text, elem)
            if src_path is not None and not moving and name(src) != "rangeExpr" and self.is_bearing(elem):
                self.own.holders[var.text] = self.own.add_loan(src_path, "alias", var.text, self.depth + 1, tokens(src)[0].line)   # item is a read loan of each element (5.3)
            if iv.RANGED_LENGTH_FACT and name(src) == "rangeExpr" and children(src)[1].text == "..<":
                cp = self.count_receiver(children(src)[2])
                # D-71: the body must neither assign nor exclusively loan `p`, a prefix of it or a field of it
                if cp is not None and not any(iv.overlaps(q, cp) for q in self.assigned_places(find(inner, "block"))):
                    self.facts.below[var.text] = cp
        self.loop_body(inner, find(inner, "block"), e, fresh=[var.text] if var.text != "_" else [])

    # ---- function exits (5.4) ------------------------------------------------------------
    def terminates(self, block, gives: bool = True) -> bool:
        """Whether the block's end is unreachable. `give` ends only its own match arm or if-expression branch (5.5), so
        it ends the block when `gives` (the block is in that arm or branch, not in a match nested in it, D-76)."""
        sts = [c for c in children(block) if name(c) == "statement"]
        if not sts:
            return False
        last = sts[-1]
        inner = children(last)[0]
        n = name(inner)
        if n is None:
            w = direct_tokens(last)[0].text
            return w in ("return", "fail", "recover", "break", "continue") or (w == "give" and gives)
        if n == "ifStmt":
            blocks = [c for c in children(inner) if name(c) == "block"]
            has_else = sum(1 for c in children(inner) if isinstance(c, Token) and c.text == "else") > sum(1 for c in children(inner) if isinstance(c, Token) and c.text == "if") - 1
            return has_else and all(self.terminates(b, gives) for b in blocks)
        if n == "loopStmt":
            return not any(t.text == "break" for t in tokens(inner))
        if n == "exprStmt":
            e = children(inner)[0]
            ts = tokens(e)
            if ts and ts[0].text == "fail":
                return True
            m = find(e, "matchExpr")
            if m is not None and ts and ts[0].text == "match":
                arms = [_a for _a in children(m) if name(_a) == "matchArm"]
                return bool(arms) and all(self.arm_terminates(a, gives=False) for a in arms)
            return self.never_call(e)
        if n == "block":
            return self.terminates(inner, gives)
        if n == "scopeStmt":
            return self.terminates(find(inner, "block"), gives)
        return False

    def arm_terminates(self, arm, gives: bool = True) -> bool:
        body = [c for c in children(arm) if name(c) in ("expr", "block")][-1]
        if name(body) == "block":
            # in a nested match, a `give` anywhere in the arm continues after the match (D-76)
            return not (not gives and self.gives_to(body)) and self.terminates(body, gives)
        ts = tokens(body)
        return bool(ts) and (ts[0].text == "fail" or self.never_call(body))

    def gives_to(self, node) -> bool:
        """Whether the node holds a `give` that ends the arm or branch the node is in, not one nested in it (5.5)."""
        for c in children(node):
            if isinstance(c, Token):
                if c.text == "give":
                    return True
            elif name(c) not in ("matchExpr", "ifExpr", "lambda") and self.gives_to(c):
                return True
        return False

    def never_call(self, e) -> bool:
        """A call whose declared result is Never."""
        pe = find(e, "postfixExpr")
        if pe is None:
            return False
        kids = children(pe)
        pk = children(kids[0])
        if len(pk) == 1 and isinstance(pk[0], Token) and pk[0].kind == "LOWER":
            fd = self.table.get(pk[0].text)
            return fd is not None and fd.kind == "func" and fd.sig is not None and fd.sig.ret == NEVER
        tr = find(kids[0], "typeRef")
        mn = find(kids[0], "memberName")
        if tr is not None and mn is not None:
            d = self.decl_of(self.dc.type_of_ref(tr))
            sig = d.methods.get(tokens(mn)[0].text) if d is not None else None
            return sig is not None and sig.ret == NEVER
        return False

    def body(self, body, env: Env, ret: Type, head_tok: Token, label: str):
        b = children(body)[0] if body is not None and children(body) and name(children(body)[0]) == "block" else None   # the body itself, not a lambda block inside an expression body
        if b is not None:
            self.walk_block(b, env)
            if known(ret) and ret != VOID and not self.terminates(b):
                self.typ("E-TYP-110", f"{label} returns {ret.show()} but a path reaches the end of its body without `return`, `fail` or a Never call", head_tok, ret.show(), "fall-through", label, "LANG-05-05")
            elif not self.terminates(b):
                self.obs_exit("success", head_tok, "the end of the body")
        else:
            e = find(body, "expr")
            self.carried_temps = []
            recording = self.lends_result(ret)                   # the parts of a `from` result are judged by their types (D-381)
            t = self.infer(e, env, expected=ret)
            # the body's value is returned: a future in it that borrows a temporary outlives it (12.3; D-31)
            temps, self.carried_temps = self.carried_temps, []
            self.temporaries_kept(temps)
            if known(ret):
                self.expect(t, ret, tokens(e)[0], f"{label} result")
                self.loan_into_owned(t, ret, tokens(e)[0], f"the {label} result")
                self.owned_into_lent(t, ret, e, env)
            if recording:
                self.node_types = None
            # an expression body is the return of its expression: an existing place needs a marker (7.1; X-118)
            rp = self.path_of_expr(e)
            if rp is not None and self.is_bearing(t) and find(e, "markerExpr") is None and not isinstance(t, LoanT):
                self.own_err("E-OWN-001", f"`=> {rp}`: an existing ownership-bearing place needs `move {rp}`, `copy {rp}` or a `from` signature with `alias {rp}`", tokens(e)[0], rp, "marker required", "LANG-07-01")
            self.obs_exit("success", tokens(e)[0], "the expression body")
            return t
        return None

    def params_env(self, sig: Sig, env: Env) -> Env:
        e = Env(env)
        for pn, pt, mode in sig.params:
            e.set(pn, pt, mutable=False, kind={"editable": "editable", "owned": "owned"}.get(mode.lstrip("."), "param"))
        return e

    # ---- module ------------------------------------------------------------------------
    def run(self, mtree):
        """Two passes over the module: the first computes every body's summary with callees at their
        written claims, the second re-types with the computed summaries and reports (the fixed point of
        11.3 for the finite effect set converges in two rounds for non-recursive modules; recursion keeps
        the written claims)."""
        del iv.LOG[:]                       # the invalidation log is per module check (D-74)
        saved_records = list(self.records)
        self.final_pass = False
        self.run_once(mtree)
        del self.records[len(saved_records):]
        self.final_pass = True
        return self.run_once(mtree)

    def run_once(self, mtree):
        self.elaborations = []
        for imp in tree_mod.imports(mtree):
            path = ".".join(imp["path"])
            if imp["members"] is None:
                self.aliases[imp["alias"] or imp["path"][-1]] = path
            else:
                for m in imp["members"]:
                    d = self.table.get(m)
                    if d is not None and d.module == path and not d.exported and d.kind != "unknown":
                        self.rec("E-NAM-004", f"{path} does not export `{m}`", "LANG-04-05", imp["token"], {"name": m, "candidates": sorted(k for k, v in self.table.decls.items() if v.module == path and v.exported)})
        for d in [d for d in self.table.decls.values() if d.module == self.module]:
            if d.kind == "func" and d.sig is not None:
                if d.exported:
                    self.check_exported_sig(d.sig, d.name, self.dc.tok_of(d))
                self.check_result_origin(d.sig, d.name, self.dc.tok_of(d))
                self.check_param_defaults(d.sig, d.name)
                self.check_ignorable(d.sig, d.name)
                if d.sig.body is not None:
                    self.current_aspects = list(d.aspects)
                    self.run_body(d.sig, None, "self", self.dc.tok_of(d), f"function {d.name}")
                    self.current_aspects = []
            elif d.kind == "aspect" and self.final_pass:
                self.check_aspect_decl(d)
                ap = tree_mod.child(d.node, "aspectParams") if d.node is not None else None
                if ap is not None:
                    self.check_defaults([c for c in children(ap) if name(c) == "param"], {pn: pt for pn, pt, _ in d.aspect["params"]}, f"aspect {d.name}")
            elif d.kind in ("class", "enum"):
                self_t = d.as_named(tuple(TypeParam(p) for p in d.type_params))
                if d.lifetime is not None:
                    self.own_err("E-OWN-053", f"{d.name} declares the lifetime parameter `{d.lifetime}`: stored loans are not in Language 1.0 (7.7, X-137); own the value, copy it, hold a Shared, or pass it to each call", self.dc.tok_of(d), d.name, "stored loan", "LANG-07-06")
                for fname, (ft, exp, _) in d.fields.items():
                    if d.exported and exp:
                        self.check_type_exported(ft, f"{d.name}.{fname}", self.dc.tok_of(d))
                    if self.loan_carrying(ft) and not (isinstance(ft, LoanT) and ft.origin == "static"):
                        self.own_err("E-OWN-053", f"field `{fname}` of {d.name} has the loan-carrying type {ft.show()}: no field stores a loan (7.7, X-137); own the value, copy it, hold a Shared, or pass it to each call", self.dc.tok_of(d), f"{d.name}.{fname}", "stored loan", "LANG-07-06")
                for m in children(d.node):
                    fd = find(m, "fieldDecl") if name(m) in ("classMember", "enumMember") else None
                    ce = next((c for c in children(fd) if name(c) == "constExpr"), None) if fd is not None else None
                    if ce is not None:
                        # outside every body: a default cannot create the capabilities (D-149)
                        self.decl_context(f"default of {d.name}.{direct_tokens(fd)[0].text}")
                        want = self.dc.type_of(find(fd, "type"))
                        self.expect(self.infer(ce, Env(), expected=want), want, tokens(ce)[0], f"default of `{direct_tokens(fd)[0].text}`")
                    cd = tree_mod.child(m, "constDecl") if name(m) == "classMember" else None
                    cce = tree_mod.child(cd, "constExpr") if cd is not None else None
                    if cce is not None:
                        # a class constant is a constant like a module's (D-149)
                        cname = next((t.text for t in direct_tokens(cd) if t.kind == "LOWER"), "?")
                        self.decl_context(f"const {d.name}.{cname}")
                        cty = tree_mod.child(cd, "type")
                        want = self.dc.type_of(cty) if cty is not None else None
                        ct = self.infer(cce, Env(), expected=want)
                        if want is not None:
                            self.expect(ct, want, tokens(cce)[0], f"const `{cname}`")
                    cl = find(m, "cleanupDecl") if name(m) == "classMember" else None
                    if cl is not None:
                        self.facts, self.own, self.depth, self.eff = Facts(), Owners(), 0, Summary()   # a fresh activation (9.4)
                        self.reported, self.schema, self.caps_calls, self.discharges = set(), None, 0, []
                        self.current_label = f"cleanup {d.name}"
                        self.self_type, self.self_mode = self_t, "editable self"
                        self.ret_type, self.in_try, self.ctx, self.loop_labels = VOID, False, ["function"], []
                        self.walk_block(find(cl, "block"), Env())
                        if self.eff.needs_auth and not self.receiver_authorities(self_t):
                            # cleanup is a method: the authority it uses is one its class holds in a field (11.6, X-270; D-173)
                            self.eff_err("E-EFF-010", f"cleanup {d.name} performs I/O, but {d.name} holds no Authority in a field: a cleanup uses the authority its class holds (11.6)",
                                         self.eff.sites.get("io", tokens(cl)[0]), "an Authority field", "io", f"cleanup {d.name}", "LANG-11-07")
                        self.summaries[f"cleanup {d.name}"] = self.eff      # contributes to every function that may drop the type (11.3)
                        self.self_type, self.ret_type, self.ctx = None, None, []
                for mname, sig in d.methods.items():
                    if d.exported and sig.exported:
                        self.check_exported_sig(sig, f"{d.name}.{mname}", sig.node and tokens(sig.node)[0] or self.dc.tok_of(d))
                    self.check_result_origin(sig, f"{d.name}.{mname}", sig.node and tokens(sig.node)[0] or self.dc.tok_of(d))
                    self.check_param_defaults(sig, f"{d.name}.{mname}")
                    self.check_ignorable(sig, f"{d.name}.{mname}")
                    if sig.body is not None:
                        self.run_body(sig, self_t, sig.receiver or "self", tokens(sig.node)[0] if sig.node is not None else self.dc.tok_of(d), f"method {d.name}.{mname}")
            elif d.kind == "mixin":
                pass                        # plug-in: mixin bodies are checked at each application site (9.6)
            elif d.kind == "const":
                ce = tree_mod.child(d.node, "constExpr")
                if ce is not None:
                    # typed whether or not it is annotated, outside every body: a constant cannot create the capabilities (D-149).
                    # an unannotated constant's type is its initialiser's, stored for later uses (D-54.4)
                    self.decl_context(f"const {d.name}")
                    annotated = tree_mod.child(d.node, "type") is not None
                    t = self.infer(ce, Env(), expected=d.alias if annotated else None)
                    if annotated:
                        self.expect(t, d.alias, tokens(ce)[0], f"const `{d.name}`")
                    elif known(strip(t)):
                        d.alias = t
                        self.note_inferred_const(d.module, d.name, t)
                if d.exported and d.alias is not None and known(strip(d.alias)):
                    self.check_type_exported(d.alias, d.name, self.dc.tok_of(d))
            elif d.kind == "type" and d.exported and d.alias is not None:
                self.check_type_exported(d.alias, d.name, self.dc.tok_of(d), allow_structural=True)
            elif d.kind == "interface":
                for mname, sig in d.methods.items():
                    if d.exported:
                        self.check_exported_sig(sig, f"{d.name}.{mname}", self.dc.tok_of(d))
                    self.check_param_defaults(sig, f"{d.name}.{mname}")
        for blk in self.table.implement_blocks:
            iface, target, module, node = blk
            if module != self.module:
                continue
            for m in [c for c in children(node) if name(c) == "implementMember"]:
                head = find(m, "methodHead")
                if head is not None:
                    sig = self.dc.signature(head, True, find(m, "funcBody"))
                    self.check_param_defaults(sig, f"{target.name}.{sig.name}")
                    if sig.body is not None:
                        self.run_body(sig, target, sig.receiver or "self", tokens(head)[0], f"method {target.name}.{sig.name}")
        for d in tree_mod.declarations(mtree):
            if d["kind"] in ("test", "property"):
                self.run_test_body(d["node"], d["kind"], d["token"])       # 14: test bodies are typed like try bodies
        return self.records

    def run_test_body(self, node, kind: str, tok: Token):
        """A test body is a block whose implicit function is `try` (14); rows are bound by `as` and typed by use."""
        self.facts, self.own, self.depth, self.eff = Facts(), Owners(), 0, Summary()
        self.reported, self.schema, self.caps_calls, self.discharges = set(), None, 0, []
        self.current_label = f"test {tok.text if tok is not None else ''}"
        env = Env()
        ts = direct_tokens(node)
        words = [t.text for t in ts]
        if kind == "test" and "as" in words:
            env.set(words[words.index("as") + 1], Unknown("test row"), mutable=False, kind="let")
        if kind == "property":
            pl = find(node, "paramList")
            for p in ([c for c in children(pl) if name(c) == "param"] if pl is not None else []):
                pts = direct_tokens(p)
                pt = find(p, "paramType")
                if pt is not None and pts and pts[0].kind == "LOWER":
                    env.set(pts[0].text, self.dc.type_of(find(pt, "type")), mutable=False, kind="param")
        self.self_type, self.self_mode = None, "self"
        self.ret_type, self.in_try, self.ctx, self.loop_labels = VOID, True, ["function"], []
        blk = [c for c in children(node) if name(c) == "block"][-1]
        self.test_body = True                       # `or fail` is implied in the body (14, X-345)
        for init in outcome_lets(blk):
            self.mark_consumed(init)                # a binding the body consumes as an outcome keeps it (14)
        try:
            self.walk_block(blk, env)
        finally:
            self.test_body = False
        self.summaries[self.current_label] = self.eff         # a test's effects, as a body's (the IR and its validator read it)
        self.self_type, self.ret_type, self.ctx = None, None, []

    def run_body(self, sig: Sig, self_t: Optional[Named], mode: str, head_tok: Token, label: str):
        """Type one body; the owner's and the method's `where` bounds are in force while it is typed (6.10)."""
        saved_bounds = self.dc.tparam_bounds
        bounds: Dict[str, Set[str]] = {}
        owner = self.decl_of(self_t) if self_t is not None else None
        for tp, iface in list(getattr(owner, "where", []) or []) + list(sig.where):
            bounds.setdefault(tp, set()).add(iface.name)
        self.dc.tparam_bounds = bounds
        saved_scope = (self.tparams_in_scope, self.where_in_scope)
        self.tparams_in_scope = set(getattr(owner, "type_params", ()) or ()) | set(sig.type_params)
        self.where_in_scope = list(getattr(owner, "where", []) or []) + list(sig.where)
        try:
            return self._run_body(sig, self_t, mode, head_tok, label)
        finally:
            self.dc.tparam_bounds = saved_bounds
            self.tparams_in_scope, self.where_in_scope = saved_scope

    def _run_body(self, sig: Sig, self_t: Optional[Named], mode: str, head_tok: Token, label: str):
        self.facts = Facts()
        self.own = Owners()
        self.depth = 0
        self.eff = Summary()
        self.fresh = set()
        self.discharges = []
        self.reported = set()
        self.current_label = label
        self.editable_params = {pn for pn, pt, pmode in sig.params if pmode.lstrip(".") == "editable"}
        for pn, pt, pmode in sig.params:
            if pmode in ("", "editable") and self.is_bearing(pt):
                self.own.holders[pn] = self.own.add_loan(pn, "editable" if pmode == "editable" else "alias", pn, 0, head_tok.line)   # a parameter is a path to its caller's owner (7.3)
            elif pmode.lstrip(".") == "owned" and self.is_linear(pt):
                self.own.obligations[pn] = (head_tok, 0)       # an owned linear parameter is owed by the callee (10.2)
        self.self_type, self.self_mode = self_t, mode
        self.ret_type, self.in_try, self.ctx, self.loop_labels = sig.ret, "try" in self.expanded_mods(sig), ["function"], []
        self.eff.authorities = self.authority_params(sig)
        self.schema, self.caps_calls, self.schema_unknown = None, 0, False
        self.apply_aspects(sig, head_tok, label)
        self.facts, self.own, self.depth = Facts(), Owners(), 0
        for pn, pt, pmode in sig.params:
            if pmode in ("", "editable") and self.is_bearing(pt):
                self.own.holders[pn] = self.own.add_loan(pn, "editable" if pmode == "editable" else "alias", pn, 0, head_tok.line)
            elif pmode.lstrip(".") == "owned" and self.is_linear(pt):
                self.own.obligations[pn] = (head_tok, 0)
        self.body(sig.body, self.params_env(sig, Env()), sig.ret, head_tok, label)
        self.summaries[label] = self.eff
        if self.final_pass:
            self.check_claims(sig, head_tok, label)
        self.self_type, self.ret_type, self.ctx = None, None, []

    def loan_carrying(self, t: Type) -> bool:
        """A loan type, a view, or a class with a lifetime parameter (7.7)."""
        if isinstance(t, LoanT):
            return True
        s = strip(t)
        if isinstance(s, OptionalT):
            return self.loan_carrying(s.inner)
        if isinstance(s, Named):
            if s.name in ("StringView", "BytesView", "ListView"):
                return True
            d = self.decl_of(s)
            return d is not None and d.kind == "class" and d.lifetime is not None
        return False

    def check_result_origin(self, sig: Sig, label: str, tok: Token):
        """A result that carries a loan names its origin with `from` (7.6, 7.7); E-OWN-053 otherwise."""
        if "trusted" in sig.modifiers or sig.body is None:
            return
        r = sig.ret
        while isinstance(r, OptionalT) and isinstance(r.inner, (LoanT, OptionalT)):
            r = r.inner                                   # `(alias T from self)?` names its origin inside the optional
        if isinstance(r, LoanT):
            return
        if self.loan_carrying(r):
            self.own_err("E-OWN-053", f"{label} returns the loan-carrying type {r.show()} without naming its origin: write `{r.show()} from <parameter>`", tok, label, "result without an origin", "LANG-07-06")

    def check_exported_sig(self, sig: Sig, label: str, tok: Token):
        core = self.dc.package == "fortis"                # the core may use structural types in its signatures (6.9: `join`)
        for pn, pt, _ in sig.params:
            self.check_type_exported(pt, f"{label}({pn})", tok, allow_structural=core)
        self.check_type_exported(sig.ret, f"{label} result", tok, allow_structural=core)

    def check_type_exported(self, t: Type, label: str, tok: Token, allow_structural=False):
        if isinstance(t, STRUCTURAL) and not allow_structural:
            self.rec("E-TYP-030", f"exported signature {label} uses a structural type {t.show()}", "LANG-04-05", tok, {"expected_type": "a nominal exported type", "actual_type": t.show(), "primary_span": label})
            return
        if isinstance(t, Named):
            if t.name == "VariantMap" and not allow_structural:
                self.rec("E-TYP-030", f"exported signature {label} uses a structural type {t.show()}", "LANG-04-05", tok, {"expected_type": "a nominal exported type", "actual_type": t.show(), "primary_span": label})
                return
            d = self.table.get(t.name)
            if d is not None and d.module == self.module and not d.exported and d.kind in ("class", "enum", "type", "interface", "error"):
                self.rec("E-TYP-030", f"exported signature {label} names non-exported type {t.name}", "LANG-04-05", tok, {"expected_type": "an exported type", "actual_type": t.name, "primary_span": label})
            for a in t.args:
                self.check_type_exported(a, label, tok, allow_structural)
        elif isinstance(t, (OptionalT, LoanT)):
            self.check_type_exported(t.inner, label, tok, allow_structural)
        elif isinstance(t, Refined):
            self.check_type_exported(t.base, label, tok, allow_structural)
        elif isinstance(t, FnT):
            for p in t.params:
                self.check_type_exported(p, label, tok, allow_structural)
            self.check_type_exported(t.ret, label, tok, allow_structural)
        elif isinstance(t, DynamicT):
            self.check_type_exported(t.iface, label, tok, allow_structural)


VisibilityPass = Typer
