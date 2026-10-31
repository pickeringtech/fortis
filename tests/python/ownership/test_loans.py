"""LR 7.3-7.7: data-level ownership judgements, with no parser or library loader."""
import support
from support import forall, table_tests
import unittest
from types import SimpleNamespace

from checker.decls import Decl, DeclChecker
from checker.owners import Owners, overlaps
from checker.typer import Typer, Env
from checker.types import Named, LoanT, OptionalT, TupleT, RecordT, FnT, TypeParam, VOID
from lexer import Token
from tree import children


def typer():
    t = Typer(DeclChecker(SimpleNamespace(modules={}, prelude=None), "unit"))
    # Only declaration heads needed by these judgements. No library files are read.
    for name in ("StringView", "BytesView", "ListView", "ListCursor", "Outcome", "Future", "Task"):
        t.table.decls[name] = Decl("class", name, True, "fortis.prelude")
    return t


def typ(spec):
    if isinstance(spec, str):
        return Named(spec)
    tag, *args = spec
    if tag == "loan":
        return LoanT(typ(args[2]), args[0], args[1])
    if tag == "optional":
        return OptionalT(typ(args[0]))
    if tag == "tuple":
        return TupleT(tuple(map(typ, args)))
    if tag == "record":
        return RecordT((("value", typ(args[0])),))
    if tag == "parameter":
        return TypeParam(args[0])
    if tag == "captures":
        return FnT((), VOID, dynamic=True, capture_origins=tuple(tuple(x) for x in args))
    if tag == "closure":
        return FnT((), VOID, origin=args[0])
    return Named(tag, tuple(map(typ, args)))


def codes(t):
    return sorted({r["code"] for r in t.records})


def check_state(self, data, expected):
    a, b = data["a"], data["b"]
    state = Owners()
    state.move(a)
    moved = state.is_moved(b)
    state.reinit(b)
    self.assertEqual([overlaps(a, b), moved, state.is_moved(a)], expected)


def check_use(self, data, expected):
    t, env = typer(), Env()
    env.set("x", Named("String"), mutable=True, kind="mut")
    env.set("y", Named("String"), mutable=True, kind="mut")
    if data["state"] == "moved":
        t.own.move("x")
    elif data["state"] != "free":
        t.own.add_loan("x", data["state"], "holder", 1, 1)
    t.use_place(data["path"], Token("LOWER", "x", 2, 1), data["use"], env)
    self.assertEqual(codes(t), expected)
    self.assertEqual(t.use_ok, not expected)


def check_storage(self, data, expected):
    t = typer()
    t.check_stored_type(typ(data["type"]), Token("LOWER", "value", 1, 1), data["field"])
    self.assertEqual(codes(t), expected)


class PlaceState(unittest.TestCase):
    pass


class PlaceUse(unittest.TestCase):
    pass


class Storage(unittest.TestCase):
    pass


table_tests(PlaceState, "ownership/state.json", check_state)
table_tests(PlaceUse, "ownership/use.json", check_use)
table_tests(Storage, "ownership/storage.json", check_storage)


def expression(spec):
    if "token" in spec:
        return Token(*spec["token"], 1, 1)
    return (spec["rule"], [expression(k) for k in spec["kids"]])


def remember_types(t, node, spec):
    if "token" in spec:
        return
    for child, data in zip(children(node), spec["kids"]):
        remember_types(t, child, data)
    # The same boundary as infer(): remember possible owners before joining branch types.
    if "type" in spec and t.temporary_value(node, typ(spec["type"])):
        t.temporary_values.add(id(t.core_expr(node)))


def check_temporary(self, data, expected):
    t, node = typer(), expression(data)
    remember_types(t, node, data)
    self.assertEqual(t.temporary_value(node), expected)


class Temporaries(unittest.TestCase):
    pass


table_tests(Temporaries, "ownership/temporaries.json", check_temporary)


class LexicalProperties(unittest.TestCase):
    @forall(lambda r: (r.randrange(1, 8), r.randrange(1, 8), r.choice(("alias", "editable"))))
    def test_lang_07_05_held_loans_end_at_their_block(self, case):
        depth, exiting, mode = case
        own = Owners()
        own.add_loan("owner", mode, "holder", depth, 1)
        own.add_loan("other", "alias", None, depth, 1)
        own.end_statement()
        self.assertEqual(len(own.active("owner")), 1)
        self.assertFalse(own.active("other"))
        own.end_block(exiting)
        self.assertEqual(bool(own.active("owner")), depth < exiting)
        self.assertEqual("holder" in own.holders, depth < exiting)

    @forall(lambda r: (r.randrange(1, 10), r.randrange(1, 10)))
    def test_lang_07_04_join_keeps_either_path_move(self, counts):
        left, right = (Owners() for _ in range(2))
        for i in range(counts[0]):
            left.move(f"left{i}")
        for i in range(counts[1]):
            right.move(f"right{i}")
        joined = left.join(right)
        self.assertEqual(joined.moved, left.moved | right.moved)
        self.assertEqual(joined.moved, right.join(left).moved)
        self.assertEqual(left.join(left).moved, left.moved)


def check_provenance(self, data, expected):
    t, env = typer(), Env()
    have, want = typ(data["have"]), typ(data["want"])
    op = data["op"]
    token = Token("LOWER", "value", 1, 1)
    if op == "owned_argument":
        result = TupleT(tuple(h for h, _ in t.closure_values(have, want)))
    elif op == "substitute":
        result = t.subst(have, {"T": want})
    elif op == "infer":
        mapping = {"T": want}
        t.unify(TypeParam("T"), have, mapping)
        result = mapping["T"]
    elif op == "annotation":
        result = t.with_closure(want, have)
    elif op == "coalesce":
        result = t.coalesce_result(want, have, None, None, env)
    else:
        result = t.unify_branches([(want, token), (have, token)], "unit")
    self.assertEqual(sorted(map(list, t.captured_loans(result).items())), expected)
    self.assertEqual(codes(t), [])


class CaptureProvenance(unittest.TestCase):
    pass


table_tests(CaptureProvenance, "ownership/provenance.json", check_provenance)
