"""LR 5.6, 6.4 and 6.5: value semantics, independent of type inference."""
import unittest
from support import table_tests, forall
from evaluation.context import evaluator


def check(test, data, expect):
    a, b = data['a'], data['b']
    if data['type'] == 'Float': a, b = float(a), float(b)
    test.assertEqual(evaluator().binary(data['op'], a, b), expect)


class Operators(unittest.TestCase):
    @forall(lambda r: (r.randint(-10000,10000),r.randint(-10000,10000)))
    def test_lr6_4_small_arithmetic_ring(self, pair):
        it = evaluator()
        a, b = pair
        self.assertEqual(it.binary('-',it.binary('+',a,b),b),a)
        self.assertEqual(it.binary('*',a,b),it.binary('*',b,a))
        self.assertEqual(it.binary('^',it.binary('^',a,b),b),a)


table_tests(Operators, 'evaluation/operators.json', check)
