import unittest
from support import ROOT, table_tests
from interpreter.intrinsics import FUNCS
from conformance.tools.gen_graphemes import generated


class Graphemes(unittest.TestCase):
    def test_generated_tables_are_current(self):
        for path, text in generated().items():
            self.assertEqual((ROOT / path).read_text(), text, path)


table_tests(Graphemes, 'text/graphemes.json',
            lambda t, d, e: t.assertEqual(FUNCS['graphemes'](None, [d['text']]), e))
