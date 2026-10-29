"""Plan compatibility (Language Reference 11.4; PROG-VM-04; LANG-11-03): a build-time step, separate from type
checking, whose records name the policy site. The reference toolchain runs it before `run` and `test`, as the
production toolchain does before `build`, `vm`, `native` and `test` (compiler/crates/fortis-types/src/plans.rs).

A `noalloc` function carrying `@observe(Schema)` emits that schema's record at every exit; a schema field whose type
is not trivial makes the record allocate, which conflicts with the claim: `E-EFF-030` at the `@observe` site."""
from __future__ import annotations

import os
import sys
from typing import List, Optional

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "conformance", "tools"))
import diagnostics  # noqa: E402
from tree import direct_tokens, find, tokens  # noqa: E402

from .decls import DeclChecker  # noqa: E402
from .typer import Typer  # noqa: E402
from .typer import strip  # noqa: E402


def check(lib, mtree, file: str) -> List[dict]:
    """One module in its own context: no package is visible to it."""
    return check_module(lib, mtree, file)


def check_module(lib, mtree, file: str, module: str = "main", package: str = "", session=None, entry: Optional[str] = "main") -> List[dict]:
    """One module as `check_tree` contexts it: the package-aware library resolves an imported type (an alias of
    `Int`, a sibling's class) exactly as the checker resolves it, so the step sees the field's real triviality.
    `entry` is that module's program entry (4.6), as `check_tree` receives it."""
    dc = DeclChecker(lib, file, module, package, session, entry)
    dc.load(mtree)
    return check_decls(dc, file)


def check_decls(dc, file: str) -> List[dict]:
    ty = Typer(dc)
    out = []
    # `@observe` is the prelude's aspect only where this module's table resolves the name to it: an aspect the module
    # declares or imports under that name attaches no core observation, and the IR builder records none for it
    # (fortis-mir `core_aspect`), so it has no plan to conflict with (11.8; D-314)
    od = dc.table.get("observe")
    if od is not None and od.module != "fortis.prelude":
        return out
    for name, d in dc.table.decls.items():
        if d.kind != "func" or d.module != dc.module_name or d.sig is None:
            continue
        if "noalloc" not in ty.expanded_mods(d.sig):
            continue
        for a in d.aspects:
            ts = direct_tokens(a)
            if len(ts) < 2 or ts[1].text != "observe":
                continue
            al = find(a, "argList")
            schema = next((t.text for t in tokens(al) if t.text[:1].isupper()), None) if al is not None else None
            sd = dc.table.get(schema) if schema else None
            if sd is None:
                continue
            # the schema's field types were written in the module that declares it: an imported observation's
            # names (an alias of Int) are in that module's table, not in the caller's
            table = dc.table if sd.module == dc.module_name else next((sub for (_, path), sub in dc.session.loaded.items() if path == sd.module), dc.table)
            allocating = [f"{f}: {strip(ft).show()}" for f, (ft, _) in sd.obs_fields.items() if not dc.is_trivial_in(strip(ft), table)]
            if allocating:
                out.append(diagnostics.make(
                    "E-EFF-030",
                    f"`{d.name}` is declared `noalloc`, but the observation it carries ({schema}) records an allocating field ({allocating[0]}): "
                    "the plan conflicts with the claim at this policy site; record a trivial field, or drop `noalloc`",
                    "LANG-11-03", file, ts[0].line, ts[0].col,
                    {"required_contract": "noalloc", "violating_effect": "alloc", "witness_path": f"@observe({schema}) -> {allocating[0]}"}))
    return out
