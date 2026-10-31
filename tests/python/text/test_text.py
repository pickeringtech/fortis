import unittest

from support import table_tests
from interpreter.intrinsics import METHODS
from interpreter.values import ListVal


def check(test, data, expect):
    # Exercise the interpreter's actual registry for both receiver types.
    for receiver in ("String", "StringView"):
        with test.subTest(receiver=receiver):
            actual = METHODS[(receiver, data["op"])](None, data["text"], data["args"])
            if isinstance(actual, ListVal):
                actual = actual.items
            test.assertEqual(actual, expect)


class Text(unittest.TestCase):
    pass


table_tests(Text, "text/intrinsics.json", check)
