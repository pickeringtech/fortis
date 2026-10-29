"""Kernel evaluator over parse trees (DEL-08)."""
from __future__ import annotations

import json
import os
import sys
from typing import Any, Dict, List, NamedTuple, Optional, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
import lexer  # noqa: E402
import parser as fparser  # noqa: E402
import tree  # noqa: E402
import reading  # noqa: E402
from lexer import Token  # noqa: E402
from tree import children, name, direct_tokens, find, find_all, tokens  # noqa: E402

from interpreter import codec
from interpreter import intrinsics
from interpreter import numeric
from interpreter import records
from interpreter import telemetry
from interpreter.executor import OBSERVATION_BOUND, CancelSignal, DefectSignal, FutureVal, JoinVal, ScopeVal, Scheduler, SleepVal, TaskVal
from interpreter.values import SharedBox, SharedVal, WeakVal
from interpreter.values import render_message  # noqa: E402
from interpreter.values import LentRecord, LentTuple, lent_parts, retuple  # noqa: E402
from interpreter.values import (VOID, BuilderVal, Capability, ClassInfo, Closure, EnumInfo, EnumValue, ErrorInfo, Failure,  # noqa: E402
                                FailureSignal, FortisError, Instance, ListVal, Report, Void, check_int, deep_copy, display)


# ---------------------------------------------------------------- control flow
class ReturnSignal(Exception):
    def __init__(self, value, lent=True): self.value, self.lent = value, lent   # lent: the value returned is a loan (D-373)

class GiveSignal(Exception):
    def __init__(self, value, lent=False): self.value, self.lent = value, lent   # lent: the value given is a loan (D-338)

class BreakSignal(Exception):
    def __init__(self, label): self.label = label

class ContinueSignal(Exception):
    def __init__(self, label): self.label = label

class RecoverSignal(Exception):
    def __init__(self, value, lent=False): self.value, self.lent = value, lent

class RegionFailure(Exception):
    """A checked operator failed inside a checked region (6.4, X-345): the region's value is this failure."""
    def __init__(self, failure): self.failure = failure

class ExpectFailure(Exception):
    """A false `expect` inside a test (14): carries the typed actual/expected evidence."""
    def __init__(self, evidence): self.evidence = evidence


class Cell:
    __slots__ = ("value", "mutable", "moved", "returned", "owns")
    def __init__(self, value, mutable, owns=True):
        self.value, self.mutable, self.moved = value, mutable, False
        self.returned = False       # `return x` of a local: its value left with the return, so the block keeps it (X-345)
        # whether the binding owns its value. A binding that holds a loan (`alias p`, `editable p`, a `get`, a `from`
        # result, a read parameter, a borrowed pattern or loop binding) is a path to the owner: its end destroys
        # nothing, so `cleanup` runs once, when the owner goes (7.2, 12.2; D-338)
        self.owns = owns


class Env:
    def __init__(self, parent: Optional["Env"] = None):
        self.parent = parent
        self.vars: Dict[str, Cell] = {}
        self.defers: List[Any] = []
        self.frame = False              # a call's own environment (its parameters): the locals of the call are below it
        # the module whose code runs in this environment: its scope resolves module-level names (X-136)
        self.module: Optional[str] = parent.module if parent is not None else None

    def declare(self, n: str, value, mutable=False, owns=True):
        self.vars[n] = Cell(value, mutable, owns)

    def cell(self, n: str) -> Cell:
        e: Optional[Env] = self
        while e is not None:
            if n in e.vars:
                return e.vars[n]
            e = e.parent
        raise FortisError(f"`{n}` is not declared")

    def get(self, n: str):
        c = self.cell(n)
        if c.moved:
            raise FortisError(f"`{n}` was moved (use after move)")        # plug-in: loans/moves (DEL-14) decide this statically
        return c.value


SPREAD = "..."                          # the name an argument list gives a constructor spread (`...copy p`, 9.2)


def param_owned(p) -> bool:
    """A parameter written `name: owned T`: its own mode, not one inside its type (`f: fn(owned T) U` is a loan)."""
    pt = tree.child(p, "paramType")
    return pt is not None and any(t.text == "owned" for t in direct_tokens(pt))


LIBRARY_MODULES = ("prelude", "text", "collections", "numeric", "time", "money", "host")
DATA_MODULES = ("time", "money")       # library modules whose classes with fields, enums and errors the interpreter registers
_LIBRARY: Optional[Dict[str, dict]] = None

# the values `destroy` reaches (7.9): a statement temporary of one of these types is destroyed at the end of
# the statement unless its ownership moved; ArrayVal is a ListVal, a record is a dict, a tuple is a tuple
TEMP_TYPES = (Instance, EnumValue, SharedVal, ListVal, intrinsics.MapVal, intrinsics.SetVal,
              intrinsics.GuardVal, intrinsics.FileVal, intrinsics.MutexVal, tuple, dict)

# A tuple is immutable, so moving one element out of a temporary replaces that element with this sentinel.
# `None` is a real Fortis value and must stay distinct. `destroy` ignores the sentinel (7.9).
_MOVED = object()


def trivial(v) -> bool:
    """A trivial value (6.11): `return` of the local that holds it copies it and leaves the binding readable (7.5);
    everything else is ownership-bearing, and the binding is moved. The numbers, `Bool`, `Void`, `null`, `Scalar`,
    `Decimal`, `Duration` and `Timestamp`; a value of a class declared `trivial` (the opt-in of 6.11, as the
    checkers' `is_trivial` reads it; D-411); an enum, a tuple or a record whose parts are all trivial."""
    if isinstance(v, (bool, int, float, numeric.F32, numeric.Dec, numeric.Dur, intrinsics.TimestampVal, Void)) or v is None:
        return True
    if type(v).__name__ == "Scalar":
        return True
    if isinstance(v, Instance):
        return v.cls.trivial
    if isinstance(v, EnumValue):
        return all(trivial(p) for p in v.payload.values())
    if type(v) is tuple:
        return all(trivial(x) for x in v)
    if type(v) is dict:
        return all(trivial(x) for x in v.values())
    return False


def library() -> Dict[str, dict]:
    """The Fortis bodies of the core library (DEL-26), parsed once per process:
    {module: {"methods": {(Class, name): (head, body)}, "funcs": {name: (head, body)}, "imports": {name: module}}}.
    Only declarations with a body appear; intrinsics are in interpreter/intrinsics.py."""
    global _LIBRARY
    if _LIBRARY is not None:
        return _LIBRARY
    rec = fparser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    out: Dict[str, dict] = {}
    for mod in LIBRARY_MODULES:
        path = os.path.join(ROOT, "library", "fortis", mod + ".ft")
        mtree = rec.parse(lexer.lex(open(path, encoding="utf-8").read()), "module")
        entry = {"methods": {}, "funcs": {}, "imports": {}, "tree": mtree}
        for imp in tree.imports(mtree):
            for m in (imp["members"] or []):
                entry["imports"][m] = ".".join(imp["path"][1:]) if imp["path"][0] == "fortis" else None
        for d in tree.declarations(mtree):
            if d["kind"] == "func":
                head, body = find(d["node"], "funcHead"), find(d["node"], "funcBody")
                if body is not None:
                    entry["funcs"][d["name"]] = (head, body)
            elif d["kind"] in ("class", "enum"):
                member_rule = "classMember" if d["kind"] == "class" else "enumMember"
                for m in children(d["node"]):
                    if name(m) != member_rule:
                        continue
                    head, body = find(m, "methodHead"), find(m, "funcBody")
                    if head is not None and body is not None:
                        entry["methods"][(d["name"], tokens(find(head, "memberName"))[0].text)] = (head, body)
        out[mod] = entry
    _LIBRARY = out
    return out


def own_expr(node):
    """The constant expression a declaration or parameter carries after its `=` (a default, a constant's value): its
    own child, never one nested in its type, which a search of the subtree would find first in `Int<1..100> = 20`."""
    return next((c for c in children(node) if name(c) == "constExpr"), None)


def own_refinement(type_ref_node):
    """A typeRef's own refinement (`List[Int<0..9>]<length: 1..3>` has `<length: 1..3>`), never one of its type
    arguments', which a search of the subtree would find first."""
    return next((c for c in children(type_ref_node) if name(c) == "refinement"), None) if type_ref_node is not None else None


def type_ref(type_node):
    """The typeRef of a `type` node naming one (`Name[…]<…>`, optionally `?`), or None (a tuple, record, function or
    loan type)."""
    nt = next((c for c in children(type_node) if name(c) == "namedType"), None) if type_node is not None else None
    return next((c for c in children(nt) if name(c) == "typeRef"), None) if nt is not None else None


class Refinement(NamedTuple):
    lo: Optional[int]
    hi: Optional[int]
    measure: Optional[str]        # None: the value's own range; "scalars" or "length": a count of the value (6.3)


def unescape_bytes(body: str) -> bytes:
    """A byte string's escapes, as the production frontend reads them (fortis-mir `unescape`): `\\n`, `\\t`, `\\r`,
    `\\0`, and any other escaped character stands for itself; the lexer has already refused non-ASCII text."""
    out, i = [], 0
    while i < len(body):
        c = body[i]
        if c == "\\" and i + 1 < len(body):
            out.append({"n": "\n", "t": "\t", "r": "\r", "0": "\0"}.get(body[i + 1], body[i + 1]))
            i += 2
            continue
        out.append(c)
        i += 1
    return "".join(out).encode("ascii")


class TypeRefVal:
    """A type name used as a value prefix (`Host`, `Int<0..9>`): members are associated functions or variants.

    `core` is set when the name was reached through an alias of a `fortis` module (D-142). A later call must
    not resolve the bare name, which a program class of the same name would win."""
    def __init__(self, name: str, bounds=None, core: bool = False):
        self.name, self.bounds, self.core = name, bounds, core


class FuncVal:
    """A module-level function used as a value (called, or passed as a callback); `module` is set for a core
    library function, whose body resolves names in its own module (DEL-26)."""
    def __init__(self, head, body, fname: str = "", module: Optional[str] = None):
        self.head, self.body, self.fname, self.module = head, body, fname, module


class ModuleRef:
    """An imported module named through its alias (`right` of `import same.right.items as right`): its members are
    that module's functions and constants."""
    def __init__(self, module: str):
        self.module = module


class BoundMember:
    """`receiver.member` not yet called: a method, associated function or capability operation."""
    def __init__(self, receiver, member: str):
        self.receiver, self.member = receiver, member


class Scalar:
    def __init__(self, text: str): self.text = text
    def __eq__(self, o): return isinstance(o, Scalar) and o.text == self.text
    def __hash__(self): return hash(self.text)
    def display(self): return self.text


