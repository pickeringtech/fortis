"""LR 4.3-4.5: names and visibility with in-memory module signatures."""
import unittest
from types import SimpleNamespace
from support import forall, read_cases, table_tests
from lexer import lex
from conformance.generated.fortis_parser import FortisParser
from names import LibraryModule, Resolver, exported


def library():
    sources = read_cases('names/modules.json')[0]['input']
    modules = {key: LibraryModule(key, None, FortisParser(lex(src)).parse('module'))
               for key, src in sources.items()}
    return SimpleNamespace(modules=modules, prelude=modules['fortis.prelude'],
                           rec=SimpleNamespace(parse=lambda tokens, start: FortisParser(tokens).parse(start)))


def check(test, source, expect):
    resolver = Resolver(library(), 'unit.ft')
    records = resolver.resolve_module(FortisParser(lex(source)).parse('module'))
    test.assertEqual(sorted({r['code'] for r in records}), expect, records)


class Resolution(unittest.TestCase):
    @forall(lambda r: r.randint(1, 12), cases=40)
    def test_lr45_nested_let_cannot_shadow_parameter(self, depth):
        source = 'func f(value: Int) { ' + '{\n' * depth + 'let value = 1' + '\n}' * (depth + 1)
        check(self, source, ['E-NAM-003'])

    def test_lr45_signature_prelude_exports(self):
        row = read_cases('names/modules.json')[0]
        self.assertEqual(exported(library().prelude), row['expect'])


table_tests(Resolution, 'names/resolution.json', check)
