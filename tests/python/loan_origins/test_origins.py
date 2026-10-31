"""LR 6.11, 7.3, 7.6, 8.3: copies, child loans, return regions and inferred captures."""
import support
from support import forall,read_cases,table_tests
import unittest
from types import SimpleNamespace
from checker.decls import Decl,DeclChecker
from checker.typer import Typer,Env
from checker.types import Named,LoanT,OptionalT,TupleT,RecordT,FnT,TypeParam,Unknown,VOID
from lexer import Token


def tok(text,kind='LOWER'):return Token(kind,text,1,1)


class FragmentBuilder:
    """Parser boundary stub: infer captures still chooses the requested head itself.

    Only a place and an already chosen capture head are requested. Capture discovery and ranking run in
    the production probe. Rust's equivalent builder is its in-memory generated fragment parser.
    """
    def __init__(self):self.head=[]
    def parse(self,tokens,rule):
        tokens=[t for t in tokens if t.kind not in ('EOF','NL')]
        if rule=='place':return ('place',tokens)
        assert rule=='lambda',rule
        self.head=[t.text for t in tokens]
        return ('lambda',[tok('fn','KW'),tok('(' ,'PUNCT'),tok(')','PUNCT'),tok('=>','PUNCT'),('expr',[('primaryExpr',[tok('0','INT')])])])


def typer():
    t=Typer(DeclChecker(SimpleNamespace(modules={},prelude=None,rec=FragmentBuilder()),'unit'))
    t.table.decls['Must']=Decl('class','Must',True,'fortis.prelude')
    t.table.decls['Box']=Decl('class','Box',False,'main',type_params=('T',),linear=('T',),fields={'value':(TypeParam('T'),True,False)})
    t.table.decls['Ticket']=Decl('class','Ticket',False,'main',fields={'value':(Named('Must'),True,False)})
    t.table.decls['Choice']=Decl('enum','Choice',False,'main',variants={'held':[('value',Named('Must'))]})
    t.table.decls['Phantom']=Decl('class','Phantom',False,'main',type_params=('T',),linear=('T',))
    t.dc.linear_params.add('L')
    t.dc.tparam_bounds['L']={'Copyable'}
    return t


def case_named(name,args=()):
    # A user type carries its declaration identity after R4/R7 (D-592).
    if name in ('Box','Ticket','Choice','Phantom'):return Decl('class',name,False,'main').as_named(args)
    return Named(name,args)


def typ(spec):
    if isinstance(spec,str):return case_named(spec)
    tag,*a=spec
    if tag=='unknown':return Unknown()
    if tag=='parameter':return TypeParam(a[0])
    if tag=='optional':return OptionalT(typ(a[0]))
    if tag=='tuple':return TupleT(tuple(map(typ,a)))
    if tag=='record':return RecordT((('value',typ(a[0])),))
    if tag=='fn':return FnT((),VOID)
    if tag=='loan':return LoanT(typ(a[0]),'alias','p')
    return case_named(tag,tuple(map(typ,a)))


def codes(t):return sorted({r['code'] for r in t.records})


def check_return(self,data,want):
    t,env=typer(),Env()
    for name in ('p','q'):env.set(name,Named('String'),kind='param')
    env.set('local',Named('String'))
    origin=data['origin'];source=data['source']
    ret=Named('StringView') if origin=='none' else LoanT(Named('String'),'alias',origin)
    t.ret_type=OptionalT(ret) if data['optional'] else ret
    token=tok(source);e=('primaryExpr',[token]);t.pending=(source,'alias',token)
    t.check_returned_loans(e,LoanT(Named('String'),'alias',source),env,'return')
    self.assertEqual(codes(t),want)


def check_child(self,data,want):
    t,env=typer(),Env();env.set('owner',Named('String'),True,'mut')
    for name in ('parent','child'):env.set(name,Named('String'))
    t.own.add_loan('owner',data['parent'],'parent',1,1)
    t.own.add_loan('parent',data['child'],'child',2,2)
    t.use_place(data['target'],tok(data['target']),data['use'],env)
    self.assertEqual(codes(t),want)


def check_copy(self,data,want):self.assertEqual(typer().dc.is_copyable(typ(data)),want)


def expression(data):
    if 'token' in data:return tok(data['token'][1],data['token'][0])
    return (data['rule'],[expression(c) for c in data['kids']])


def check_capture(self,data,want):
    t,env=typer(),Env();env.set('value',Named(data['type']),True,'mut')
    node=expression(data['tree'])
    if data['direct']:t.infer_capture_node=id(node)
    t.infer_lambda_captures(node,env,None)
    self.assertEqual({'codes':codes(t),'head':t.dc.lib.rec.head},want)
    self.assertFalse(t.own.moved) # the probe must not move the enclosing binding
    self.assertFalse(t.eff.effects) # or leak its effects into the caller


class ReturnedLoans(unittest.TestCase):pass
class ChildLoans(unittest.TestCase):pass
class Copyability(unittest.TestCase):pass
class InferredCaptures(unittest.TestCase):pass
for cls,path,check in [(ReturnedLoans,'returns',check_return),(ChildLoans,'children',check_child),(Copyability,'copies',check_copy),(InferredCaptures,'captures',check_capture)]:
    table_tests(cls,'loan_origins/'+path+'.json',check)
del cls  # Do not expose the last TestCase twice to unittest discovery.


class OriginProperties(unittest.TestCase):
    @forall(lambda r:(r.randrange(1,8),r.randrange(1,8)))
    def test_lr_7_3_child_ends_without_ending_parent(self,depths):
        parent,child=sorted(depths);child+=1
        t=typer();t.own.add_loan('owner','editable','parent',parent,1);t.own.add_loan('parent','alias','child',child,1)
        t.own.end_block(child)
        self.assertIn('parent',t.own.holders)
        self.assertNotIn('child',t.own.holders)
        self.assertEqual(t.own.owner_path('parent.field'),'owner.field')


class ResultLoans(unittest.TestCase):
    pass

def check_result_loans(test, data, expect):
    test.assertEqual(typer().carries_onward(typ(data)), expect)

table_tests(ResultLoans, "loan_origins/results.json", check_result_loans)
