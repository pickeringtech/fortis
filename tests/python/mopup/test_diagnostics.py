import unittest
from support import table_tests
from nominal_identity.test_identity import typ
from checker.types import diagnostic_types

class Diagnostics(unittest.TestCase):
    pass

def check(test, data, expect):
    a, b = typ(data['a']), typ(data['b'])
    before = (a.show(), b.show())
    test.assertEqual(list(diagnostic_types(a, b)), expect)
    test.assertEqual((a.show(), b.show()), before)

table_tests(Diagnostics, "mopup/diagnostics.json", check)
