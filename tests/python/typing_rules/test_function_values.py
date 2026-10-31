"""LR 8.1 and 9.7: callable signatures, shared with Rust."""
import unittest
from support import table_tests
from mopup.test_checker import check

class FunctionValues(unittest.TestCase):
    pass

table_tests(FunctionValues, "types/function_values.json", check)
