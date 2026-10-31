import unittest

from support import table_tests
from checker.literals import assignable, origin_clash
from checker.types import DynamicT, FnT, LoanT, Named, OptionalT, RecordT, Refined, TupleT, core_name


def typ(data):
    if "name" in data:
        return Named(data["name"], tuple(map(typ, data.get("args", []))),
                     data.get("package", ""), data.get("origin", ""))
    if "optional" in data:
        return OptionalT(typ(data["optional"]))
    if "tuple" in data:
        return TupleT(tuple(map(typ, data["tuple"])))
    if "record" in data:
        return RecordT(tuple((key, typ(value)) for key, value in data["record"]))
    if "fn" in data:
        params, result = data["fn"]
        return FnT(tuple(map(typ, params)), typ(result))
    if "loan" in data:
        return LoanT(typ(data["loan"]), "alias", "owner")
    if "dynamic" in data:
        return DynamicT(typ(data["dynamic"]))
    if "refined" in data:
        return Refined(typ(data["refined"]), 0, 10)
    raise AssertionError(data)


def check(test, data, expect):
    a = typ(data["a"])
    if data["op"] == "core":
        test.assertEqual(core_name(a), expect)
        return
    b = typ(data["b"])
    if data["op"] == "interface":
        test.assertEqual(a.interface_key() == b.interface_key(), expect)
        return
    # A nominal mismatch must be found in either direction, before structural equality.
    for have, want in [(a, b), (b, a)]:
        test.assertEqual(bool(origin_clash(have, want)), expect["clash"])
        test.assertEqual(assignable(None, have, want)[0], expect["assignable"])


class Identity(unittest.TestCase):
    pass


table_tests(Identity, "identity/nominal.json", check)
