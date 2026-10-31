"""The static checker entry points: a module (names, declarations, visibility) and a package set.

`CODES` is the closed set of diagnostic codes this checker claims to judge at its current layer; the
runner holds a fixture to exactly those (a fixture rejected for another layer's code must produce
none of them). Layers append to it as they land (DEL-10 types, DEL-12 expressions, DEL-13 refinements)."""
from __future__ import annotations

import os
import sys
from typing import Dict, List, Optional

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
import elaboration  # noqa: E402
import lexer  # noqa: E402
import names  # noqa: E402
import resolve  # noqa: E402

from checker.decls import DeclChecker, Session  # noqa: E402
from checker.typer import Typer  # noqa: E402

CODES = {
    "E-NAM-001", "E-NAM-003", "E-NAM-004", "E-NAM-010",                       # names and visibility (DEL-06, DEL-07)
    "E-TYP-010", "E-TYP-020", "E-TYP-030",                                    # DEL-07 / DEL-10 declaration-level
    "E-TYP-060", "E-TYP-070", "E-TYP-080", "E-TYP-090", "E-TYP-100",          # DEL-10
    "E-OWN-030", "E-MIX-001",                                                 # DEL-10
    "E-NAM-002", "E-MAT-001", "E-MAT-002", "E-MAT-003", "E-TYP-050", "E-TYP-110", "E-TYP-120", "E-TYP-130",   # DEL-12
    "E-TYP-160",                                                                                  # DEL-40: uninferred type arguments (6.10, X-128)
    "E-TYP-170", "E-TYP-171",                                                                     # Language 1.0 limits (6.10, X-180)
    "E-TYP-172",                                                                                  # dynamic eligibility (9.5, LANG-09-07)
    "E-EFF-050",                                                                                  # contradictory contract (11.5)
    "E-TYP-180", "E-TYP-181",                                                                     # v0.1 R18: conversions, `as` (6.13, X-475)
    "E-TYP-182", "E-TYP-183", "E-TYP-184",                                                        # v0.1 R18: declared `convert` members (6.13, X-551)
    "E-REF-001", "E-REF-002", "E-REF-003",                                    # DEL-13: the interval domain
    "E-OWN-001", "E-OWN-002", "E-OWN-010", "E-OWN-011", "E-OWN-012", "E-OWN-020", "E-OWN-040",   # DEL-14: loans and moves
    "E-OWN-050", "E-OWN-051", "E-OWN-052", "E-OWN-053",                       # DEL-15: stored loans and origins
    "E-FUN-010", "E-FUN-011",                                                 # DEL-16: closures
    "E-OBL-001", "E-OBL-002", "E-OBL-006", "E-OBL-007", "E-ERR-001",          # DEL-17: outcomes and obligations
    "E-OBL-003",                                                              # v0.1 R11: a generic holding `T` by value instantiated at a linear type without `T: linear` (6.12, 10.7)
    "E-OBL-005", "E-TYP-140",                                                 # DEL-18: linear types and discard
    "E-EFF-001", "E-EFF-020",                                                 # DEL-19: effect summaries
    "E-EFF-010", "E-TYP-150", "E-OBL-008", "E-OBS-001", "E-OBS-010", "E-OBS-011", "E-OBS-012",   # DEL-20: contracts, authorities, aspects, observation (static)
    "E-DEF-001", "E-DEF-002",                                                 # DEL-22: defer
    "E-ASY-001", "E-ASY-002",                                                 # DEL-23: tasks and guards
    "E-ASY-003",                                                              # DEL-24: pool launches
    "E-OWN-090",                                                              # DEL-25: unsupported ownership shapes (C.2)
    "E-NAM-034",                                                              # a head whose kit is not installed (LR 16; D-90, X-231)
    "E-MAT-004",                                                              # a `let` pattern that can fail to match (LR 5.1; D-243)
    "E-MAT-005",                                                              # a range pattern that is not well formed (LR 9.8; X-517)
}
# Codes the checker judges only for the forms a layer has reached: a fixture expecting one is PASS when
# the checker produces it at the marked lines, INFO ("not judged at this layer") when it produces
# nothing, FAIL when it produces it elsewhere; an accept fixture must still produce none.
PARTIAL = {"E-TYP-040": "expression typing (DEL-12); unjudged only where a type comes from a package outside the corpus"}


