"""R5 optional argument obligation regressions, shared with Rust."""
import unittest
from support import table_tests
from mopup.test_checker import LIB
from checker.check import check_source

def check(test, data, expect):
    records = check_source(LIB, data["source"], "mopup.ft")
    test.assertEqual(sorted({r["code"] for r in records}), sorted(expect["codes"]), records)
    if "message" in expect:
        test.assertTrue(any(expect["message"] in r["message"] for r in records), records)
    if "at" in expect:
        test.assertTrue(any([r["line"], r["column"]] == expect["at"] and "an argument of `?.`" in r["message"] for r in records), records)

class Rc2R5(unittest.TestCase):
    pass

table_tests(Rc2R5, "types/rc2_r5.json", check)
