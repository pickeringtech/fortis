"""An evaluator over supplied syntax, with file-loading dependencies replaced by empty data.

No checker or whole program runs. The generated parser is imported as code and parses
only the expression/statement passed by a unit. Every evaluation method is production code.
"""
from types import SimpleNamespace
from unittest.mock import patch
from support import ROOT
from conformance.generated.fortis_parser import FortisParser
from interpreter.eval import Interpreter, LIBRARY_MODULES


class Syntax:
    @staticmethod
    def parse(tokens, rule):
        parser = FortisParser(tokens)
        return parser.parse(rule)


def evaluator():
    empty = ('module', [])
    library = {m: {'tree': empty, 'methods': {}, 'funcs': {}, 'imports': {}} for m in LIBRARY_MODULES}
    # A supplied linked tree bypasses checking, just as a checked linked module does.
    # Patch only the two I/O boundaries, keeping normal state initialisation intact.
    with patch('interpreter.eval.fparser.load_grammar', return_value=Syntax()), \
         patch('interpreter.eval.library', return_value=library):
        return Interpreter('', linked=(None, SimpleNamespace(tree=empty, key=None)))