INFERRED_HEADS = {}
LAMBDA_MODES = {}


def elaborate_captures(mtree, file: str):
    """Attach checked heads to a fresh runtime parse, before its declarations are loaded (LR 8.2)."""
    import copy
    import tree
    heads = INFERRED_HEADS.get(os.path.abspath(file), {})
    for node in tree.find_all(mtree, "lambda"):
        if tree.child(node, "captureList") is not None:
            continue
        tok = next(t for t in tree.direct_tokens(node) if t.text == "fn")
        if (tok.line, tok.col) in heads:
            node[1].append(("inferredLambdaHead", copy.deepcopy(heads[(tok.line, tok.col)])))
    for node in tree.find_all(mtree, "lambda"):
        tok = next(t for t in tree.direct_tokens(node) if t.text == "fn")
        modes = LAMBDA_MODES.get(os.path.abspath(file), {}).get((tok.line, tok.col), ())
        lp = tree.child(node, "lambdaParams")
        if lp is not None:
            for p, mode in zip(tree.find_all(lp, "lambdaParam"), modes):
                if mode and tree.child(p, "paramType") is None:
                    p[1].append(("inferredParamMode", mode))
    return mtree


def check_tree(lib: names.Library, mtree, file: str, module: str = "main", package: str = "", session: Optional[Session] = None, entry: Optional[str] = "main") -> List[dict]:
    """One module. `entry` is the module key of the program's entry module (4.6): a single file is its own ("main")."""
    # a module of a package (a non-empty `package`) is judged by the manifest's entry (resolve.py `check_entry`);
    # a module checked on its own is its package's entry module (LR 4.6, D-51.2)
    r = names.Resolver(lib, file, entry=not package)
    records = list(r.resolve_module(mtree))
    dc = DeclChecker(lib, file, module, package, session, entry)
    dc.load(mtree)
    dc.check()
    ty = Typer(dc)
    if RECORD_TYPES:
        ty.node_types = {}
    ty.run(mtree)
    INFERRED_HEADS[os.path.abspath(file)] = dict(ty.inferred_heads)
    LAMBDA_MODES[os.path.abspath(file)] = dict(ty.lambda_modes)
    global LAST_ELABORATIONS, LAST_TYPER, LAST_DC
    LAST_ELABORATIONS = list(ty.elaborations)
    LAST_TYPER, LAST_DC = ty, dc
    IMPLIED[file] = set(ty.implied_fails)
    CONVERSIONS[os.path.abspath(file)] = dict(ty.conversions)
    for key in [key for key in MIXIN_CONVERSIONS if key[1] == os.path.abspath(file)]:
        del MIXIN_CONVERSIONS[key]
    MIXIN_CONVERSIONS.update(ty.mixin_conversions)
    return records + dc.records


MIXIN_CONVERSIONS: Dict[tuple, dict] = {}  # lexical file, applying file and class -> per-application conversion facts
CONVERSIONS: Dict[str, dict] = {}    # absolute file -> resolved `as` rows (newtypes erase their runtime identity)
IMPLIED: Dict[str, set] = {}        # file -> the positions of its test bodies' calls with an implied `or fail` (14; D-319)
LAST_ELABORATIONS: List[str] = []   # aspect elaboration records of the last checked module (golden tests, LANG-11-09)
RECORD_TYPES = False                 # when set, the typer records each expression's type (experimental lowering, DEL-25)
LAST_TYPER = None
LAST_DC = None


