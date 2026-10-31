"""LR 4.5 and 6.13: construction and declared text conversions (D-420, D-421)."""
import unittest
from support import table_tests, ROOT
from checker.check import check_source
import names
import parser

LIB = names.Library(parser.load_grammar(str(ROOT / "reference/grammar/fortis.ebnf")))

def check(test, data, expect):
    records = check_source(LIB, data["source"], "mopup.ft")
    test.assertEqual(sorted(set(r["code"] for r in records)), sorted(expect["codes"]), records)
    if "message" in expect:
        test.assertTrue(any(expect["message"] in r["message"] for r in records), records)

    if "fields" in expect:
        test.assertEqual(records[0]["fields"], expect["fields"])

class MopUp(unittest.TestCase):
    pass

table_tests(MopUp, "mopup/checker.json", check)
