"""LR 6.3, 6.10 and 6.13: direct, in-memory front-end rules."""
import unittest
from types import SimpleNamespace

from support import forall, read_cases, table_tests
from checker import conversions, intervals
from checker.types import Named, Refined, OptionalT, TupleT, RecordT, TypeParam
from checker.literals import assignable
from checker.decls import DeclChecker
from checker.typer import Typer


def typ(value):
    if isinstance(value, str):
        return Named(value)
    tag, *args = value
    if tag == 'ref':
        return Refined(Named(args[0]), args[1], args[2], args[3] if len(args) > 3 else 'range')
    if tag == 'opt':
        return OptionalT(typ(args[0]))
    if tag == 'tuple':
        return TupleT(tuple(map(typ, args)))
    if tag == 'record':
        return RecordT(tuple((k, typ(v)) for k, v in args))
    if tag == 'param':
        return TypeParam(args[0])
    if tag == 'named':
        return Named(args[0], tuple(map(typ, args[1:])))
    raise AssertionError(tag)


def fresh_typer():
    return Typer(DeclChecker(SimpleNamespace(modules={}, prelude=None), 'unit.ft'))


def conversion(test, pair, expect):
    conv = conversions.resolve(*pair)
    actual = None if conv is None else 'fallible' if conv.fallible else 'total'
    test.assertEqual(actual, expect)
    if conv is not None:
        test.assertEqual((conv.source, conv.target), tuple(pair))


def assignment(test, inp, expect):
    test.assertIs(assignable(None, typ(inp['have']), typ(inp['want']))[0], expect)


def narrowing(test, inp, expect):
    got = intervals.narrow(inp['op'], tuple(inp['p']), tuple(inp['c']))
    test.assertEqual(list(got) if got is not None else None, expect)


def unification(test, inp, expect):
    ty = fresh_typer()
    found = {}
    ty.unify(typ(inp['pattern']), typ(inp['actual']), found)
    test.assertEqual(found, {k: typ(v) for k, v in expect.items()})


class Conversions(unittest.TestCase):
    @forall(lambda r: r.sample(['Int8', 'Int16', 'UInt8', 'UInt16'], 2))
    def test_lr613_integer_total_iff_range_contains(self, pair):
        bounds = {'Int8': (-128, 127), 'Int16': (-32768, 32767),
                  'UInt8': (0, 255), 'UInt16': (0, 65535)}
        a, b = map(bounds.get, pair)
        conversion(self, pair, 'total' if b[0] <= a[0] and a[1] <= b[1] else 'fallible')


class Assignment(unittest.TestCase):
    pass


class Narrowing(unittest.TestCase):
    @forall(lambda r: (r.randint(-8, 0), r.randint(0, 8), r.randint(-8, 8)))
    def test_lr63_all_comparisons_contain_satisfying_values(self, case):
        lo, hi, c = case
        for op in ['<', '<=', '>', '>=', '==', '!=']:
            got = intervals.narrow(op, (lo, hi), (c, c))
            values = [x for x in range(lo, hi + 1) if {
                '<': x < c, '<=': x <= c, '>': x > c, '>=': x >= c,
                '==': x == c, '!=': x != c}[op]]
            self.assertEqual(got, (min(values), max(values)) if values else None)


class Unification(unittest.TestCase):
    pass


table_tests(Conversions, 'types/conversions.json', conversion)
table_tests(Assignment, 'types/assignability.json', assignment)
table_tests(Narrowing, 'types/narrowing.json', narrowing)
table_tests(Unification, 'types/unification.json', unification)


class KnownDefects(unittest.TestCase):
    def check_case(self, name):
        row = next(r for r in read_cases('types/defects.json') if r['name'] == name)
        assignment(self, row['input'], row['expect'])

    def test_d635_tuple_wrong(self):
        """D-635: LR 6.9 rejects a tuple with different element types."""
        self.check_case('lr6_tuple_wrong')

    def test_d636_optional_widens(self):
        """D-636: LR 6.3 permits optional refinement widening (X-685)."""
        self.check_case('lr6_optional_widens')


class Structural(unittest.TestCase):
    pass

table_tests(Structural, 'types/structural.json', assignment)
