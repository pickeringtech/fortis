"""rc.1 checker regressions, shared with Rust."""
import unittest
from support import table_tests
from mopup.test_checker import check

class Rc1Checkers(unittest.TestCase):
    pass

table_tests(Rc1Checkers, "types/rc1_checkers.json", check)
