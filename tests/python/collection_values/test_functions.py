"""LR 6.7: call each real library method body as a unit, with data and callback doubles.

Fortis implementation modules are loaded once as code, just as Python implementation
modules are imported. No test calls main, checks a program, or performs I/O.
"""
import unittest
from types import SimpleNamespace
from unittest.mock import patch
from support import table_tests
from evaluation.context import Syntax
from interpreter import eval as evaluation
from interpreter.eval import Interpreter, Env
from interpreter.values import VOID, Closure, Failure, ListVal, Instance
from interpreter.intrinsics import MapVal

# Code loading only: use the generated parser, without loading a grammar or checking a module.
with patch('interpreter.eval.fparser.load_grammar',return_value=Syntax()):
    LIBRARY = evaluation.library()


def evaluator():
    with patch('interpreter.eval.fparser.load_grammar',return_value=Syntax()), \
         patch('interpreter.eval.library',return_value=LIBRARY):
        return Interpreter('',linked=(None,SimpleNamespace(tree=('module',[]),key=None)))


def invoke(it, typ, method, receiver, args):
    module,(head,body)=it.library_method(typ,method)
    return it.call_library(module,head,body,[(None,x) for x in args],receiver)


def normal(value):
    if value is VOID: return None
    if isinstance(value,Failure): return dict(code=value.code,payload=value.payload)
    if isinstance(value,ListVal): return [normal(x) for x in value.items]
    if isinstance(value,Instance):
        if value.cls.name=='Deque': return [normal(x) for x in value.fields['front'].items[::-1]+value.fields['back'].items]
        return normal(value.fields['items'])
    if isinstance(value,MapVal): return [[normal(k),normal(v)] for k,v in value.entries.values()]
    if isinstance(value,tuple): return [normal(x) for x in value]
    return value


def check(test,data,expect):
    it=evaluator();typ=data['type'];items=ListVal(data['items'][:])
    receiver=items if typ=='List' else invoke(it,typ,'fromList',None,[items])
    seen=[];failures=[];callback=Closure(['item'],None,Env());original=it.call_value
    def call(value,args):
        if value is not callback: return original(value,args)
        values=[x for _,x in args];seen.append(values)
        cb=data['callback']
        if cb.startswith('fail_') and len(seen)-1==int(cb[5:]):
            failure=Failure('unit.stop',{'item':values[-1]},origin='callback')
            failures.append(failure);return failure
        if cb=='double': return values[-1]*2
        if cb=='odd': return values[-1]%2==1
        if cb=='sum' or data['method']=='tryFold': return sum(values)
        return VOID if data['method'] in ('each','tryEach') else values[-1]
    it.call_value=call
    args=data['args'][:]
    if data['callback'] is not None:args.append(callback)
    actual=invoke(it,typ,data['method'],receiver,args)
    test.assertEqual(dict(value=normal(actual),visited=seen),expect)
    if failures:
        test.assertIs(actual,failures[0])
        test.assertEqual(actual.origin,'callback')


class FunctionalMethods(unittest.TestCase): pass

table_tests(FunctionalMethods,'collections/functions.json',check)
