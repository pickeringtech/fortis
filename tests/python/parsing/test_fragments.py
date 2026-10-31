"""LR 3.2, 4, 5, 6, 8, 9 and 16: isolated grammar rules and LR 5.6 trees.

No module is checked and no grammar file is read: use the generated in-memory
parser, just as the reference checker's normal parse path does.
"""
import unittest

from support import forall, table_tests
from lexer import Token, lex
from conformance.generated.fortis_parser import FortisParser as Parser, ParseError


def fragment(test, inp, expect):
    source, start = inp['source'], inp['start']
    # Generated follow sets require statement and pattern delimiters.
    if start == 'statement':
        source, start = '{ ' + source + '\n}', 'block'
    elif start == 'pattern':
        source, start = 'match x { ' + source + ' => 0 }', 'expr'
    parser = Parser(lex(source))
    if expect:
        parser.parse(start)
        test.assertEqual(parser.fallbacks, 0)
    else:
        with test.assertRaises(ParseError) as caught:
            parser.parse(start)
        test.assertTrue(caught.exception.expected)
        test.assertGreaterEqual(caught.exception.tok.line, 1)
        # Local backtracking failure must not poison a subsequent parse.
        Parser(lex('x')).parse('expr')


OPERATORS = {'asExpr', 'multiplicativeExpr', 'additiveExpr', 'shiftExpr',
             'relationalExpr', 'equalityExpr', 'bitAndExpr', 'bitXorExpr',
             'bitOrExpr', 'andExpr', 'orExpr', 'coalesceExpr', 'unaryExpr'}


def shape(node):
    if isinstance(node, Token):
        return node.text
    name, children = node
    if name in OPERATORS and len(children) > 1:
        if name == 'unaryExpr':
            return [children[0].text, shape(children[1])]
        result = shape(children[0])
        for index in range(1, len(children), 2):
            result = [children[index].text, result, shape(children[index + 1])]
        return result
    if name == 'primaryExpr' and isinstance(children[0], Token) and children[0].text == '(':
        return shape(children[1])
    if len(children) == 1:
        return shape(children[0])
    raise AssertionError(f'unhandled tree shape: {node!r}')


def precedence(test, source, expect):
    test.assertEqual(shape(Parser(lex(source)).parse('expr')), expect)


class Fragments(unittest.TestCase):
    @forall(lambda r: r.randint(1, 20), cases=40)
    def test_lr56_parentheses_preserve_grouping(self, depth):
        source = '(' * depth + 'a + b * c' + ')' * depth
        precedence(self, source, ['+', 'a', ['*', 'b', 'c']])


class Precedence(unittest.TestCase):
    pass


table_tests(Fragments, 'parser/fragments.json', fragment)
table_tests(Precedence, 'parser/precedence.json', precedence)
