import unittest

from support import forall, table_tests
from checker import intervals as iv
from checker.literals import assignable
from checker.types import Named, Refined


def check(test, data, expect):
    op = data["op"]
    if op == "subtype":
        def typ(bounds):
            return Named("Int") if bounds is None else Refined(Named("Int"), *bounds)
        actual = assignable(None, typ(data["a"]), typ(data["b"]))[0]
    elif op == "meet":
        actual = iv.meet(tuple(data["a"]), tuple(data["b"]))
    else:
        actual = iv.arith(op, tuple(data["a"]), tuple(data["b"]))
    if isinstance(actual, tuple):
        actual = list(actual)
    test.assertEqual(actual, expect)


class Intervals(unittest.TestCase):
    pass


table_tests(Intervals, "intervals/arithmetic.json", check)


def operands(rng):
    a = sorted((rng.randint(-20, 20), rng.randint(-20, 20)))
    b = sorted((rng.randint(-20, 20), rng.randint(-20, 20)))
    return a, b


class Properties(unittest.TestCase):
    # Exhaust every concrete operand pair in each generated small interval.
    def contains_results(self, op, value):
        a, b = value
        lo, hi = iv.arith(op, a, b)
        for x in range(a[0], a[1] + 1):
            for y in range(b[0], b[1] + 1):
                result = {"+": lambda: x + y, "-": lambda: x - y, "*": lambda: x * y}[op]()
                self.assertTrue(lo <= result <= hi, (x, y, result, lo, hi))

    @forall(operands)
    def test_add_contains_concrete_results(self, value):
        self.contains_results("+", value)

    @forall(operands)
    def test_subtract_contains_concrete_results(self, value):
        self.contains_results("-", value)

    @forall(operands)
    def test_multiply_contains_concrete_results(self, value):
        self.contains_results("*", value)
