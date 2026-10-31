"""LR 8.4, 12.3: async status is invariant (D-749)."""
import unittest
from support import table_tests
from checker.literals import assignable
from checker.types import FnT, Named, OptionalT, TupleT, RecordT


def typ(modifiers, wrap):
    f = FnT((Named("Int"),), Named("Int"), modifiers=tuple(modifiers))
    return {
        "fn": lambda: f,
        "optional": lambda: OptionalT(f),
        "tuple": lambda: TupleT((f,)),
        "record": lambda: RecordT((("work", f),)),
        "generic": lambda: Named("Holder", (f,)),
        "result": lambda: FnT((), f),
    }[wrap]()


def check(test, data, expect):
    got, why = assignable(None, typ(data["have"], data["wrap"]), typ(data["want"], data["wrap"]))
    test.assertIs(got, expect)
    if "async" in data["want"] and "async" not in data["have"]:
        test.assertIn("declare the function `async`, or wrap the call in an async lambda", why)


class AsyncAssignment(unittest.TestCase):
    pass


table_tests(AsyncAssignment, "types/async_assignment.json", check)
