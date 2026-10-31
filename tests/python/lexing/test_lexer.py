import unittest

from support import table_tests
from conformance.tools.lexer import lex


def check(test, source, expect):
    # EOF has its own implicit token; every other token, including newlines, is compared.
    actual = [[t.kind, t.text] for t in lex(source) if t.kind != "EOF"]
    test.assertEqual(actual, expect)


class Lexer(unittest.TestCase):
    pass


table_tests(Lexer, "lexer/tokens.json", check)
