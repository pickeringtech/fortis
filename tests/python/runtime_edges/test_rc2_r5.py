"""Resolved array sizes, structural records and editable reborrows."""
import unittest
from support import table_tests
from mopup.test_checker import LIB
from checker.check import check_tree
from interpreter.link import LinkedModule
from interpreter.type_facts import array_sizes
from interpreter.eval import Interpreter, Env
from interpreter.values import FortisError
from lexer import lex

def check(test, data, expect):
    # Reuse parsed library declarations, as the checker case tables do. Each case
    # still checks its own tree, resolves its own facts and gets a fresh runtime.
    source, file = data["source"], "rc2-r5.ft"
    module = LIB.rec.parse(lex(source), "module")
    test.assertEqual(check_tree(LIB, module, file), [])
    entry = LinkedModule("main", file, module, None)
    entry.array_sizes = array_sizes(module, LIB, file)
    runtime = Interpreter(source, file, linked=(None, entry))
    test.assertEqual(runtime.run_main([]), 0)
    test.assertEqual(runtime.out, expect)


class Rc2R5(unittest.TestCase):
    def test_local_loan_permissions(self):
        runtime = Interpreter("func f() {}", "unit.ft")
        env = Env()
        env.declare("s", {"n": 1}, owns=False)
        env.cell("s").editable = True
        for source, editable in [("let t = editable s", True), ("let t = alias s", False)]:
            local = Env(env)
            runtime.statement_body(runtime.rec.parse(lex(source), "statement"), local)
            self.assertEqual(local.cell("t").editable, editable)
            self.assertFalse(local.cell("t").mutable)
            self.assertFalse(local.cell("t").owns)
            place = runtime.rec.parse(lex("t.n"), "place")
            if editable:
                runtime.assign(place, 9, local)
                self.assertEqual(env.get("s")["n"], 9)
            else:
                with self.assertRaises(FortisError):
                    runtime.assign(place, 10, local)


table_tests(Rc2R5, "runtime/rc2_r5.json", check)
