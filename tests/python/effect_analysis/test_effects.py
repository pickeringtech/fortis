"""LR 11.1-11.6: claims, summaries, contracts, authority and callback effects."""
import support
from support import forall, table_tests
import unittest
from types import SimpleNamespace
from checker.decls import Decl, DeclChecker, Sig
from checker.typer import Typer, Env
from checker.types import Named, FnT, VOID
from checker.effects import Summary, contract_contradictions
from lexer import Token


def typer(): return Typer(DeclChecker(SimpleNamespace(modules={},prelude=None),'unit'))
def sig(mods=(),params=()): return Sig('unit',list(params),VOID,None,tuple(mods),(),False)
def statuses(summary): return {e:summary.status(e) for e in summary.effects}
def codes(t): return sorted({r['code'] for r in t.records})


def check_callable(self,data,want):
    t=typer()
    self.assertEqual(statuses(t.summary_from_modifiers(sig(data))),want)
    if not set(data)-{'pure','noalloc'}:
        # A pure callback can allocate. Only noalloc removes that possibility (LR 11.3, D-539).
        callback=FnT((),VOID,modifiers=tuple(data))
        actual=t.callee_summary(None,sig(params=[('callback',callback,'')]),'function unit')
        self.assertEqual(statuses(actual),want)


def check_import(self,data,want):
    source=Summary();effect=data['effect'];certainty=data['certainty']
    if certainty=='certain':source.add(effect,'trusted')
    else:source.add_uncertain(effect,'trusted',certainty)
    dest=Summary();dest.imports(source,Token('LOWER','call',2,1))
    self.assertEqual(list(dest.effects),[want['effect']])
    self.assertEqual(dest.status(effect),want['status'])
    self.assertEqual(dest.effects[effect],'trusted')
    self.assertEqual(dest.sites[effect].line,2)


def check_claim(self,data,want):
    t=typer();t.eff.add(data['effect'])
    t.check_claims(sig([data['modifier']]),Token('LOWER','unit',1,1),'function unit')
    self.assertEqual(codes(t),want)


def check_contract(self,data,want):
    parts=[(origin,{k:set(v) for k,v in items.items()}) for origin,items in data]
    self.assertEqual([list(row) for row in contract_contradictions(parts,False)],want)
    # The report path must use E-EFF-050, not merely detect an abstract contradiction.
    t=typer();signature=sig();signature.contracts=tuple(origin for origin,_ in parts)
    for origin,items in parts:t.table.decls[origin]=Decl('contract',origin,False,'main',items=items)
    t.check_contradictions(signature,Token('LOWER','unit',1,1),'function unit')
    self.assertEqual(codes(t),['E-EFF-050'] if want else [])


def check_authority(self,data,want):
    for shared in (False,True):
        t=typer();name=data['name']
        d=Decl('class',name,False,'main' if data['shadow'] else 'fortis.prelude')
        if not data['shadow']:d.implements=[Named('Authority')]
        t.table.decls[name]=d
        ty=Named('Shared',(d.as_named(),)) if shared else d.as_named()
        self.assertEqual(t.is_authority(ty),want)
        t.eff.add('io');t.eff.needs_auth=True
        t.check_claims(sig(params=[('authority',ty,'')]),Token('LOWER','unit',1,1),'function unit')
        self.assertEqual(codes(t),[] if want else ['E-EFF-010'])


def check_footprint(self,data,want):
    t,env=typer(),Env();env.set('value',Named('Int'),kind=data['kind'])
    place='value'
    if data['alias']:
        t.own.add_loan('value','editable','child',1,1);place='child'
    callee=Summary();callee.footprint.add('argument')
    t.import_call(callee,Token('LOWER','call',2,1),None,[('argument',place)],env,False,[])
    self.assertEqual({'effects':sorted(t.eff.effects),'footprint':sorted(t.eff.footprint)},want)


class CallableSummaries(unittest.TestCase):pass
class Imports(unittest.TestCase):pass
class Claims(unittest.TestCase):pass
class Contracts(unittest.TestCase):pass
class Authorities(unittest.TestCase):pass
class Footprints(unittest.TestCase):pass
for cls,path,check in [(CallableSummaries,'callables',check_callable),(Imports,'imports',check_import),
                       (Claims,'claims',check_claim),(Contracts,'contracts',check_contract),
                       (Authorities,'authorities',check_authority),(Footprints,'footprints',check_footprint)]:
    table_tests(cls,'effects/'+path+'.json',check)
del cls  # Do not expose the last TestCase twice to unittest discovery.


EFFECTS=('alloc','io','mutate','nondet','suspend','block','fail','recovers')


def summary(mask):
    out=Summary()
    for i,e in enumerate(EFFECTS):
        if mask>>i&1:out.add(e)
    return out