# ---------------------------------------------------------------- interpreter
class Interpreter:
    def __init__(self, src: str, file: str = "<module>", linked=None):
        """`linked`: the (Link, entry module) of a package program (interpreter/link.py); the entry module's tree is
        the linked one, whose colliding names carry their program keys."""
        self.file = file
        self.src = src
        self.implied: Optional[set] = None         # the checker's implied `or fail` calls in test bodies (14; D-319)
        self.regions: List[set] = []               # the operators of the checked regions being evaluated (6.4, X-345)
        self.region_cache: Dict[int, set] = {}     # id(left operand) -> its region's operators
        self.rec = fparser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
        self.link, entry = linked if linked is not None else (None, None)
        self.module = entry.tree if entry is not None else self.rec.parse(lexer.lex(src), "module")
        self.module_key: Optional[str] = entry.key if entry is not None else None
        self.loading_module: Optional[str] = self.module_key
        self.head_module: Dict[int, str] = {}       # id(function or method head) -> the module that declares it
        self.const_module: Dict[str, Optional[str]] = {}
        self.out: List[str] = []
        self.stream = None                         # where a command-line run writes console lines as they are printed
        self.reports: List[dict] = []
        self.classes: Dict[str, ClassInfo] = {}
        self.mixin_methods: Dict[str, dict] = {}
        self.enums: Dict[str, EnumInfo] = {}
        self.core_enums: Dict[str, EnumInfo] = {}   # library enums, kept when a program enum reuses the name (D-142)
        self.errors: Dict[str, ErrorInfo] = {}
        self.funcs: Dict[str, Tuple[Any, Any]] = {}
        self.consts: Dict[str, Any] = {}
        self.assoc_consts: Dict[Tuple[int, str], Any] = {}             # a class's or enum's constants, by owner (9.3)
        self._open_names: Optional[set] = None
        self.const_nodes: Dict[str, Any] = {}
        self.imported: Dict[str, str] = {}         # name -> import path (kit names fail at use)
        self.interp_cache: Dict[str, Any] = {}
        self.aspects: Dict[str, dict] = {}         # aspect declarations (11.8)
        self.aspect_homes: Dict[str, set] = {}     # aspect name -> the modules (program keys) that declare it
        self.func_aspects: Dict[str, list] = {}    # function name -> aspectUse nodes in application order
        self.observations: Dict[str, List[str]] = {}
        self.schema_versions: Dict[str, int] = {}
        self.newtypes: Dict[str, Any] = {}          # newtype name -> refinement node of its representation (6.9)
        self.newtype_repr: Dict[str, Any] = {}      # newtype name -> representation type node (codec, 6.10)
        self.decl_file: Dict[str, str] = {}         # function or method name -> declaring module file (other modules)
        self.tests: List[dict] = []
        self.sched = Scheduler(self)               # the cooperative executor (12.5)
        self.lib = library()
        self.lib_ctx: List[str] = []               # the core library module whose body is running (name resolution)
        for mod in DATA_MODULES:
            self.load_declarations(self.lib[mod]["tree"], library=True)
        self.load_declarations(self.lib["prelude"]["tree"], library=True, kinds=("enum",))   # Ordering.Less, `Less` (9.7)
        self.trace: List[str] = []                 # scheduler and unwind events (`// execute-trace:`)
        self.json_out = False
        self.in_test_body = False                    # running a test or property body: `or fail` is implied (14, X-345)
        self.returned_lent = True                    # whether the last call that ran returned a loan or its own value (D-373)
        self.test_label: Optional[str] = None        # the running test's function name, as the VM names it (`test "title"`)
        self.load_declarations()

    # ---- declarations ---------------------------------------------------
    def load_module(self, mtree, file: str, key: Optional[str] = None):
        """Another module of the program (its package or a dependency): its declarations join the kernel's table,
        after the checker has resolved every import; its tests stay with that module. Linked (`key`), a name two
        modules declare is entered under its program key (X-136); unlinked, such a name is outside the kernel."""
        if key is None:
            taken = set(self.funcs) | set(self.classes) | set(self.enums) | set(self.errors) | set(self.consts) | set(self.newtypes) | set(self.const_nodes)
            for d in tree.declarations(mtree):
                if d["kind"] not in ("test", "property", "implement", "mixin", "contract", "interface") and d["name"] in taken:
                    raise FortisError(f"`{d['name']}` is declared by two modules ({file} and another); the kernel runs one namespace")
        tests = list(self.tests)
        self.loading_module = key
        self.load_declarations(mtree)
        self.loading_module = self.module_key
        self.tests = tests
        for d in tree.declarations(mtree):                   # a failure's origin names the module that declares it (10.3)
            self.decl_file.setdefault(d["name"], file)
            if d["kind"] in ("class", "enum"):
                for m in children(d["node"]):
                    head = find(m, "methodHead") if name(m) in ("classMember", "enumMember") else None
                    if head is not None:
                        self.decl_file.setdefault(tokens(find(head, "memberName"))[0].text, file)

    def load_declarations(self, mtree=None, library: bool = False, kinds: Optional[Tuple[str, ...]] = None):
        """The module's declarations; with `library`, a core library module's data classes, enums and errors
        (its functions are resolved through `library_name`, its intrinsics through the registry)."""
        mtree = self.module if mtree is None else mtree
        if not library:
            for imp in tree.imports(mtree):
                path = ".".join(imp["path"])
                for m in (imp["members"] or [imp["alias"] or imp["path"][-1]]):
                    self.imported[m] = path
        for d in tree.declarations(mtree):
            node, kind, nm = d["node"], d["kind"], d["name"]
            if library and kind not in ("class", "enum", "error") or kinds is not None and kind not in kinds:
                continue
            if library and kind == "class" and find(node, "fieldDecl") is None:
                continue                                # a class with no fields is an intrinsic type (Duration, Timestamp)
            if kind == "class":
                fields, exported, methods, field_types, consts = {}, {}, {}, {}, {}
                for m in children(node):
                    if name(m) != "classMember":
                        continue
                    if self.load_const_member(m, consts):
                        continue
                    exp = any(isinstance(c, Token) and c.text == "export" for c in children(m))
                    fd = find(m, "fieldDecl")
                    if fd is not None:
                        ts = direct_tokens(fd)
                        fields[ts[0].text] = next((c for c in children(fd) if name(c) == "constExpr"), None)
                        exported[ts[0].text] = exp
                        field_types[ts[0].text] = next((c for c in children(fd) if name(c) == "type"), None)
                        continue
                    head = find(m, "methodHead")
                    if head is not None:
                        methods[tokens(find(head, "memberName"))[0].text] = (head, find(m, "funcBody"))
                        self.note_module(head)
                self.classes[nm] = ClassInfo(nm, fields, exported, methods, node)
                self.classes[nm].field_types = field_types
                self.classes[nm].consts = consts
                # the `trivial` modifier, read as the checkers read it (decls.py): a trivial class is copied by `return`
                self.classes[nm].trivial = any(isinstance(c, Token) and c.text == "trivial" for c in children(node))
                tp = next((c for c in children(node) if name(c) == "typeParams"), None)
                self.classes[nm].type_params = [t.text for t in tokens(tp) if t.kind == "UPPER"] if tp is not None else []
                impl = []
                kids_ = children(node)
                for i, c in enumerate(kids_):
                    if isinstance(c, Token) and c.text == "implements" and i + 1 < len(kids_) and not isinstance(kids_[i + 1], Token):
                        impl += [t.text for t in tokens(kids_[i + 1]) if t.kind == "UPPER"]
                self.classes[nm].implements = impl
                # the mixins it takes methods from (`with M, N`, 9.6), merged once every declaration is loaded
                self.classes[nm].mixins = [t.text for i, c in enumerate(kids_) if isinstance(c, Token) and c.text == "with" and i + 1 < len(kids_)
                                           for t in (tokens(kids_[i + 1]) if not isinstance(kids_[i + 1], Token) else [kids_[i + 1]]) if t.kind == "UPPER"]
                cl = find(node, "cleanupDecl")
                if cl is not None:
                    self.classes[nm].cleanup = find(cl, "block")
            elif kind == "enum":
                variants, methods, ptypes, consts = {}, {}, {}, {}
                for m in children(node):
                    if name(m) != "enumMember":
                        continue
                    if self.load_const_member(find(m, "classMember"), consts):
                        continue
                    v = find(m, "variant")
                    if v is not None:
                        vt = direct_tokens(v)
                        fl = find(v, "fieldList")
                        variants[vt[0].text] = [t.text for t in direct_tokens(fl) if t.kind == "LOWER"] if fl else []
                        if fl is not None:
                            fks = children(fl)
                            ptypes[vt[0].text] = [(fks[i].text, fks[i + 2]) for i in range(len(fks)) if isinstance(fks[i], Token) and fks[i].kind == "LOWER" and i + 2 < len(fks) and name(fks[i + 2]) == "type"]
                        else:
                            ptypes[vt[0].text] = []
                        continue
                    head = find(m, "methodHead")
                    if head is not None:
                        methods[tokens(find(head, "memberName"))[0].text] = (head, find(m, "funcBody"))
                        self.note_module(head)
                self.enums[nm] = EnumInfo(nm, variants, methods)
                self.enums[nm].payload_types = ptypes
                self.enums[nm].consts = consts
                tp = next((c for c in children(node) if name(c) == "typeParams"), None)
                self.enums[nm].type_params = [t.text for t in tokens(tp) if t.kind == "UPPER"] if tp is not None else []
                if library:
                    self.core_enums[nm] = self.enums[nm]
            elif kind == "error":
                ts = direct_tokens(node)
                code = next(t.text for t in ts if t.kind == "STR")
                fl = find(node, "fieldList")
                payload = [t.text for t in direct_tokens(fl) if t.kind == "LOWER"] if fl else []
                msg = next((t.text for t in ts if t.kind == "STR" and t is not ts[[x.kind for x in ts].index("STR")]), None)
                # the message template is kept as written between its quotes, like the code (10.3; D-340)
                self.errors[nm] = ErrorInfo(nm, code[1:-1], payload, msg[1:-1] if msg is not None and len(msg) >= 2 else msg)
            elif kind == "func":
                key = self.decl_key(nm)
                self.funcs[key] = (find(node, "funcHead"), find(node, "funcBody"))
                self.note_module(self.funcs[key][0])
                if d.get("aspects"):
                    self.func_aspects[key] = list(d["aspects"])
            elif kind == "const":
                key = self.decl_key(nm)
                self.const_nodes[key] = own_expr(node)
                self.const_module[key] = self.loading_module
            elif kind == "type":
                self.newtypes[nm] = own_refinement(type_ref(next((c for c in children(node) if name(c) == "type"), None)))
                self.newtype_repr[nm] = next((c for c in children(node) if name(c) == "type"), None)
            elif kind == "aspect":
                self.aspects[nm] = self.load_aspect(node)
                self.aspect_homes.setdefault(nm, set()).add(self.loading_module)
            elif kind == "observation":
                bounds = {}
                for it in [c for c in children(node) if name(c) == "observationItem"]:
                    fname = direct_tokens(it)[0].text
                    if fname == "version":
                        self.schema_versions[nm] = int(direct_tokens(it)[2].text)
                    if fname in ("version", "encoding"):
                        continue
                    ref = find(it, "refinement")
                    if ref is not None and "scalars" in [t.text for t in tokens(ref)]:
                        bounds[fname] = self.range_bounds(ref, Env(None))[1]
                self.observations[nm] = bounds
            elif kind in ("test", "property"):
                self.tests.append(self.load_test(node, kind))
            elif kind == "mixin":
                # the methods a mixin contributes: those with a body (its `requires` items have none)
                ms = {}
                for m in children(node):
                    head = find(m, "methodHead") if not isinstance(m, Token) else None
                    body = find(m, "funcBody") if head is not None else None
                    if head is not None and body is not None:
                        ms[tokens(find(head, "memberName"))[0].text] = (head, body)
                        self.note_module(head)
                self.mixin_methods[nm] = ms
            # type, interface, implement, contract, aspect, observation: no runtime behaviour in the kernel
        for ci in self.classes.values():
            for mx in getattr(ci, "mixins", []):
                for mname, hb in self.mixin_methods.get(mx, {}).items():
                    ci.methods.setdefault(mname, hb)
        self.globals = Env()
        self._open_names = None                  # recomputed when first needed, over every declaration loaded

    def load_const_member(self, member, consts: Dict[str, Any]) -> bool:
        """An associated constant of a class or enum, `const size = 8`, read as `Name.size` (9.3)."""
        cd = next((c for c in children(member) if name(c) == "constDecl"), None) if member is not None else None
        if cd is None:
            return False
        consts[direct_tokens(cd)[1].text] = (own_expr(cd), self.loading_module)
        return True

    def associated_const(self, owner, member: str):
        key = (id(owner), member)
        if key not in self.assoc_consts:
            node, module = owner.consts[member]
            env = Env(self.globals)
            env.module = module or env.module
            v = self.eval(node, env)
            self.fresh_remove(v)                          # the cached constant outlives the statement that first reads it (9.3)
            self.assoc_consts[key] = v
        return self.assoc_consts[key]

    # per-task state (frames, function stack, cleanup depth, statement temporaries) lives on the running task
    @property
    def frames(self): return self.sched.current.frames
    @property
    def stack(self): return self.sched.current.stack
    @property
    def writers(self): return self.sched.current.writers
    @property
    def cleanup_depth(self): return self.sched.current.cleanup_depth
    @cleanup_depth.setter
    def cleanup_depth(self, v): self.sched.current.cleanup_depth = v
    @property
    def temps(self): return self.sched.current.temps

    # ---- loans (7.3, 12.2; D-338) -------------------------------------------------------
    # A loan and its owner are the same Python value, so the value cannot say which one a binding holds. The
    # running statement records the values it produced as loans: a marker `alias p` or `editable p`, a read
    # through a binding that holds a loan, a lending intrinsic (`get`, a cursor's `next`) and the result of a call
    # whose declared result is a loan (`from`). A binding initialised with one holds a loan.
    def lend(self, v):
        if isinstance(v, TEMP_TYPES):
            self.sched.current.lent[id(v)] = v
        return v

    def is_lent(self, v) -> bool:
        return id(v) in self.sched.current.lent

    def unlend(self, v):
        self.sched.current.lent.pop(id(v), None)

    def lent_scope(self):
        """A statement's own record of loans; the caller restores the enclosing statement's when it ends."""
        t = self.sched.current
        saved, t.lent = t.lent, {}
        return t, saved

    def call_result(self, r, ret):
        """A call's result. Declared `from` a parameter, it is a loan the caller's binding holds; a guard so declared
        is a loan-carrying value its binding owns, and dropping it unlocks (7.7, 12.7). Any other result is a new
        owned value, a temporary of the statement until something takes it (7.9; D-338). A `from` result that the
        callee's `return` gave as a value of its own (a `??` whose fallback it moved or made, a moved local) is owned
        too, by whatever receives it, so its cleanup runs even where a check was missed (D-373)."""
        if self.returns_loan(ret) and not isinstance(r, intrinsics.GuardVal) and self.returned_lent:
            return self.lend(r)
        return self.fresh_add(r)

    @staticmethod
    def returns_loan(ret) -> bool:
        """A declared result type that is a loan or carries one (`StringView from user`, `(alias T from self)?`,
        `ListCursor[T] from self`): the caller's binding holds a loan, never the value (7.6, 7.7)."""
        return ret is not None and any(t.text == "from" for t in tokens(ret))

    # ---- linked names (X-136) ---------------------------------------------------------
    def decl_key(self, nm: str) -> str:
        """The table key of a declaration of the module being loaded: its program key when two modules declare it."""
        if self.link is None or self.loading_module is None:
            return nm
        return self.link.modules[self.loading_module].scope.get(nm, nm)

    def note_module(self, head):
        if self.loading_module is not None and head is not None:
            self.head_module[id(head)] = self.loading_module

    def resolve(self, n: str, env) -> str:
        """The table key of a module-level name as the running module writes it."""
        if self.link is None:
            return n
        m = (env.module if env is not None else None) or self.module_key
        return self.link.modules[m].scope.get(n, n) if m in self.link.modules else n

    def module_alias(self, n: str, env) -> Optional[ModuleRef]:
        if self.link is None:
            return None
        m = (env.module if env is not None else None) or self.module_key
        target = self.link.modules[m].aliases.get(n) if m in self.link.modules else None
        return ModuleRef(target) if target is not None else None

    def module_member(self, ref: ModuleRef, n: str):
        """A function or constant of an aliased module."""
        key = self.link.key(ref.module, n)
        if key in self.const_nodes:
            return self.const(key)
        if key in self.funcs:
            return FuncVal(*self.funcs[key], key)
        raise FortisError(f"module {ref.module} has no member `{n}`")

    def load_aspect(self, node) -> dict:
        a = {"params": [], "needs": [], "before": [], "after": None, "on_failure": None, "base": None, "rest": False}
        ap = find(node, "aspectParams")
        if ap is not None:
            for p in [c for c in children(ap) if name(c) == "param"]:
                if find(p, "restParam") is not None:
                    a["rest"] = True
                    continue
                ts = direct_tokens(p)
                a["params"].append((next(t.text for t in ts if t.kind == "LOWER"), own_expr(p)))
        for part in [c for c in children(node) if name(c) == "aspectPart"]:
            ws = [t.text for t in direct_tokens(part)]
            blk = find(part, "block")
            if ws[0] == "needs" and len(ws) > 2:
                pt = find(part, "paramType")
                a["needs"].append((ws[1], next(t.text for t in tokens(pt) if t.kind == "UPPER")))
            elif ws[0] == "before":
                a["before"].append(blk)
            elif ws[0] == "after":
                a["after"] = (ws[2], blk)
            elif ws[0] == "on":
                a["on_failure"] = (ws[3], blk)
        kids = children(node)
        eq = next((i for i, c in enumerate(kids) if isinstance(c, Token) and c.text == "="), None)
        if eq is not None:
            base = next(t for t in kids[eq + 1:] if isinstance(t, Token) and t.kind == "LOWER")
            a["base"] = (base.text, find(node, "argList"))
        return a

    def load_test(self, node, kind) -> dict:
        ts = direct_tokens(node)
        title = next(t.text for t in ts if t.kind == "STR")[1:-1]
        t = {"kind": kind, "title": title, "block": [c for c in children(node) if name(c) == "block"][-1], "rows": [], "row_var": None, "params": None}
        if kind == "test":
            words = [x.text for x in ts]
            if "as" in words:
                t["row_var"] = words[words.index("as") + 1]
            for cr in [c for c in children(node) if name(c) == "caseRow"]:
                t["rows"].append((tokens(cr)[0].text[1:-1], find(cr, "recordLit")))
        else:
            t["params"] = find(node, "paramList")
        return t

    def const(self, nm):
        if nm not in self.consts:
            env = Env(self.globals)
            env.module = self.const_module.get(nm) or env.module
            v = self.eval(self.const_nodes[nm], env)
            self.fresh_remove(v)                          # the cached constant outlives the statement that first reads it (9.3)
            self.consts[nm] = v
        return self.consts[nm]

    # ---- entry -------------------------------------------------------------
    def run_main(self, args: List[str]) -> int:
        if "main" not in self.funcs:
            raise FortisError("no main")
        telemetry.mode()                             # the adapter opens its channel before any frame completes (SP-09 design 3.1.5)
        head, body = self.funcs["main"]
        try:
            result = self.call(head, body, self.entry_arguments(head, args), None, self.func_aspects.get("main"))
            if isinstance(result, FutureVal):
                result = self.run_future(result)          # an async main runs as the main task (4.6)
        except FailureSignal as f:
            result = f.failure
        except DefectSignal as d:
            self.defect_record(d.kind, "main")
            self.flush_records()
            raise
        except RecursionError:
            self.defect_record("stack_exhaustion", "main")
            self.flush_records()
            raise DefectSignal("stack_exhaustion")
        self.sched.drain_detached()
        if isinstance(result, Failure):
            self.emit_report(result, "main")
            self.flush_records()
            return 1
        self.flush_records()
        return 0

    def entry_arguments(self, head, args: List[str]):
        """The entry `main`'s arguments, each supplied by its parameter's type (4.6, X-345): the command-line arguments
        for `List[String]`, and for a capability type that capability, all created once, before `main` runs (11.6)."""
        kinds = reading.entry_param_kinds(find(head, "funcSignature"))
        caps = intrinsics.host_capabilities(self, []) if any(k != reading.ARGS for _, k in kinds) else None
        return [(None, ListVal(list(args)) if k == reading.ARGS else caps.fields[reading.CAPABILITY_FIELDS[k]]) for _, k in kinds]

    def defect_record(self, kind: str, task: str):
        self.emit({"kind": "defect", "class": kind, "task": task, "rule": "LANG-10-06"})

    # ---- tests (14) ----------------------------------------------------------------
    def run_tests(self) -> Tuple[int, int]:
        """Run every test and property declaration; one test-result record per case. Returns (passed, failed)."""
        import random
        passed = failed = 0
        for t in self.tests:
            cases: List[Tuple[str, Env]] = []
            if t["kind"] == "test":
                if t["row_var"] is None:
                    cases.append(("", Env(self.globals)))
                for rname, rl in t["rows"]:
                    e = Env(self.globals)
                    e.declare(t["row_var"], self.eval(rl, self.globals))
                    cases.append((rname, e))
            else:
                rng = random.Random(sum(ord(c) for c in t["title"]) * 7919)
                gens = self.generators(t["params"])
                for i in range(25):
                    e = Env(self.globals)
                    for pn, g in gens:
                        e.declare(pn, g(rng))
                    cases.append((f"case {i + 1}", e))
            for cname, env in cases:
                status, evidence = "pass", {}
                try:
                    self.frames.append(None)
                    self.in_test_body = True             # `or fail` is implied in the body (14, X-345)
                    self.test_label = f'test "{t["title"]}"'
                    try:
                        self.run_block(t["block"], env)
                    finally:
                        self.frames.pop()
                        self.in_test_body = False
                        self.test_label = None
                except ExpectFailure as x:
                    status, evidence = "fail", x.evidence
                except FailureSignal as f:
                    status, evidence = "fail", {"actual": f"failure {f.failure.code}", "expected": "success", "line": 0}
                except ReturnSignal:
                    pass
                except FortisError as x:
                    status, evidence = "error", {"actual": str(x), "expected": "a well-formed run", "line": 0}
                self.emit({"kind": "test-result", "test": t["title"], "case": cname, "status": status, "evidence": evidence, "rule": "LANG-14-02"})
                if status == "pass":
                    passed += 1
                else:
                    failed += 1
        self.flush_records()
        return passed, failed

    def generators(self, pl):
        """Generators for a property's parameters from their types (8.2): Int, Int<lo..hi>, Bool, String, List[...]."""
        out = []
        for p in ([c for c in children(pl) if name(c) == "param"] if pl is not None else []):
            ts = direct_tokens(p)
            pn = next(t.text for t in ts if t.kind == "LOWER")
            out.append((pn, self.generator_for(find(find(p, "paramType"), "type"))))
        return out

    def generator_for(self, tnode):
        text = "".join(t.text for t in tokens(tnode))
        tr = find(tnode, "typeRef")
        base = next(t.text for t in tokens(tr) if t.kind == "UPPER") if tr is not None else text
        if base == "Bool":
            return lambda rng: rng.random() < 0.5
        if base == "String":
            return lambda rng: "".join(rng.choice("abcxyz ") for _ in range(rng.randint(0, 8)))
        if base == "List":
            ta = find(tr, "typeArgs")
            inner = self.generator_for(find(ta, "type"))
            return lambda rng: ListVal([inner(rng) for _ in range(rng.randint(0, 8))])
        refinement = find(tr, "refinement") if tr is not None else None
        lo, hi = (-1000, 1000)
        if refinement is not None:
            b = self.range_bounds(refinement, self.globals)
            lo = b[0] if b[0] is not None else lo
            hi = b[1] if b[1] is not None else hi
        return lambda rng: rng.randint(lo, hi)

    def runtime_inventory(self) -> dict:
        """What runtime work the run started (Programme 14 A/A2: none unless requested): tasks per executor, detached
        tasks, virtual time slept, runtime records by kind. The kernel has no DI registry and no telemetry exporter;
        observation frames are written to the record stream only when a function is annotated `@observe`."""
        tasks = self.sched.all
        kinds: Dict[str, int] = {}
        for r in self.reports:
            kinds[r.get("kind", "?")] = kinds.get(r.get("kind", "?"), 0) + 1
        return {"kind": "runtime-inventory", "tasks": len(tasks),
                "tasks_by_executor": {e: sum(1 for t in tasks if getattr(t, "executor", "cooperative") == e) for e in ("cooperative", "parallel", "blocking")},
                "detached": len(self.sched.detached), "virtual_time_ns": self.sched.now, "records": kinds,
                "di_registry": False, "telemetry_exporters": 0}

    def emit(self, rec: dict):
        records.validate(rec)
        if len(self.reports) >= OBSERVATION_BOUND:
            # the reporter's buffer has a declared bound (PROG-NFR-13): overflow is visible as one record whose count
            # grows; it replaces the held record and is written when the run ends, with its final count
            last = self.reports[-1]
            if last.get("kind") != "buffer-overflow":
                last = {"kind": "buffer-overflow", "bound": OBSERVATION_BOUND, "dropped": 1, "rule": "PROG-NFR-13"}   # the replaced record counts
                self.reports[-1] = last
            last["dropped"] += 1
            return
        self.reports.append(rec)
        if len(self.reports) == OBSERVATION_BOUND:
            return                                   # the record at the bound is held: a later record replaces it (D-24)
        sys.stderr.write((json.dumps(rec, sort_keys=True) if self.json_out else records.render(rec)) + "\n")

    def flush_records(self):
        """The end of a run: the held record, or the overflow record with its final count (as the VM and native)."""
        if len(self.reports) == OBSERVATION_BOUND and not getattr(self, "_flushed", False):
            self._flushed = True
            last = self.reports[-1]
            sys.stderr.write((json.dumps(last, sort_keys=True) if self.json_out else records.render(last)) + "\n")

    def implied_calls(self) -> set:
        """The calls of the module's test bodies that have an implied `or fail`, by the position of their name token:
        the reference checker records them, and only them, as the IR builder reads the production checker's (14,
        LANG-14-03; X-345, D-319). A call of a function that returns an Outcome it built, an `or`/`catch` operand, a
        `match` scrutinee and a binding the body consumes as an outcome are not among them. A package's tests get
        the positions from the package check (bin/fortis); a module on its own is checked here when nobody has."""
        if self.implied is None:
            if self.link is not None:
                raise FortisError("the tests of a package module run with the package check's implied `or fail` positions")
            from checker import check
            import names
            check.check_source(names.Library(self.rec), self.src, self.file)
            self.implied = set(check.LAST_TYPER.implied_fails)
        return self.implied

    def implied_fail(self, v, tok):
        """The call named at `tok` in a test body: with the implied `or fail` the checker recorded there, its failure
        fails the test, as `or fail` would (14, LANG-14-03; X-345). Any other expression's value, an outcome bound
        or passed on included, is what it is (D-319)."""
        if tok is not None and isinstance(v, Failure) and (tok.line, tok.col) in self.implied_calls():
            raise FailureSignal(v)
        return v

    def names_something(self, tok, env: Env) -> bool:
        """Whether the name of `tok` resolves to a binding or declaration here (a `log` statement needs it not to, 13)."""
        try:
            self.literal(tok, env)
            return True
        except FortisError:
            return False

    def log_statement(self, level: str, al, env: Env):
        """`log.info|warn|error(message)` (13, LANG-13-04): one `log` record on the diagnostic plane, never stdout."""
        args = self.args(al, env) if al is not None else []
        message = display(args[0][1]) if args else ""
        where = self.stack[-1] if self.stack else (self.test_label or "main")       # a test body is named as the VM names it
        if self.writers and self.writers[-1][1] == len(self.stack):
            where = self.writers[-1][0]                  # a lambda or a cleanup, as the IR names it (`lambda@3:11`, `cleanup Box`; D-323)
        self.emit({"kind": "log", "level": level, "message": message, "function": where, "rule": "LANG-13-04"})

    def emit_report(self, f: Failure, where: str):
        chain, c = [], f.cause
        while c is not None:
            chain.append(c.code)
            c = c.cause
        shown = {k: display(v) for k, v in f.payload.items()}
        # the message template rendered with the payload as the record shows it (10.3; D-340)
        self.emit({"kind": "runtime-failure", "code": f.code, "message": render_message(f.message or "", shown), "payload": shown,
                   "origin": {"file": self.decl_file.get(f.origin or where, self.file), "function": f.origin or where}, "cause": chain, "annotations": list(f.annotations),
                   "reported_by": "cleanup" if self.cleanup_depth else where, "rule": "LANG-10-06"})

    # ---- calls ---------------------------------------------------------------
    def params_of(self, head) -> List[Tuple[str, Optional[Any], bool]]:
        """[(name, default constExpr node or None, is_self)] from a funcHead/methodHead."""
        sig = find(head, "funcSignature")
        pl = find(sig, "paramList")
        out = []
        if pl is None:
            return out
        for p in [c for c in children(pl) if name(c) == "param"]:
            if find(p, "selfParam") is not None:
                out.append(("self", None, True))
                continue
            ts = [t for t in direct_tokens(p) if t.text != "..."]
            if not ts:
                continue                     # restParam (aspects are outside the kernel)
            out.append((ts[0].text, own_expr(p), False))
        return out

    def owned_params(self, head) -> List[str]:
        """The parameters the function owns (`owned` parameters and an `owned self` receiver), in order: they are
        the function's own values, destroyed at its exit unless moved (7.9)."""
        sig = find(head, "funcSignature")
        pl = find(sig, "paramList") if sig is not None else None
        out = []
        for p in ([c for c in children(pl) if name(c) == "param"] if pl is not None else []):
            sp = find(p, "selfParam")
            if sp is not None:
                if any(t.text == "owned" for t in tokens(sp)):
                    out.append("self")
                continue
            ts = [t for t in direct_tokens(p) if t.text != "..."]
            if ts and param_owned(p):
                out.append(ts[0].text)
        return out

    def is_try(self, head) -> bool:
        mods = find(head, "modifiers")
        return mods is not None and any(t.text == "try" for t in tokens(mods))

    def func_name(self, head) -> str:
        mn = find(head, "memberName")
        if mn is not None:
            return tokens(mn)[0].text
        ts = direct_tokens(head)
        return ts[[t.text for t in ts].index("func") + 1].text

    def is_async(self, head) -> bool:
        mods = find(head, "modifiers")
        return mods is not None and any(t.text == "async" for t in tokens(mods))

    def call(self, head, body, args: List[Tuple[Optional[str], Any]], self_value, aspects: Optional[list] = None):
        """Bind arguments (positional, then named, then defaults) and run the body. A `try`
        function returns a Failure value instead of raising. Aspect hooks wrap the body (11.8).
        An `async` call binds its arguments and returns a Future without running (12.3)."""
        env = self.bind(head, args, self_value)
        if self.is_async(head) and body is not None:
            return FutureVal(self.func_name(head), head, body, env, self.is_try(head))
        in_test, self.in_test_body = self.in_test_body, False      # a callee's body is not the test's (14)
        try:
            if aspects:
                r = self.call_with_aspects(head, body, env, aspects)
            else:
                r = self.run_bound(head, body, env)
        finally:
            self.in_test_body = in_test
        # a result declared `from` a parameter is a loan the caller's binding holds; any other is a new owned value,
        # a temporary of the statement until something takes it (7.6, 7.9; D-338)
        return self.call_result(r, self.return_type(head))

    def run_future(self, fut: FutureVal):
        """Run a future's body in the current task (inline at its first await, or as a launched task)."""
        fut.polled = True
        if fut.head is None:
            # launchParallel / launchBlocking work (DEL-24 pools run it here), or an async closure's call
            if fut.consumes is None:
                return self.call_value(fut.closure, [])
            return self.consuming(fut.consumes, lambda: self.call_value(fut.closure, []))
        return self.run_bound(fut.head, fut.body, fut.env)

    def consuming(self, closure, run):
        """Run a call that consumes `closure` (`move f(…)`, launched work), then destroy the closure: its owned
        captures that the body has not moved out, after the body's own locals (7.9, X-250; D-30). A defect runs no
        cleanup (10.8)."""
        defect = False
        try:
            return run()
        except DefectSignal:
            defect = True
            raise
        finally:
            if not defect:
                self.destroy(closure)

    def await_value(self, v):
        """`await e` (12.3): a never-polled future runs inline; a task is awaited through the scheduler;
        cancellation of the current task is observed here and only here."""
        me = self.sched.current
        if me.cancelled:
            self.trace.append(f"cancelled {me.name}")
            raise CancelSignal()
        if isinstance(v, intrinsics.ReadyVal):
            return self.fresh_add(v.value)                # an async intrinsic that completed without suspending
        if isinstance(v, JoinVal):
            # both futures run to completion inside the parent, even when one fails or the parent is cancelled (12.5)
            tasks = [self.sched.launch(f, None) for f in v.futures]
            results = []
            for t in tasks:
                try:
                    self.sched.block_until(t)
                except CancelSignal:
                    for u in tasks:
                        self.sched.cancel(u)
                    for u in tasks:
                        if not u.done:
                            self.sched.block_until(u, observe_cancel=False)
                    self.trace.append(f"cancelled {me.name}")
                    raise
                self.trace.append(f"join {t.name}")
                t.consumed = True
                if t.error is not None:
                    raise t.error
                results.append(t.result)
            return self.fresh_add(tuple(results))
        if isinstance(v, SleepVal):
            try:
                self.sched.block_until(None, self.sched.now + v.ns)
            except CancelSignal:
                self.trace.append(f"cancelled {me.name}")
                raise
            return VOID
        if isinstance(v, FutureVal):
            if v.task is None:
                return self.fresh_add(self.run_future(v))
            v = v.task
        if isinstance(v, TaskVal):
            if not v.done:
                try:
                    self.sched.block_until(v)
                except CancelSignal:
                    self.trace.append(f"cancelled {me.name}")
                    raise
            self.trace.append(f"join {v.name}")
            v.consumed = True
            if v.error is not None:
                raise v.error
            if v.completion == "Cancelled":
                raise FortisError(f"awaited task {v.name} was cancelled: cancellation of a child reaches the parent (12.5)")
            return self.fresh_add(v.result)
        raise FortisError("await of a value that is not a future or task")

    def not_started(self, fut: Optional[FutureVal]):
        self.emit({"kind": "task-lifecycle", "function": fut.fname if fut else "", "state": "not_started", "rule": "LANG-12-03"})
        if fut is not None and self.schema_of(fut.fname):
            self.observation(self.schema_of(fut.fname), fut.fname, {}, "not_started", [])
        if fut is not None:
            self.destroy_unrun(fut)                          # what the task's work owned goes with it (D-30)

    def task_completion(self, t: TaskVal):
        self.emit({"kind": "task-completion", "task": t.name, "outcome": t.completion, "rule": "LANG-12-04"})

    def bind(self, head, args, self_value) -> Env:
        env = Env(self.globals)
        env.frame = True
        env.module = self.head_module.get(id(head), env.module)
        params = [p for p in self.params_of(head) if not p[2]]
        owned = set(self.owned_params(head))
        if self_value is not None or any(p[2] for p in self.params_of(head)):
            env.declare("self", self_value, mutable=True, owns="self" in owned)
            if "self" in owned:
                self.release(self_value)               # an owned self: the callee becomes its owner (7.9)
        positional = [v for n, v in args if n is None]
        named = {n: v for n, v in args if n is not None}
        ptypes = self.param_type_nodes(head) if self.open_names() else {}
        for i, (pn, default, _) in enumerate(params):
            # a plain or `editable` parameter holds a loan; an `owned` one owns its argument (7.6)
            if i < len(positional):
                v = positional[i]
                env.declare(pn, self.expected(v, ptypes.get(pn)), mutable=False, owns=pn in owned)
            elif pn in named:
                v = named.pop(pn)
                env.declare(pn, self.expected(v, ptypes.get(pn)), mutable=False, owns=pn in owned)
            elif default is not None:
                v = self.eval(default, env)
                env.declare(pn, v, mutable=False, owns=pn in owned)
            else:
                raise FortisError(f"missing argument `{pn}`")
            if pn in owned:
                self.release(v)                        # an owned parameter owns the value, even a field of a temporary (7.9)
        if named:
            raise FortisError(f"unknown named arguments {sorted(named)}")
        if len(positional) > len(params):
            raise FortisError("too many positional arguments")
        return env

    def run_bound(self, head, body, env: Env):
        if body is None:
            raise FortisError("call to a trusted function with no body in the kernel")
        fname = self.func_name(head)
        schema = self.schema_of(fname)
        self.frames.append({} if schema else None)
        self.stack.append(fname)
        outcome = "success"
        lent = True                                      # whether the value returned is a loan; a `from` result that is
        try:                                             # the function's own is owned by the binding that receives it (D-373)
            blk = find(body, "block") if name(children(body)[0]) == "block" else None
            if blk is not None:
                self.run_block(blk, env)
                return VOID
            v = self.eval(find(body, "expr"), env)
            lent = self.is_lent(v)
            return self.expected(v, self.return_type(head))
        except ReturnSignal as r:
            lent = r.lent
            return self.expected(r.value, self.return_type(head))
        except FailureSignal as f:
            outcome = "failure"
            if self.is_try(head):
                return f.failure
            raise FortisError(f"`fail` in a function without `try` ({f.failure.code})")   # plug-in: effects (DEL-19)
        except CancelSignal:
            # the outcome class of an activation that ends by cancellation or a defect (13)
            outcome = "cancelled"
            raise
        except DefectSignal:
            outcome = "defect"
            raise
        finally:
            if outcome != "defect":                      # a defect runs no cleanup (10.8)
                # the owned parameters are the function's oldest values: destroyed last, unless moved (7.9)
                for pn in reversed(self.owned_params(head)):
                    cell = env.vars.get(pn)
                    if cell is not None and not cell.moved:
                        self.announce_drop(cell.value)
                        cell.moved = True
                        self.destroy(cell.value)
            self.stack.pop()
            frame = self.frames.pop()
            if schema:
                truncated = frame.pop("__truncated", [])
                self.observation(schema, fname, {k: display(v) for k, v in frame.items()}, outcome, truncated)
            self.returned_lent = lent                    # last, so the destructors' own calls cannot overwrite it (D-373)

    def observation(self, schema: str, function: Optional[str], fields: Dict[str, str], outcome: Optional[str], truncated: List[str]):
        """A completed frame (`function` and `outcome` given) or an executed `record`: under a telemetry policy the
        canonical permitted record (Platform 6.3), and the diagnostic plane's record, which carries captured values
        only under `permit-all` (Language Reference 15)."""
        telemetry.observe(schema, self.schema_versions.get(schema, 1), function, outcome, list(fields.items()), truncated)
        values = telemetry.diagnostic_values()             # a cut is about the value too: withheld with it
        shown = fields if values else {}
        if function is None:
            self.emit({"kind": "observation-event", "schema": schema, "fields": shown, "rule": "LANG-13-01"})
        else:
            self.emit({"kind": "observation-frame", "schema": schema, "function": function, "fields": shown, "outcome": outcome, "truncated": truncated if values else [], "rule": "LANG-13-02"})

    def schema_of(self, fname: str) -> Optional[str]:
        for use in self.func_aspects.get(fname, []):
            uts = direct_tokens(use)
            if uts[1].text == "observe" and self.core_aspect("observe"):
                al = find(use, "argList")
                return tokens(al)[0].text if al is not None else None
        return None

    # ---- aspects (11.8) ---------------------------------------------------------
    def core_aspect(self, aname: str, head=None) -> bool:
        """`@observe` and `@ignorable` name the prelude's metadata aspects unless the module that declares the function
        (`head`) declares or imports an aspect of that name, which is then the one applied, hooks and all: the aspect
        is resolved, not its spelling (D-314). A module on its own is the program's only module."""
        if aname not in ("observe", "ignorable") or aname not in self.aspects:
            return aname in ("observe", "ignorable")
        module = self.head_module.get(id(head)) if head is not None else None
        if self.link is None or module is None or module not in self.link.modules:
            return False
        return module not in self.aspect_homes.get(aname, set()) and aname not in self.link.modules[module].scope

    def resolve_aspect(self, aname: str, args: List[Tuple[Optional[str], Any]]):
        """Follow specialisations to the base aspect, binding arguments; `...rest` forwards the rest."""
        a = self.aspects.get(aname)
        if a is None:
            raise FortisError(f"`@{aname}` is not a declared aspect")
        names_ = [p[0] for p in a["params"]]
        bound, extra = {}, []
        for lbl, v in args:
            if lbl is None:
                bound[names_[0]] = v
            elif lbl in names_:
                bound[lbl] = v
            else:
                extra.append((lbl, v))
        for pn, default in a["params"]:
            if pn not in bound and default is not None:
                bound[pn] = self.eval(default, self.globals)
        if a["base"] is None:
            return a, bound
        bname, bal = a["base"]
        level = Env(self.globals)
        for pn, v in bound.items():
            level.declare(pn, v)
        fargs = []
        for arg in ([c for c in children(bal) if name(c) == "arg"] if bal is not None else []):
            ks = children(arg)
            if isinstance(ks[0], Token) and ks[0].text == "...":
                fargs.extend(extra)
            elif isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                fargs.append((ks[0].text, self.eval(ks[2], level)))
            else:
                fargs.append((None, self.eval(ks[0], level)))
        return self.resolve_aspect(bname, fargs)

    def return_type(self, head):
        """The function's own declared result type (a search of the head would find a function-typed parameter's)."""
        sig = find(head, "funcSignature") if head is not None else None
        return next((c for c in children(sig) if name(c) == "returnType"), None) if sig is not None else None

    def param_type_nodes(self, head) -> Dict[str, Any]:
        pl = find(find(head, "funcSignature"), "paramList")
        return {direct_tokens(p)[0].text: find(p, "paramType") for p in (children(pl) if pl is not None else [])
                if name(p) == "param" and direct_tokens(p) and direct_tokens(p)[0].kind == "LOWER"}

    def param_types(self, head) -> Dict[str, str]:
        out = {}
        pl = find(find(head, "funcSignature"), "paramList")
        for p in ([c for c in children(pl) if name(c) == "param"] if pl is not None else []):
            pt = find(p, "paramType")
            ts = direct_tokens(p)
            if pt is not None and ts and ts[0].kind == "LOWER":
                out[ts[0].text] = next((t.text for t in tokens(pt) if t.kind == "UPPER"), "")
        return out

    def call_with_aspects(self, head, body, env: Env, uses: list):
        """Elaboration (11.8): before hooks in application order, the body, after hooks in reverse,
        failure handlers innermost first; a handler that returns completes the function."""
        chain = []
        ptypes = self.param_types(head)
        for use in uses:
            uts = direct_tokens(use)
            if self.core_aspect(uts[1].text, head):
                continue                                 # core aspects with no hooks (11.8): `ignorable` is the checker's (5.5)
            al = find(use, "argList")
            a, bound = self.resolve_aspect(uts[1].text, self.args(al, self.globals) if al is not None else [])
            henv = Env(self.globals)
            for pn, v in bound.items():
                henv.declare(pn, v)
            for n2, tname in a["needs"]:
                pn = next((p for p, t in ptypes.items() if t == tname), None)
                if pn is not None:
                    henv.vars[n2] = env.cell(pn)             # the hook uses the target's parameter under the needs name
            chain.append((a, henv))
        def failing(step):
            try:
                v = step()
            except FailureSignal as f:
                return f.failure, True
            return v, isinstance(v, Failure) and self.is_try(head)
        def run_hook(blk, henv, extra=None):
            e = Env(henv)
            for k, v in (extra or {}).items():
                e.declare(k, v)
            try:
                self.run_block(blk, e)
            except ReturnSignal as r:
                return r.value
            return VOID
        failure = None
        for a, henv in chain:
            for blk in a["before"]:
                _, failed = failing(lambda: run_hook(blk, henv))
                if failed:
                    failure = _
                    break
            if failure is not None:
                break
        result = VOID
        lent = True
        if failure is None:
            result, failed = failing(lambda: self.run_bound(head, body, env))
            lent = self.returned_lent                      # the body's own answer, kept across the hooks' calls (D-373)
            if failed:
                failure = result
        if failure is None:
            for a, henv in reversed(chain):
                if a["after"] is not None:
                    rn, blk = a["after"]
                    _, failed = failing(lambda: run_hook(blk, henv, {rn: result}))
                    if failed:
                        failure = _
                        break
        if failure is None:
            self.returned_lent = lent
            return result
        for a, henv in reversed(chain):
            if a["on_failure"] is not None:
                fn, blk = a["on_failure"]
                try:
                    v = run_hook(blk, henv, {fn: failure})
                except FailureSignal as f:
                    failure = f.failure
                    continue
                return v                                   # a handler that returns a value completes the function
        if self.is_try(head):
            return failure
        raise FortisError(f"`fail` in a function without `try` ({failure.code})")

    # ---- statements ----------------------------------------------------------
    def run_block(self, blk, env: Env, scope: Optional[ScopeVal] = None):
        inner = Env(env)
        inner.defers = []
        inner.scope = scope
        unwinding = False
        terminated = False
        try:
            for st in [c for c in children(blk) if name(c) == "statement"]:
                self.run_statement(st, inner)
        except (FailureSignal, CancelSignal):
            unwinding = True
            raise
        except (DefectSignal, RecursionError):
            terminated = True                # a defect terminates the task at once: no cleanup runs (10.8)
            raise
        finally:
            if not terminated:
                self.exit_block(inner, unwinding)

    @staticmethod
    def returned_local(e, env: Env) -> Optional[Cell]:
        """The cell of `x` in `return x` when `x` is a local of the running call: a binding of one of its blocks, not a
        parameter, a capture or a module name. Its value leaves with the return and is not destroyed at block exit
        (7.5, X-345); it stays readable, so a trivial local is read exactly as before. `x` is read structurally, as
        the checkers' place path reads it: through single-child rules and parentheses, so `return (x)` is `return x`
        (D-313); a marked `return (move x)` moves `x` itself."""
        n = e
        while not isinstance(n, Token):
            kids = children(n)
            if name(n) == "primaryExpr" and len(kids) == 3 and isinstance(kids[0], Token) and kids[0].text == "(":
                n = kids[1]
            elif len(kids) == 1:
                n = kids[0]
            else:
                return None
        if n.kind != "LOWER":
            return None
        cur: Optional[Env] = env
        while cur is not None and not cur.frame and cur.parent is not None:
            if n.text in cur.vars:
                return cur.vars[n.text]
            cur = cur.parent
        return None

    def exit_block(self, inner: Env, unwinding: bool):
        """Block exit (5.1, 12.2, 12.4): the block's loans end and the tasks launched on a scope declared
        in it are cancelled and joined; then deferred blocks LIFO; then, when unwinding, `abandon` on every
        live Abandonable local in reverse initialisation order; then locals drop in reverse order."""
        scope = getattr(inner, "scope", None)
        abandonables = [(n, c) for n, c in inner.vars.items() if not c.moved and c.owns and n != "self" and isinstance(c.value, Instance) and "Abandonable" in c.value.cls.implements] if unwinding else []
        noisy = bool(inner.defers) or (scope is not None and scope.tasks) or abandonables or any(self.cleanup_runs(c.value) and not c.moved and not c.returned and c.owns for c in inner.vars.values())
        if noisy:
            self.trace.append("loans-end")
        if scope is not None:
            for t in list(scope.tasks):
                if not t.done:
                    self.sched.cancel(t)
            for t in list(scope.tasks):
                if getattr(t, "consumed", False):
                    continue                     # already awaited: joined there
                if not t.done:
                    self.sched.block_until(t)
                self.trace.append(f"join {t.name}")
                if t.error is not None and not unwinding:
                    raise t.error
                if isinstance(t.result, Failure):
                    self.emit_report(t.result, "abandoned")     # a never-awaited result: its failure is reported (12.5)
            scope.tasks.clear()
        for d in reversed(inner.defers):
            self.trace.append("defer")
            self.cleanup_depth += 1
            try:
                self.run_block(d, inner)
            finally:
                self.cleanup_depth -= 1
        for n, c in reversed(abandonables):
            self.trace.append(f"abandon {c.value.cls.name}")
            head, body = c.value.cls.methods["abandon"]
            self.call(head, body, [], c.value)
            c.moved = True
        self.drop_locals(inner)

    def drop_locals(self, env: Env):
        """Block exit (7.9): locals are destroyed in reverse initialisation order; a destroyed class value
        runs its cleanup first; dropping the last strong Shared handle destroys the value."""
        for n, cell in reversed(list(env.vars.items())):
            if cell.moved or cell.returned or not cell.owns or n == "self":
                continue                                 # a binding that holds a loan destroys nothing (D-338)
            self.announce_drop(cell.value)
            self.destroy(cell.value)

    def cleanup_runs(self, v) -> bool:
        """Destroying this owned value runs a class's `cleanup`: a class value that declares one and that its owner
        has not destroyed yet (7.9; D-348)."""
        return isinstance(v, Instance) and v.cls.cleanup is not None and not v.destroyed

    def announce_drop(self, v):
        """The trace's `drop` event for an owned value about to be destroyed, exactly when its `cleanup` runs (D-351)."""
        if self.cleanup_runs(v):
            self.trace.append(f"drop {v.cls.name}")

    def writing(self, label: str, run):
        """Run a lambda's body or a cleanup as the function `label` that a log record names, until it ends however it
        ends (13; D-323)."""
        self.writers.append((label, len(self.stack)))
        try:
            return run()
        finally:
            self.writers.pop()

    def destroy(self, v):
        if v is _MOVED:
            return
        if isinstance(v, intrinsics.GuardVal):
            v.mutex.held = False                       # dropping the guard unlocks (12.7)
            return
        if isinstance(v, intrinsics.FileVal):
            v.closed = True                            # dropping an open file closes it
            return
        if isinstance(v, Instance):
            # a class value: its own cleanup if declared, then its fields in declaration order (7.9), whether or
            # not the class declares a cleanup. Only an owner destroys, and a value already destroyed is not
            # destroyed again, so cleanup runs exactly once (12.2; D-338, D-348)
            if v.destroyed:
                return
            v.destroyed = True
            if v.cls.cleanup is not None:
                e = Env(None)
                e.declare("self", v, mutable=True)
                self.cleanup_depth += 1                  # a report from `cleanup` is labelled as cleanup's (12.1; D-154)
                try:
                    self.writing(f"cleanup {v.cls.name}", lambda: self.run_block(v.cls.cleanup, e))
                except ReturnSignal:
                    pass
                finally:
                    self.cleanup_depth -= 1
            for fv in v.fields.values():
                self.destroy(fv)
        elif isinstance(v, EnumValue):
            for pv in v.payload.values():
                self.destroy(pv)
        elif isinstance(v, SharedVal):
            v.box.strong -= 1
            if v.box.strong == 0:
                self.destroy(v.box.value)
        elif isinstance(v, ListVal):
            for x in v.items:
                self.destroy(x)
        elif isinstance(v, tuple):
            lent = lent_parts(v)                            # a borrowed element is its owner's, not the tuple's (D-349)
            for i, x in enumerate(v):
                if i not in lent:
                    self.destroy(x)
        elif isinstance(v, dict):
            lent = lent_parts(v)
            for k, x in v.items():                          # a record literal: its fields in written order (7.9)
                if k not in lent:
                    self.destroy(x)
        elif isinstance(v, intrinsics.MapVal):
            for k, x in v.entries.values():
                self.destroy(k)
                self.destroy(x)
        elif isinstance(v, intrinsics.SetVal):
            for x in v.items.values():
                self.destroy(x)
        elif isinstance(v, Closure):
            # a closure owns its `copy`, `move` and bare captures: destroying it destroys those its body has not
            # moved out, in capture-list order; `alias` and `editable` captures are loans (7.9, X-250; D-30)
            for n in v.owned:
                cell = v.env.vars.get(n)
                if cell is not None and not cell.moved:
                    cell.moved = True
                    self.destroy(cell.value)
        elif isinstance(v, intrinsics.MutexVal):
            inner, v.value = v.value, VOID                  # a Mutex owns its value (7.9, X-250; D-30)
            self.destroy(inner)
        elif isinstance(v, FutureVal):
            # an async call that never ran owns its owned arguments: they go with it (7.9, 12.3; reachable only
            # through a future held in shared state, D-33)
            if v.task is None and not v.polled:
                self.destroy_unrun(v)

    def destroy_unrun(self, v: FutureVal):
        """A future that never ran, destroyed (held in shared state, D-33) or cancelled before its task started: its
        owned arguments go with it, and so does a closure it consumed and that closure's owned parameters (7.9, 12.3,
        X-250; D-30)."""
        if v.released:
            return
        v.released = v.polled = True
        if v.head is not None:
            for pn in reversed(self.owned_params(v.head)):
                cell = v.env.vars.get(pn)
                if cell is not None and not cell.moved:
                    cell.moved = True
                    self.destroy(cell.value)
            return
        wrapper = v.closure
        if wrapper is not None and wrapper is not v.consumes:
            for pn in reversed(wrapper.owned_params):       # an async closure's call: its bound owned parameters
                cell = wrapper.env.vars.get(pn)
                if cell is not None and not cell.moved:
                    cell.moved = True
                    self.destroy(cell.value)
        if v.consumes is not None:
            c, v.consumes = v.consumes, None
            self.destroy(c)                                  # the closure it owns goes with it

    # ---- statement temporaries (5.7, 7.9) -------------------------------------------
    def fresh_add(self, v):
        """Record a newly created owned value as a temporary of the innermost running statement: unless its
        ownership moves before the statement ends, it is destroyed there. Evaluations outside a statement
        (a constant's first use, a program's entry) record nothing."""
        if self.temps and isinstance(v, TEMP_TYPES):
            self.temps[-1][id(v)] = v
        return v

    def fresh_remove(self, v):
        """Ownership of `v` moved (a binding, a store, an owned argument, a signal): it is no temporary."""
        for d in self.temps:
            d.pop(id(v), None)

    def release(self, v):
        """Ownership of `v` moved. It is no temporary, and a temporary that still holds it as a field, a payload
        or a record field must not destroy it again (7.9). Call this before storing `v` into a value `_detach`
        walks: doing it after would delete the field just stored."""
        self.fresh_remove(v)
        self.detach_from_temps(v)

    def _statement_temp(self, v) -> bool:
        return any(id(v) in d for d in self.temps)

    def _moves_source(self, node) -> bool:
        """True when this for-source expression is itself a `move`, not a move nested in an argument."""
        if isinstance(node, Token):
            return False
        if name(node) == "markerExpr":
            kids = children(node)
            ts = tokens(kids[0]) if kids else []
            return bool(ts) and ts[0].text == "move"
        kids = children(node)
        nested = [k for k in kids if not isinstance(k, Token)]
        if len(nested) == 1 and len(kids) - len(nested) <= 1:
            return self._moves_source(nested[0])
        return False

    def _moves_place(self, node) -> bool:
        """True when `node` is exactly `move` of a place (`discard move p`), not `move p.method()` or `move f()`."""
        if isinstance(node, Token):
            return False
        n = name(node)
        kids = children(node)
        if n == "markerExpr":
            if len(kids) != 2:
                return False
            ts = tokens(kids[0])
            return bool(ts) and ts[0].text == "move" and name(kids[1]) == "place"
        if n == "primaryExpr":
            toks = [k for k in kids if isinstance(k, Token)]
            nested = [k for k in kids if not isinstance(k, Token)]
            if toks and toks[0].text == "(" and toks[-1].text == ")" and len(nested) == 1:
                return self._moves_place(nested[0])
            return False
        nested = [k for k in kids if not isinstance(k, Token)]
        ops = [k for k in kids if isinstance(k, Token) and k.kind != "NL"]
        if len(nested) == 1 and not ops:
            return self._moves_place(nested[0])
        return False

    def _place_tokens(self, node):
        """Tokens of a place-shaped expression: a bare name or `self`, then `.member` or `[int]`, including
        parentheses. A call is not a place. None when the expression is not a place."""
        if isinstance(node, Token):
            if node.kind == "LOWER" or node.text == "self":
                return [node]
            return None
        n = name(node)
        kids = children(node)
        if n == "primaryExpr":
            if len(kids) == 1 and isinstance(kids[0], Token) and (kids[0].kind == "LOWER" or kids[0].text == "self"):
                return [kids[0]]
            toks = [k for k in kids if isinstance(k, Token)]
            nested = [k for k in kids if not isinstance(k, Token)]
            if toks and toks[0].text == "(" and toks[-1].text == ")" and len(nested) == 1:
                return self._place_tokens(nested[0])
            return None
        if n == "postfixExpr":
            base = self._place_tokens(kids[0]) if kids else None
            if base is None:
                return None
            for k in kids[1:]:
                step = self._place_step(k)
                if step is None:
                    return None
                base.extend(step)
            return base
        nested = [k for k in kids if not isinstance(k, Token)]
        ops = [k for k in kids if isinstance(k, Token) and k.kind != "NL"]
        if len(nested) == 1 and not ops:
            return self._place_tokens(nested[0])
        return None

    def _place_step(self, k):
        """The tokens one postfix adds to a place, or None when that postfix is not place syntax."""
        if name(k) != "postfix":
            return None
        pk = children(k)
        if not pk or not isinstance(pk[0], Token):
            return None
        op = pk[0].text
        if op == "." and len(pk) >= 2:
            second = pk[1]
            if isinstance(second, Token):
                return [pk[0], second]
            ts = tokens(second)
            if len(ts) == 1:
                return [pk[0], ts[0]]
            return None
        if op == "[" and len(pk) >= 2:
            ts = tokens(pk[1])
            if len(ts) == 1 and ts[0].kind == "INT":
                close = pk[2] if len(pk) > 2 and isinstance(pk[2], Token) else None
                return [pk[0], ts[0]] if close is None else [pk[0], ts[0], close]
        return None

    def detach_from_temps(self, v):
        """`v` was moved out of a statement temporary that still holds it as a field, a payload, a record
        field or a tuple element. That temporary must not destroy it again (7.9). A tuple is rebuilt with a
        sentinel in the moved element's place. The map keeps that rebuilt tuple in the same creation-order
        slot, so the other elements drop at the end of the statement, after the consumer (D-312). Not used
        for a list or a map: storing into one and then `fresh_remove` is how the stored value stops being a
        temporary of its own. A trivial value is copied, not moved, and small integers are interned, so
        detaching one would delete an unrelated field."""
        if not isinstance(v, TEMP_TYPES):
            return
        for d in self.temps:
            replaced = {}
            for key in list(d):
                parent = d[key]
                if parent is v:
                    continue
                new_parent = self._detach(parent, v)
                if new_parent is not parent:
                    replaced[key] = new_parent
            if not replaced:
                continue
            items = []
            for key, parent in d.items():
                if key in replaced:
                    nv = replaced[key]
                    items.append((id(nv), nv))
                else:
                    items.append((key, parent))
            d.clear()
            d.update(items)

    def _detach(self, parent, v):
        """Remove `v` from `parent`. A mutable aggregate loses the field. A tuple is rebuilt, because a Python
        tuple cannot be edited, and the result is the parent the temporary map must track afterwards."""
        if isinstance(parent, tuple):
            changed = False
            items = []
            for x in parent:
                if x is v:
                    items.append(_MOVED)
                    changed = True
                else:
                    y = self._detach(x, v)
                    if y is not x:
                        changed = True
                    items.append(y)
            return retuple(parent, items) if changed else parent
        items = parent.fields if isinstance(parent, Instance) else parent.payload if isinstance(parent, EnumValue) else parent if isinstance(parent, dict) else None
        if items is None:
            return parent
        for k in list(items):
            x = items[k]
            if x is v:
                del items[k]
            else:
                y = self._detach(x, v)
                if y is not x:
                    items[k] = y
        return parent

    def mark_given(self, env: Env, value):
        """An arm binding that the arm gives back, returns or recovers is not destroyed with the arm (7.9)."""
        if value is None:
            return
        for cell in env.vars.values():
            if cell.value is value:
                cell.moved = True

    def is_temp(self, v) -> bool:
        return any(id(v) in d for d in self.temps)

    def destroy_temps(self, temps):
        """The temporaries of one statement, in reverse creation order (7.9); a destroyed class value is
        announced exactly as a dropped local is."""
        for v in reversed(list(temps.values())):
            self.announce_drop(v)
            self.destroy(v)

    def eval_cond_temps(self, cond, env):
        """A condition runs on its own: the temporaries of each evaluation are destroyed at the end of that
        evaluation, before the branch or the next iteration, so a loop cannot accumulate them (7.9; decision
        for the integrator: the Reference gives a condition no full-statement end of its own)."""
        temps = {}
        self.temps.append(temps)
        task, lent = self.lent_scope()
        terminated = False
        try:
            return self.eval(cond, env)
        except (DefectSignal, RecursionError):
            terminated = True
            raise
        finally:
            task.lent = lent
            self.temps.pop()
            if not terminated:
                self.destroy_temps(temps)

    def excise(self, v, bound):
        """Cut the bound values out of a structure so its destruction cannot reach them: an arm's bindings own
        the parts of a scrutinee they bound (7.9), and the rest of the scrutinee still destroys in order.
        A tuple cannot be mutated, so this returns a new one with a hole where a bound element was, including
        an element bound inside a tuple that itself was not bound. Other structures are updated in place."""
        if isinstance(v, tuple):
            return retuple(v, [_MOVED if id(x) in bound else self.excise(x, bound) for x in v])
        if isinstance(v, ListVal):
            v.items = [self.excise(x, bound) for x in v.items if id(x) not in bound]
            return v
        items = v.fields if isinstance(v, Instance) else v.payload if isinstance(v, EnumValue) else v if isinstance(v, dict) else None
        if items is None:
            return v
        for k in list(items):
            x = items[k]
            if id(x) in bound:
                del items[k]
            else:
                items[k] = self.excise(x, bound)
        return v

    def take_scrutinee_parts(self, v, aenv, fresh: bool):
        """A matched arm whose scrutinee was moved or is a statement temporary: the arm's bindings own the
        parts they bound. Bound parts are excised. A field matched by `_` is not a binding. A temporary's
        remainder stays tracked and destroys at the end of the statement (D-150). A moved variable's
        remainder is returned and destroys after the arm (7.9, 9.8). A pattern that binds nothing returns
        the moved value so the caller can drop an opened pattern after the arm, or put a whole-value `_`
        back (D-300, D-315). Returns the remainder to destroy, or None."""
        bound = {id(c.value) for c in aenv.vars.values()}
        if not bound:
            if fresh:
                return None                               # a wildcard of a temporary dies at statement end (D-150)
            return v                                      # a moved local: the caller drops it, or puts a whole `_` back
        if id(v) in bound:
            if fresh:
                self.fresh_remove(v)                      # the arm's binding becomes the whole value's owner
            return None
        if isinstance(v, tuple):
            if fresh:
                self.fresh_remove(v)
            rest = self.excise(v, bound)
            if fresh:
                self.fresh_add(rest)                      # unbound elements die at statement end, after the arm
                return None
            return rest
        self.excise(v, bound)
        return None if fresh else v

    def destroy_unbound(self, v, bound):
        """A destructuring `let` moved its source. A pattern that binds nothing still consumes the value:
        every field matched by `_` dies at the `let` (7.9, 9.8). Parts a pattern did bind stay with those
        bindings and drop with the block. A bound element nested inside an unbound tuple or class is holed
        first, so the remainder cannot destroy it again (D-301)."""
        if v is _MOVED or id(v) in bound:
            return
        if not bound:
            self.destroy(v)
            return
        self.destroy(self.excise(v, bound))

    def intrinsic_ownership(self, tn, mname, pos, out):
        """Ownership at the Python-registry intrinsics (7.9): an argument the intrinsic stored in persistent
        state stops being a statement temporary, and a result that is a new owned value becomes one. A result
        loaning a stored value (`get`, `at`, a cursor's `next`) is not recorded. The prelude declares these
        modes (`item: owned T`); the registry runs them in Python, so the kernel mirrors them here."""
        if isinstance(out, Failure):
            return out                                    # a failed intrinsic stored nothing
        if (tn, mname) in (("List", "get"), ("Array", "get"), ("Map", "get"), ("ListCursor", "next")):
            return self.lend(out)                         # `(alias T from self)?`: a loan of the stored value (D-338)
        if tn == "List" and mname == "push":
            self.release(pos[0])
        elif tn in ("List", "Array") and mname == "set":
            self.release(pos[1])
        elif tn == "Map" and mname == "set":
            self.release(pos[0])
            self.release(pos[1])
        elif tn == "Set" and mname == "add":
            if out is True:
                self.release(pos[0])                       # a duplicate was not stored
        elif tn == "List" and mname == "of":
            for x in pos:
                self.release(x)
            return self.fresh_add(out)
        elif tn == "Outcome" and mname == "collect":
            self.release(pos[0])
            return self.fresh_add(out)
        elif tn in ("Shared", "Mutex") and mname == "create":
            self.release(pos[0])
            return self.fresh_add(out)
        elif tn == "MutexGuard" and mname == "set":
            self.release(pos[0])                           # `value: owned T` is stored; the guard drops the old one (12.7)
        elif tn == "MutexGuard" and mname == "replace":
            self.release(pos[0])                           # the stored value leaves the statement; the old one returns to the caller
            return self.fresh_add(out)
        elif (tn, mname) in (("List", "pop"), ("Shared", "retain"), ("Weak", "upgrade"),
                             ("Mutex", "lockBlocking"), ("FileSystem", "open"), ("FileSystem", "create")):
            return self.fresh_add(out)
        return out

    def run_statement(self, st, env: Env):
        """One statement's temporary scope (5.7, 7.9): the values the statement creates and neither binds nor
        moves out are destroyed at its end, in reverse creation order; a defect destroys nothing (10.8)."""
        temps = {}
        stack = self.temps
        stack.append(temps)
        task, lent = self.lent_scope()
        terminated = False
        try:
            self.statement_body(st, env)
        except (DefectSignal, RecursionError):
            terminated = True                # a defect terminates the task at once: no cleanup runs (10.8)
            raise
        finally:
            task.lent = lent
            stack.pop()
            if not terminated:
                self.destroy_temps(temps)

    def statement_body(self, st, env: Env):
        first = children(st)[0]
        n = name(first)
        if n is None:
            words = [t.text for t in direct_tokens(st)]
            kw = words[0]
            exprs = [c for c in children(st) if name(c) == "expr"]
            if kw == "return":
                v = self.eval(exprs[0], env) if exprs else VOID
                local = self.returned_local(exprs[0], env) if exprs else None
                if local is not None:
                    local.returned = True                  # `return x` of a local moves it, as `return move x` (X-345)
                    if not trivial(v):
                        local.moved = True                 # so a deferred block that still names it fails loudly (12.1; D-391)
                lent = self.is_lent(v)
                if not lent:
                    self.release(v)                        # the value moves with the signal, field and all (7.9)
                raise ReturnSignal(v, lent)               # a `from` result of the function's own is owned where it lands (D-373)
            if kw == "give":
                v = self.eval(exprs[0], env)
                lent = self.is_lent(v)
                if not lent:
                    self.release(v)
                raise GiveSignal(v, lent)                  # a loan given stays a loan where it lands (D-338)
            if kw == "recover":
                v = self.eval(exprs[0], env)
                lent = self.is_lent(v)
                if not lent:
                    self.release(v)
                raise RecoverSignal(v, lent)
            if kw == "report":
                f = self.eval(exprs[0], env)
                if not isinstance(f, Failure):
                    raise FortisError("report of a non-failure")
                self.emit_report(f, "report")
                if "as" in words:
                    env.declare(words[words.index("as") + 1], Report(f.code))
                return
            if kw == "discard":
                v = self.eval(exprs[0], env)
                if isinstance(v, FutureVal) and not v.polled:
                    self.not_started(v)      # a never-polled future may be discarded (12.3)
                    return
                # `discard move p` consumes the place now (7.9). A read, a statement temporary and
                # `move place.method()` do not: the temporary dies at statement end, and the call already
                # moved its receiver.
                if self._moves_place(exprs[0]):
                    if self.is_lent(v):
                        return                             # a loan owns nothing to destroy (7.9; D-338)
                    self.announce_drop(v)
                    self.fresh_remove(v)
                    self.destroy(v)
                return
            if kw == "break":
                raise BreakSignal(words[1] if len(words) > 1 else None)
            if kw == "continue":
                raise ContinueSignal(words[1] if len(words) > 1 else None)
            raise FortisError(f"statement `{kw}` is outside the kernel")
        if n == "letStmt":
            ts = direct_tokens(first)
            mutable = any(t.text == "mut" for t in ts)
            pat = next((c for c in children(first) if name(c) == "pattern"), None)
            if pat is not None:
                value = self.eval_place(next(c for c in children(first) if name(c) == "place"), env, move=True)
                before = set(env.vars)
                if not self.match(pat, value, env):
                    raise FortisError("destructuring pattern did not match")
                self.destroy_unbound(value, {id(c.value) for n, c in env.vars.items() if n not in before})
                return
            nm = next(t for t in ts if t.kind == "LOWER")
            value = self.eval(next(c for c in children(first) if name(c) == "expr"), env)
            # a loan the statement produced is held, not owned: the binding's end destroys nothing (7.3; D-338)
            owns = not self.is_lent(value)
            env.declare(nm.text, self.expected(value, next((c for c in children(first) if name(c) == "type"), None)), mutable, owns)
            if owns:
                self.release(value)                        # the local owns the value, even a field of a temporary (7.9)
            return
        if n == "assignStmt":
            place = find(first, "place")
            op = next(t.text for t in direct_tokens(first) if t.text in ("=", "+=", "-=", "*=", "/=", "%="))
            if op == "=":
                self.assign(place, self.eval(find(first, "expr"), env), env)
                return
            # `x op= e` is sugar for `x = x op e` (1.1) and operands are evaluated left to right (5.7): `x` is read
            # before `e` runs, and the result is stored into the place `x` names after `e` has run (D-60)
            old = self.eval_place(place, env, move=False)
            value = self.eval(find(first, "expr"), env)
            self.assign(place, self.binary(op[0], old, value), env)
            return
        if n == "ifStmt":
            kids = children(first)
            i = 0
            while i < len(kids):
                k = kids[i]
                if isinstance(k, Token) and k.text == "if":
                    cond = self.eval_cond_temps(kids[i + 1], env)
                    self.expect_bool(cond)
                    if cond:
                        self.run_block(kids[i + 2], env)
                        return
                    i += 3
                elif isinstance(k, Token) and k.text == "else":
                    if name(kids[i + 1]) == "block":
                        self.run_block(kids[i + 1], env)
                        return
                    i += 1
                else:
                    i += 1
            return
        if n in ("whileStmt", "loopStmt", "forStmt"):
            label = None
            lbl = find(first, "label")
            if lbl is not None:
                label = direct_tokens(lbl)[0].text
            blk = find(first, "block")
            if n == "whileStmt":
                cond = find(first, "expr")
                while True:
                    c = self.eval_cond_temps(cond, env)
                    self.expect_bool(c)
                    if not c:
                        break
                    if self.loop_body(blk, env, label) == "break":
                        break
                return
            if n == "loopStmt":
                while True:
                    if self.loop_body(blk, env, label) == "break":
                        break
                return
            ts = direct_tokens(first)
            var = ts[[t.text for t in ts].index("for") + 1].text
            src = next(c for c in children(first) if name(c) in ("rangeExpr", "expr", "place"))
            if name(src) == "rangeExpr":
                kids = children(src)
                lo = self.eval(kids[0], env)
                op = kids[1].text
                hi = self.eval(kids[2], env)
                seq = range(lo, hi + 1) if op == ".." else range(lo, hi)
                for item in seq:
                    body_env = Env(env)
                    if var != "_":
                        body_env.declare(var, item, mutable=False)
                    if self.loop_body(blk, body_env, label) == "break":
                        break
                return
            else:
                coll = self.eval_place(src, env, move=True) if name(src) == "place" else self.eval(src, env)
                # a read of a binding leaves the collection intact. A `move`, a place the IR moves, or a fresh
                # temporary is owned by the loop: the source keeps none of the elements, so destroying the
                # source cannot destroy one the body moved, and elements still unyielded die with the loop (7.9).
                owns = name(src) == "place" or self._statement_temp(coll) or self._moves_source(src)
                if isinstance(coll, (ListVal, intrinsics.ArrayVal)):
                    if owns:
                        seq = coll.items
                        coll.items = []
                    else:
                        seq = list(coll.items)
                elif isinstance(coll, intrinsics.SetVal):
                    seq = list(coll.items.values())                       # first-insertion order (6.6)
                    if owns:
                        coll.items.clear()
                elif isinstance(coll, intrinsics.MapVal):
                    seq = [tuple(kv) for kv in coll.entries.values()]     # (key, value) in first-insertion order (6.6)
                    if owns:
                        coll.entries.clear()
                else:
                    raise FortisError("for over a non-collection value is outside the kernel")
                if not owns:
                    for item in seq:
                        body_env = Env(env)
                        if var != "_":
                            body_env.declare(var, item, mutable=False, owns=False)     # a loan of the item (7.5; D-338)
                        if self.loop_body(blk, body_env, label) == "break":
                            break
                    return
                rest = list(seq)
                defect = False
                try:
                    while rest:
                        item = rest.pop(0)
                        body_env = Env(env)
                        if var != "_":
                            body_env.declare(var, item, mutable=False)
                        action = None
                        try:
                            action = self.loop_body(blk, body_env, label)
                        except (DefectSignal, RecursionError):
                            defect = True                # a defect destroys nothing (10.8)
                            raise
                        finally:
                            if not defect:
                                # the binding is not a block local: its cleanup runs, and it is not traced.
                                # native's loop variable is not a block cleanup either (parity under --core).
                                if var != "_":
                                    cell = body_env.vars[var]
                                    if not cell.moved:
                                        cell.moved = True
                                        self.destroy(cell.value)
                                else:
                                    self.destroy(item)
                        if action == "break":
                            break
                finally:
                    if not defect:
                        for item in rest:
                            self.destroy(item)
                return
        if n == "block":
            self.run_block(first, env)
            return
        if n == "exprStmt":
            lc = reading.log_call(first)
            if lc is not None and not self.names_something(lc[0], env):
                self.log_statement(lc[1].text, lc[2], env)
                return
            self.eval(first, env)
            return
        if n == "captureStmt":
            ts = direct_tokens(first)
            v = self.eval(find(first, "expr"), env)
            if not self.frames or self.frames[-1] is None:
                raise FortisError("capture outside a function carrying @observe")
            frame = self.frames[-1]
            fname_ = ts[1].text
            bound = self.observations.get(self.schema_of(self.stack[-1]) or "", {}).get(fname_)
            if isinstance(v, str) and bound is not None and len(v) > bound:
                v = v[:bound]                                   # truncated as the schema declares; the frame records it (13)
                frame.setdefault("__truncated", []).append(fname_)
            frame[fname_] = deep_copy(v)                     # a snapshot: later mutation cannot affect it (13)
            return
        if n == "recordStmt":
            ts = direct_tokens(first)
            al = find(first, "argList")
            fields = {k: display(deep_copy(v)) for k, v in (self.args(al, env) if al is not None else [])}
            self.observation(ts[1].text, None, fields, None, [])
            return
        if n == "expectStmt":
            e = find(first, "expr")
            node = self.comparison_of(e)
            if node is not None:
                # a comparison's operands are evaluated once; the evidence shows those values (14; X-121)
                kids = children(node)
                a = self.eval(kids[0], env)
                b = self.eval(kids[2], env)
                if not self.binary(kids[1].text, a, b):
                    raise ExpectFailure({"actual": self.show(a), "expected": f"{kids[1].text} {self.show(b)}", "line": tokens(e)[0].line})
                return
            v = self.eval(e, env)
            self.expect_bool(v)
            if not v:
                raise ExpectFailure({"actual": "false", "expected": "true", "line": tokens(e)[0].line})
            return
        if n == "deferStmt":
            env.defers.append(find(first, "block"))
            return
        if n == "scopeStmt":
            sname = direct_tokens(first)[1].text
            sc = ScopeVal(sname)
            senv = Env(env)
            senv.declare(sname, sc, mutable=True)
            self.run_block(find(first, "block"), senv, scope=sc)
            return
        raise FortisError(f"unknown statement {n}")

    def comparison_of(self, e):
        """The comparison node of an `expect` condition (`a == b`, `a < b`), or None: its operands are the typed
        actual/expected evidence (14)."""
        node = e
        while name(node) in ("expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr") and len(children(node)) == 1:
            node = children(node)[0]
        kids = children(node)
        if name(node) in ("equalityExpr", "relationalExpr") and len(kids) == 3:
            return node
        return None

    def loop_body(self, blk, env, label) -> Optional[str]:
        try:
            self.run_block(blk, env)
        except BreakSignal as b:
            if b.label is None or b.label == label:
                return "break"
            raise
        except ContinueSignal as c:
            if c.label is None or c.label == label:
                return None
            raise
        return None

    def expect_bool(self, v):
        if not isinstance(v, bool):
            raise FortisError("condition is not Bool")                  # plug-in: typing (DEL-12)

    def assign(self, place, value, env: Env):
        """`place = value`, with `value` already evaluated (a compound assignment arrives here expanded, D-60)."""
        ts = tokens(place)
        root = ts[0].text
        path = [t.text for t in ts[1:] if t.text not in (".", "[", "]")]
        if root == "self":
            target = env.get("self")
            cell = None
        else:
            cell = env.cell(root)
            target = cell.value
        if not path:
            if cell is None:
                raise FortisError("cannot assign to self")
            if not cell.mutable:
                raise FortisError(f"`{root}` is not mutable (E-NAM-002)")     # plug-in: typing (DEL-12)
            owns = not self.is_lent(value)             # a loan stored stays a loan: the binding holds it (D-350)
            if owns:
                self.release(value)                    # before the store: the place may be a temporary record or object (7.9)
            if not cell.moved and cell.owns:
                self.destroy(cell.value)               # 7.9: the new value is evaluated first, then the old value is dropped, then the store
            cell.value = value                         # a binding that held a loan drops nothing: the loan ends (D-350)
            cell.moved = False
            cell.owns = owns
            return
        if cell is not None and not cell.mutable and not isinstance(target, Instance):
            raise FortisError(f"`{root}` is not mutable")
        for seg in path[:-1]:
            target = self.field(target, seg)
        last = path[-1]
        self.release(value)                            # before the store, so detaching cannot delete the field just written (7.9)
        if isinstance(target, Instance):
            self.destroy(target.fields.get(last))
            target.fields[last] = value
        elif isinstance(target, dict):
            target[last] = value
        else:
            raise FortisError("assignment into a non-object")

    # ---- expressions -----------------------------------------------------------
    def eval(self, node, env: Env):
        if isinstance(node, Token):
            return self.literal(node, env)
        n = name(node)
        kids = children(node)
        if self.in_test_body and ((n in tree.ARITH_RULES and len(kids) > 1) or (n == "unaryExpr" and len(kids) == 2 and isinstance(kids[0], Token) and kids[0].text == "-")):
            ops = self.region_cache.get(id(node))
            if ops is None:
                ops = self.region_cache[id(node)] = tree.region_operators(node)
            if ops and not (self.regions and ops & self.regions[-1]):
                # integer arithmetic in a test body is a checked region with an implied `or fail`: the first checked
                # operator that fails fails the test (14, 6.4; X-345)
                self.regions.append(ops)
                try:
                    return self.eval_node(node, n, kids, env)
                except RegionFailure as rf:
                    raise FailureSignal(rf.failure)
                finally:
                    self.regions.pop()
        return self.eval_node(node, n, kids, env)

    def eval_node(self, node, n, kids, env: Env):
        if n in ("expr", "constExpr", "exprStmt", "literal") and len(kids) == 1:
            return self.eval(kids[0], env)
        if n == "constExpr":
            return self.chain(kids, env)
        if n == "constTerm":
            return self.const_term(node, env)
        if n == "outcomeExpr":
            if len(kids) == 1:
                return self.eval(kids[0], env)
            words = [k.text for k in kids if isinstance(k, Token)]
            if words[:2] == ["or", "fail"]:
                v = self.eval_left(kids[0], env)
                if isinstance(v, Failure):
                    raise FailureSignal(v)
                return v
            if words[:2] == ["or", "recover"]:
                v = self.eval_left(kids[0], env)
                return self.eval(kids[-1], env) if isinstance(v, Failure) else v
            if words[0] == "catch":
                v = self.eval_left(kids[0], env)
                if not isinstance(v, Failure):
                    return v
                ident = next(k for k in kids if isinstance(k, Token) and k.kind == "LOWER")
                cenv = Env(env)
                cenv.declare(ident.text, v)
                try:
                    self.run_block(find(node, "block"), cenv)
                except RecoverSignal as r:
                    return self.lend(r.value) if r.lent else self.fresh_add(r.value)
                raise FortisError("catch block fell through without recover/return/fail")      # plug-in: obligations (DEL-17)
        if n == "coalesceExpr":
            if len(kids) == 1:
                return self.eval(kids[0], env)
            left = self.eval(kids[0], env)
            return left if left is not None else self.eval(kids[2], env)
        if n in ("orExpr", "andExpr"):
            v = self.eval(kids[0], env)
            if len(kids) == 1:
                return v
            for i in range(1, len(kids), 2):
                self.expect_bool(v)
                if (n == "orExpr") == bool(v):
                    return bool(v)
                v = self.eval(kids[i + 1], env)
            self.expect_bool(v)
            return v
        if n in ("bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr"):
            return self.chain(kids, env)
        if n == "unaryExpr":
            if isinstance(kids[0], Token):
                op = kids[0].text
                if op == "await":
                    return self.await_value(self.eval(kids[1], env))
                v = self.eval(kids[1], env)
                if op == "-":
                    r = self.checked(kids[0], 0, v)          # `-x` in a checked region is `Int.sub(0, x)` (6.4, X-345)
                    if r is not None:
                        return r
                    return check_int(-v, "neg") if isinstance(v, int) and not isinstance(v, bool) else self.negate(v)
                if op == "!":
                    self.expect_bool(v)
                    return not v
            return self.eval(kids[0], env)
        if n == "markerExpr":
            marker = tokens(kids[0])[0].text
            place = kids[1]
            pts = tokens(place)
            called = len(kids) > 2 and isinstance(kids[2], Token) and kids[2].text == "("
            if marker == "move" and called and len(pts) >= 3 and pts[-2].text == ".":
                # `move items.consumeEach(f)`: the marker moves the receiver; the last segment names the method (7.1)
                v = BoundMember(self.eval_place_tokens(pts[:-2], env, move=True), pts[-1].text)
            else:
                v = self.eval_place(place, env, move=(marker == "move"))
            # a binding that holds a loan moves that loan: a tuple, a record or a binding that takes it borrows it, and
            # its end destroys nothing (7.9; D-368)
            held = marker == "move" and len(pts) == 1 and pts[0].text != "self" and not env.cell(pts[0].text).owns
            if marker in ("alias", "editable"):
                self.lend(v)                                 # a loan of the place, held by whatever takes it (7.3; D-338)
            elif marker == "move" and not held:
                self.unlend(v)                               # ownership moves with the value
            if marker == "copy":
                v = deep_copy(v)
            rest = kids[2:]
            i = 0
            while i < len(rest):
                k = rest[i]
                if name(k) == "methodCall":
                    v = self.method_call(v, k, env)
                elif name(k) == "postfix":
                    v = self.postfix(v, k, env)
                elif isinstance(k, Token) and k.text == "(":
                    al = rest[i + 1] if name(rest[i + 1]) == "argList" else None
                    args = self.args(al, env) if al else []
                    if marker == "move" and i == 0 and isinstance(v, Closure):
                        # `move f(…)` consumes the closure (8.2): the call owns it (7.9, X-250; D-30)
                        closure = v
                        if closure.is_async:
                            v = self.call_value(closure, args)
                            v.consumes = closure             # the future owns it until its body has run
                        else:
                            v = self.consuming(closure, lambda: self.call_value(closure, args))
                    else:
                        v = self.call_value(v, args)
                    i += 2 if al else 1
                i += 1
            if self.in_test_body:
                v = self.implied_fail(v, reading.call_name_token(node))
            return v
        if n == "postfixExpr":
            # a test body's implied `or fail` (14) at each call the checker recorded: the primary's own call
            # (`Code.validate(s)`, D-308), then each postfix that makes a call, the last one or a receiver
            prim, posts = kids[0], kids[1:]
            v = self.eval(prim, env)
            if self.in_test_body:
                v = self.implied_fail(v, reading.primary_call_token(prim))
            for j, k in enumerate(posts):
                v = self.postfix(v, k, env)
                if self.in_test_body:
                    v = self.implied_fail(v, reading.postfix_call_token(posts, j, prim))
            return v
        if n == "primaryExpr":
            return self.primary(node, env)
        if n == "lambda":
            lp = tree.child(node, "lambdaParams")                 # the lambda's own, not a nested lambda's (D-34)
            params = [direct_tokens(p)[0].text for p in children(lp) if name(p) == "lambdaParam"] if lp else []
            owned_params = [direct_tokens(p)[0].text for p in children(lp) if name(p) == "lambdaParam" and param_owned(p)] if lp else []
            cl = tree.child(node, "captureList")
            cenv = env
            owned = []
            if cl is not None:
                cenv = Env(self.globals)                     # an explicit capture list scopes the body (8.3)
                for cap in [c for c in children(cl) if name(c) == "capture"]:
                    ck = children(cap)
                    if isinstance(ck[0], Token):
                        cenv.declare(ck[0].text, deep_copy(env.get(ck[0].text)))        # bare name: a trivial value, copied
                        owned.append(ck[0].text)
                        continue
                    marker = tokens(ck[0])[0].text
                    root = tokens(ck[1])[0].text
                    if marker in ("alias", "editable"):
                        cenv.vars[root] = env.cell(root)     # the closure holds the loan: the same cell
                        continue
                    if marker == "copy":
                        cenv.declare(root, deep_copy(env.get(root)))
                    else:
                        moved = self.eval_place(ck[1], env, move=True)
                        if self.is_lent(moved):
                            # a loan moved into the closure stays a loan: the closure holds it, and destroys nothing
                            # when it goes (7.9; D-338)
                            cenv.declare(root, moved, mutable=True, owns=False)
                            continue
                        cenv.declare(root, moved, mutable=True)
                    owned.append(root)                       # the closure owns a copy or moved value (7.9; D-30)
            words = [t.text for t in direct_tokens(node)]
            kind = "fn once" if "once" in words else ("fn mut" if "mut" in words else "fn")
            mods = tree.child(node, "modifiers")
            is_async = mods is not None and any(t.text == "async" for t in tokens(mods))
            is_try = mods is not None and any(t.text == "try" for t in tokens(mods))
            ptypes = [find(p, "paramType") for p in children(lp) if name(p) == "lambdaParam"] if lp else []
            ret = next((c for c in children(node) if name(c) == "type"), None)
            fn_tok = next((t for t in direct_tokens(node) if t.text == "fn"), tokens(node)[0])
            return Closure(params=params, body=tree.lambda_body(node), env=cenv, kind=kind, is_async=is_async,
                           ptypes=ptypes, ret=ret, owned=owned, owned_params=owned_params, label=f"lambda@{fn_tok.line}:{fn_tok.col}",
                           is_try=is_try)
        if n == "matchExpr":
            return self.match_expr(node, env)
        if n == "ifExpr":
            return self.if_expr(node, env)
        if n == "tupleLit":
            items = [self.eval(k, env) for k in kids if name(k) == "expr"]
            # an element the statement produced as a loan stays a loan: the tuple borrows it and owns the rest (D-349)
            lent = [i for i, x in enumerate(items) if self.is_lent(x)]
            for i, x in enumerate(items):
                if i not in lent:
                    self.release(x)                        # the tuple takes its components, even a field of a temporary (7.9)
            return self.fresh_add(LentTuple(items, lent) if lent else tuple(items))
        if n == "recordLit":
            out = {}
            ks = [k for k in kids if not (isinstance(k, Token) and k.text in ("{", "}", ",", ":", "\n")) and name(k) != "NL"]
            ks = [k for k in ks if not (isinstance(k, Token) and k.kind == "NL")]
            for i in range(0, len(ks), 2):
                key = ks[i].text if isinstance(ks[i], Token) else tokens(ks[i])[0].text
                out[key] = self.eval(ks[i + 1], env)
            lent = [k for k, x in out.items() if self.is_lent(x)]
            for k, x in out.items():
                if k not in lent:
                    self.release(x)                        # the record takes its fields, even a field of a temporary (7.9)
            return self.fresh_add(LentRecord(out, lent) if lent else out)
        if n == "rangeExpr":
            raise FortisError("range outside a for")
        if n == "place":
            return self.eval_place(node, env, move=False)
        raise FortisError(f"cannot evaluate {n}")

    def eval_left(self, node, env):
        """The left operand of an outcome operator. Integer arithmetic there is a checked region (6.4, X-345): each
        operator is the checked operation, and the first that fails ends the region with its failure, which is then the
        operand's value (a proved operator cannot fail, so checking it too changes nothing)."""
        ops = self.region_cache.get(id(node))
        if ops is None:
            ops = self.region_cache[id(node)] = tree.region_operators(node)
        if not ops:
            return self.eval(node, env)
        self.regions.append(ops)
        try:
            return self.eval(node, env)
        except RegionFailure as rf:
            return rf.failure
        finally:
            self.regions.pop()

    CHECKED = {"+": intrinsics.int_add, "-": intrinsics.int_sub, "*": intrinsics.int_mul, "/": intrinsics.int_div, "%": intrinsics.int_rem, "<<": intrinsics.int_shl}

    def checked(self, tok, a, b):
        """The checked operation of an operator in the current region, or None outside one (6.4, X-345)."""
        if not self.regions or (tok.line, tok.col) not in self.regions[-1] or tok.text not in self.CHECKED:
            return None
        if not isinstance(a, int) or not isinstance(b, int) or isinstance(a, bool) or isinstance(b, bool):
            return None
        r = self.CHECKED[tok.text](self, [a, b])
        if isinstance(r, Failure):
            raise RegionFailure(r)
        return r

    def chain(self, kids, env):
        v = self.eval(kids[0], env)
        for i in range(1, len(kids), 2):
            w = self.eval(kids[i + 1], env)
            r = self.checked(kids[i], v, w)
            v = r if r is not None else self.binary(kids[i].text, v, w)
        return v

    def binary(self, op, a, b):
        if op in ("==", "!="):
            eq = self.equal(a, b)
            return eq if op == "==" else not eq
        if op in ("<", "<=", ">", ">="):
            if isinstance(a, intrinsics.TimestampVal) and isinstance(b, intrinsics.TimestampVal):
                a, b = a.ns, b.ns
            if isinstance(a, bool) or isinstance(b, bool) or type(a) is not type(b) or not isinstance(a, (int, str, float, numeric.F32, numeric.Dec, numeric.Dur)):
                raise FortisError(f"`{op}` on non-comparable values")     # plug-in: typing (DEL-12)
            if isinstance(a, numeric.Dec):
                c = numeric.dec_cmp(a, b)
                return {"<": c < 0, "<=": c <= 0, ">": c > 0, ">=": c >= 0}[op]
            if isinstance(a, numeric.Dur):
                a, b = a.ns, b.ns
            if isinstance(a, numeric.F32):
                a, b = a.v, b.v
            return {"<": a < b, "<=": a <= b, ">": a > b, ">=": a >= b}[op]   # IEEE: every comparison with NaN is false
        if isinstance(a, float) and isinstance(b, float) and op in ("+", "-", "*", "/"):
            return numeric.f64_op(op, a, b)
        if isinstance(a, numeric.F32) and isinstance(b, numeric.F32) and op in ("+", "-", "*", "/"):
            return numeric.f32_op(op, a, b)
        if not isinstance(a, int) or not isinstance(b, int) or isinstance(a, bool) or isinstance(b, bool):
            raise FortisError(f"`{op}` on non-integer values")
        if op == "+": return check_int(a + b, "add")
        if op == "-": return check_int(a - b, "sub")
        if op == "*": return check_int(a * b, "mul")
        if op == "/":
            if b == 0: raise FortisError("division by zero (proof-required; use Int.div)")
            q = abs(a) // abs(b)
            return check_int(q if (a >= 0) == (b >= 0) else -q, "div")
        if op == "%":
            if b == 0: raise FortisError("remainder by zero")
            r = abs(a) % abs(b)
            return r if a >= 0 else -r
        if op == "<<": return check_int(a << b, "shiftLeft")
        if op == ">>": return a >> b
        if op == "&": return a & b
        if op == "|": return a | b
        if op == "^": return a ^ b
        raise FortisError(f"operator {op}")

    def equal(self, a, b) -> bool:
        if isinstance(a, EnumValue) and isinstance(b, EnumValue):
            return (a.enum == b.enum or "" in (a.enum, b.enum)) and a.variant == b.variant and all(self.equal(a.payload[k], b.payload.get(k)) for k in a.payload)
        if isinstance(a, Instance) and isinstance(b, Instance):
            if "equals" in a.cls.methods:
                head, body = a.cls.methods["equals"]
                return bool(self.call(head, body, [(None, b)], a))
            return a.cls is b.cls and all(self.equal(a.fields[k], b.fields.get(k)) for k in a.fields)
        if isinstance(a, ListVal) and isinstance(b, ListVal):
            return len(a.items) == len(b.items) and all(self.equal(x, y) for x, y in zip(a.items, b.items))
        if isinstance(a, bool) != isinstance(b, bool):
            return False
        if isinstance(a, numeric.Dec) and isinstance(b, numeric.Dec):
            return numeric.dec_cmp(a, b) == 0
        if isinstance(a, intrinsics.TimestampVal) and isinstance(b, intrinsics.TimestampVal):
            return a.ns == b.ns
        if isinstance(a, numeric.F32) and isinstance(b, numeric.F32):
            return a.v == b.v
        return a == b                     # floats: IEEE equality, NaN != NaN

    def literal(self, tok: Token, env):
        if tok.kind == "INT":
            t = tok.text.replace("_", "")
            return int(t, 0) if t[:2].lower() in ("0x", "0b", "0o") else int(t)
        if tok.kind == "STR":
            return self.interpolate(tok, env)
        if tok.kind == "RAW":
            return tok.text[3:-3]
        if tok.kind == "BYTES":
            return intrinsics.BytesVal(unescape_bytes(tok.text[2:-1]))     # `b"…"`: static ASCII bytes (3.5)
        if tok.kind == "KW":
            if tok.text == "true": return True
            if tok.text == "false": return False
            if tok.text == "null": return None
            if tok.text == "self": return self.read_binding(env, "self")
        if tok.kind == "LOWER":
            key = self.resolve(tok.text, env)
            if key in self.const_nodes:
                return self.const(key)
            try:
                return self.read_binding(env, tok.text)
            except FortisError:
                if self.lib_ctx:
                    v = self.library_name(self.lib_ctx[-1], tok.text)
                    if v is not None:
                        return v
                if key in self.funcs:
                    return FuncVal(*self.funcs[key], key)
                ref = self.module_alias(tok.text, env)
                if ref is not None:
                    return ref
                path = self.imported.get(tok.text)
                if path is not None and path.startswith("fortis."):
                    v = self.library_name(path[len("fortis."):], tok.text)
                    if v is not None:
                        return v
                v = self.library_name("prelude", tok.text)
                if v is not None:
                    return v
                raise
        if tok.kind == "DEC":
            return self.numeric_call(numeric.Dec.parse, tok.text)
        if tok.kind == "FLOAT":
            t = tok.text.replace("_", "")
            if t.endswith("f32"):
                return numeric.F32.of(float(t[:-3]))
            return float(t)
        if tok.kind == "DUR":
            return self.numeric_call(numeric.Dur.parse, tok.text)
        raise FortisError(f"literal {tok}")

    def numeric_call(self, fn, *args):
        """Run an intrinsic; its Failure becomes a Failure value (Outcome) for `or fail` / `catch`."""
        try:
            return fn(*args)
        except numeric.Failure as f:
            return Failure(f.code, f.payload)

    def interpolate(self, tok: Token, env) -> str:
        body = tok.text[1:-1]
        out, i = [], 0
        while i < len(body):
            c = body[i]
            if c == "\\":
                nxt = body[i + 1]
                out.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\", "$": "$"}.get(nxt, nxt))
                i += 2
            elif body.startswith("${", i):
                depth, j = 1, i + 2
                while depth:
                    if body[j] == "{": depth += 1
                    elif body[j] == "}": depth -= 1
                    j += 1
                src = body[i + 2:j - 1]
                node = getattr(tok, "interp_trees", {}).get(src)      # linked: parsed with its program keys (X-136)
                if node is None:
                    if src not in self.interp_cache:
                        self.interp_cache[src] = self.rec.parse(lexer.lex(src), "expr")
                    node = self.interp_cache[src]
                out.append(self.show(self.eval(node, env)))
                i = j
            else:
                out.append(c)
                i += 1
        return "".join(out)

    def show(self, v) -> str:
        """A value's display form, as interpolation writes it (3.6): a class or enum whose type declares its own
        `display` (Displayable, 11.1) runs it into a fresh Builder; a class or enum without one shows its fields, and a
        list its items, each the same way; anything else as `display` shows it (D-83)."""
        if isinstance(v, SharedVal):
            return self.show(v.box.value)
        if isinstance(v, Instance) and v.cls.name != "Scalar":
            if "display" in v.cls.methods and self.displayable_impl(v.cls.methods["display"]):
                return self.run_display(v, v.cls.methods["display"])
            return f"{v.cls.name}(" + ", ".join(f"{k}: {self.show(x)}" for k, x in v.fields.items()) + ")"
        if isinstance(v, EnumValue):
            info = self.enums.get(v.enum)
            if info is not None and "display" in info.methods and self.displayable_impl(info.methods["display"]):
                return self.run_display(v, info.methods["display"])
            if not v.payload:
                return v.variant
            return f"{v.variant}(" + ", ".join(f"{k}: {self.show(x)}" for k, x in v.payload.items()) + ")"
        if isinstance(v, ListVal):
            return "[" + ", ".join(self.show(x) for x in v.items) + "]"
        return display(v)

    @staticmethod
    def displayable_impl(method) -> bool:
        """Whether a method is Displayable's `display(self, into: editable Builder) Void` (11.1), not merely a method of
        that name."""
        head = method[0]
        sig = find(head, "funcSignature")
        pl = find(sig, "paramList") if sig is not None else None
        ps = [p for p in children(pl) if name(p) == "param" and find(p, "selfParam") is None] if pl is not None else []
        if len(ps) != 1:
            return False
        sp = find(head, "selfParam")
        mods = find(head, "modifiers")
        if sp is None or [t.text for t in tokens(sp)] != ["self"] or (mods is not None and any(t.text in ("async", "try") for t in tokens(mods))):
            return False
        pt = find(ps[0], "paramType")
        rt = next((c for c in children(sig) if name(c) == "returnType"), None) if sig is not None else None
        ret = "Void" if rt is None else "".join(t.text for t in tokens(rt))     # no return type: Void (8.1, X-345)
        return pt is not None and any(t.text == "editable" for t in direct_tokens(pt)) and tokens(pt)[-1].text == "Builder" and ret == "Void"

    def run_display(self, v, method) -> str:
        head, body = method[0], method[1]
        into = BuilderVal()
        self.call(head, body, [(None, into)], v)
        return "".join(into.parts)

    # ---- places, fields, members ------------------------------------------------
    def eval_place(self, place, env: Env, move: bool):
        return self.eval_place_tokens(tokens(place), env, move)

    def eval_place_tokens(self, ts, env: Env, move: bool):
        root = ts[0].text
        if root == "self":
            v = self.read_binding(env, "self")
        else:
            cell = env.cell(root)
            if cell.moved:
                raise FortisError(f"`{root}` was moved (use after move)")
            v = cell.value
            if move and len(ts) == 1:
                cell.moved = True                                     # plug-in: loans/moves (DEL-14)
            if not cell.owns:
                self.lend(v)                                          # moved or read, a loan stays a loan (D-338)
        for t in ts[1:]:
            if t.text in (".", "[", "]"):
                continue
            if t.kind == "INT":
                v = self.through(v, self.index(v, int(t.text)))
            else:
                v = self.through(v, self.field(v, t.text))
        if move and len(ts) > 1:
            raise FortisError("partial move (E-OWN-040)")
        return v

    def read_binding(self, env: Env, n: str):
        """A binding's value. Read through a binding that holds a loan, it is that loan (7.3; D-338)."""
        c = env.cell(n)
        if c.moved:
            raise FortisError(f"`{n}` was moved (use after move)")
        if not c.owns:
            self.lend(c.value)
        return c.value

    def through(self, base, part):
        """A field or element read through a loan is a loan of that part (7.3; D-338), and so is an element a tuple or
        a record borrows (D-349)."""
        if self.is_lent(base) or (lent_parts(base) and any(base_part is part for base_part in self.borrowed(base))):
            self.lend(part)
        return part

    @staticmethod
    def borrowed(v):
        """The parts an aggregate borrows rather than owns (D-349)."""
        lent = lent_parts(v)
        if isinstance(v, tuple):
            return [x for i, x in enumerate(v) if i in lent]
        if isinstance(v, dict):
            return [x for k, x in v.items() if k in lent]
        return []

    def field(self, v, fname):
        if isinstance(v, ModuleRef):
            return self.module_member(v, fname)
        if isinstance(v, TypeRefVal):
            return self.type_dispatch(v.name, fname, [], False, v.bounds)
        if isinstance(v, SharedVal):
            return self.field(v.box.value, fname)              # reading through a Shared is a read loan (7.8)
        if isinstance(v, Instance):
            if fname in v.fields:
                return v.fields[fname]
            return BoundMember(v, fname)
        if isinstance(v, dict):
            return v[fname]
        if isinstance(v, EnumValue) and fname in v.payload:
            return v.payload[fname]
        if isinstance(v, Failure):
            if fname == "code": return v.code
            return v.payload[fname]
        if isinstance(v, Report) and fname == "code":
            return v.code
        if isinstance(v, tuple) and fname.isdigit():
            return v[int(fname)]
        return BoundMember(v, fname)

    def index(self, v, i):
        if isinstance(v, ListVal):
            if not isinstance(i, int) or i < 0 or i >= len(v.items):
                raise FortisError(f"index {i} out of range 0..{len(v.items)} (proof-required)")   # plug-in: refinements (DEL-13)
            return v.items[i]
        if isinstance(v, tuple):
            return v[i]
        raise FortisError("indexing a non-list")

    def postfix(self, v, k, env):
        kids = children(k)
        first = kids[0]
        if isinstance(first, Token) and first.text == ".":
            second = kids[1]
            if isinstance(second, Token) and second.kind == "INT":
                return self.through(v, self.index(v, int(second.text)))
            return self.through(v, self.field(v, tokens(second)[0].text))
        if isinstance(first, Token) and first.text == "?.":
            return None if v is None else self.through(v, self.field(v, tokens(kids[1])[0].text))
        if name(first) == "methodCall":
            return self.method_call(v, first, env)
        if isinstance(first, Token) and first.text == "(":
            al = next((c for c in kids if name(c) == "argList"), None)
            return self.call_value(v, self.args(al, env) if al else [])
        if isinstance(first, Token) and first.text == "[":
            return self.index(v, self.eval(kids[1], env))
        raise FortisError("postfix")

    def args(self, al, env, target: Optional[ClassInfo] = None) -> List[Tuple[Optional[str], Any]]:
        nodes = [c for c in children(al) if name(c) in ("arg", "constArg")]    # a constant call's arguments too (D-22)

        def is_spread(a) -> bool:
            ks = children(a)
            return bool(ks) and isinstance(ks[0], Token) and ks[0].text == "..."

        # a bare local name is `name: name` where a positional argument cannot stand: after a named argument, and in a
        # class's generated constructor (5.8, 9.2, X-345)
        puns = set()
        seen_named = False
        for i, a in enumerate(nodes):
            ks = children(a)
            if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                seen_named = True
            elif (seen_named or target is not None) and tree.pun_name(a) is not None:
                puns.add(i)
                seen_named = True

        def eval_one(a):
            ks = children(a)
            if isinstance(ks[0], Token) and ks[0].text == "...":
                # a spread is a whole source value; the constructor takes from it only the fields its class has (9.2)
                marker = ks[1].text
                v = self.eval_place(ks[2], env, move=(marker == "move"))
                source = v.fields if isinstance(v, Instance) else v if isinstance(v, dict) else {}
                if marker == "move" and target is not None:
                    self.drop_untaken(source, target)                 # the spread runs before the explicit fields (9.2)
                return (SPREAD, (source, marker == "copy", target is not None))
            if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                return (ks[0].text, self.eval(ks[2], env))
            return (None, self.eval(ks[0], env))

        # 9.2: a constructor evaluates the spread first, then the explicit fields, left to right. The result
        # stays in source order so each named field is still assigned by name.
        order = list(range(len(nodes)))
        if target is not None and any(is_spread(a) for a in nodes):
            order = [i for i, a in enumerate(nodes) if is_spread(a)] + [i for i, a in enumerate(nodes) if not is_spread(a)]
        out: List[Any] = [None] * len(nodes)
        for i in order:
            out[i] = eval_one(nodes[i])
            if i in puns:
                out[i] = (tree.pun_name(nodes[i]).text, out[i][1])
        return out

    # ---- the canonical codec (6.10) ------------------------------------------------
    INT_RANGES = {"Int": (-(2 ** 63), 2 ** 63 - 1), "Int64": (-(2 ** 63), 2 ** 63 - 1), "Int32": (-(2 ** 31), 2 ** 31 - 1),
                  "Int16": (-(2 ** 15), 2 ** 15 - 1), "Int8": (-128, 127), "UInt": (0, 2 ** 64 - 1), "UInt64": (0, 2 ** 64 - 1),
                  "UInt32": (0, 2 ** 32 - 1), "UInt16": (0, 2 ** 16 - 1), "UInt8": (0, 255), "Byte": (0, 255)}
    SIMPLE = {"Bool": ("bool",), "Float": ("float",), "Float32": ("float32",), "Decimal": ("decimal",), "Scalar": ("scalar",),
              "Bytes": ("bytes",), "Duration": ("duration",), "Timestamp": ("timestamp",)}

    def type_desc(self, node, tenv: Optional[Dict[str, Any]] = None, depth: int = 0):
        """The codec descriptor of a `type` or `typeRef` node (6.10); tenv maps type parameters to descriptors."""
        tenv = tenv or {}
        if depth > 40:
            raise FortisError("a recursive type without indirection is outside the codec")
        if name(node) == "type":
            kids = children(node)
            if kids and name(kids[0]) in ("fnType", "loanType"):
                raise FortisError("closures and loans are not Encodable")
            nt = kids[0]
            d = self.type_desc(nt, tenv, depth + 1)
            return ("opt", d) if any(isinstance(k, Token) and k.text == "?" for k in kids) else d
        if name(node) == "namedType":
            kids = children(node)
            if name(kids[0]) == "typeRef":
                return self.type_desc(kids[0], tenv, depth + 1)
            if isinstance(kids[0], Token) and kids[0].text == "(":
                inner = [k for k in kids if name(k) == "type"]
                return self.type_desc(inner[0], tenv, depth + 1) if len(inner) == 1 else ("tuple", [self.type_desc(t, tenv, depth + 1) for t in inner])
            if isinstance(kids[0], Token) and kids[0].text == "{":
                fl = find(node, "fieldList")
                fks = children(fl)
                return ("record", [(fks[i].text, self.type_desc(fks[i + 2], tenv, depth + 1)) for i in range(len(fks)) if isinstance(fks[i], Token) and fks[i].kind == "LOWER" and i + 2 < len(fks) and name(fks[i + 2]) == "type"])
        # typeRef: [alias "."] Name [typeArgs] [refinement]
        ts = direct_tokens(node)
        tname = next(t.text for t in ts if t.kind == "UPPER")
        ta = find(node, "typeArgs")
        args = [self.type_desc(find(a, "type"), tenv, depth + 1) if find(a, "type") is not None else ("n", int(tokens(a)[0].text))
                for a in children(ta) if name(a) == "typeArg"] if ta is not None else []
        ref = own_refinement(node)                      # the type's own: `List[Int<0..9>]` is not length-refined
        if tname in tenv:
            return tenv[tname]
        if tname in self.INT_RANGES:
            lo, hi = self.INT_RANGES[tname]
            rlo = rhi = None
            if ref is not None and not any(t.text == "scalars" for t in tokens(ref)):
                rlo, rhi = self.range_bounds(ref, self.globals)
            return ("int", lo, hi, rlo, rhi)
        if tname in self.SIMPLE:
            return self.SIMPLE[tname]
        if tname in ("String", "StringView"):
            lo = hi = None
            if ref is not None:
                lo, hi = self.range_bounds(ref, self.globals)
            return ("string", lo, hi)
        if tname in ("List", "ListView", "Set", "Map"):
            d = ("list", args[0]) if tname in ("List", "ListView") else ("set", args[0]) if tname == "Set" else ("map", args[0], args[1])
            if ref is not None:                             # a length refinement (List[T]<length: 0..100>) checks the count
                lo, hi = self.range_bounds(ref, self.globals)
                return ("sized", d, lo, hi)
            return d
        if tname == "Array": return ("array", args[0], args[1][1])
        if tname in self.newtypes:
            rep = self.type_desc(self.newtype_repr[tname], tenv, depth + 1)
            lo, hi = (self.range_bounds(self.newtypes[tname], self.globals) if self.newtypes[tname] is not None else (None, None))
            return ("newtype", tname, rep, lo, hi)
        if tname in self.classes:
            cls = self.classes[tname]
            env2 = dict(zip(cls.type_params, args))
            return ("class", cls, {f: self.type_desc(t, env2, depth + 1) for f, t in cls.field_types.items()})
        if tname in self.enums:
            en = self.enums[tname]
            env2 = dict(zip(en.type_params, args))
            return ("enum", en, {v: [(f, self.type_desc(t, env2, depth + 1)) for f, t in en.payload_types.get(v, [])] for v in en.variants})
        raise FortisError(f"{tname} is not Decodable")

    def class_default(self, cls, field_name: str):
        node = cls.fields.get(field_name)
        return None if node is None else self.eval(node, self.globals)

    def decode(self, which: str, tr, data):
        d = self.type_desc(tr)
        try:
            if which == "fromText":
                return codec.from_text(d, data, self)
            return codec.from_bytes(d, data.data if isinstance(data, intrinsics.BytesVal) else data, self)
        except codec.Invalid as e:
            return Failure(e.code, {"detail": e.detail}, origin=self.stack[-1] if self.stack else "main")

    def library_name(self, module: str, n: str):
        """A module-level name of a core library module: a Fortis body, an intrinsic, or an import of it."""
        entry = self.lib.get(module)
        if entry is None:
            return None
        if n in entry["funcs"]:
            return FuncVal(*entry["funcs"][n], n, module=module)
        if n in intrinsics.FUNCS:
            return BoundMember(TypeRefVal(n), n)
        src = entry["imports"].get(n)
        if src is not None and src != module:
            return self.library_name(src, n)
        return None

    def call_library(self, module: str, head, body, args, self_value):
        self.lib_ctx.append(module)
        try:
            return self.call(head, body, args, self_value)
        finally:
            self.lib_ctx.pop()

    def library_method(self, tname: str, mname: str):
        for mod in LIBRARY_MODULES:
            hb = self.lib[mod]["methods"].get((tname, mname))
            if hb is not None:
                return mod, hb
        return None

    def call_value(self, v, args):
        if isinstance(v, BoundMember):
            return self.invoke_method(v.receiver, v.member, args)
        if isinstance(v, FuncVal):
            if v.module is not None:
                return self.call_library(v.module, v.head, v.body, args, None)
            return self.call(v.head, v.body, args, None, self.func_aspects.get(v.fname))
        if isinstance(v, TypeRefVal):
            return self.type_dispatch(v.name, None, args, True, v.bounds)
        if isinstance(v, Closure):
            env = Env(v.env)
            env.frame = True
            for i, (pn, (_, av)) in enumerate(zip(v.params, args)):
                env.declare(pn, self.expected(av, v.ptypes[i] if i < len(v.ptypes) else None), owns=pn in v.owned_params)
            if v.is_async:
                # an async lambda's call binds its arguments and returns a Future; its body runs when polled (12.3)
                fut = FutureVal("lambda", None, None, None, False)
                fut.closure = Closure(params=[], body=v.body, env=env, kind=v.kind, is_async=False,
                                      owned_params=v.owned_params, label=v.label, is_try=v.is_try)
                return fut
            body = v.body
            blk = find(body, "block") if name(children(body)[0]) == "block" else None
            defect = False
            in_test, self.in_test_body = self.in_test_body, False  # a lambda's body is not the test's (14)
            self.writers.append((v.label, len(self.stack)))        # a log record in the body names the lambda (13; D-323)
            try:
                if blk is not None:
                    self.run_block(blk, env)
                    return VOID
                rv = self.eval(find(body, "expr"), env)
                self.returned_lent = self.is_lent(rv)
                return self.call_result(self.expected(rv, v.ret), v.ret)
            except ReturnSignal as r:
                self.returned_lent = r.lent
                return self.call_result(self.expected(r.value, v.ret), v.ret)
            except FailureSignal as f:
                if v.is_try:
                    return f.failure                         # a `try fn` lambda's failure is its call's outcome (10.2)
                raise
            except DefectSignal:
                defect = True                                # a defect runs no cleanup (10.8)
                raise
            finally:
                self.in_test_body = in_test
                try:
                    if not defect:
                        # the owned parameters are the call's oldest values: destroyed last, unless moved (7.9; D-152)
                        for pn in reversed(v.owned_params):
                            cell = env.cell(pn)
                            if not cell.moved:
                                self.announce_drop(cell.value)
                                cell.moved = True
                                self.destroy(cell.value)
                finally:
                    self.writers.pop()
        # a payload-free variant used as its constructor (`let f: fn() Coin = Coin.Penny`) is the value itself;
        # calling it yields that value (LR 9.7). `let c: Coin = Coin.Penny` never comes through here.
        if isinstance(v, EnumValue) and not v.payload and not args:
            return v
        raise FortisError(f"call of a non-callable: {display(v)}")

    def method_call(self, v, mc, env):
        mname = tokens(find(mc, "memberName"))[0].text
        al = find(mc, "argList")
        args = self.args(al, env) if al else []
        return self.invoke_method(v, mname, args)

    def variant_name(self, v) -> str:
        """A payload-free variant of a prelude enum reaches a builtin as a bound member (RoundingMode.HalfEven)."""
        if isinstance(v, EnumValue): return v.variant
        if isinstance(v, BoundMember): return v.member
        raise FortisError("expected an enum variant")

    def negate(self, v):
        if isinstance(v, numeric.Dec): return numeric.Dec(not v.neg and v.coeff != 0, v.coeff, v.scale)
        if isinstance(v, numeric.F32): return numeric.F32.of(-v.v)
        if isinstance(v, numeric.Dur): return self.numeric_call(numeric.Dur.make, -v.ns)
        if isinstance(v, bool) or not isinstance(v, (int, float)): raise FortisError("unary minus on a non-numeric value")
        return -v

    def invoke_method(self, v, mname, args):
        if isinstance(v, ModuleRef):
            return self.call_value(self.module_member(v, mname), args)
        pos = [a for _, a in args]
        if mname in ("toText", "toBytes"):
            # the canonical pair (6.10), never overridable
            return codec.to_text(v) if mname == "toText" else intrinsics.BytesVal(codec.to_bytes(v, self))
        tn = intrinsics.type_name(v)
        if tn is not None:
            for t in (tn,) + intrinsics.ALIASES.get(tn, ()):
                lm = self.library_method(t, mname)
                if lm is not None:
                    return self.call_library(lm[0], lm[1][0], lm[1][1], args, v)   # a Fortis body of the core (DEL-26)
            fn = intrinsics.lookup_method(v, mname)
            if fn is not None and tn != "Scope":
                return self.intrinsic_ownership(tn, mname, pos, fn(self, v, pos))
        if isinstance(v, SharedVal):
            return self.invoke_method(v.box.value, mname, args)          # reading through a Shared is a read loan (7.8)
        if isinstance(v, ScopeVal):
            pos = [a for _, a in args]
            if mname in ("launch", "launchParallel", "launchBlocking"):
                work = pos[0]
                if isinstance(work, Closure):
                    work = FutureVal("work", None, None, work, False)
                    work.closure = pos[0]
                    work.consumes = pos[0]                   # the task owns its work (7.9, X-250; D-30)
                return self.sched.launch(work, v, {"launch": "cooperative", "launchParallel": "parallel", "launchBlocking": "blocking"}[mname])
            if mname == "detach":
                t = pos[0]
                t.detached = True
                if t in v.tasks:
                    v.tasks.remove(t)
                self.sched.detach(t)
                self.trace.append(f"detach {t.name}")
                return VOID
        if isinstance(v, TypeRefVal):
            # a core member kept as a value (`clock.Duration.seconds`, `money.Currency.GBP`) stays the core
            # member. A program class or enum of the same bare name is not it (D-142).
            if v.core:
                cen = self.core_enums.get(v.name)
                if cen is not None and mname in cen.variants:
                    if not cen.variants[mname] and not args:
                        return self.variant(cen, mname, [])
                    return self.variant(cen, mname, args)
                return self.builtin(v.name, mname, args, True)
            return self.type_dispatch(v.name, mname, args, True, v.bounds)
        pos = [a for _, a in args]
        if isinstance(v, Instance):
            if mname in v.cls.methods:
                head, body = v.cls.methods[mname]
                return self.call(head, body, args, v)
            if v.cls.name == "Capabilities":
                raise FortisError("capability field access, not a method")
        if isinstance(v, EnumValue) and v.enum == "":
            owners = [en for en in self.enums.values() if v.variant in en.variants and mname in en.methods]
            if len(owners) == 1:
                v.enum = owners[0].name
        if isinstance(v, EnumValue) and v.enum in self.enums and mname in self.enums[v.enum].methods:
            head, body = self.enums[v.enum].methods[mname]
            return self.call(head, body, args, v)
        if isinstance(v, Capability):
            raise FortisError(f"capability {v.kind}.{mname} has no development adapter")
        if isinstance(v, Failure) and mname == "code":
            return v.code
        raise FortisError(f"no method `{mname}` on {display(v)} in the kernel")

    # ---- primary expressions ----------------------------------------------------
    def primary(self, node, env):
        kids = children(node)
        first = kids[0]
        if isinstance(first, Token):
            if first.text == "(":
                return self.eval(kids[1], env)
            if first.text == "present":
                return self.eval(kids[2], env)
            if first.text == "fail":
                v = self.eval(kids[1], env)
                if not isinstance(v, Failure):
                    raise FortisError("fail of a non-failure value")
                raise FailureSignal(v)
            return self.literal(first, env)
        n = name(first)
        if n == "typeRef":
            return self.type_member(kids, env)
        if n in ("literal",):
            return self.literal(children(first)[0], env)
        return self.eval(first, env)

    def type_member(self, kids, env):
        tr = kids[0]
        trs = direct_tokens(tr)
        via_core = None
        if trs[0].kind == "LOWER":                     # module alias: alias.Type
            alias, tname = trs[0].text, trs[2].text
            tname = alias + "." + tname
            type_tok_text = trs[2].text
            mod = self.imported.get(alias)
            # `import fortis.host as platform`, then `platform.Host`: the core type, not a program type of the
            # same bare name. The two share the empty identity, so a bare lookup cannot tell them apart (D-142).
            if isinstance(mod, str) and (mod == "fortis" or mod.startswith("fortis.")):
                via_core = mod
        else:
            tname = trs[0].text
            type_tok_text = tname
        member = None
        idx = 1
        if idx < len(kids) and isinstance(kids[idx], Token) and kids[idx].text == ".":
            member = kids[idx + 1].text if isinstance(kids[idx + 1], Token) else tokens(kids[idx + 1])[0].text
            idx += 2
        args = []
        called = False
        if idx < len(kids) and isinstance(kids[idx], Token) and kids[idx].text == "(":
            called = True
            al = next((c for c in kids[idx:] if name(c) in ("argList", "constArgList")), None)
            args = self.args(al, env, self.classes.get(type_tok_text) if member is None else None) if al else []
        refinement = own_refinement(tr)                  # the type's own, not one of its arguments' (`List[Int<0..9>]`)
        bounds = self.refinement(refinement, env) if refinement is not None else None
        ta = find(tr, "typeArgs")
        if type_tok_text == "Array" and ta is not None:
            ints = [t for t in tokens(ta) if t.kind == "INT"]
            self.array_n = int(ints[-1].text.replace("_", "")) if ints else None    # Array[T, N]: the size from the type
        if member is None and not called:
            if refinement is None and ta is None and trs[0].kind != "LOWER" and self.declares_variant(type_tok_text):
                return self.unprefixed_variant(type_tok_text, [])          # `return Pound` where a `Coin` is expected (9.7)
            return TypeRefVal(type_tok_text, bounds)
        if member in ("fromText", "fromBytes") and called:
            return self.decode(member, tr, args[0][1])              # the canonical decoders (6.10)
        # a program method must not shadow a core intrinsic reached through the alias (D-142)
        if via_core is not None and member is not None and (
                intrinsics.ASSOC.get((type_tok_text, member)) is not None or self.library_method(type_tok_text, member) is not None):
            if not called:
                return BoundMember(TypeRefVal(type_tok_text, bounds, core=True), member)
            return self.builtin(type_tok_text, member, args, called)
        # a core enum's variant, not a program enum of the same bare name (`money.Currency.GBP`)
        if via_core is not None and member is not None:
            cen = self.core_enums.get(type_tok_text)
            if cen is not None and member in cen.variants:
                if called or not cen.variants[member]:
                    return self.variant(cen, member, args if called else [])
                return BoundMember(TypeRefVal(type_tok_text, bounds, core=True), member)
        return self.type_dispatch(type_tok_text, member, args, called, bounds)

    def type_dispatch(self, type_tok_text, member, args, called, bounds=None):
        owner = self.classes.get(type_tok_text) or self.enums.get(type_tok_text)
        if member is not None and not called and owner is not None and member in owner.consts:
            return self.associated_const(owner, member)                 # `Grid.size` is the constant's value (9.3)
        if member is not None and not called:
            # a variant without payload is a value; a variant with payload, and any function, waits for its call
            if type_tok_text in self.enums and member in self.enums[type_tok_text].variants:
                if not self.enums[type_tok_text].variants[member]:
                    return self.variant(self.enums[type_tok_text], member, [])
                return BoundMember(TypeRefVal(type_tok_text, bounds), member)
            if type_tok_text in self.enum_variant_owner() and member is None:
                pass
            return BoundMember(TypeRefVal(type_tok_text, bounds), member)
        # refinement validate (6.3): `Int<lo..hi>.validate(x)`, `String<scalars: …>`, `List[T]<length: …>`, `Bytes<length: …>`
        if bounds is not None and member == "validate":
            return self.validate(args[0][1], bounds)
        if type_tok_text in self.newtypes:
            # a newtype's value is its representation; `Name.validate(raw)` checks the representation's refinement,
            # `Name(lit)` is proved statically (6.9); a representation without a refinement accepts every value
            x = args[0][1] if args else None
            if member == "validate":
                ref = self.newtypes[type_tok_text]
                return x if ref is None else self.validate(x, self.refinement(ref, self.globals))
            if member is None and called:
                return x
        if type_tok_text in self.classes:
            cls = self.classes[type_tok_text]
            if member is None:
                return self.construct(cls, args)
            if member in cls.methods and cls.methods[member][1] is not None:
                head, body = cls.methods[member]
                return self.call(head, body, args, None)
            fn = intrinsics.ASSOC.get((cls.name, member))
            if fn is not None:
                # a trusted intrinsic of a library class (Money.multiply)
                return self.intrinsic_ownership(cls.name, member, [v for _, v in args], fn(self, [v for _, v in args]))
            if member == "default" and called and self.derives(cls, "Default"):
                return self.derived_default(cls)               # `derive Default`: every field at its initialiser (9.2; D-170)
            raise FortisError(f"{cls.name} has no associated function `{member}`")
        if type_tok_text in self.enums:
            en = self.enums[type_tok_text]
            if member in en.variants:
                return self.variant(en, member, args)
            if member is None:
                raise FortisError(f"enum {en.name} used as a value")
            if member in en.methods:
                head, body = en.methods[member]
                return self.call(head, body, args, None)
            raise FortisError(f"{en.name} has no variant or associated function `{member}`")
        if type_tok_text in self.errors:
            e = self.errors[type_tok_text]
            payload = {}
            named = {k: v for k, v in args if k is not None}
            for i, (k, v) in enumerate(args):
                if k is None and i < len(e.payload):
                    payload[e.payload[i]] = v
            payload.update(named)
            return Failure(e.code, payload, origin=self.stack[-1] if self.stack else "main", message=e.message or "")
        if member is None and self.declares_variant(type_tok_text):
            return self.unprefixed_variant(type_tok_text, args if called else [])
        return self.builtin(type_tok_text, member, args, called)

    def enum_variant_owner(self) -> Dict[str, EnumInfo]:
        return {v: en for en in self.enums.values() for v in en.variants}

    def declares_variant(self, n: str) -> bool:
        """Some enum declares `n` as a variant. Written bare, without a call, it is that variant even when a type has
        the same name: a type is no value, and the checker has placed it where the enum is expected (9.7)."""
        return any(n in en.variants for en in self.enums.values())

    def open_names(self) -> set:
        """The variant names an unprefixed use cannot resolve without its expected type: those several enums declare,
        and those that are also a type's name. Empty for most programs, which makes `expected` free."""
        if self._open_names is None:
            count: Dict[str, int] = {}
            for en in self.enums.values():
                for vn in en.variants:
                    count[vn] = count.get(vn, 0) + 1
            types = set(self.classes) | set(self.enums) | set(self.newtypes) | set(self.errors)
            self._open_names = {vn for vn, c in count.items() if c > 1 or vn in types}
        return self._open_names

    def unprefixed_variant(self, vname: str, args):
        """A variant written without its enum's prefix where that enum is the expected type (9.7). The checker has
        resolved it; the interpreter keeps no types, so it takes the one enum that declares the variant. When several
        do, the enum stays open (`""`), its payload unresolved, until a typed position names it (`expected`); a
        `match` arm and `==` compare the variant alone, which a checked program makes sufficient."""
        owners = [en for en in self.enums.values() if vname in en.variants]
        if len(owners) == 1:
            return self.variant(owners[0], vname, args)
        fieldnames = owners[0].variants[vname]
        ev = EnumValue("", vname, {k if k is not None else fieldnames[i]: v for i, (k, v) in enumerate(args)})
        for _, v in args:
            self.release(v)                                # the payload values move into the variant (7.9)
        return self.fresh_add(ev)

    def expected(self, v, type_node, tenv: Optional[Dict[str, Any]] = None):
        """`v` where the declared type `type_node` is expected (a return, an annotated let, an argument, a field, a
        payload, a lambda's parameter or result): an open unprefixed variant takes the enum the type names, through
        collections, optionals, tuples, records, payloads, newtypes and a generic class's fields, with the type's
        arguments for its parameters (`tenv`: parameter -> (type node, its own tenv)); a variant value of a class's
        name where a function is expected is that class's constructor, and a value of a class named like one of the
        enum's variants is that variant (9.7). An enum value is never changed in place: the same value may be bound
        at another type."""
        if type_node is None or v is None or not self.open_names():
            return v
        t = type_node if name(type_node) == "type" else find(type_node, "type")
        if t is None:
            return v
        if name(children(t)[0]) == "fnType":
            if isinstance(v, EnumValue) and not v.payload and v.variant in self.classes:
                return TypeRefVal(v.variant)                  # `let make: fn(Int) Green = Green`: the constructor
            return v
        nt = find(t, "namedType")
        if nt is None:
            return v
        first = children(nt)[0]
        if name(first) == "typeRef":
            tname = next((x.text for x in direct_tokens(first) if x.kind == "UPPER"), "")
            ta = next((c for c in children(first) if name(c) == "typeArgs"), None)
            targs = [find(a, "type") for a in children(ta) if name(a) == "typeArg"] if ta is not None else []
            if tenv and tname in tenv:
                node, outer = tenv[tname]
                return self.expected(v, node, outer)
            en = self.enums.get(tname)
            if en is not None:
                if isinstance(v, Instance) and v.cls.name != tname and v.cls.name in en.variants:
                    v = EnumValue(en.name, v.cls.name, dict(v.fields))
                if isinstance(v, EnumValue) and v.variant in en.variants and v.enum in ("", en.name):
                    inner = {p: (a, tenv) for p, a in zip(en.type_params, targs)}
                    ptypes = dict(en.payload_types.get(v.variant, []))
                    payload = {k: self.expected(x, ptypes.get(k), inner) for k, x in v.payload.items()}
                    if v.enum == "" or any(payload[k] is not v.payload[k] for k in payload):
                        v = EnumValue(en.name, v.variant, payload)
                return v
            cls = self.classes.get(tname)
            if isinstance(v, Instance) and cls is not None and v.cls is cls and cls.type_params and targs:
                inner = {p: (a, tenv) for p, a in zip(cls.type_params, targs)}
                for fname, ftype in cls.field_types.items():
                    if fname in v.fields:
                        v.fields[fname] = self.expected(v.fields[fname], ftype, inner)
            elif isinstance(v, ListVal) and targs:
                v.items[:] = [self.expected(x, targs[0], tenv) for x in v.items]
            elif isinstance(v, intrinsics.SetVal) and targs:
                for k in list(v.items):
                    v.items[k] = self.expected(v.items[k], targs[0], tenv)
            elif isinstance(v, intrinsics.MapVal) and len(targs) >= 2:
                for k, (kv, xv) in list(v.entries.items()):
                    v.entries[k] = (self.expected(kv, targs[0], tenv), self.expected(xv, targs[1], tenv))
            elif tname in self.newtype_repr:
                return self.expected(v, self.newtype_repr[tname], tenv)
            return v
        if isinstance(v, tuple):
            subs = [c for c in children(nt) if name(c) == "type"]
            return retuple(v, [self.expected(x, s, tenv) for x, s in zip(v, subs)]) if len(subs) == len(v) else v
        if isinstance(v, dict):
            fl = find(nt, "fieldList")
            fks = children(fl) if fl is not None else []
            for i in range(len(fks) - 2):
                if isinstance(fks[i], Token) and fks[i].kind == "LOWER" and fks[i].text in v and name(fks[i + 2]) == "type":
                    v[fks[i].text] = self.expected(v[fks[i].text], fks[i + 2], tenv)
        return v

    def refinement(self, refinement, env) -> "Refinement":
        """A refinement's bounds and what it measures: None for a range of the value itself (`Int<0..100>`), or the
        measure it names (`scalars`, `length`) for a text, byte or collection type (6.3)."""
        lo, hi = self.range_bounds(refinement, env)
        ts = direct_tokens(find(refinement, "refineBody"))
        return Refinement(lo, hi, ts[0].text if ts and ts[0].kind == "LOWER" else None)

    def validate(self, x, r: "Refinement"):
        """`T.validate(x)` (6.3): `x`, or `refinement.out_of_range` with the measured value and the bounds, as the VM
        and native build it. A range refines an integer; `scalars` counts a text's Unicode scalars and `length` the
        elements of a list or array or the bytes of a byte string."""
        if r.measure is None:
            if not isinstance(x, int) or isinstance(x, bool):
                raise FortisError("validate of a non-integer")       # plug-in: typing (DEL-12)
            n = x
        elif isinstance(x, str):
            n = len(x)                                                # a Python str is a sequence of Unicode scalars
        elif isinstance(x, (ListVal, intrinsics.ArrayVal)):
            n = len(x.items)
        elif isinstance(x, intrinsics.BytesVal):
            n = len(x.data)
        else:
            raise FortisError(f"validate of a {r.measure} refinement on {display(x)}")
        if (r.lo is not None and n < r.lo) or (r.hi is not None and n > r.hi):
            # the outcome of a fallible call: a test body's implied `or fail` fails the test with it (14; D-308)
            return Failure("refinement.out_of_range", {"value": n, "low": r.lo, "high": r.hi}, origin=self.stack[-1] if self.stack else "main")
        return x

    def range_bounds(self, refinement, env):
        rb = find(refinement, "refineBody")
        rg = find(rb, "range")
        kids = children(rg)
        lo = hi = None
        if not (isinstance(kids[0], Token) and kids[0].text == ".."):
            lo = self.eval(kids[0], env)
            rest = kids[1:]
        else:
            rest = kids
        if len(rest) >= 2 and name(rest[1]) == "constExpr":
            hi = self.eval(rest[1], env)
            if isinstance(rest[0], Token) and rest[0].text == "..<":
                hi -= 1
        return lo, hi

    @staticmethod
    def derives(cls: ClassInfo, iface: str) -> bool:
        """Whether the class's declaration derives `iface` (`class X derive Default`)."""
        kids = children(cls.node) if cls.node is not None else []
        for i, k in enumerate(kids):
            if isinstance(k, lexer.Token) and k.text == "derive" and i + 1 < len(kids) and not isinstance(kids[i + 1], lexer.Token):
                return any(t.text == iface for t in direct_tokens(kids[i + 1]))
        return False

    def derived_default(self, cls: ClassInfo, building=()):
        """`Name.default()` of a class deriving `Default` (6.10, X-345): each field at its initialiser, and a field without
        one at its type's default. `building` names the classes and newtypes whose defaults are being built."""
        building = self.enter_default(cls.name, building)
        args = [(f, self.type_default(cls.field_types.get(f), building)) for f, init in cls.fields.items() if init is None]
        return self.construct(cls, args)

    @staticmethod
    def enter_default(tn: str, building):
        """`building` with `tn` added. A class or newtype whose default is already being built is a cycle, which the
        checker rejects (E-TYP-090; D-335, D-336): it stops here with an error naming it, never a stack overflow."""
        if tn in building:
            cycle = list(building[building.index(tn):]) + [tn]
            raise FortisError(f"the default of {tn} contains itself ({' -> '.join(cycle)}), which the checker rejects (E-TYP-090)")
        return building + (tn,)

    INT_NAMES = ("Int", "Int8", "Int16", "Int32", "Int64", "UInt", "UInt8", "UInt16", "UInt32", "UInt64", "Byte")

    def type_default(self, tnode, building=()):
        """The value of a type's `Default` (6.10): `false`, `0`, `0.0`, `0s`, empty text, bytes and collections, `null` for
        an optional, `default()` of a class that derives it, and a newtype's representation's default, at any depth
        (D-242, D-337): a chain of newtypes is unfolded in a loop, with no limit on its length. A class of this program
        is itself, whatever its name (a user's `class Array`). The checker has proved the type has one. `building`
        names the classes and newtypes whose defaults are being built (`enter_default`)."""
        while True:
            if tnode is None:
                raise FortisError("a field without an initialiser or a type")
            if name(tnode) == "type":
                if any(isinstance(k, Token) and k.text == "?" for k in children(tnode)):
                    return None
                tnode = children(tnode)[0]
            if name(tnode) == "namedType":
                tnode = next((c for c in children(tnode) if name(c) in ("type", "typeRef")), None)
                continue
            if name(tnode) != "typeRef":
                raise FortisError("a field type without a default")
            tn = next(t.text for t in direct_tokens(tnode) if t.kind == "UPPER")
            if tn not in self.newtype_repr:
                break
            # a newtype's value is its representation (6.9), so its default is the representation's
            building = self.enter_default(tn, building)
            tnode = self.newtype_repr[tn]
        if tn in self.classes and self.derives(self.classes[tn], "Default"):
            return self.derived_default(self.classes[tn], building)
        # a class or an enum that implements Default itself: its `default()`
        own = self.classes.get(tn) or self.enums.get(tn)
        if own is not None and "default" in own.methods and own.methods["default"][1] is not None:
            head, body = own.methods["default"]
            return self.call(head, body, [], None)
        if tn == "Bool":
            return False
        if tn in self.INT_NAMES:
            return 0
        if tn == "Float":
            return 0.0
        if tn == "Float32":
            return numeric.F32.of(0.0)
        if tn == "Duration":
            return numeric.Dur(0)
        if tn == "String":
            return ""
        if tn == "Bytes":
            return intrinsics.BytesVal(b"")
        fn = intrinsics.ASSOC.get((tn, "default"))
        if fn is not None:
            return fn(self, [])
        raise FortisError(f"{tn} has no default")

    def construct(self, cls: ClassInfo, args):
        fields = {}
        spread = next((v for k, v in args if k == SPREAD), None)
        named = {k: v for k, v in args if k is not None and k != SPREAD}
        positional = [v for k, v in args if k is None]
        names_in_order = list(cls.fields)
        for i, v in enumerate(positional):
            fields[names_in_order[i]] = v
        for k, v in named.items():
            if k not in cls.fields:
                raise FortisError(f"{cls.name} has no field `{k}`")
            fields[k] = v
        if spread is not None:
            # `...copy p` / `...move p` fills every field of the target that the source has, and nothing else: a
            # source field the target lacks is neither copied nor stored (9.2, D-53)
            source, copying, dropped = spread
            for fname in cls.fields:
                if fname not in fields and fname in source:
                    fields[fname] = deep_copy(source[fname]) if copying else source[fname]
            if not copying and not dropped:
                self.drop_untaken(source, cls)
        for fname, default in cls.fields.items():
            if fname not in fields:
                if default is None:
                    raise FortisError(f"{cls.name}: field `{fname}` not given")
                fields[fname] = self.eval(default, self.globals)
        # the fields in declaration order, whatever order they were given in: a derived `Displayable` prints them,
        # and destruction visits them, in that order (9.2, 7.9)
        given = dict(fields)
        final = {fname: self.expected(fields[fname], cls.field_types.get(fname)) for fname in cls.fields}
        for fname in cls.fields:
            self.release(given.get(fname))                 # the field values move into the instance (7.9)
            self.release(final[fname])                     # expected() may have replaced an open variant's wrapper
        return self.fresh_add(Instance(cls, final))

    def drop_untaken(self, source: Dict[str, Any], target: ClassInfo):
        """`...move p` consumes the whole of p: the fields the target lacks are dropped at the spread, in declaration
        order (9.2, 7.9)."""
        for fname, fv in source.items():
            if fname not in target.fields:
                self.announce_drop(fv)
                self.destroy(fv)

    def variant(self, en: EnumInfo, vname, args):
        payload = {}
        fieldnames = en.variants[vname]
        for i, (k, v) in enumerate(args):
            payload[k if k is not None else fieldnames[i]] = v
        if payload and self.open_names():
            for fname, ftype in en.payload_types.get(vname, []):
                if fname in payload:
                    payload[fname] = self.expected(payload[fname], ftype)
        for _, v in args:
            self.release(v)                                # the payload values move into the variant (7.9)
        for v in payload.values():
            self.release(v)                                # expected() may have replaced an open variant's wrapper
        return self.fresh_add(EnumValue(en.name, vname, payload))

    def builtin(self, tname, member, args, called):
        """An associated function or module-level name of the core: a Fortis body first (DEL-26), then the
        intrinsic registry (interpreter/intrinsics.py), then the host and kit boundaries."""
        pos = [v for _, v in args]
        if member is not None:
            lm = self.library_method(tname, member)
            if lm is not None:
                return self.call_library(lm[0], lm[1][0], lm[1][1], args, None)
            fn = intrinsics.ASSOC.get((tname, member))
            if fn is not None:
                if (tname, member) == ("Array", "filled"):
                    return intrinsics.array_filled(self, pos, getattr(self, "array_n", None))
                return self.intrinsic_ownership(tname, member, pos, fn(self, pos))
        if tname in intrinsics.FUNCS and member in (None, tname):
            return self.intrinsic_ownership(tname, member, pos, intrinsics.FUNCS[tname](self, pos))
        if tname in self.imported and self.imported[tname] not in ("fortis.prelude", "fortis.text", "fortis.collections", "fortis.numeric", "fortis.time", "fortis.money", "fortis.host"):
            raise FortisError(f"kit not available: `{tname}` comes from {self.imported[tname]}, which this kernel does not implement")
        raise FortisError(f"`{tname}.{member}` is not implemented in the kernel")

    def place_path(self, e):
        """The place path of an expression that is a place (a binding, self, or a member path), else None.
        The same shapes as the checker's `path_of_expr`: single-child wrappers, a parenthesised primary, and
        a postfix of member names. An integer postfix is a tuple field, not a place path."""
        if e is None or isinstance(e, Token):
            return None
        n = name(e)
        kids = children(e)
        if n in ("constExpr", "expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr",
                 "shiftExpr", "additiveExpr", "multiplicativeExpr", "unaryExpr", "exprStmt") and len(kids) == 1:
            return self.place_path(kids[0])
        if n == "place":
            return self.place_path_of_place(e)
        if n == "markerExpr" and len(kids) == 2:
            pl = find(e, "place")
            return self.place_path_of_place(pl) if pl is not None else None
        if n == "primaryExpr":
            if len(kids) == 1 and isinstance(kids[0], Token) and (kids[0].kind == "LOWER" or kids[0].text == "self"):
                return kids[0].text
            if len(kids) == 3 and isinstance(kids[0], Token) and kids[0].text == "(":
                return self.place_path(kids[1])
            return None
        if n == "postfixExpr":
            root = self.place_path(kids[0])
            if root is None:
                return None
            for pf in kids[1:]:
                k = children(pf)
                if k and isinstance(k[0], Token) and k[0].text == "." and len(k) > 1 and name(k[1]) == "memberName":
                    root += "." + tokens(k[1])[0].text
                else:
                    return None
            return root
        return None

    def place_path_of_place(self, pl):
        if pl is None:
            return None
        ts = direct_tokens(pl)
        if not ts:
            return None
        parts = [ts[0].text]
        for k in children(pl)[1:]:
            if name(k) == "memberName":
                parts.append(tokens(k)[0].text)
            elif isinstance(k, Token) and k.kind == "INT":
                parts.append("[" + k.text + "]")
        return ".".join(parts)

    def pattern_binds(self, pat) -> bool:
        """True when a pattern takes ownership of some part of the scrutinee. `_` does not."""
        kids = children(pat)
        if not kids:
            return False
        first = kids[0]
        if isinstance(first, Token):
            t = first.text
            if t == "_":
                return False
            if first.kind == "LOWER":
                return True
            if t in ("present", "success", "failure"):
                return any(self.pattern_binds(k) for k in kids if name(k) == "pattern")
            if t == "otherwise":
                return any(isinstance(k, Token) and k.kind == "LOWER" for k in kids)
            if t == "is":
                if any(isinstance(k, Token) and k.text == "as" for k in kids):
                    return True
                pf = find(pat, "patternFields")
                return pf is not None and self.fields_bind(pf)
            if t == "(":
                return any(self.pattern_binds(k) for k in kids if name(k) == "pattern")
            if t == "{":
                return any(self.pattern_binds(k) for k in kids if name(k) == "pattern")
            if first.kind == "UPPER":
                pf = find(pat, "patternFields")
                return pf is not None and self.fields_bind(pf)
        return False

    def fields_bind(self, pf) -> bool:
        for f in children(pf):
            if name(f) != "patternField":
                continue
            ks = children(f)
            sub = ks[2] if len(ks) > 2 and isinstance(ks[1], Token) and ks[1].text == ":" else ks[0]
            if name(sub) == "pattern" and self.pattern_binds(sub):
                return True
        return False

    def pattern_opens(self, pat) -> bool:
        """True when the pattern destructures fields or elements. A whole-value `_` does not (D-300, D-315)."""
        kids = children(pat)
        if not kids:
            return False
        first = kids[0]
        if not isinstance(first, Token):
            return False
        t = first.text
        if t in ("present", "success", "failure"):
            inner = next((k for k in kids if name(k) == "pattern"), None)
            return inner is not None and self.pattern_opens(inner)
        if t in ("(", "{"):
            return any(name(k) == "pattern" for k in kids)
        if t == "is" or first.kind == "UPPER":
            pf = find(pat, "patternFields")
            return pf is not None and any(name(k) == "patternField" for k in children(pf))
        return False

    def release_wildcard_move(self, cell, aenv, any_binds, v):
        """The matched arm bound nothing. No arm binds: the local still owns the value and the block drops it
        once, with its trace. Some other arm binds: this scrutinee was moved, and it drops now, the way Rust
        drops a wildcard arm, so the trace stays on the binding that took it (7.9)."""
        if cell is None or aenv.vars:
            return
        if any_binds:
            self.destroy(v)
            return
        cell.moved = False
        self.fresh_remove(cell.value)

    def whole_place_root(self, e):
        """A place that is one local or `self`, with no field or index. A partial path is not a whole move."""
        path = self.place_path(e)
        if path is None or "." in path or "[" in path:
            return None
        return path

    # ---- match and if expressions -----------------------------------------------
    def match_expr(self, node, env):
        kids = children(node)
        scrut_node = next(c for c in kids if name(c) in ("expr", "place"))
        moving = any(isinstance(k, Token) and k.text == "move" for k in kids[:3])
        # a temporary that already existed is a read (`match self` on a call's receiver). One the scrutinee
        # expression itself creates (`match Make()`) is what this match owns.
        already = {i for d in self.temps for i in d}
        # `match [move] expr`: the scrutinee is an expr even when it is a place. `move` of a whole local or
        # `self` consumes that binding (9.8). The grammar's `place` child never appears, so the move used to
        # be ignored and the binding destroyed the value a second time on the way out (7.9).
        root = self.whole_place_root(scrut_node) if moving else None
        cell = None
        if root is not None:
            cell = env.cell(root)
            if cell.moved:
                raise FortisError(f"`{root}` was moved (use after move)")
            v = cell.value
            cell.moved = True
            moved_place = True
        else:
            v = self.eval(scrut_node, env)
            moved_place = False
        arms = [a for a in children(node) if name(a) == "matchArm"]
        # an arm that binds nothing drops the moved value only when some other arm takes it. When none do,
        # Rust leaves the local in place and the block drops it once (7.9). A pattern that opens the value
        # moves it, so a field matched by `_` drops after the arm (D-315).
        any_binds = any(self.pattern_binds(p) for a in arms for p in children(a) if name(p) == "pattern")
        for arm in arms:
            aenv = Env(env)
            # an or-pattern `A | B` matches when an alternative does, tried left to right; alternatives bind nothing (9.8)
            pat = next((p for p in children(arm) if name(p) == "pattern" and self.match(p, v, aenv)), None)
            if pat is None:
                continue
            ak = children(arm)
            if any(isinstance(k, Token) and k.text == "when" for k in ak):
                guard = next(c for c in ak if name(c) == "expr")
                g = self.eval(guard, aenv)
                self.expect_bool(g)
                if not g:
                    continue
                body = ak[-1]
            else:
                body = ak[-1]
            fresh = self.is_temp(v) and id(v) not in already
            owning = moved_place or fresh          # a read of an existing value shares it: the binding is not its owner
            if not owning:
                for c in aenv.vars.values():
                    c.owns = False                 # a borrowed scrutinee's bindings hold loans of its parts (7.5; D-338)
            if name(body) == "block":
                rest = self.take_scrutinee_parts(v, aenv, fresh) if owning else None
                terminated = False
                given = None
                given_lent = False
                try:
                    self.run_block(body, aenv)
                except GiveSignal as g:
                    given, given_lent = g.value, g.lent
                except ReturnSignal as r:
                    self.mark_given(aenv, r.value)   # the returned binding leaves with the return (7.9)
                    raise
                except RecoverSignal as r:
                    self.mark_given(aenv, r.value)
                    raise
                except (DefectSignal, RecursionError):
                    terminated = True
                    raise
                finally:
                    if not terminated:
                        if given is not None:
                            self.mark_given(aenv, given)
                        if owning:
                            self.drop_locals(aenv)   # an owning arm's bindings die at the end of the arm (7.9, 9.8)
                        if moved_place and not self.pattern_opens(pat):
                            self.release_wildcard_move(cell, aenv, any_binds, v)
                        elif rest is not None:
                            self.destroy(rest)       # an opened moved scrutinee: what no binding took drops after the arm (7.9)
                if given is not None:
                    return self.lend(given) if given_lent else self.fresh_add(given)
                return VOID                      # a statement-position arm block (the checker requires `give` in expression position)
            result = self.eval(body, aenv)
            if owning:
                rest = self.take_scrutinee_parts(v, aenv, fresh)
                self.mark_given(aenv, result)
                self.drop_locals(aenv)
                if moved_place and not self.pattern_opens(pat):
                    self.release_wildcard_move(cell, aenv, any_binds, v)
                elif rest is not None:
                    self.destroy(rest)
                return self.fresh_add(result)
            return result
        raise FortisError("no match arm matched (E-MAT-001 is static)")

    def if_expr(self, node, env):
        kids = children(node)
        cond = self.eval_cond_temps(kids[1], env)
        self.expect_bool(cond)
        def run(b):
            if name(b) == "ifExpr":
                return self.if_expr(b, env)
            try:
                self.run_block(b, env)
            except GiveSignal as g:
                return self.lend(g.value) if g.lent else self.fresh_add(g.value)
            raise FortisError("if-expression block without give")
        return run(kids[2]) if cond else run(kids[4])

    def match(self, pat, v, env: Env) -> bool:
        kids = children(pat)
        first = kids[0]
        if isinstance(first, Token):
            t = first.text
            if t == "_":
                return True
            if first.kind == "LOWER":
                env.declare(t, v)
                return True
            if t == "null":
                return v is None
            if t == "present":
                return v is not None and self.match(kids[2], v, env)
            if t == "success":
                return not isinstance(v, Failure) and self.match(kids[2], v, env)
            if t == "failure":
                return isinstance(v, Failure) and self.match(kids[2], v, env)
            if t == "otherwise":
                if len(kids) >= 3 and isinstance(kids[2], Token):
                    env.declare(kids[2].text, v)
                return True
            if t == "is":
                kind = kids[1].text
                if not isinstance(v, Failure) or kind not in self.errors or self.errors[kind].code != v.code:
                    return False
                pf = find(pat, "patternFields")
                if pf is not None and not self.match_fields(pf, v.payload, env):
                    return False
                if "as" in [k.text for k in kids if isinstance(k, Token)]:
                    env.declare(kids[-1].text, v)
                return True
            if first.kind == "UPPER":
                if isinstance(v, Instance) and v.cls.name == t:            # class destructuring (9.8): every field named
                    pf = find(pat, "patternFields")
                    return pf is None or self.match_fields(pf, v.fields, env)
                if not isinstance(v, EnumValue) or v.variant != t:
                    return False
                pf = find(pat, "patternFields")
                return pf is None or self.match_fields(pf, v.payload, env)
            if t == "(":
                subs = [k for k in kids if name(k) == "pattern"]
                if not (isinstance(v, tuple) and len(v) == len(subs)):
                    return False
                lent = lent_parts(v)
                return all(self.match_part(p, x, env, i in lent) for i, (p, x) in enumerate(zip(subs, v)))
            if t == "{":
                ks = [k for k in kids if not (isinstance(k, Token) and k.text in ("{", "}", ",", ":"))]
                lent = lent_parts(v)
                for i in range(0, len(ks), 2):
                    fname = ks[i].text
                    if not isinstance(v, dict) or fname not in v or not self.match_part(ks[i + 1], v[fname], env, fname in lent):
                        return False
                return True
        if name(first) == "literal":
            return self.equal(v, self.literal(children(first)[0], env))
        raise FortisError("pattern")

    def match_part(self, pat, v, env: Env, lent: bool) -> bool:
        """Match an element of a tuple or a record. The bindings inside an element the aggregate borrows hold loans, so
        their end destroys nothing, even in an arm that owns the rest of the scrutinee (7.9; D-349)."""
        before = set(env.vars)
        if not self.match(pat, v, env):
            return False
        if lent:
            for n, c in env.vars.items():
                if n not in before:
                    c.owns = False
        return True

    def match_fields(self, pf, payload: Dict[str, Any], env) -> bool:
        fields = [c for c in children(pf) if name(c) == "patternField"]
        keys = list(payload)
        for i, f in enumerate(fields):
            ks = children(f)
            if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                key, sub = ks[0].text, ks[2]
            else:
                key, sub = keys[i], ks[0]
            if key not in payload or not self.match(sub, payload[key], env):
                return False
        return True

    def const_term(self, node, env):
        kids = children(node)
        first = kids[0]
        if isinstance(first, Token):
            if first.text == "null": return None
            if first.text == "-": return self.negate(self.eval(kids[1], env))
            if first.text == "(": return self.eval(kids[1], env)
            if first.text == "{":
                out = {}
                ks = [k for k in kids if not (isinstance(k, Token) and (k.text in ("{", "}", ",", ":") or k.kind == "NL"))]
                for i in range(0, len(ks), 2):
                    key = ks[i].text if isinstance(ks[i], Token) else tokens(ks[i])[0].text
                    out[key] = self.eval(ks[i + 1], env)
                return out
            if first.kind == "LOWER":
                return self.literal(first, env)
        if name(first) == "literal":
            return self.literal(children(first)[0], env)
        if name(first) == "typeRef":
            return self.type_member(kids, env)
        raise FortisError("constant form")
