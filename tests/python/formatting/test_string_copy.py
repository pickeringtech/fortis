"""LR 6.13: both spellings retain the allocation, including through a caller."""
import unittest
from support import table_tests
from refined_reads.test_reads import check as check_source


def check(test, source, rewritten):
    ty = "Holder" if source.startswith("class Holder") else "String"
    caller = f"noalloc func caller(value: owned {ty}) String {{ return fromMove(move value) }}\n"
    for body in (source, rewritten):
        check_source(test, {"source": body}, [])
        check_source(test, {"source": body + caller}, ["E-EFF-001"])


class StringCopy(unittest.TestCase):
    pass


table_tests(StringCopy, "formatter/string_copy.json", check)
