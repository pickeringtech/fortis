"""LR 8.1: named function values wrap once and preserve effects."""
import unittest
from support import table_tests
from mopup.test_checker import check

class Rc3R6(unittest.TestCase):
    pass

table_tests(Rc3R6, "types/rc3_r6.json", check)