class EffectProperties(unittest.TestCase):
    @forall(lambda r:(r.randrange(256),r.randrange(256),r.randrange(256)))
    def test_lr_11_3_union_is_associative_and_idempotent(self,masks):
        a,b,c=map(summary,masks)
        left=Summary();left.imports(a);left.imports(b);left.imports(c)
        bc=Summary();bc.imports(b);bc.imports(c)
        right=Summary();right.imports(a);right.imports(bc);right.imports(a)
        self.assertEqual(left.effects,right.effects)
        self.assertEqual(set(left.effects),set(a.effects)|set(b.effects)|set(c.effects))

    @forall(lambda r:r.sample(range(256),256),cases=1)
    def test_lr_11_3_call_imports_all_effect_subsets(self,masks):
        for mask in masks:
            for asynchronous in (False,True):
                t=typer();callee=summary(mask)
                t.import_call(callee,Token('LOWER','call',1,1),None,[],Env(),asynchronous,[])
                # fail belongs to disposition of the outcome; suspend to awaiting the returned future.
                omitted={'fail','suspend'} if asynchronous else {'fail'}
                self.assertEqual(set(t.eff.effects),set(callee.effects)-omitted)

    @forall(lambda r:r.sample(['certain','possible','unknown'],3))
    def test_certain_evidence_dominates_callback_and_trust_uncertainty(self,order):
        s=Summary()
        for certainty in order:
            if certainty=='certain':s.add('alloc')
            else:s.add_uncertain('alloc','trusted',certainty)
        self.assertEqual(s.status('alloc'),'violated')
        self.assertEqual(s.effects['alloc'],'proved')

    @forall(lambda r:r.choice([('try','fail'),('async','suspend'),('recovers','recovers')]))
    def test_lr_11_5_same_contract_is_checked_once(self,pair):
        mod,effect=pair
        parts=[('C',{'modifiers':{mod},'forbids':{effect}})]
        self.assertEqual(len(contract_contradictions(parts,False)),1)
        self.assertFalse(contract_contradictions(parts,True))


class ContractBodies(unittest.TestCase):
    @forall(lambda r:r.sample(EFFECTS,len(EFFECTS)),cases=1)
    def test_lr_11_5_forbidden_effects_are_transitive(self,effects):
        for effect in effects:
            for present in (False,True):
                t=typer();t.table.decls['C']=Decl('contract','C',False,'main',items={'forbids':{effect}})
                callee=Summary()
                if present:callee.add(effect)
                t.eff.imports(callee)
                signature=sig();signature.contracts=('C',)
                t.check_contracts(signature,Token('LOWER','unit',1,1),'function unit')
                self.assertEqual(codes(t),['E-EFF-001'] if present else [])

    @forall(lambda r:r.sample(['requires','excludes'],2),cases=1)
    def test_lr_11_5_authority_requirements_distinguish_parameters_and_callees(self,clauses):
        for clause in clauses:
            for place in ('absent','parameter','shared_parameter','callee'):
                t=typer();t.table.decls['C']=Decl('contract','C',False,'main',items={clause:{'Console'}})
                signature=sig();signature.contracts=('C',)
                if place.endswith('parameter'):
                    ty=Named('Shared',(Named('Console'),)) if place=='shared_parameter' else Named('Console')
                    signature.params=[('console',ty,'')]
                if place=='callee':t.eff.authorities.add('Console')
                t.check_contracts(signature,Token('LOWER','unit',1,1),'function unit')
                rejects=(place not in ('parameter','shared_parameter')) if clause=='requires' else place!='absent'
                self.assertEqual(codes(t),['E-EFF-001'] if rejects else [])



def check_capability(self,data,want):
    t=typer();t.current_label='function main' if data['entry'] else 'function worker'
    if data['context']!='body':t.ctx.append(data['context'])
    signature=sig(params=[('console',Named('Console'),'')] if data['parameter'] else [])
    t.table.decls['main']=Decl('func','main',False,'main',sig=signature)
    for i in range(data['calls']):t.capabilities_use(Token('LOWER','capabilities',i+1,1),data['called'])
    t.check_claims(signature,Token('LOWER','main',1,1),t.current_label)
    self.assertEqual(codes(t),want)


class Capabilities(unittest.TestCase):pass

table_tests(Capabilities,'effects/capabilities.json',check_capability)


class LazyMetadata(unittest.TestCase):
    def test_data_only_summary_does_not_load_files(self):
        from unittest.mock import patch
        from checker.effects import effect_data, library_summary
        effect_data.cache_clear()
        with patch('checker.effects.load_effect_data',side_effect=AssertionError('unexpected file read')):
            self.assertFalse(library_summary(None,None,'unit',[],False,None).effects)

    def test_metadata_is_cached_after_the_first_library_lookup(self):
        from unittest.mock import patch
        from checker.effects import effect_data, library_summary
        effect_data.cache_clear()
        try:
            with patch('checker.effects.load_effect_data',return_value={('fortis.unit',None,'make'):('alloc',)}) as loader:
                for _ in range(2):
                    self.assertEqual(library_summary('fortis.unit',None,'make',[],False,None).effects,{'alloc':'trusted'})
                loader.assert_called_once_with()
        finally:
            effect_data.cache_clear()
