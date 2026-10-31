"""LR 6.7 and 8.1: constructor facts and callable runtime regressions."""
import unittest
import os
from types import SimpleNamespace
from unittest.mock import patch
from support import table_tests
from evaluation.context import Syntax
from interpreter.eval import Interpreter
from interpreter.type_facts import array_sizes
from lexer import lex
import names

SYNTAX = Syntax()
LIBRARY = names.Library(SYNTAX)


def check(test, data, expect):
    module = SYNTAX.parse(lex(data['source']), 'module')
    entry = SimpleNamespace(tree=module, key=None,
                            array_sizes=array_sizes(module, LIBRARY, 'rc1.ft'))
    with patch('interpreter.eval.fparser.load_grammar', return_value=SYNTAX):
        runtime = Interpreter(data['source'], 'rc1.ft', linked=(None, entry))
    test.assertEqual(runtime.run_main([]), 0)
    test.assertEqual(runtime.out, expect)


class Rc3R6(unittest.TestCase):
    def test_constructor_type_reference_sizes(self):
        for spelling in ("2", "0x2", "0b10", "0o2", "0_2"):
            for element in ("Int", "Int<0..9>"):
                with self.subTest(spelling=spelling, element=element):
                    source = ("func main() {\n"
                              f"  let a = Array[{element}, {spelling}].filled(1).count()\n"
                              f"  let b = Array[{element}, {spelling}].filled(1)\n"
                              f"  let c: Array[{element}, 2] = Array[{element}].filled(1)\n"
                              "}\n")
                    module = SYNTAX.parse(lex(source), "module")
                    sizes = array_sizes(module, LIBRARY, "sizes.ft")
                    file = os.path.abspath("sizes.ft")
                    self.assertEqual(sizes.get((file, 2, 11)), 2)
                    self.assertEqual(sizes.get((file, 3, 11)), 2)
                    self.assertEqual(sizes.get((file, 4, 23 + len(element))), 2)

table_tests(Rc3R6, "runtime/rc3_r6.json", check)
