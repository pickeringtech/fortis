"""R4 evaluator regressions over supplied syntax, shared with MIR/VM tests.

Checker rules have their own shared table. Supply the parsed module here, as the
linked-program path does, so each evaluator test does not check the whole program
again. Array sizes still come from the production type-fact collector.
"""
import unittest
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


class FunctionValues(unittest.TestCase):
    pass

table_tests(FunctionValues, "runtime/function_values.json", check)
