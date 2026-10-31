"""Runtime constants resolved by the existing type checker (LR 6.10)."""
from checker.decls import DeclChecker
from checker.typer import Typer
from checker.types import Named, FnT, strip_loan
from checker.literals import literal_type
from tree import tokens, children
from lexer import Token
import os


def array_sizes(module, library, file, module_name="main", package="", session=None, entry="main"):
    """Keep each constructor's checked size, including sizes supplied by its expected type.

    These facts belong to source positions, never to the last constructor executed. The
    linker obtains them before renaming type tokens. Positions also cover expressions
    parsed from interpolation holes, which have different node identities at runtime.
    """
    if not any(t.text == "Array" or (t.kind == "STR" and "Array" in t.text) for t in tokens(module)):
        return {}
    dc = DeclChecker(library, file, module_name, package, session, entry)
    dc.load(module)
    sizes = {}
    class RuntimeTyper(Typer):
        def type_primary(self, node, env, expected):
            result = super().type_primary(node, env, expected)
            ty = strip_loan(result)
            if isinstance(ty, FnT):
                ty = strip_loan(ty.ret)
            if isinstance(ty, Named) and ty.name == "Array" and not ty.package and len(ty.args) == 2 and node is not None and not isinstance(node, Token):
                # The constructor's own type reference survives a longer postfix chain.
                ts = tokens(children(node)[0])
                if ts:
                    try:
                        sizes[os.path.abspath(file), ts[0].line, ts[0].col] = literal_type(Token("INT", ty.args[1].show().lstrip("#"), ts[0].line, ts[0].col)).lo
                    except ValueError:
                        pass
            return result
    RuntimeTyper(dc).run(module)
    return sizes
