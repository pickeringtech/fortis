"""LR 7.9, 10.8 and 12.2: cleanups, fields, owned locals and exits."""
import unittest
from itertools import product
from support import forall
from evaluation.context import evaluator, Syntax
from interpreter.eval import Env, ReturnSignal, GiveSignal, BreakSignal, ContinueSignal, RecoverSignal
from interpreter.values import ClassInfo, Instance, ListVal, LentTuple, LentRecord, Failure, FailureSignal
from interpreter.executor import DefectSignal, CancelSignal
import lexer


def value(name, fields=None):
    return Instance(ClassInfo(name,{}, {}, {}, cleanup=name),fields or {})


def recorder():
    it=evaluator(); seen=[]
    it.run_block=lambda block,env:seen.append(block)
    return it,seen


class Destruction(unittest.TestCase):
    @forall(lambda r:[r.choice(('owner','loan','moved','returned','self')) for _ in range(r.randrange(8))])
    def test_lr7_9_binding_states(self, states):
        it,seen=recorder();env=Env()
        for i,state in enumerate(states):
            name='self' if state=='self' else str(i)
            # Only one self binding exists; choose distinct values for all other locals.
            env.declare(name,value(str(i)),owns=state!='loan')
            env.vars[name].moved=state=='moved';env.vars[name].returned=state=='returned'
        it.drop_locals(env)
        self.assertEqual(seen,[str(i) for i,s in reversed(list(enumerate(states))) if s=='owner'])

    @forall(lambda r:r.sample(list('abcdef'),r.randrange(7)))
    def test_lr12_2_nested_fields_declaration_order(self, names):
        it,seen=recorder();parent=value('parent',{n:value(n) for n in names})
        it.destroy(parent);it.destroy(parent)
        self.assertEqual(seen,['parent']+names)
        self.assertEqual(it.cleanup_depth,0)
        self.assertEqual(it.writers,[])


def aggregate_test(kind,count):
    def test(self):
        it,seen=recorder();xs=[value(str(i)) for i in range(count)]
        if kind=='list':container=ListVal(xs)
        elif kind=='tuple':container=tuple(xs)
        elif kind=='record':container={str(i):x for i,x in enumerate(xs)}
        elif kind=='lent_tuple':container=LentTuple(xs,range(0,count,2))
        else:container=LentRecord({str(i):x for i,x in enumerate(xs)},[str(i) for i in range(0,count,2)])
        it.destroy(container)
        self.assertEqual(seen,[str(i) for i in range(count) if not kind.startswith('lent') or i%2])
    return test
for kind,count in product(('list','tuple','record','lent_tuple','lent_record'),(0,1,4)):
    setattr(Destruction,f'test_lr7_9_{kind}_{count}',aggregate_test(kind,count))


def exit_test(kind,unwinding):
    def test(self):
        it=evaluator();seen=[];original=it.run_block
        signal={'return':ReturnSignal(1),'give':GiveSignal(1),'break':BreakSignal(None),'continue':ContinueSignal(None),'recover':RecoverSignal(1),'failure':FailureSignal(Failure('unit.failure')),'cancel':CancelSignal(),'defect':DefectSignal('unit.defect')}[kind]
        def statement(st,env):
            env.declare('a',value('a'));env.declare('b',value('b'))
            env.defers.extend(('first','second'))
            raise signal
        def block(node,env):
            if isinstance(node,str):seen.append(node)
            else:original(node,env)
        it.run_statement=statement;it.run_block=block
        with self.assertRaises(type(signal)):
            it.run_block(Syntax.parse(lexer.lex('{ discard 0 }'),'block'),Env())
        self.assertEqual(seen,[] if kind=='defect' else ['second','first','b','a'])
        self.assertEqual(it.cleanup_depth,0)
    return test
for kind in ('return','give','break','continue','recover','failure','cancel','defect'):
    setattr(Destruction,'test_lr12_2_exit_'+kind,exit_test(kind,kind in ('failure','cancel')))
