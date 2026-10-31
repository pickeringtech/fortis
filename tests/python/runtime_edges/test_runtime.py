"""LR 6.4, 6.6, 6.13 and 10.3, through the production intrinsic registries."""
import unittest
from types import SimpleNamespace
from support import table_tests, forall
from interpreter.intrinsics import ASSOC, METHODS, BytesVal
from interpreter.convert import convert
from interpreter.numeric import Dec
from interpreter.values import Failure, ListVal, render_message


def normal(value, stringify=False):
    if isinstance(value, Failure):
        return {"code": value.code, "payload": {k: normal(v, stringify) for k, v in value.payload.items()}}
    if stringify:
        if isinstance(value, bool):
            return str(value).lower()
        if isinstance(value, (int, float)):
            return str(int(value))
        if isinstance(value, Dec):
            return value.display()
        if hasattr(value, "text"):
            return value.text
    return value


def integer(test, data, expect):
    test.assertEqual(normal(ASSOC['Int', data['op']](None, [data['a'], data['b']])), expect)


def conversion(test, data, expect):
    value = data['value']
    if data['kind'] == 'int': value = int(value)
    elif data['kind'] == 'decimal': value = Dec.parse(value)
    elif data['kind'] == 'bytes': value = BytesVal(bytes(value))
    actual = convert(value, data['target'], 'unit.origin', lambda s: SimpleNamespace(text=s), BytesVal)
    test.assertEqual(normal(actual, True), expect)
    if isinstance(actual, Failure): test.assertEqual(actual.origin, 'unit.origin')


def text(test, data, expect):
    for receiver in ('String', 'StringView'):
        with test.subTest(receiver=receiver):
            value = METHODS[receiver, data['op']](None, data['text'], [])
            test.assertEqual(value.items if isinstance(value, ListVal) else value, expect)


class Integers(unittest.TestCase):
    @forall(lambda rng: (rng.randint(-(2**63), 2**63-1), rng.randint(-100,100)))
    def test_lr6_4_wrapping_add_sub_inverse(self, pair):
        a, b = pair
        result = ASSOC['Int','wrappingAdd'](None, [a,b])
        self.assertEqual(ASSOC['Int','wrappingSub'](None, [result,b]), a)

    @forall(lambda rng: (rng.randint(-(2**63), 2**63-1), rng.choice([-19,-3,-1,1,3,19])))
    def test_lr6_4_division_remainder_identity(self, pair):
        a, b = pair
        q, r = (ASSOC['Int',op](None, [a,b]) for op in ('div','rem'))
        if isinstance(q, Failure):
            self.assertEqual((a,b), (-(2**63),-1))
        else:
            self.assertEqual(q*b+r, a)
            self.assertLess(abs(r), abs(b))
            self.assertTrue(r == 0 or (r > 0) == (a > 0))

class Conversions(unittest.TestCase): pass
class TextEdges(unittest.TestCase): pass
class FailureMessages(unittest.TestCase): pass

table_tests(Integers, 'runtime/integers.json', integer)
table_tests(Conversions, 'runtime/conversions.json', conversion)
table_tests(TextEdges, 'runtime/text_edges.json', text)
table_tests(FailureMessages, 'runtime/messages.json', lambda t,d,e: t.assertEqual(render_message(d['template'], d['payload']),e))

table_tests(Integers, 'runtime/modular.json', integer)

class NumericText(unittest.TestCase): pass
table_tests(NumericText, 'runtime/numeric_text.json', conversion)
