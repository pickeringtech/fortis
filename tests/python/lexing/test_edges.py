"""LR 3.1-3.7: continuation, delimiters and rejected lexical forms."""
import unittest

from support import forall, table_tests
from lexer import LexError, lex


def tokens(test, source, expect):
    test.assertEqual([[t.kind, t.text] for t in lex(source) if t.kind != 'EOF'], expect)


def error(test, source, expect):
    with test.assertRaises(LexError) as caught:
        lex(source)
    test.assertEqual(caught.exception.code, expect)
    # A failed scan has no state that leaks into the next invocation (LR 3.1).
    test.assertEqual([t.text for t in lex('x')], ['x', '\n', ''])


class Continuation(unittest.TestCase):
    @forall(lambda r: (r.choice(['+', '-', '*', '/', '==', '&&']), r.randint(1, 8)))
    def test_lr32_trailing_with_comments(self, case):
        op, count = case
        source = 'a ' + op + '\n' + '// comment\n' * count + 'b'
        tokens(self, source, [['LOWER', 'a'], ['PUNCT', op], ['LOWER', 'b'], ['NL', '\n']])


class Errors(unittest.TestCase):
    pass


table_tests(Continuation, 'lexer/continuation.json', tokens)
table_tests(Errors, 'lexer/errors.json', error)
