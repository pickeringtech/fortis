import unittest
from support import table_tests
from conformance.tools.frontend_check import canonical_tree_dump


class ParseDump(unittest.TestCase):
    pass


table_tests(ParseDump, 'runtime/parse_dump.json',
            lambda t, d, e: t.assertEqual(
                canonical_tree_dump(d['left']) == canonical_tree_dump(d['right']), e))
