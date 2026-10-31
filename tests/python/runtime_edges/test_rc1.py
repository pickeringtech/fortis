"""Small runtime regressions from the rc.1 review, shared with MIR/VM tests."""
import unittest
from support import table_tests
from interpreter.eval import Interpreter, Env
from interpreter.values import FortisError
from lexer import lex


def check(test, data, expect):
    runtime = Interpreter(data['source'], 'rc1.ft')
    test.assertEqual(runtime.run_main([]), 0)
    test.assertEqual(runtime.out, expect)


class RuntimeFixes(unittest.TestCase):
    def test_editable_parameter_does_not_rebind(self):
        runtime = Interpreter('func poke(s: editable { n: Int }) { s.n = 9 }', 'rc1.ft')
        runtime.globals = Env()
        record = {'n': 1}
        head, _body = runtime.funcs['poke']
        env = runtime.bind(head, [(None, record)], None)
        self.assertFalse(env.cell('s').mutable)
        self.assertTrue(env.cell('s').editable)
        runtime.assign(runtime.rec.parse(lex('s.n'), 'place'), 9, env)
        self.assertEqual(record['n'], 9)
        with self.assertRaises(FortisError):
            runtime.assign(runtime.rec.parse(lex('s'), 'place'), {'n': 2}, env)


table_tests(RuntimeFixes, 'runtime/rc1.json', check)
