"""LR 6.3 and 6.10: infer one expression or narrow one condition, in memory."""
import unittest
from support import read_cases, table_tests
from lexer import lex
from conformance.generated.fortis_parser import FortisParser
from checker.typer import Env
from typing_rules.test_rules import typ, fresh_typer


def check(test, inp, expect):
    ty = fresh_typer()
    if 'head' in inp:
        head = FortisParser(lex(inp['head'])).parse('module')
        ty.dc.load_module_tree(head, 'main', '', library=False)
    env = Env()
    for name, value in inp['env'].items():
        t = typ(value)
        # U1's in-memory declarations have the same nominal identity as source signatures.
        if isinstance(value, str) and (d := ty.dc.table.get(value)) is not None:
            t = d.as_named()
        env.set(name, t)
    source = inp['source']
    if inp['op'] == 'statement':
        from tree import find
        expr = find(FortisParser(lex('{ ' + source + '\n}')).parse('block'), 'statement')
    else:
        expr = FortisParser(lex(source)).parse('expr')
    if inp['op'] == 'narrow':
        yes, no, _ = ty.narrow(expr, env)
        test.assertEqual([list(f.ints.get('x', (0, 9))) for f in [yes, no]], expect)
    elif inp['op'] == 'statement':
        ty.statement(expr, env)
        test.assertEqual(sorted({r['code'] for r in ty.records}), expect)
        return
    else:
        expected = typ(inp['expected']) if 'expected' in inp else None
        actual = ty.infer(expr, env, expected)
        if inp['op'] == 'codes':
            test.assertEqual(sorted({r['code'] for r in ty.records}), expect)
            return
        test.assertEqual(actual, typ(expect))
    test.assertEqual(ty.records, [])


class Expressions(unittest.TestCase):
    pass


table_tests(Expressions, 'types/expressions.json', check)


class KnownDefects(unittest.TestCase):
    def check_case(self, name):
        row = next(r for r in read_cases("types/expression_defects.json") if r["name"] == name)
        check(self, row["input"], row["expect"])

    def test_lr63_condition_parentheses(self):
        """D-637: LR 6.3 narrowing and joins preserve refinement facts."""
        self.check_case("lr63_condition_parentheses")

    def test_lr63_if(self):
        """D-638: LR 6.3 narrowing and joins preserve refinement facts."""
        self.check_case("lr63_if")

    def test_lr63_match(self):
        """D-638: LR 6.3 narrowing and joins preserve refinement facts."""
        self.check_case("lr63_match")

    def test_lr63_range_exclusive(self):
        """D-638: LR 6.3 narrowing and joins preserve refinement facts."""
        self.check_case("lr63_range_exclusive")


class CheckerFixes(unittest.TestCase):
    pass

table_tests(CheckerFixes, 'types/checker_fixes.json', check)
