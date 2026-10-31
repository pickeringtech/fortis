"""LR 6.7: sequence tests check each result and the container left behind."""
import unittest
from support import table_tests, forall
from evaluation.context import evaluator
from interpreter.intrinsics import ASSOC, METHODS, ArrayVal, MapVal, SetVal
from interpreter.values import VOID, Failure, ListVal, ClassInfo, Instance


def normal(value):
    if value is VOID: return None
    if isinstance(value, Failure): return dict(code=value.code,payload=value.payload)
    return value


def check(test, data, expect):
    it = evaluator()
    typ, initial = data['type'], data['initial']
    if typ == 'List': value = ListVal(initial[:])
    elif typ == 'Array': value = ArrayVal(initial[:])
    elif typ == 'Deque': value = Instance(ClassInfo('Deque',{}, {}, {}),dict(front=ListVal(initial[0][:]),back=ListVal(initial[1][:])))
    else:
        value = ASSOC[typ,'default'](it,[])
        for x in initial: METHODS[typ,'set' if typ=='Map' else 'add'](it,value,x if typ=='Map' else [x])
    results=[]
    for op,*args in data['ops']:
        actual = METHODS[typ,op](it,value,args)
        if op == 'iterate':
            count = len(initial) if typ == 'List' else sum(map(len,initial))
            actual = [METHODS['ListCursor','next'](it,actual,[]) for _ in range(count+2)]
        results.append(normal(actual))
    if isinstance(value,MapVal): items=[list(x) for x in value.entries.values()]
    elif isinstance(value,SetVal): items=list(value.items.values())
    elif typ=='Deque': items=[value.fields['front'].items,value.fields['back'].items]
    else: items=value.items
    test.assertEqual(dict(results=results,items=items),expect)


class Collections(unittest.TestCase):
    @forall(lambda r: [r.randint(-4,4) for _ in range(r.randint(0,30))])
    def test_lr6_7_set_first_insertion_order(self, values):
        it=evaluator(); value=SetVal()
        for n in values: METHODS['Set','add'](it,value,[n])
        self.assertEqual(list(value.items.values()),list(dict.fromkeys(values)))
        for n in values: METHODS['Set','remove'](it,value,[n])
        self.assertEqual(METHODS['Set','count'](it,value,[]),0)

    @forall(lambda r: [r.randint(-4,4) for _ in range(r.randint(0,30))])
    def test_lr6_7_map_replacement_preserves_first_position(self, values):
        it=evaluator(); value=MapVal(); expected={}
        for i,n in enumerate(values):
            METHODS['Map','set'](it,value,[n,i]);expected[n]=i
        self.assertEqual(list(value.entries.values()),list(expected.items()))


table_tests(Collections,'collections/intrinsics.json',check)
