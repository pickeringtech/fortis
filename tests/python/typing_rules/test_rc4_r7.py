"""D-749: every function-value conversion checks async status."""
import unittest
from support import table_tests
from mopup.test_checker import check

class Rc4R7(unittest.TestCase):
    pass

table_tests(Rc4R7, "types/rc4_r7.json", check)
