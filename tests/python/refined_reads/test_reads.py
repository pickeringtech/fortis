"""LR 6.3 and 6.13, using the same typed fragments as the Rust checker."""
import unittest
from functools import lru_cache
from support import table_tests
import lexer, names
from checker.decls import DeclChecker
from checker.typer import Typer
from evaluation.context import Syntax

@lru_cache(maxsize=1)
def library():
    return names.Library(Syntax())

def check(test, data, expect):
    tree = Syntax.parse(lexer.lex(data['source']), 'module')
    dc = DeclChecker(library(), 'unit.ft')
    dc.load(tree)
    dc.check()
    Typer(dc).run(tree)
    test.assertEqual(sorted({r['code'] for r in dc.records}), expect, dc.records)

class RefinedReads(unittest.TestCase): pass

table_tests(RefinedReads, 'types/refined_reads.json', check)
