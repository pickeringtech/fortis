"""LR 6.12, 7.7 and 10.7: structural linearity and the source/holder/exit matrix."""
import support
from support import forall, read_cases, table_tests
import unittest
from types import SimpleNamespace
from checker.decls import Decl, DeclChecker
from checker.typer import Typer, Env
from checker.types import Named, LoanT, OptionalT, TupleT, RecordT, FnT, TypeParam, VOID
from lexer import Token


def case_named(name,args=()):
    # A user type carries its declaration identity after R4/R7 (D-592).
    if name in ('Box','Choice','Wrapper','Generic'):return Decl('class',name,False,'main').as_named(args)
    return Named(name,args)


def typ(spec):
    if isinstance(spec,str): return case_named(spec)
    tag,*args=spec
    if tag=='loan': return LoanT(typ(args[2]),args[0],args[1])
    if tag=='optional': return OptionalT(typ(args[0]))
    if tag=='tuple': return TupleT(tuple(map(typ,args)))
    if tag=='record': return RecordT((('value',typ(args[0])),))
    if tag=='fn': return FnT((),VOID,kind=args[0])
    return case_named(tag,tuple(map(typ,args)))


def typer():
    t=Typer(DeclChecker(SimpleNamespace(modules={},prelude=None),'unit'))
    for name in ('Outcome','Future','Task','List'):
        t.table.decls[name]=Decl('class',name,True,'fortis.prelude')
    for name,kind in [('Box','class'),('Choice','enum'),('Wrapper','type')]:
        d=Decl(kind,name,False,'main',type_params=('T',),linear=('T',))
        if kind=='class': d.fields['value']=(TypeParam('T'),False,False)
        if kind=='enum': d.variants['Some']=[('value',TypeParam('T'))]
        if kind=='type': d.alias=TypeParam('T')
        t.table.decls[name]=d
    return t


def codes(t): return sorted({r['code'] for r in t.records})


def matrix(data):
    t,env=typer(),Env()
    ty=typ(data['type'])
    t.depth=2
    # A field obligation names its projection; a temporary has no source binding.
    path={'field':'owner.value','temporary':'$temporary'}.get(data['source'],'value')
    env.set(path,ty)
    tok=Token('LOWER',path,1,1)
    t.check_stored_type(ty,tok,data['holder']=='class_field')
    if t.is_linear(ty): t.obligate(path,tok)
    kind=data['exit']
    if data['holder']=='returned_value' and kind=='return': t.own.move(path)
    if kind in ('fail','or_fail'):
        t.unwind_point(Token('KW',kind,2,1),kind,env)
    elif kind=='loop_early_exit' and data['holder']=='collection_element' and t.is_linear(ty):
        # The collection is transferred into a consuming cursor. An early exit still owes its remainder.
        t.own.move(path)
        t.linear_loops.append((2,Token('LOWER','item',1,1)))
        t.linear_loop_exit(2,Token('KW','break',2,1),'break')
    else:
        t.report_open(0 if kind=='return' else 2,kind)
    return codes(t)


class ExitMatrix(unittest.TestCase):
    pass


# Seven seeded properties, each exhaustively traversing its 6 holders x 9 exits. Rust exposes all 378 cells
# separately. Shuffle only traversal order; every cell is always run, whatever the replay seed.
for source in ('local','parameter_loan','field','temporary','literal','call_result','fallback'):
    rows=[r for r in read_cases('obligations/matrix.json') if r['input']['source']==source]
    def make(rows):
        @forall(lambda rng: rng.sample(rows,len(rows)),cases=1)
        def check(self, shuffled):
            for row in shuffled:
                with self.subTest(cell=row['name']):
                    self.assertEqual(matrix(row['input']),row['expect'])
        return check
    setattr(ExitMatrix,'test_lr_6_12_'+source,make(rows))


def check_type(self,data,want):
    t=typer(); ty=typ(data['type']);tok=Token('LOWER','value',1,1)
    if data['op']=='linear': actual=t.is_linear(ty)
    elif data['op']=='instantiate':
        t.table.decls['Generic']=Decl(data['kind'],'Generic',False,'main',type_params=('T',),linear=('T',) if data['bound'] else ())
        t.dc.check_linear_instantiation(ty,tok); actual=codes(t)
    else:
        t.obligate('value',tok)
        if data['action']=='consume':t.consume('value')
        elif data['action']=='move':t.own.move('value')
        else:
            env=Env();env.set('value',ty)
            t.unwind_point(tok,'await',env,keep='value')
            self.assertIn('value',t.own.obligations)
            return self.assertEqual(codes(t),want)
        t.report_open(0,'end of block');actual=codes(t)
    self.assertEqual(actual,want)


class LinearTypes(unittest.TestCase): pass

table_tests(LinearTypes,'obligations/types.json',check_type)


class GenericLoans(unittest.TestCase):
    def test_d646_lent_type_parameter_is_not_held_by_value(self):
        """D-646: LR 6.12 permits generic code that only borrows a linear T."""
        t=typer()
        self.assertFalse(t.holds_by_value(LoanT(TypeParam('T'),'alias','x'),'T',result=True))


from checker.literals import assignable


def check_callback(self,data,want):
    make=lambda mode: FnT((typ(data['type']),),VOID,modes=(mode,))
    self.assertEqual(assignable(typer().dc,make(data['have']),make(data['want']))[0],want)


class CallbackModes(unittest.TestCase): pass

table_tests(CallbackModes,'obligations/callbacks.json',check_callback)



def check_loop(self,data,want):
    t=typer()
    t.linear_loops=[(d,Token('LOWER',f'item{d}',d,1)) for d in range(1,4)]
    t.linear_loop_exit(data['depth'],Token('KW','exit',9,1),data['exit'],data['unwind'])
    self.assertEqual([[r['fields']['unconsumed_path'].removeprefix('remainder of '),r['code']] for r in t.records],want)


class LoopRemainders(unittest.TestCase): pass

table_tests(LoopRemainders,'obligations/loops.json',check_loop)
