"""LR 4.4: qualified calls are resolved by the expression typer."""
import unittest
from support import table_tests
from lexer import lex
from conformance.generated.fortis_parser import FortisParser
from checker.decls import DeclChecker
from checker.typer import Typer, Env
from name_rules.test_resolution import library


def check(test, inp, expect):
    dc = DeclChecker(library(), 'unit.ft')
    dc.load(FortisParser(lex(inp['head'])).parse('module'))
    ty = Typer(dc)
    ty.aliases['items'] = 'acme.items'
    ty.infer(FortisParser(lex(inp['source'])).parse('expr'), Env())
    test.assertEqual(sorted({r['code'] for r in ty.records}), expect, ty.records)


class Members(unittest.TestCase):
    pass


table_tests(Members, 'names/member_calls.json', check)