def check_source(lib: names.Library, src: str, file: str, module: str = "main", package: str = "", entry: Optional[str] = "main") -> List[dict]:
    return check_tree(lib, lib.rec.parse(lexer.lex(src), "module"), file, module, package, entry=entry)


def entry_key(ps: resolve.PackageSet) -> Optional[str]:
    """The module key of a package set's entry module: its root package's `entry` (4.6), or None for a library."""
    return f"{ps.root.name}.{ps.root.entry}" if ps.root.entry else None


def package_libs(ps: resolve.PackageSet):
    """The libraries of a package set, each complete before any module is checked: every library holds the modules
    of exactly the packages visible to its package (check_package_set, check_package_set_plans)."""
    libs = {p.identity: names.Library(ps.rec) for p in ps.packages.values()}
    session = Session()                            # the package modules' tables, built once for this check
    for p in ps.packages.values():                 # every library is complete before any module is checked
        lib = libs[p.identity]
        visible = dict(ps.visible_packages(p))
        visible[p.name] = p
        for q in visible.values():
            for m in q.modules.values():
                if m.tree is None:
                    continue
                lm = names.LibraryModule(m.file, ps.rec, m.tree, q.identity)
                lm.library = libs[q.identity]
                lib.modules[f"{q.name}.{m.path}"] = lm
                if q.root == m.path:
                    lib.modules[q.name] = lm
    return libs, session


def seed_inferred_consts(ps: resolve.PackageSet, libs, session: Session, entry: Optional[str]) -> None:
    """Note every unannotated module constant before any module is typed, so an importer sees the initialiser's
    type whichever module is checked first (D-54.4). Each round can see what the previous round noted. The seed's
    records are discarded; the real pass reports them."""
    modules = [(p, m) for p in ps.packages.values() for m in p.modules.values() if m.tree is not None]
    for _ in range(max(len(modules), 1)):
        before = len(session.inferred_consts)
        for p, m in modules:
            key = f"{p.name}.{m.path}"
            dc = DeclChecker(libs[p.identity], os.path.relpath(m.file, ROOT), key, p.identity, session, entry)
            dc.load(m.tree)
            Typer(dc).seed_consts(m.tree)
        if len(session.inferred_consts) == before:
            break


def check_package_set(ps: resolve.PackageSet) -> List[dict]:
    """Every module of every package in the set, each seeing exactly the packages visible to it."""
    out: List[dict] = []
    entry = entry_key(ps)
    libs, session = package_libs(ps)
    seed_inferred_consts(ps, libs, session, entry)
    for p in ps.packages.values():
        for m in p.modules.values():
            if m.tree is None:
                continue
            key = f"{p.name}.{m.path}"
            out += check_tree(libs[p.identity], m.tree, os.path.relpath(m.file, ROOT), key, p.identity, session, entry)
    # modules that use the Dependency injection kit were read elaborated (project.py); its findings join the
    # core's as fortis-di reports them (D-90)
    return elaboration.report(out, [m for p in ps.packages.values() for m in p.modules.values()])


def check_package_set_plans(ps: resolve.PackageSet) -> List[dict]:
    """The plan step (PROG-VM-04) of every module of the root package, each seeing exactly the packages visible to
    it as check_package_set gives them: an imported type (a sibling's alias of `Int`) resolves as the checker
    resolves it, so a trivial field is not wrongly held allocating (a standalone plan context cannot see the
    sibling's modules). The root package's modules are the step's scope, as in production (fortis-types
    plans::check_path walks the directory's own src/); a dependency's modules were plan-checked as their own
    package."""
    from . import plans
    out: List[dict] = []
    entry = entry_key(ps)
    libs, session = package_libs(ps)
    p = ps.root
    for m in p.modules.values():
        if m.tree is None:
            continue
        key = f"{p.name}.{m.path}"
        out += plans.check_module(libs[p.identity], m.tree, os.path.relpath(m.file, ROOT), key, p.identity, session, entry)
    return out
