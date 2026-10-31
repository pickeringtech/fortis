"""LR 4.4, 6.13, 9.1, 9.5 and 9.6: declaration rules, without checking bodies."""
import unittest
from support import read_cases, table_tests
from lexer import lex
from conformance.generated.fortis_parser import FortisParser
from checker.decls import DeclChecker
from checker.typer import Typer
from name_rules.test_resolution import library


def check(test, source, expect):
    dc = DeclChecker(library(), 'unit.ft')
    dc.load(FortisParser(lex(source)).parse('module'))
    dc.check()
    ty = Typer(dc)
    for d in list(dc.table.decls.values()):
        if d.module == 'main' and d.exported and d.sig is not None:
            ty.check_exported_sig(d.sig, d.name, dc.tok_of(d))
    test.assertEqual(sorted({r['code'] for r in dc.records}), expect, dc.records)


class Declarations(unittest.TestCase):
    pass


table_tests(Declarations, 'names/declarations.json', check)


class KnownDefects(unittest.TestCase):
    def test_lr91_duplicate_field_constant(self):
        """D-639: LR 9.1 puts constants in the shared member namespace."""
        row = next(r for r in read_cases("names/defects.json") if r["name"] == "lr91_duplicate_field_constant")
        check(self, row["input"], row["expect"])

    def test_lr91_duplicate_method_constant(self):
        """D-639: LR 9.1 puts constants in the shared member namespace."""
        row = next(r for r in read_cases("names/defects.json") if r["name"] == "lr91_duplicate_method_constant")
        check(self, row["input"], row["expect"])

    def test_lr91_duplicate_constant_field(self):
        """D-639: LR 9.1 puts constants in the shared member namespace."""
        row = next(r for r in read_cases("names/defects.json") if r["name"] == "lr91_duplicate_constant_field")
        check(self, row["input"], row["expect"])

    def test_lr91_duplicate_constant_method(self):
        """D-639: LR 9.1 puts constants in the shared member namespace."""
        row = next(r for r in read_cases("names/defects.json") if r["name"] == "lr91_duplicate_constant_method")
        check(self, row["input"], row["expect"])

    def test_lr91_duplicate_constant_constant(self):
        """D-639: LR 9.1 puts constants in the shared member namespace."""
        row = next(r for r in read_cases("names/defects.json") if r["name"] == "lr91_duplicate_constant_constant")
        check(self, row["input"], row["expect"])


class MemberNamespace(unittest.TestCase):
    pass

table_tests(MemberNamespace, 'names/member_namespace.json', check)
