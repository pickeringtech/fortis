"""LR 3, 5, 6, 9, 10 and 12: individual expression and statement evaluation."""
import unittest
from support import table_tests
from evaluation.context import evaluator, Syntax
from interpreter.eval import Env, ReturnSignal
from interpreter.values import VOID, ListVal, Failure
from interpreter.numeric import Dec, Dur
from interpreter.intrinsics import BytesVal
import lexer


def normal(v):
    if v is VOID: return None
    if isinstance(v, ListVal): return [normal(x) for x in v.items]
    if isinstance(v, tuple): return [normal(x) for x in v]
    if isinstance(v, dict): return {k: normal(x) for k,x in v.items()}
    if isinstance(v, Dec): return {'decimal': v.display()}
    if isinstance(v, Dur): return {'duration': v.ns}
    if isinstance(v, BytesVal): return {'bytes':list(v.data)}
    if isinstance(v, float): return {'float':str(v)}
    if isinstance(v, Failure): return {'code':v.code,'payload':v.payload}
    return v


def check(test,data,expect):
    it=evaluator();env=Env()
    for p in data['params']:
        v=p['value'];env.declare(p['name'],ListVal(v[:]) if isinstance(v,list) else v,owns=False)
    if data['prelude']:it.load_declarations(Syntax.parse(lexer.lex(data['prelude']),'module'))
    node=Syntax.parse(lexer.lex(data['source']), 'expr' if data['kind']=='expr' else 'block')
    if data['kind']=='expr':value=it.eval(node,env)
    else:
        with test.assertRaises(ReturnSignal) as result:it.run_block(node,env)
        value=result.exception.value
    test.assertEqual(normal(value),expect['value'])


class Fragments(unittest.TestCase):pass

table_tests(Fragments,'evaluation/fragments.json',check)

table_tests(Fragments,'evaluation/known_limits.json',check)
