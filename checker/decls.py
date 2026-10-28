"""Declaration table and declaration-level checks (DEL-10: Language Reference 6.1, 6.9–6.11, 9.1–9.7).

Builds, for a module plus the prelude and its imports, the table of classes, enums, newtypes,
interfaces, mixins, errors, functions and constants with their member signatures as `Type`s, then
checks: duplicate members (E-TYP-070), `implements` completeness and signature compatibility and
`requires` satisfaction (E-TYP-080), `derive` applicability (E-TYP-090), coherence of `implement`
blocks (E-TYP-100), requirement cycles (E-TYP-060), float refinements (E-TYP-020), `trivial`
classes (E-OWN-030), derive-plus-hand-implementation and mixin conflicts (E-MIX-001). It also
computes the trivial, copyable and linear sets of 6.11/6.12."""
from __future__ import annotations

import os
import sys
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Set, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
import diagnostics  # noqa: E402
import lexer  # noqa: E402
import names  # noqa: E402
import tree  # noqa: E402
from lexer import Token  # noqa: E402
from tree import children, name, direct_tokens, find, find_all, tokens  # noqa: E402

from checker.effects import PURE_FORBIDS, ROOT, standard_packages  # noqa: E402
from checker.literals import known  # noqa: E402
from checker.types import (BOOL, COPYABLE_CONTAINERS, COPYABLE_HEAP, FLOAT_TYPES, INT, INT_TYPES, LINEAR_BUILTINS,  # noqa: E402
                           NOT_COPYABLE, STRING, TRIVIAL_BUILTINS, VOID, DynamicT, FnT, LoanT, Named, OptionalT, RecordT,
                           Refined, TupleT, Type, TypeParam, Unknown, base_name)

BUILTIN_IMPLEMENTS = {
    # which built-in interfaces each built-in type implements (6.10, 6.11)
    "Int": {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Default", "Copyable"},
    "Bool": {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Default", "Copyable"},
    "String": {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Default", "Copyable"},
    "StringView": {"Equatable", "Hashable", "Comparable", "Displayable"},
    "Bytes": {"Equatable", "Hashable", "Encodable", "Decodable", "Default", "Copyable"},
    "Float": {"Displayable", "Encodable", "Decodable", "Default", "Copyable"},      # no Equatable/Hashable/Comparable (6.5)
    "Float32": {"Displayable", "Encodable", "Decodable", "Default", "Copyable"},
    "Secret": {"Displayable"},          # a fingerprint display, never the value (15)
    "Decimal": {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Copyable"},
    "Duration": {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Default", "Copyable"},
    "Timestamp": {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Copyable"},
    "Instant": {"Equatable", "Hashable", "Comparable", "Copyable"},
    "Scalar": {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Copyable"},
    "Void": {"Equatable", "Copyable"},
    "Money": {"Equatable", "Hashable", "Displayable", "Encodable", "Decodable", "Copyable"},
    "Currency": {"Equatable", "Hashable", "Displayable", "Encodable", "Decodable", "Copyable"},
    "Path": {"Equatable", "Hashable", "Displayable", "Copyable"},
    "Report": {"Equatable", "Copyable"},
}
for _n in INT_TYPES:
    BUILTIN_IMPLEMENTS.setdefault(_n, set(BUILTIN_IMPLEMENTS["Int"]))
BUILTIN_IFACES = {"Equatable", "Hashable", "Comparable", "Displayable", "Iterable", "Iterator", "Consumable", "Consumer", "Copyable",
                  "Shareable", "Abandonable", "Default", "Encodable", "Decodable", "Codec", "Authority", "From"}
DERIVABLE = {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Codec", "Default", "From"}
IFACE_REQUIRES = {"Hashable": ["Equatable"], "Comparable": ["Equatable"], "Codec": ["Encodable", "Decodable"]}
NOT_ENCODABLE = {"Secret", "File", "FileSystem", "Network", "Clock", "Random", "Environment", "Process", "Supervisor", "Console", "SecretAccess", "RawSql",
                 "Future", "Task", "Shared", "Weak", "Mutex", "MutexGuard", "AtomicInt", "AtomicBool", "StringView", "BytesView", "ListView", "Failure", "Outcome"}
DEFAULT_BUILTINS = {"Bool", "Int", "Int64", "Int32", "Int16", "Int8", "UInt", "UInt64", "UInt32", "UInt16", "UInt8", "Byte", "Float", "Float32", "Duration", "String", "Bytes", "List", "Map", "Set"}


def standard(name: str) -> bool:
    """Whether a package or module name is the standard package's (`fortis`, `fortis.prelude`, `fortis.http.server`):
    a user package may be named `fortissimo`, which is not (D-133)."""
    return name == "fortis" or name.startswith("fortis.") or name.startswith("fortis@")


@dataclass
class Sig:
    """A function or method signature."""
    name: str
    params: List[Tuple[str, Type, str]]       # (name, type, mode: "" | "editable" | "owned")
    ret: Type
    receiver: Optional[str]                   # None (associated) | "self" | "editable self" | "owned self"
    modifiers: Tuple[str, ...]
    contracts: Tuple[str, ...]
    exported: bool
    node: object = None
    body: object = None
    type_params: Tuple[str, ...] = ()
    where: Tuple[Tuple[str, "Named"], ...] = ()     # method-level `where T implements I` bounds (6.10)

    def show(self) -> str:
        recv = (self.receiver + ", ") if self.receiver else ""
        ps = ", ".join(f"{n}: {(m + ' ') if m else ''}{t.show()}" for n, t, m in self.params)
        return f"{' '.join(self.modifiers)} func {self.name}({recv}{ps}) {self.ret.show()}".strip()


@dataclass
class Decl:
    kind: str                                 # class | enum | type | interface | mixin | error | func | const
    name: str
    exported: bool
    module: str
    package: str = ""
    core_home: str = ""                   # prelude copy of a type one other standard module declares: that module
    type_params: Tuple[str, ...] = ()
    fields: Dict[str, Tuple[Type, bool, bool]] = field(default_factory=dict)   # name -> (type, exported, has_default)
    methods: Dict[str, Sig] = field(default_factory=dict)
    variants: Dict[str, List[Tuple[str, Type]]] = field(default_factory=dict)
    derives: List[Tuple[str, Tuple[Type, ...]]] = field(default_factory=list)
    implements: List[Named] = field(default_factory=list)
    mixins: List[Named] = field(default_factory=list)
    requires: List[Named] = field(default_factory=list)
    constants: Dict[str, Type] = field(default_factory=dict)
    trivial: bool = False
    lifetime: Optional[str] = None
    alias: Optional[Type] = None              # for `type` newtypes: the representation
    where: List[Tuple[str, Named]] = field(default_factory=list)
    code: str = ""                            # errors
    payload: Dict[str, Type] = field(default_factory=dict)
    sig: Optional[Sig] = None                 # funcs
    aspects: list = field(default_factory=list)   # aspectUse nodes applied to a func (11.8)
    items: dict = field(default_factory=dict)     # contract: {modifiers, forbids, requires, excludes} (11.5)
    obs_fields: dict = field(default_factory=dict)   # observation: field -> (type, required class) (13)
    aspect: dict = field(default_factory=dict)    # aspect: params, type_param, needs, parts, base (11.8)
    node: object = None
    has_cleanup: bool = False

    @property
    def type_identity(self) -> str:
        """A type's nominal identity (LANG-04-04): its declaring package@version and module, so two modules' types of
        one name are different types; empty for the core library's types and a single-file program's."""
        return "" if not self.package or standard(self.package) else f"{self.package}/{self.module}"

    def as_named(self, args: Tuple[Type, ...] = ()) -> Named:
        """The type this declaration is. A non-prelude core type keeps its module as origin, the stamp `named_of`
        gives a type reference, so a constructor, a variant and a signature name one type. A prelude declaration
        of a name one other standard module also declares carries that module (`core_home`): the two are one type."""
        if self.kind != "unknown" and not self.type_identity and self.module.startswith("fortis") and self.module != "fortis.prelude":
            origin = self.module
        elif self.module == "fortis.prelude":
            origin = self.core_home
        else:
            origin = ""
        return Named(self.name, tuple(args), self.type_identity, origin)


class Table:
    """All declarations visible to one module, by name, plus `implement` blocks."""
    def __init__(self):
        self.decls: Dict[str, Decl] = {}
        self.implement_blocks: List[Tuple[Named, Named, str, object]] = []     # (interface, type, module, node)
        self.module_of: Dict[str, str] = {}

    def get(self, n: str) -> Optional[Decl]:
        return self.decls.get(n)


class DeclChecker:
    def __init__(self, library: names.Library, file: str, module_name: str = "main", package: str = "", session: Optional["Session"] = None, entry: Optional[str] = "main"):
        self.lib = library
        self.session = session if session is not None else Session()     # one check's package-module tables
        self.file = file
        self.module_name = module_name
        self.package = package
        # the module key of the program's entry module (4.6): a single file is its own entry ("main"); a package set's
        # is its root package's `entry`, and a set without one has none. Only that module's `main` is the entry `main`,
        # the one function that creates the capabilities (11.6, LANG-11-07)
        self.entry_module = entry
        self.records: List[dict] = []
        self.table = Table()
        self.type_params: Set[str] = set()
        self.tparam_bounds: Dict[str, Set[str]] = {}     # type parameter -> interfaces its `where` clauses grant in the body being typed
        # heads of this module's own declarations, consulted by `named_of` only while the module is loading and only
        # for a name the table does not already hold, so a forward reference is stamped with this module's identity
        # (LANG-04-04) without being inserted early (that would trip E-TYP-070)
        self.pending_heads: Dict[str, Decl] = {}
        # the module whose declarations are being parsed, so a core signature can name a core type that is not in
        # the table yet (`Scope.detach`'s `Supervisor`, written in the prelude and declared in fortis.host)
        self.loading_module = ""

    def rec(self, code, msg, rule, tok, fields=None, status="violated"):
        f = fields or {}
        cls = diagnostics.class_of(code)
        for need in (cls["fields"] if cls else []):
            f.setdefault(need, "")
        r = diagnostics.make(code, msg, rule, self.file, tok.line if tok else 1, tok.col if tok else 1, f, status=status)
        if not any(x["code"] == r["code"] and x["line"] == r["line"] and x["message"] == r["message"] for x in self.records):
            self.records.append(r)

    # ---- loading --------------------------------------------------------------
    def load(self, mtree, is_library=False):
        """Load prelude + imports (as library declarations), then this module's declarations."""
        if self.lib.prelude is not None and self.module_name != "fortis.prelude":   # the prelude does not import itself
            self.loading_module = "fortis.prelude"
            try:
                self.load_module_tree(self.lib.prelude.tree, "fortis.prelude", "fortis", library=True)
            finally:
                self.loading_module = ""
        for imp in tree.imports(mtree):
            path = ".".join(imp["path"])
            lm = self.lib.modules.get(path)
            # a package's root module is registered under the package's name too; its declarations belong to the
            # module's own key (`acme.tools.tools`), which is where they are built, however the import names it (D-134)
            if lm is not None:
                path = next((k for k, v in self.lib.modules.items() if v is lm and len(k) > len(path) and k.startswith(path + ".")), path)
            if lm is not None and getattr(lm, "library", None) is not None and getattr(lm, "package", "fortis") != "fortis":
                # a package module: its signatures mean what they mean in its own package (its imports, its dependency versions)
                sub = _loaded(self.session, lm, path)
                only = set(imp["members"]) if imp["members"] else None
                alias = (imp["alias"] or imp["path"][-1]) if imp["members"] is None else None
                for nm, d in sub.decls.items():
                    if d.module != path or (only is not None and nm not in only):
                        continue
                    self.table.decls[(alias + "." + nm) if alias else nm] = d
                for blk in sub.implement_blocks:
                    if blk not in self.table.implement_blocks:
                        self.table.implement_blocks.append(blk)
            elif lm is not None:
                # a standard-library module is loaded into this table, not as its own checker, so its heads are not
                # pending. A method parsed before its class is inserted would otherwise name that class with no
                # origin, while a later signature that mentions it would keep the module (Path.parse vs readText).
                pkg = getattr(lm, "package", "fortis")
                saved_heads = self.pending_heads
                saved_loading = self.loading_module
                self.pending_heads = {**heads(lm.tree, path, pkg).decls, **saved_heads}
                self.loading_module = path
                try:
                    self.load_module_tree(lm.tree, path, pkg, library=True,
                                          only=set(imp["members"]) if imp["members"] else None, alias=(imp["alias"] or imp["path"][-1]) if imp["members"] is None else None)
                finally:
                    self.pending_heads = saved_heads
                    self.loading_module = saved_loading
            else:
                # a package the corpus does not contain: its imported names are unknown types
                for m in (imp["members"] or []):
                    self.table.decls.setdefault(m, Decl("unknown", m, True, path, path))
        self.pending_heads = dict(heads(mtree, self.module_name, self.package).decls)
        saved_loading = self.loading_module
        self.loading_module = self.module_name
        try:
            self.load_module_tree(mtree, self.module_name, self.package, library=is_library)
        finally:
            self.pending_heads = {}
            self.loading_module = saved_loading

    def load_module_tree(self, mtree, module: str, package: str, library: bool, only: Optional[Set[str]] = None, alias: Optional[str] = None):
        if only is not None:
            # an imported declaration brings the declarations it reaches through its types (fields, parameters,
            # results, bases) into the table, so a Console reached through Capabilities is known without being
            # visible by name (names.py still rejects the bare name when it is not imported)
            by_name = {d["name"]: d for d in tree.declarations(mtree) if d["name"] is not None}
            want = set(only)
            frontier = list(want)
            while frontier:
                d0 = by_name.get(frontier.pop())
                if d0 is None:
                    continue
                for t in tokens(d0["node"]):
                    if t.kind == "UPPER" and t.text in by_name and t.text not in want:
                        want.add(t.text)
                        frontier.append(t.text)
            only = want
        local_aspects: Dict[str, Decl] = {}
        for d in tree.declarations(mtree):
            if d["kind"] == "aspect" and d["name"] is not None:
                local_aspects[d["name"]] = self.load_decl(d, module, package)
        for a in local_aspects.values():
            if a.aspect.get("base") is not None:
                a.aspect["base_decl"] = local_aspects.get(a.aspect["base"][0])
        for d in tree.declarations(mtree):
            if d["kind"] in ("head", "test", "property", "implement"):
                if d["kind"] == "implement" and not library:
                    self.load_implement(d["node"], module)
                continue
            if d["name"] is None:
                continue
            if only is not None and d["name"] not in only:
                continue
            key = (alias + "." + d["name"]) if alias else d["name"]
            if not library and key in self.table.decls and self.table.decls[key].module == module:
                self.rec("E-TYP-070", f"`{key}` is declared twice in this module", "LANG-09-01", d["token"], {"expected_type": "one declaration", "actual_type": "two", "primary_span": key})
                continue
            decl = local_aspects[d["name"]] if d["kind"] == "aspect" else self.load_decl(d, module, package)
            if library and key in self.table.decls and self.table.decls[key].module != module and not alias:
                pass   # an import may re-introduce a prelude name (e.g. Timestamp from fortis.time): same nominal type
            self.table.decls[key] = decl
            if alias is None and d["name"] != key:
                pass

    def load_decl(self, d, module, package) -> Decl:
        node, kind, nm = d["node"], d["kind"], d["name"]
        decl = Decl(kind, nm, d["exported"], module, package, node=node)
        if module == "fortis.prelude":
            decl.core_home = self.core_module(nm)
        tp = find(node, "typeParams")
        if tp is not None and name(children(node)[0] if children(node) else None) != "funcHead":
            decl.type_params = tuple(t.text for t in tokens(tp) if t.kind == "UPPER")
        self.type_params = set(decl.type_params)
        lp = find(node, "lifetimeParam")
        if lp is not None:
            decl.lifetime = tokens(lp)[2].text
        if kind == "func":
            decl.aspects = list(d.get("aspects", []))
        if kind == "contract":
            items = {"modifiers": set(), "forbids": set(), "requires": set(), "excludes": set()}
            for it in [c for c in children(node) if name(c) == "contractItem"]:
                ws = [t.text for t in tokens(it)]
                if ws[0] == "forbids": items["forbids"].add(ws[1])
                elif ws[0] == "requires": items["requires"].add(ws[-1])
                elif ws[0] == "excludes": items["excludes"].add(ws[-1])
                else: items["modifiers"].add(ws[0])
            decl.items = items
        if kind == "observation":
            for it in [c for c in children(node) if name(c) == "observationItem"]:
                ws = [t.text for t in direct_tokens(it)]
                if ws[0] in ("version", "encoding"):
                    decl.items[ws[0]] = ws[2] if len(ws) > 2 else ""
                    continue
                ft = self.type_of(find(it, "type"))
                decl.obs_fields[ws[0]] = (ft, ws[-1])
        if kind == "aspect":
            a = {"params": [], "type_param": decl.type_params[0] if decl.type_params else None, "needs": [], "needs_contracts": [],
                 "before": [], "after": None, "on_failure": None, "metadata": False, "base": None, "rest": False}
            ap = find(node, "aspectParams")
            if ap is not None:
                for p in [c for c in children(ap) if name(c) == "param"]:
                    if find(p, "restParam") is not None:
                        a["rest"] = True
                        continue
                    ts = direct_tokens(p)
                    nm2 = next(t for t in ts if t.kind == "LOWER")
                    pt = find(p, "paramType")
                    a["params"].append((nm2.text, self.type_of(find(pt, "type")) if pt is not None else Unknown("aspect param"), find(p, "constExpr") is not None))
            for part in [c for c in children(node) if name(c) == "aspectPart"]:
                ws = [t.text for t in direct_tokens(part)]
                blk = find(part, "block")
                if ws[0] == "needs" and len(ws) > 2:
                    a["needs"].append((ws[1], self.type_of(find(find(part, "paramType"), "type")), next((t.text for t in direct_tokens(find(part, "paramType")) if t.text in ("editable", "owned")), "")))
                elif ws[0] == "needs":
                    a["needs_contracts"].append(ws[1])
                elif ws[0] == "before":
                    a["before"].append(blk)
                elif ws[0] == "after":
                    a["after"] = (ws[2], self.type_of(find(part, "type")), blk)
                elif ws[0] == "on":
                    a["on_failure"] = (ws[3], self.type_of(find(part, "type")), blk, direct_tokens(part)[3])
                elif ws[0] == "metadata":
                    a["metadata"] = True
            kids = children(node)
            eq = next((i for i, c in enumerate(kids) if isinstance(c, Token) and c.text == "="), None)
            if eq is not None:
                base = next(t for t in kids[eq + 1:] if isinstance(t, Token) and t.kind == "LOWER")
                a["base"] = (base.text, find(node, "argList"), base)
            decl.aspect = a
        if kind in ("class", "enum"):
            decl.trivial = any(isinstance(c, Token) and c.text == "trivial" for c in children(node))
            ts = direct_tokens(node)
            words = [t.text for t in ts]
            # derive / implements / with lists
            for i, c in enumerate(children(node)):
                if isinstance(c, Token) and c.text == "derive":
                    ul = children(node)[i + 1]
                    for it in self.upper_list(ul):
                        decl.derives.append(it)
                if isinstance(c, Token) and c.text in ("implements", "with"):
                    tl = children(node)[i + 1]
                    for t in [x for x in children(tl) if name(x) == "type"]:
                        ty = self.type_of(t)
                        if isinstance(ty, Named):
                            (decl.implements if c.text == "implements" else decl.mixins).append(ty)
            for m in children(node):
                if name(m) not in ("classMember", "enumMember"):
                    continue
                exp = any(isinstance(c, Token) and c.text == "export" for c in children(m))
                v = find(m, "variant")
                if v is not None and name(m) == "enumMember":
                    vt = direct_tokens(v)
                    fl = find(v, "fieldList")
                    decl.variants[vt[0].text] = self.field_list(fl) if fl else []
                    continue
                fd = find(m, "fieldDecl")
                if fd is not None:
                    ft = direct_tokens(fd)
                    fname = ft[0].text
                    if fname in decl.fields or fname in decl.methods:
                        self.rec("E-TYP-070", f"member `{fname}` of {nm} is declared twice", "LANG-09-01", ft[0], {"expected_type": "one member", "actual_type": "two", "primary_span": fname})
                    decl.fields[fname] = (self.type_of(find(fd, "type")), exp, any(name(c) == "constExpr" for c in children(fd)))
                    continue
                cd = find(m, "constDecl")
                if cd is not None and find(m, "methodHead") is None:
                    ct = direct_tokens(cd)
                    decl.constants[ct[1].text] = self.type_of(find(cd, "type")) if find(cd, "type") else Unknown("untyped const")
                    continue
                cl = find(m, "cleanupDecl")
                if cl is not None:
                    decl.has_cleanup = True
                    continue
                head = find(m, "methodHead")
                if head is not None:
                    sig = self.signature(head, exp, find(m, "funcBody"))
                    if sig.name in decl.methods or sig.name in decl.fields:
                        self.rec("E-TYP-070", f"member `{sig.name}` of {nm} is declared twice", "LANG-09-01", tokens(head)[0], {"expected_type": "one member", "actual_type": "two", "primary_span": sig.name})
                    decl.methods[sig.name] = sig
        elif kind == "interface":
            for i, c in enumerate(children(node)):
                if isinstance(c, Token) and c.text == "requires":
                    for t in [x for x in children(children(node)[i + 1]) if name(x) == "type"]:
                        ty = self.type_of(t)
                        if isinstance(ty, Named):
                            decl.requires.append(ty)
            wc = find(node, "whereClause")
            if wc is not None:
                decl.where = self.where_of(wc)
            for m in [c for c in children(node) if name(c) == "interfaceMember"]:
                head = find(m, "methodHead")
                if head is not None:
                    sig = self.signature(head, True, None)
                    decl.methods[sig.name] = sig
                else:
                    ts = direct_tokens(m)
                    decl.constants[ts[1].text] = self.type_of(find(m, "type"))
        elif kind == "mixin":
            for m in [c for c in children(node) if name(c) == "mixinMember"]:
                ts = direct_tokens(m)
                head = find(m, "methodHead")
                if ts and ts[0].text == "requires":
                    if head is not None:
                        sig = self.signature(head, True, None)
                        decl.requires.append(Named("__method__:" + sig.name))
                        decl.constants[sig.name] = Unknown("required method")
                        decl.methods["requires:" + sig.name] = sig
                    else:
                        for t in [x for x in children(find(m, "typeList")) if name(x) == "type"]:
                            ty = self.type_of(t)
                            if isinstance(ty, Named):
                                decl.requires.append(ty)
                elif head is not None:
                    sig = self.signature(head, any(isinstance(c, Token) and c.text == "export" for c in children(m)), find(m, "funcBody"))
                    decl.methods[sig.name] = sig
        elif kind == "type":
            decl.alias = self.type_of(find(node, "type"))
            if isinstance(decl.alias, Refined) and decl.alias.base.name in FLOAT_TYPES:
                self.rec("E-TYP-020", f"float refinement in `type {nm}`", "LANG-06-05", d["token"], {"expected_type": "an integer, text, byte or collection base", "actual_type": decl.alias.show(), "primary_span": nm})
        elif kind == "error":
            ts = direct_tokens(node)
            decl.code = next(t.text for t in ts if t.kind == "STR")[1:-1]
            fl = find(node, "fieldList")
            if fl is not None:
                decl.payload = dict(self.field_list(fl))
        elif kind == "func":
            decl.sig = self.signature(find(node, "funcHead"), d["exported"], find(node, "funcBody"))
        elif kind == "const":
            t = find(node, "type")
            decl.alias = self.type_of(t) if t is not None else Unknown("inferred const")
        self.type_params = set()
        return decl

    def load_implement(self, node, module):
        ts = [c for c in children(node) if name(c) == "type"]
        iface, target = self.type_of(ts[0]), self.type_of(ts[1])
        if isinstance(iface, Named) and isinstance(target, Named):
            self.table.implement_blocks.append((iface, target, module, node))

    def upper_list(self, ul) -> List[Tuple[str, Tuple[Type, ...]]]:
        out = []
        kids = children(ul)
        i = 0
        while i < len(kids):
            k = kids[i]
            if isinstance(k, Token) and k.kind == "UPPER":
                args: Tuple[Type, ...] = ()
                if i + 1 < len(kids) and name(kids[i + 1]) == "typeArgs":
                    args = tuple(self.type_of(a) for a in find_all(kids[i + 1], "type") if a in children(find(kids[i + 1], "typeArg")) or True)
                    args = tuple(self.type_arg(a) for a in children(kids[i + 1]) if name(a) == "typeArg")
                    i += 1
                out.append((k.text, args))
            i += 1
        return out

    def field_list(self, fl) -> List[Tuple[str, Type]]:
        out = []
        kids = children(fl)
        for i, k in enumerate(kids):
            if isinstance(k, Token) and k.kind == "LOWER" and i + 2 < len(kids) and name(kids[i + 2]) == "type":
                out.append((k.text, self.type_of(kids[i + 2])))
        return out

    def where_of(self, wc) -> List[Tuple[str, Named]]:
        out = []
        for c in find_all(wc, "constraint"):
            ts = direct_tokens(c)
            t = find(c, "type")
            if t is not None:
                ty = self.type_of(t)
                if isinstance(ty, Named):
                    out.append((ts[0].text, ty))
        return out

    # ---- signatures and types -------------------------------------------------
    def signature(self, head, exported, body) -> Sig:
        mods = find(head, "modifiers")
        words = [t.text for t in tokens(mods)] if mods else []
        contracts = tuple(w for w in words if w[0].isupper())
        modifiers = tuple(w for w in words if not w[0].isupper())
        mn = find(head, "memberName")
        fname = tokens(mn)[0].text if mn is not None else direct_tokens(head)[[t.text for t in direct_tokens(head)].index("func") + 1].text
        sig = find(head, "funcSignature")
        tp = find(sig, "typeParams")
        tparams = tuple(t.text for t in tokens(tp) if t.kind == "UPPER") if tp else ()
        saved = set(self.type_params)
        self.type_params |= set(tparams)
        params, receiver = [], None
        pl = find(sig, "paramList")
        if pl is not None:
            for p in [c for c in children(pl) if name(c) == "param"]:
                sp = find(p, "selfParam")
                if sp is not None:
                    receiver = " ".join(t.text for t in tokens(sp))
                    continue
                ts = direct_tokens(p)
                pt = find(p, "paramType")
                if pt is None:
                    self.rec("E-TYP-150", "`...rest` forwards an aspect's remaining arguments and is only permitted in an aspect's parameter list (11.8)", "LANG-11-09", tokens(p)[0], {"expected_type": "a typed parameter", "actual_type": "...rest", "primary_span": fname})
                    continue                 # restParam: an aspect's `...rest` carries no type
                mode = next((t.text for t in direct_tokens(pt) if t.text in ("editable", "owned")), "")
                variadic = ts[0].text == "..."
                nm = next(t for t in ts if t.kind == "LOWER")
                params.append((nm.text, self.type_of(find(pt, "type")), ("..." + mode) if variadic else mode))
        ret = self.type_of(find(find(sig, "returnType"), "type"))
        self.type_params = saved
        wc = find(sig, "whereClause")
        where = tuple(self.where_of(wc)) if wc is not None else ()
        return Sig(fname, params, ret, receiver, modifiers, contracts, exported, head, body, tparams, where)

    def type_arg(self, ta) -> Type:
        t = find(ta, "type")
        if t is not None:
            return self.type_of(t)
        tk = direct_tokens(ta)[0]
        return Named("#" + tk.text)      # an integer type argument (Array[T, N])

    def type_of(self, tnode) -> Type:
        """Convert a `type` node to a Type."""
        if tnode is None:
            return Unknown("missing type")
        kids = children(tnode)
        first = kids[0]
        n = name(first)
        if n == "fnType":
            return self.fn_type(first)
        if n == "loanType":
            ks = children(first)
            words = [k.text for k in ks if isinstance(k, Token)]
            if "dynamic" in words:
                tr = find(first, "typeRef")
                inner = DynamicT(self.named_of(tr))
                return OptionalT(inner) if "?" in words else inner
            mode = "editable" if "editable" in words else "alias"
            nt = find(first, "namedType")
            inner: Type = self.named_type(nt)
            if "?" in words:
                inner = OptionalT(inner)
            org = find(first, "origin")
            return LoanT(inner, mode, tokens(org)[0].text)
        if n == "namedType":
            t = self.named_type(first)
            if len(kids) > 1 and isinstance(kids[1], Token) and kids[1].text == "?":
                return OptionalT(t)
            return t
        return Unknown("type form")

    def named_type(self, nt) -> Type:
        kids = children(nt)
        first = kids[0]
        if isinstance(first, Token):
            if first.text == "Self":
                return Named("Self")
            if first.text == "(":
                ts = [self.type_of(k) for k in kids if name(k) == "type"]
                return ts[0] if len(ts) == 1 else TupleT(tuple(ts))
            if first.text == "{":
                fl = find(nt, "fieldList")
                return RecordT(tuple(self.field_list(fl)))
        return self.named_of(first)

    def type_of_ref(self, tr) -> Type:
        return self.named_of(tr)

    def core_module(self, name: str) -> str:
        """The one non-prelude standard module that declares `name`, or "" when none does or several do.

        The prelude's own copy of that name is the same type (`Scalar` in `fortis.text`). A core signature parsed
        before that module is loaded (`Supervisor` in the prelude) still names that module."""
        index = getattr(self, "_core_modules", None)
        if index is None:
            found: Dict[str, Set[str]] = {}
            for path, lm in getattr(self.lib, "modules", {}).items():
                pkg = getattr(lm, "package", "fortis").split("@")[0]
                if pkg != "fortis" or not str(path).startswith("fortis"):
                    continue
                for d in tree.declarations(lm.tree):
                    if d["name"] and d["kind"] in ("class", "enum", "error", "interface", "mixin", "observation", "type"):
                        found.setdefault(d["name"], set()).add(path)
            index = {}
            for nm, mods in found.items():
                rest = [m for m in mods if m != "fortis.prelude"]
                if len(rest) == 1:
                    index[nm] = rest[0]
            self._core_modules = index
        return index.get(name, "")

    def origin_of(self, d: Decl) -> str:
        """The core module a declaration's type carries. A user type and a prelude-only type carry none. A name the
        prelude and exactly one other standard module both declare carries that other module on both copies."""
        if d.kind == "unknown" or d.type_identity or not d.module.startswith("fortis"):
            return ""
        if d.module != "fortis.prelude":
            return d.module
        return d.core_home or self.core_module(d.name)

    def named_of(self, tr) -> Type:
        ts = direct_tokens(tr)
        if ts[0].kind == "LOWER":
            nm = ts[0].text + "." + ts[2].text
            base_tok = ts[2]
        else:
            nm = ts[0].text
            base_tok = ts[0]
        args: Tuple[Type, ...] = ()
        ta = next((c for c in children(tr) if name(c) == "typeArgs"), None)        # direct children only: nested typeRefs carry their own
        if ta is not None:
            args = tuple(self.type_arg(a) for a in children(ta) if name(a) == "typeArg")
        if nm in self.type_params:
            return TypeParam(nm)
        # a type is named by its declaration, however the annotation spells it (`one.Node` is `Node` of its module),
        # and carries its identity (LANG-04-04); the standard package is the anonymous default. A name this module
        # declares later is not in the table yet: its head supplies the identity, and an import already bound wins.
        d = self.table.decls.get(nm)
        head = self.pending_heads.get(nm)
        # the module being parsed names its own declaration, not a prelude binding of the same name already in
        # the table. `fortis.text` redeclares Builder; `create() Builder` must name text's Builder. An import
        # already bound wins over a head from a different module.
        if head is not None and head.kind != "unknown" and head.module == self.loading_module and (d is None or d.kind == "unknown" or d.module != self.loading_module):
            d = head
        elif d is None or d.kind == "unknown":
            d = head if head is not None else d
        if d is not None and d.kind != "unknown":
            canonical, pkg = d.name, d.type_identity
            # a core type keeps the module that declared it. The prelude's copy of a name one other standard
            # module declares keeps that module, so the two copies are one type and a user's class is not.
            origin = self.origin_of(d)
        else:
            canonical, pkg = nm, ""
            # the prelude names Supervisor before fortis.host is loaded. Only a core module being parsed may take
            # that module from the standard library; a user annotation that has not bound the name stays unresolved.
            origin = self.core_module(nm) if "." not in nm and self.loading_module.startswith("fortis") else ""
        named = Named(canonical, args, pkg, origin)
        ref = next((c for c in children(tr) if name(c) == "refinement"), None)
        if ref is not None:
            if nm in FLOAT_TYPES:
                self.rec("E-TYP-020", f"float refinement `{nm}<…>`", "LANG-06-05", base_tok, {"expected_type": "an integer, text, byte or collection base", "actual_type": nm, "primary_span": nm})
                return named
            rb = find(ref, "refineBody")
            rkids = children(rb)
            kind = "range"
            if isinstance(rkids[0], Token) and rkids[0].kind == "LOWER":
                kind = rkids[0].text
            rg = find(rb, "range")
            lo, hi = self.range_of(rg)
            return Refined(named, lo, hi, kind)
        return named

    def range_of(self, rg) -> Tuple[Optional[int], Optional[int]]:
        kids = children(rg)
        lo = hi = None
        if not (isinstance(kids[0], Token) and kids[0].text == ".."):
            lo = self.const_int(kids[0])
            rest = kids[1:]
        else:
            rest = kids
        if len(rest) >= 2 and name(rest[1]) == "constExpr":
            hi = self.const_int(rest[1])
            if isinstance(rest[0], Token) and rest[0].text == "..<" and hi is not None:
                hi -= 1
        return lo, hi

    def const_int(self, node) -> Optional[int]:
        """A literal (possibly negative) constant bound; other constant forms are not folded yet."""
        ts = tokens(node)
        if len(ts) == 1 and ts[0].kind == "INT":
            return int(ts[0].text.replace("_", ""), 0)
        if len(ts) == 2 and ts[0].text == "-" and ts[1].kind == "INT":
            return -int(ts[1].text.replace("_", ""), 0)
        return None

    def fn_type(self, ft) -> Type:
        kids = children(ft)
        words = [k.text for k in kids if isinstance(k, Token)]
        kind = "fn once" if "once" in words else ("fn mut" if "mut" in words else "fn")
        mods = find(ft, "modifiers")
        modifiers = tuple(t.text for t in tokens(mods)) if mods else ()
        params = tuple(self.type_of(find(pt, "type")) for pt in [c for c in kids if name(c) == "paramType"])
        ret_nodes = [c for c in kids if name(c) == "type"]
        ret = self.type_of(ret_nodes[-1]) if ret_nodes else VOID
        origin = ""
        if len(kids) >= 2 and isinstance(kids[-2], Token) and kids[-2].text == "from" and isinstance(kids[-1], Token):
            origin = kids[-1].text
        elif isinstance(ret, LoanT) and not self.loan_carrying_inner(ret.inner):
            origin, ret = ret.origin, ret.inner        # `fn(Line) Bool from threshold`: the region is the closure's (8.3)
        return FnT(params, ret, kind, modifiers, origin, "dynamic" in words)

    _TYPE_KINDS = ("class", "enum", "error", "interface", "mixin", "observation", "type")

    def decl_by_identity(self, name: str, identity: str) -> Optional[Decl]:
        """The type declaration `identity` names (package@version/module plus the type's name, LANG-04-04).

        The module that declares it is the one that decides its fields, derives and lifetime. A finished table in
        this check's session wins over a head an importer copied in while the module was still loading; an identity
        never falls back to a same-named declaration, nor to a prelude declaration whose identity is empty."""
        if not identity or "/" not in identity:
            return None
        module = identity.split("/", 1)[1]

        def pick(table: Optional[Table]) -> Optional[Decl]:
            if table is None:
                return None
            d = table.get(name)
            if d is not None and d.name == name and d.kind in self._TYPE_KINDS and d.type_identity == identity:
                return d
            return next((x for x in table.decls.values() if x.name == name and x.type_identity == identity and x.kind in self._TYPE_KINDS), None)

        if module == self.module_name:
            found = pick(self.table)
            if found is not None:
                return found
            h = self.pending_heads.get(name)
            if h is not None and h.kind in self._TYPE_KINDS and h.type_identity == identity:
                return h
        for (_, path), sub in list(self.session.loaded.items()):
            if path == module:
                found = pick(sub)
                if found is not None:
                    return found
        for (_, path), sub in list(self.session.loading.items()):
            if path == module:
                found = pick(sub)
                if found is not None:
                    return found
        found = pick(self.table)
        if found is not None:
            return found
        for sub in list(self.session.loaded.values()):
            found = pick(sub)
            if found is not None:
                return found
        return None

    def decl_from_loaded(self, name: str, package: str) -> Optional[Decl]:
        """A type of this name and identity in a module the check has already loaded (D-54.11).

        The declaration is that module's own, so it carries the identity. A same-named declaration in the module
        being checked is not this one."""
        for sub in self.session.loaded.values():
            for x in sub.decls.values():
                if x.name == name and x.type_identity == package and x.kind in self._TYPE_KINDS:
                    return x
        return None

    def decl_by_module(self, name: str, module: str, home: Optional[Table] = None) -> Optional[Decl]:
        """The core declaration `module` exports under `name` (`fortis.host` and `FileSystem`).

        Its package identity is empty, so the bare name in the module being checked is a different declaration.
        The type remembers `module` from the reference that named it."""
        def pick(table: Optional[Table]) -> Optional[Decl]:
            if table is None:
                return None
            d = table.get(name)
            if d is not None and d.name == name and d.module == module and not d.type_identity and d.kind in self._TYPE_KINDS:
                return d
            return next((x for x in table.decls.values() if x.name == name and x.module == module and not x.type_identity and x.kind in self._TYPE_KINDS), None)

        found = pick(home)
        if found is not None:
            return found
        found = pick(self.table)
        if found is not None:
            return found
        h = self.pending_heads.get(name)
        if h is not None and h.name == name and h.module == module and not h.type_identity and h.kind in self._TYPE_KINDS:
            return h
        for sub in list(self.session.loaded.values()):
            found = pick(sub)
            if found is not None:
                return found
        for sub in list(self.session.loading.values()):
            found = pick(sub)
            if found is not None:
                return found
        return self._prelude_twin(name, module)

    def _prelude_twin(self, name: str, module: str) -> Optional[Decl]:
        """The prelude declaration of a name whose one other standard home is `module`.

        Facts of that type still resolve when the home module was not imported. A user class of the same name is
        not this declaration."""
        if self.core_module(name) != module:
            return None

        def pick(table: Optional[Table]) -> Optional[Decl]:
            if table is None:
                return None
            d = table.get(name)
            if d is not None and d.name == name and d.module == "fortis.prelude" and not d.type_identity and d.kind in self._TYPE_KINDS:
                return d
            return next((x for x in table.decls.values() if x.name == name and x.module == "fortis.prelude" and not x.type_identity and x.kind in self._TYPE_KINDS), None)

        found = pick(self.table)
        if found is not None:
            return found
        h = self.pending_heads.get(name)
        if h is not None and h.name == name and h.module == "fortis.prelude" and not h.type_identity and h.kind in self._TYPE_KINDS:
            return h
        for sub in list(self.session.loaded.values()) + list(self.session.loading.values()):
            found = pick(sub)
            if found is not None:
                return found
        return None

    def _stamped_core(self, d: Decl, home: Optional[Table]) -> Decl:
        """The core declaration a fallback found, as the module its origin names.

        An alias-qualified core type (D-140) and a core class found in a loaded module carry that module, the
        same stamp a type reference gets. A declaration that already carries a package identity is unchanged."""
        if d.type_identity or not d.module.startswith("fortis"):
            return d
        module = self.origin_of(d) or d.module
        return self.decl_by_module(d.name, module, home) or d

    def resolve_type(self, t: Type, home: Optional[Table] = None) -> Optional[Decl]:
        """The declaration of a named type. A set identity is that declaration, wherever it was imported; an empty
        identity is a name of `home` (the module whose types are being read) or, with no home, of this module.
        A core type carries its declaring module and does not fall back to a same-named declaration here."""
        if isinstance(t, Refined):
            t = t.base
        if not isinstance(t, Named):
            return None
        if t.package:
            # the loaded-module search is the same identity (D-54.11), never a same-named local declaration
            return self.decl_by_identity(t.name, t.package) or self.decl_from_loaded(t.name, t.package)
        if t.origin:
            return self.decl_by_module(t.name, t.origin, home)
        table = home if home is not None else self.table
        d = table.get(t.name)
        if d is not None and d.kind != "unknown":
            return d
        # a core type reached only through a module alias (`import fortis.host`, then `host.Host`; D-140),
        # in the module whose table is being read. The declaration is the core one, stamped with its module.
        found = next((x for k, x in table.decls.items() if "." in k and x.name == t.name and not x.type_identity and x.module.startswith("fortis") and x.kind in self._TYPE_KINDS), None)
        if found is not None:
            return self._stamped_core(found, table)
        loaded = self.decl_from_loaded(t.name, "")
        return self._stamped_core(loaded, table) if loaded is not None else None

    def home_table(self, d: Decl) -> Table:
        """The table `d`'s field and alias types were written against: the finished table of its own module."""
        if d.module == self.module_name:
            return self.table
        for (_, path), sub in list(self.session.loaded.items()):
            if path == d.module:
                hit = sub.get(d.name)
                if hit is not None and hit.type_identity == d.type_identity:
                    return sub
        for (_, path), sub in list(self.session.loading.items()):
            if path == d.module:
                hit = sub.get(d.name)
                if hit is not None and hit.type_identity == d.type_identity:
                    return sub
        return self.table

    def _fact_key(self, t: Type, d: Optional[Decl]):
        if not isinstance(t, Named):
            return ("", "")
        return (t.package or t.origin or (d.type_identity if d is not None else ""), t.name)

    def _next_home(self, d: Decl, home: Optional[Table]) -> Table:
        if d.type_identity:
            return self.home_table(d)
        return home if home is not None else self.table

    def loan_carrying_inner(self, t: Type) -> bool:
        s = t.inner if isinstance(t, OptionalT) else t
        if isinstance(s, Named):
            if s.name in ("StringView", "BytesView", "ListView", "ListCursor"):
                return True
            d = self.resolve_type(s)
            return d is not None and d.kind == "class" and d.lifetime is not None
        return isinstance(s, LoanT)

    def fn_result(self, fr) -> Type:
        """fnResult = fnType | namedType ["?"] | "(" loanType ")": the result of a function type."""
        kids = children(fr)
        first = kids[0]
        if name(first) == "fnType":
            return self.fn_type(first)
        if name(first) == "loanType":
            return self.type_of(("type", [first]))
        if isinstance(first, Token) and first.text == "(":
            inner = next(c for c in kids if name(c) == "loanType")
            return self.type_of(("type", [inner]))
        t = self.named_type(first)
        if len(kids) > 1 and isinstance(kids[1], Token) and kids[1].text == "?":
            return OptionalT(t)
        return t

    # ---- sets (6.11, 6.12) ---------------------------------------------------------
    def is_trivial(self, t: Type, seen=()) -> bool:
        return self.is_trivial_in(t, self.table, seen)

    def is_trivial_in(self, t: Type, table, seen=()) -> bool:
        """Triviality. A type that carries an identity is judged from the module that declares it; a name with no
        identity means what it means in `table` (an observation's field types, read in the module that declares the
        observation, which is not always the module that carries it)."""
        return self._trivial(t, seen, table)

    def _trivial(self, t: Type, seen, home: Optional[Table]) -> bool:
        if isinstance(t, OptionalT): return self._trivial(t.inner, seen, home)
        if isinstance(t, TupleT): return all(self._trivial(x, seen, home) for x in t.items)
        if isinstance(t, RecordT): return all(self._trivial(x, seen, home) for _, x in t.fields)
        if isinstance(t, Refined): return self._trivial(t.base, seen, home)
        if isinstance(t, (FnT, LoanT, DynamicT, TypeParam, Unknown)): return False
        if isinstance(t, Named):
            if t.name in TRIVIAL_BUILTINS: return True
            d = self.resolve_type(t, home)
            key = self._fact_key(t, d)
            if d is None or key in seen: return False
            nxt, seen2 = self._next_home(d, home), seen + (key,)
            if d.kind == "type": return self._trivial(d.alias, seen2, nxt)
            if d.kind == "enum": return all(self._trivial(ft, seen2, nxt) for v in d.variants.values() for _, ft in v)
            if d.kind == "class": return d.trivial
        return False

    def is_copyable(self, t: Type, seen=()) -> bool:
        return self._copyable(t, seen, None)

    def _copyable(self, t: Type, seen, home: Optional[Table]) -> bool:
        if self._trivial(t, seen, home if home is not None else self.table): return True
        if isinstance(t, OptionalT): return self._copyable(t.inner, seen, home)
        if isinstance(t, TupleT): return all(self._copyable(x, seen, home) for x in t.items)
        if isinstance(t, RecordT): return all(self._copyable(x, seen, home) for _, x in t.fields)
        if isinstance(t, Refined): return self._copyable(t.base, seen, home)
        if isinstance(t, TypeParam): return "Copyable" in self.tparam_bounds.get(t.name, ())
        if isinstance(t, (FnT, LoanT, DynamicT, Unknown)): return False
        if isinstance(t, Named):
            if t.name in NOT_COPYABLE: return False
            if t.name in COPYABLE_HEAP: return True
            if t.name in COPYABLE_CONTAINERS: return all(self._copyable(a, seen, home) for a in t.args if not (isinstance(a, Named) and a.name.startswith("#")))
            d = self.resolve_type(t, home)
            key = self._fact_key(t, d)
            if d is None or key in seen: return False
            nxt, seen2 = self._next_home(d, home), seen + (key,)
            if d.kind == "type": return self._copyable(d.alias, seen2, nxt)
            if d.kind == "enum": return all(self._copyable(ft, seen2, nxt) for v in d.variants.values() for _, ft in v)
            if d.kind == "class":
                if d.name in ("Capabilities",) or any(isinstance(i, Named) and i.name == "Authority" for i in d.implements): return False
                return all(self._copyable(ft, seen2, nxt) for ft, _, _ in d.fields.values())
        return False

    def is_linear(self, t: Type, seen=()) -> bool:
        return self._linear(t, seen, None)

    def _linear(self, t: Type, seen, home: Optional[Table]) -> bool:
        if isinstance(t, OptionalT): return self._linear(t.inner, seen, home)
        if isinstance(t, TupleT): return any(self._linear(x, seen, home) for x in t.items)
        if isinstance(t, RecordT): return any(self._linear(x, seen, home) for _, x in t.fields)
        if isinstance(t, Refined): return False
        if isinstance(t, FnT): return t.kind == "fn once"
        if isinstance(t, Named):
            if t.name in LINEAR_BUILTINS: return True
            if t.name in COPYABLE_CONTAINERS: return any(self._linear(a, seen, home) for a in t.args)
            d = self.resolve_type(t, home)
            key = self._fact_key(t, d)
            if d is None or key in seen: return False
            nxt, seen2 = self._next_home(d, home), seen + (key,)
            if d.kind == "type": return self._linear(d.alias, seen2, nxt)
            if d.kind == "enum": return any(self._linear(ft, seen2, nxt) for v in d.variants.values() for _, ft in v)
            if d.kind == "class": return any(self._linear(ft, seen2, nxt) for ft, _, _ in d.fields.values())
        return False

    # ---- implementation lookups --------------------------------------------------
    def implements(self, t: Type, iface: str, seen=()) -> bool:
        """Does type t implement interface iface (declared, derived, built-in, bundle or structural)?"""
        return self._implements(t, iface, seen, None)

    def _block_implements(self, t: Type, iface: str) -> bool:
        """An `implement` block in this module applies to `t` only when it names that same declaration.

        A block whose target has no identity matches only a type that has none (a single file, the core library).
        A block for this module's `Cee` does not make another module's `Cee` implement the interface (D-130).
        Origin is not part of that identity, so it is compared on its own: a user's class and a core type of
        the same name are different declarations when both package identities are empty."""
        if not isinstance(t, Named):
            return False
        for i, tt, _, _ in self.table.implement_blocks:
            if i.name != iface or tt.name != t.name or tt.origin != t.origin:
                continue
            if t.package:
                if tt.package == t.package:
                    return True
            elif not tt.package:
                return True
        return False

    def _implements(self, t: Type, iface: str, seen, home: Optional[Table]) -> bool:
        if iface == "Copyable": return self._copyable(t, (), home)
        if iface == "Shareable": return self._shareable(t, (), home)
        if isinstance(t, Refined): return self._implements(t.base, iface, seen, home)
        if isinstance(t, OptionalT):
            if iface == "Default": return True      # `T?` defaults to null (6.10)
            if iface in ("Encodable", "Decodable") and isinstance(t.inner, OptionalT): return False   # `T??`: present(null) has no canonical form (6.10)
            return iface in ("Equatable", "Hashable", "Encodable", "Decodable", "Displayable") and self._implements(t.inner, iface, seen, home)
        if isinstance(t, TupleT): return iface in ("Equatable", "Hashable", "Comparable", "Encodable", "Decodable") and all(self._implements(x, iface, seen, home) for x in t.items)
        if isinstance(t, RecordT): return iface in ("Equatable", "Hashable", "Encodable", "Decodable") and all(self._implements(x, iface, seen, home) for _, x in t.fields)
        if isinstance(t, TypeParam): return True   # bounds are checked at use (DEL-12)
        if isinstance(t, Unknown): return True
        if not isinstance(t, Named): return False
        if t.name in BUILTIN_IMPLEMENTS and iface in BUILTIN_IMPLEMENTS[t.name]: return True
        if t.name in COPYABLE_CONTAINERS and iface in ("Equatable", "Hashable", "Encodable", "Decodable", "Displayable", "Default"):
            if iface == "Default": return True
            return all(self._implements(a, iface, seen, home) for a in t.args if not (isinstance(a, Named) and a.name.startswith("#")))
        if t.name in ("Shared", "Weak", "Mutex") and iface == "Shareable": return True
        d = self.resolve_type(t, home)
        key = (self._fact_key(t, d)[0], t.name, iface)
        if d is None or key in seen: return False
        seen = seen + (key,)
        nxt = self._next_home(d, home)
        # the interface name is the one the question was asked with: a name of the module doing the checking
        idecl = self.table.get(iface)
        if d.kind == "type":
            if iface in ("Encodable", "Decodable", "Codec", "Equatable", "Hashable", "Comparable", "Displayable", "Default"):
                return self._implements(d.alias, iface, seen, nxt)
        if any(i.name == iface for i in d.implements): return True
        if any(n == iface for n, _ in d.derives): return True
        if iface in ("Encodable", "Decodable") and any(n == "Codec" for n, _ in d.derives): return True
        if self._block_implements(t, iface): return True
        # bundle: an interface with requirements and no members of its own
        if idecl is not None and idecl.kind == "interface" and not idecl.methods and not idecl.constants and idecl.requires:
            return all(self._implements(t, r.name, seen, home) for r in idecl.requires)
        if iface == "Codec" and idecl is None:
            return self._implements(t, "Encodable", seen, home) and self._implements(t, "Decodable", seen, home)
        return False

    def is_shareable(self, t: Type, seen=()) -> bool:
        return self._shareable(t, seen, None)

    def _shareable(self, t: Type, seen, home: Optional[Table]) -> bool:
        if self._trivial(t, seen, home if home is not None else self.table): return True
        if isinstance(t, TypeParam): return "Shareable" in self.tparam_bounds.get(t.name, ())
        if isinstance(t, (LoanT, DynamicT, Unknown)): return False
        if isinstance(t, OptionalT): return self._shareable(t.inner, seen, home)
        if isinstance(t, Refined): return self._shareable(t.base, seen, home)
        if isinstance(t, Named):
            if t.name in ("StringView", "BytesView", "ListView", "MutexGuard", "Future", "Task"): return False   # 12.7: loans, views, guards, futures, tasks
            if t.name in COPYABLE_HEAP: return True
            if t.name in COPYABLE_CONTAINERS: return all(self._shareable(a, seen, home) for a in t.args if not (isinstance(a, Named) and a.name.startswith("#")))
            if t.name in ("Shared", "Weak", "Mutex"): return all(self._shareable(a, seen, home) for a in t.args)
            if t.name in ("AtomicInt", "AtomicBool"): return True
            d = self.resolve_type(t, home)
            key = self._fact_key(t, d)
            if d is None or key in seen: return False
            nxt, seen2 = self._next_home(d, home), seen + (key,)
            if d.kind == "class":
                if d.lifetime: return False
                if any(i.name == "Authority" for i in d.implements): return True
                return all(self._shareable(ft, seen2, nxt) for ft, _, _ in d.fields.values())
            if d.kind == "enum": return all(self._shareable(ft, seen2, nxt) for v in d.variants.values() for _, ft in v)
            if d.kind == "type": return self._shareable(d.alias, seen2, nxt)
        if isinstance(t, (TupleT,)): return all(self._shareable(x, seen, home) for x in t.items)
        if isinstance(t, RecordT): return all(self._shareable(x, seen, home) for _, x in t.fields)
        if isinstance(t, FnT): return t.kind in ("fn", "fn once")    # captures are a value property; `fn mut` never
        return False

    # ---- checks ----------------------------------------------------------------------
    def check(self):
        for d in list(self.table.decls.values()):
            if d.module != self.module_name:
                continue
            if d.kind == "observation":
                for fname, (ft, req) in d.obs_fields.items():
                    if isinstance(ft, Named) and ft.name == "String":
                        self.rec("E-OBS-012", f"observation {d.name}: String field `{fname}` must declare a scalar bound (`String<scalars: 0..n>`) so the frame stays bounded (13)", "LANG-13-03", self.tok_of(d), {"frame_field": fname, "exit_path": "declaration"})
                    if known(ft) and not self.encodable(ft):
                        self.rec("E-TYP-090", f"observation {d.name}: field `{fname}: {ft.show()}` is not Encodable; secrets, authorities, loans, resources, closures, futures and tasks cannot be captured (13)", "LANG-13-01", self.tok_of(d), {"expected_type": "Encodable", "actual_type": ft.show(), "primary_span": fname})
            if d.kind in ("class", "enum"):
                self.check_type_decl(d)
            elif d.kind == "interface":
                self.check_interface(d)
            self.check_value_types(d)
            self.check_trusted(d)
        self.check_trusted_blocks()
        self.check_coherence()
        return self.records

    def std_module_name(self, module: str) -> bool:
        """Is `module` one of the toolchain's standard library modules, of a package the published compatibility set
        lists (library/fortis/compatibility.yaml)? A package that merely names itself `fortis.*` is not (D-146)."""
        if module == "fortis.prelude":
            return True
        lm = self.lib.modules.get(module)
        if lm is None or getattr(lm, "package", "fortis").split("@")[0] not in standard_packages():
            return False
        std = tuple(os.path.join(ROOT, d) + os.sep for d in ("library", os.path.join("reference", "library")))
        return os.path.realpath(lm.path).startswith(std)

    def standard_module(self) -> bool:
        """The module being checked is a standard library module, checked from the standard library's own sources (the
        shipped library/ or its reference copy), not from a package that merely names itself `fortis.*` (D-146)."""
        std = tuple(os.path.join(ROOT, d) + os.sep for d in ("library", os.path.join("reference", "library")))
        return self.std_module_name(self.module_name) and os.path.realpath(os.path.join(ROOT, self.file)).startswith(std)

    def authority_qualifiers(self, d: Decl) -> List[Optional[str]]:
        """How d's own declaration names each `Authority` it implements: None for the bare name, else the module alias
        that qualifies it (`implements pre.Authority`), in its `implements` list or an implementation block (D-172)."""
        def qual(tr) -> Optional[Tuple[Optional[str]]]:
            ts = direct_tokens(tr)
            if len(ts) >= 3 and ts[0].kind == "LOWER" and ts[1].text == "." and ts[2].text == "Authority":
                return (ts[0].text,)
            return (None,) if ts and ts[0].text == "Authority" else None
        out: List[Optional[str]] = []
        after = False
        for k in (children(d.node) if d.node is not None else []):
            if isinstance(k, Token):
                after = k.text == "implements"
            elif name(k) == "typeList" and after:
                for ty in [c for c in children(k) if name(c) == "type"]:
                    tr = find(ty, "typeRef")
                    q = qual(tr) if tr is not None else None
                    if q is not None:
                        out.append(q[0])
                after = False
        for i, tt, _, n in self.table.implement_blocks:
            if i.name == "Authority" and tt.name == d.name:
                tr = find(n, "typeRef")
                q = qual(tr) if tr is not None else None
                if q is not None:
                    out.append(q[0])
        return out

    def check_trusted_blocks(self):
        """An implementation block's members are functions too: none may be `trusted` outside the standard packages
        (D-146)."""
        if self.standard_module():
            return
        for _, target, module, node in self.table.implement_blocks:
            if module != self.module_name:
                continue
            for m in [c for c in children(node) if name(c) == "implementMember"]:
                head = find(m, "methodHead")
                if head is None:
                    continue
                sig = self.signature(head, True, find(m, "funcBody"))
                if "trusted" in sig.modifiers:
                    label = f"{target.name}.{sig.name}"
                    self.rec("E-EFF-001", f"{label} is declared `trusted`, which only the standard packages may write: a program's effects are proved, never declared (11.3)", "LANG-11-04", tokens(head)[0],
                             {"required_contract": "a standard package", "violating_effect": "trusted", "witness_path": label})

    def implements_core_authority(self, d: Decl) -> bool:
        """Does declaration d implement the core marker `Authority` (fortis.prelude), resolved as d's own module names
        it: a program's own `interface Authority` is not the marker (D-172)."""
        if not (any(i.name == "Authority" for i in d.implements) or any(i.name == "Authority" and tt.name == d.name for i, tt, _, _ in self.table.implement_blocks)):
            return False
        if self.std_module_name(d.module):
            return True
        table = self.table if d.module == self.module_name else next((sub for (_, path), sub in list(self.session.loaded.items()) if path == d.module), None)
        if table is None:
            return True                      # a module whose names are not loaded here: the marker by its name
        for q in self.authority_qualifiers(d):
            a = table.decls.get(f"{q}.Authority") if q else table.get("Authority")
            if a is not None and self.std_module_name(a.module):
                return True
        return False

    def check_trusted(self, d: Decl):
        """`trusted` declares effects the checker does not prove, so only the standard library and its audited first-party
        adapters, the packages of the published compatibility set, may write it (11.3; D-146, the integrator's decision)."""
        if self.standard_module():
            return
        sigs = [(d.name, d.sig)] if d.kind == "func" and d.sig is not None else [(f"{d.name}.{m}", s) for m, s in d.methods.items()]
        for label, sig in sigs:
            if "trusted" in sig.modifiers:
                tok = tokens(sig.node)[0] if sig.node is not None else self.tok_of(d)
                self.rec("E-EFF-001", f"{label} is declared `trusted`, which only the standard packages may write: a program's effects are proved, never declared (11.3)",
                         "LANG-11-04", tok, {"required_contract": "a standard package", "violating_effect": "trusted", "witness_path": label})

    def bare_interfaces(self, t: Type) -> List[str]:
        """The interfaces `t` names as value types, without `dynamic` (9.5): no value has such a type (D-21)."""
        out: List[str] = []

        def walk(x):
            if isinstance(x, Named):
                d = self.table.get(x.name)
                if d is None or (x.package and d.type_identity != x.package):
                    # an interface reached through a module alias (`data.Connection`), even past a same-named local
                    # declaration: by its identity (D-21)
                    d = next((v for v in self.table.decls.values() if v.name == x.name and v.kind == "interface" and (not x.package or v.type_identity == x.package)), None)
                if d is not None and d.kind == "interface" and (not x.package or not d.type_identity or d.type_identity == x.package):
                    out.append(x.name)
                for a in x.args:
                    walk(a)
            elif isinstance(x, (OptionalT, LoanT)):
                walk(x.inner)
            elif isinstance(x, TupleT):
                for i in x.items:
                    walk(i)
            elif isinstance(x, RecordT):
                for _, f in x.fields:
                    walk(f)
            elif isinstance(x, FnT):
                for p in x.params:
                    walk(p)
                walk(x.ret)
        walk(t)
        return out

    def check_value_types(self, d: Decl):
        """A field, variant payload, parameter or result whose type is an interface itself is refused at its declaration
        (D-21): an interface is used through `dynamic I`, `Shared[dynamic I]` or a type parameter bounded by it (9.5)."""
        def refuse(t: Type, what: str, tok):
            for i in self.bare_interfaces(t):
                self.rec("E-TYP-040", f"{what} has the interface type {i}, which no value has: write `dynamic {i}`, `Shared[dynamic {i}]` or a type parameter bounded by {i} (9.5)",
                         "LANG-06-01", tok, {"expected_type": f"dynamic {i}", "actual_type": i, "primary_span": what})
        tok = self.tok_of(d)
        for fname, (ft, _, _) in d.fields.items():
            refuse(ft, f"field `{fname}` of {d.name}", tok)
        for vname, payload in d.variants.items():
            for fname, ft in payload:
                refuse(ft, f"field `{fname}` of variant {d.name}.{vname}", tok)
        sigs = [(d.name, d.sig)] if d.kind == "func" and d.sig is not None else [(f"{d.name}.{m}", s) for m, s in d.methods.items()]
        for label, sig in sigs:
            at = tokens(sig.node)[0] if sig.node is not None else tok
            for pn, pt, _ in sig.params:
                refuse(pt, f"parameter `{pn}` of {label}", at)
            refuse(sig.ret, f"the result of {label}", at)
        if d.module == self.module_name and d.kind in ("func", "class", "enum", "interface"):
            for q, n, qtok in self.shadowed_qualified_interfaces(d.node):
                self.rec("E-TYP-040", f"`{q}` is the interface {n}, which no value has: write `dynamic {q}`, `Shared[dynamic {q}]` or a type parameter bounded by it (9.5)", "LANG-06-01", qtok, {"expected_type": f"dynamic {q}", "actual_type": n, "primary_span": q})

    def shadowed_qualified_interfaces(self, node) -> List[Tuple[str, str, Token]]:
        """Alias-qualified type references in a declaration's own types (not its bodies, constants, `where` bounds,
        `implements`, `requires` or `with` lists, nor the name after `dynamic`) that name an interface whose bare name
        this module gives to another declaration. A type carries only the name, so the walk over types cannot tell
        `data.Connection` from a local `Connection` (D-21)."""
        out: List[Tuple[str, str, Token]] = []

        def walk(n, dynamic_ref: bool = False):
            if n is None or isinstance(n, Token):
                return
            nm = name(n)
            if nm in ("funcBody", "block", "constExpr", "whereClause", "typeList", "aspectPart"):
                return
            if nm == "typeRef" and not dynamic_ref:
                ts = direct_tokens(n)
                if len(ts) >= 3 and ts[0].kind == "LOWER" and ts[1].text == ".":
                    d = self.table.decls.get(ts[0].text + "." + ts[2].text)
                    bare = self.table.get(ts[2].text)
                    if d is not None and d.kind == "interface" and bare is not None and bare is not d:
                        out.append((ts[0].text + "." + ts[2].text, ts[2].text, ts[2]))
            dyn = nm in ("loanType", "fnType") and any(t.text == "dynamic" for t in direct_tokens(n))
            for c in children(n):
                walk(c, dyn and name(c) == "typeRef")

        walk(node)
        return out

    def tok_of(self, d: Decl) -> Token:
        return next((t for t in tokens(d.node) if t.kind in ("UPPER", "LOWER")), tokens(d.node)[0])

    def check_type_decl(self, d: Decl):
        tok = self.tok_of(d)
        self_t = d.as_named(tuple(TypeParam(p) for p in d.type_params))
        field_types = [ft for ft, _, _ in d.fields.values()] if d.kind == "class" else [ft for v in d.variants.values() for _, ft in v]
        # trivial opt-in
        if d.trivial:
            for fname, (ft, _, _) in d.fields.items():
                if not self.is_trivial(ft):
                    self.rec("E-OWN-030", f"trivial class {d.name} has non-trivial field `{fname}: {ft.show()}`", "LANG-06-07", tok,
                             {"place": fname, "conflict": "trivial", "spans": fname, "loan_region": ""})
        # derive
        derived = {n for n, _ in d.derives}
        for iface, args in d.derives:
            if iface not in DERIVABLE:
                self.rec("E-TYP-090", f"`{iface}` cannot be derived", "LANG-06-07", tok, {"expected_type": "a derivable interface", "actual_type": iface, "primary_span": d.name})
                continue
            if any(i.name == iface for i in d.implements):
                self.rec("E-MIX-001", f"{d.name} both derives and implements `{iface}`", "LANG-09-04", tok, {"expected_type": "derive or implement", "actual_type": "both", "primary_span": iface})
            for hand in sorted(self.BUILTIN_MEMBERS.get(iface, set()) & set(d.methods)):
                self.rec("E-MIX-001", f"{d.name} derives {iface} and also declares `{hand}`, which the derivation would generate", "LANG-09-04", tok, {"expected_type": "one source of the member", "actual_type": "derived and declared", "primary_span": hand})
            if iface == "From":
                if not args:
                    self.rec("E-TYP-090", "derive From needs a source type", "LANG-06-07", tok, {"expected_type": "From[S]", "actual_type": "From", "primary_span": d.name})
                else:
                    self.check_derive_from(d, args[0], tok)
                continue
            if iface == "Default":
                for fname, (ft, _, has_default) in d.fields.items():
                    if not has_default and not self.implements(ft, "Default"):
                        self.rec("E-TYP-090", f"derive Default: field `{fname}: {ft.show()}` has no initialiser and its type has no Default", "LANG-06-07", tok,
                                 {"expected_type": "an initialiser or a Default type", "actual_type": ft.show(), "primary_span": fname})
                continue
            needed = ["Encodable", "Decodable"] if iface == "Codec" else [iface]
            for need in needed:
                if need in ("Encodable", "Decodable"):
                    for fname, (ft, exported, _) in d.fields.items():
                        if not exported:
                            self.rec("E-TYP-090", f"derive {need}: field `{fname}` is not exported", "LANG-06-07", tok, {"expected_type": "every field exported", "actual_type": f"private {fname}", "primary_span": fname})
                        elif not self.encodable(ft):
                            self.rec("E-TYP-090", f"derive {need}: field `{fname}: {ft.show()}` is not {need}", "LANG-06-07", tok, {"expected_type": need, "actual_type": ft.show(), "primary_span": fname})
                    for vname, payload in d.variants.items():
                        for fname, ft in payload:
                            if not self.encodable(ft):
                                self.rec("E-TYP-090", f"derive {need}: payload `{vname}.{fname}: {ft.show()}` is not {need}", "LANG-06-07", tok, {"expected_type": need, "actual_type": ft.show(), "primary_span": fname})
                else:
                    for fname, ft in [(f, t) for f, (t, _, _) in d.fields.items()] + [(f"{v}.{f}", t) for v, p in d.variants.items() for f, t in p]:
                        if not self.implements(ft, need):
                            self.rec("E-TYP-090", f"derive {need}: field `{fname}: {ft.show()}` does not implement {need}", "LANG-06-07", tok, {"expected_type": need, "actual_type": ft.show(), "primary_span": fname})
                    if need in IFACE_REQUIRES:
                        for req in IFACE_REQUIRES[need]:
                            if not (req in derived or self.implements(self_t, req)):
                                self.rec("E-TYP-080", f"derive {need} requires {req}, which {d.name} neither derives nor implements", "LANG-09-03", tok, {"expected_type": req, "actual_type": d.name, "primary_span": d.name})
        # implements: completeness, compatibility, requires
        for iface in d.implements:
            self.check_implements(d, iface, d.methods, tok)
        # mixins: requires satisfied, conflicts
        provided: Dict[str, str] = {}
        for mx in d.mixins:
            md = self.table.get(mx.name)
            if md is None or md.kind != "mixin":
                self.rec("E-TYP-080", f"`{mx.name}` is not a mixin", "LANG-09-04", tok, {"expected_type": "a mixin", "actual_type": mx.name, "primary_span": d.name})
                continue
            for req in md.requires:
                if req.name.startswith("__method__:"):
                    mname = req.name.split(":", 1)[1]
                    if mname not in d.methods and mname not in self.interface_members(d):
                        self.rec("E-TYP-080", f"mixin {mx.name} requires method `{mname}`, which {d.name} lacks", "LANG-09-04", tok, {"expected_type": mname, "actual_type": "absent", "primary_span": d.name})
                elif not self.implements(self_t, req.name):
                    self.rec("E-TYP-080", f"mixin {mx.name} requires {req.name}, which {d.name} does not implement", "LANG-09-04", tok, {"expected_type": req.name, "actual_type": d.name, "primary_span": d.name})
            for mname in md.methods:
                if mname.startswith("requires:"):
                    continue
                if mname in d.methods:
                    continue    # the class's own method takes precedence
                if mname in provided:
                    self.rec("E-MIX-001", f"mixins {provided[mname]} and {mx.name} both provide `{mname}` and {d.name} declares no `{mname}`", "LANG-09-04", tok, {"expected_type": "one source of the member", "actual_type": "two mixins", "primary_span": mname})
                provided[mname] = mx.name

    BUILTIN_MEMBERS = {"Equatable": {"equals"}, "Hashable": {"hash"}, "Comparable": {"compare"}, "Displayable": {"display"}, "Encodable": {"encode"},
                       "Decodable": {"decode"}, "Codec": {"encode", "decode"}, "Default": {"default"}, "From": {"from"}, "Abandonable": {"abandon"}}

    def interface_members(self, d: Decl) -> Set[str]:
        """Members a type has by derivation or by implementing an interface (generated or required)."""
        out: Set[str] = set()
        for iface, _ in d.derives:
            out |= self.BUILTIN_MEMBERS.get(iface, set())
            idecl = self.table.get(iface)
            if idecl is not None:
                out |= set(idecl.methods) | set(idecl.constants)
        for iface in d.implements:
            out |= self.BUILTIN_MEMBERS.get(iface.name, set())
        return out

    def encodable(self, t: Type) -> bool:
        if isinstance(t, Named) and t.name in NOT_ENCODABLE: return False
        if isinstance(t, (LoanT, FnT, DynamicT)): return False
        if isinstance(t, Named):
            d = self.resolve_type(t)
            if d is not None and d.kind == "class" and any(i.name == "Authority" for i in d.implements):
                return False
        return self.implements(t, "Encodable")

    def check_derive_from(self, d: Decl, source: Type, tok):
        sd = self.resolve_type(source)
        if sd is None or sd.kind != "class":
            if isinstance(source, Unknown) or sd is None and isinstance(source, Named):
                return     # unknown source: not provable here (PROG-DIA-02 unknown)
            self.rec("E-TYP-090", f"derive From[{source.show()}]: the source is not a class", "LANG-06-07", tok, {"expected_type": "a class", "actual_type": source.show(), "primary_span": d.name})
            return
        for fname, (ft, _, _) in d.fields.items():
            if fname not in sd.fields:
                self.rec("E-TYP-090", f"derive From[{source.show()}]: {source.show()} has no field `{fname}`", "LANG-06-07", tok, {"expected_type": fname, "actual_type": "absent", "primary_span": fname})
                continue
            st, exp, _ = sd.fields[fname]
            if not exp and sd.module != d.module:
                self.rec("E-TYP-090", f"derive From[{source.show()}]: field `{fname}` of the source is not visible", "LANG-06-07", tok, {"expected_type": "an exported field", "actual_type": fname, "primary_span": fname})
            if not (self.same_type(st, ft) or self.implements(ft, "From")):
                self.rec("E-TYP-090", f"derive From[{source.show()}]: field `{fname}` has type {st.show()}, needs {ft.show()}", "LANG-06-07", tok, {"expected_type": ft.show(), "actual_type": st.show(), "primary_span": fname})
            if not self.is_copyable(st):
                self.rec("E-TYP-090", f"derive From[{source.show()}]: field `{fname}: {st.show()}` is not Copyable", "LANG-06-07", tok, {"expected_type": "Copyable", "actual_type": st.show(), "primary_span": fname})

    def same_type(self, a: Type, b: Type) -> bool:
        return a == b or isinstance(a, Unknown) or isinstance(b, Unknown)

    def check_implements(self, d: Decl, iface: Named, methods: Dict[str, Sig], tok, via_block=False):
        idecl = self.table.get(iface.name)
        if idecl is None:
            if iface.name in ("Authority", "Copyable", "Shareable", "Default", "Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Codec", "Abandonable", "From"):
                return
            self.rec("E-TYP-080", f"`{iface.name}` is not an interface", "LANG-09-03", tok, {"expected_type": "an interface", "actual_type": iface.name, "primary_span": d.name}, status="unknown")
            return
        if idecl.kind != "interface":
            self.rec("E-TYP-080", f"`{iface.name}` is not an interface", "LANG-09-03", tok, {"expected_type": "an interface", "actual_type": idecl.kind, "primary_span": d.name})
            return
        self_t = d.as_named(tuple(TypeParam(p) for p in d.type_params))
        for mname, isig in idecl.methods.items():
            have = methods.get(mname)
            if have is None:
                self.rec("E-TYP-080", f"{d.name} implements {iface.name} but lacks `{isig.show()}`", "LANG-09-03", tok, {"expected_type": isig.show(), "actual_type": "absent", "primary_span": mname})
                continue
            problems = self.compatible(isig, have, iface, d)
            for p in problems:
                self.rec("E-TYP-080", f"{d.name}.{mname} does not satisfy {iface.name}.{mname}: {p}", "LANG-09-03", tok, {"expected_type": isig.show(), "actual_type": have.show(), "primary_span": mname})
        for cname in idecl.constants:
            if cname not in d.constants:
                self.rec("E-TYP-080", f"{d.name} implements {iface.name} but lacks constant `{cname}`", "LANG-09-03", tok, {"expected_type": cname, "actual_type": "absent", "primary_span": cname})
        for req in idecl.requires:
            if not self.implements(self_t, req.name):
                self.rec("E-TYP-080", f"{d.name} implements {iface.name}, which requires {req.name}; {d.name} does not implement it", "LANG-09-03", tok, {"expected_type": req.name, "actual_type": d.name, "primary_span": d.name})

    def contract_decl(self, c: str, module: Optional[str]) -> Optional[Decl]:
        """A contract named in a signature, as its declaring module names it, whatever this module imports (review of
        D-61): this table, then the declaring package module's own table."""
        d = self.table.get(c)
        if d is not None and d.kind == "contract" and (module is None or d.module == module):
            return d
        x = next((v for v in self.table.decls.values() if v.kind == "contract" and v.name == c and v.module == module), None)
        if x is not None:
            return x
        for (_, path), sub in list(self.session.loaded.items()):
            if path == module:
                y = sub.get(c)
                if y is not None and y.kind == "contract":
                    return y
        return d if d is not None and d.kind == "contract" else None

    def claims(self, sig: Sig, module: Optional[str] = None) -> Tuple[Set[str], Set[str], Set[str]]:
        """A signature's claims (11.2, 11.5): its modifiers with those its contracts expand to, the effects it rules out
        (a `pure` or `noalloc` claim, a contract's `forbids`, and each of `suspend`, `fail`, `recovers` its modifiers do
        not permit), and the authorities its contracts exclude."""
        mods, forbids, excludes = set(sig.modifiers), set(), set()
        for c in sig.contracts:
            cd = self.contract_decl(c, module)
            if cd is not None and cd.kind == "contract":
                mods |= cd.items.get("modifiers", set())
                forbids |= cd.items.get("forbids", set())
                excludes |= cd.items.get("excludes", set())
        if "pure" in mods:
            forbids |= set(PURE_FORBIDS)
        if "noalloc" in mods:
            forbids.add("alloc")
        for effect, mod in (("suspend", "async"), ("fail", "try"), ("recovers", "recovers")):
            if mod not in mods:
                forbids.add(effect)
        return mods, forbids, excludes

    def compatible(self, isig: Sig, have: Sig, iface: Named, d: Decl) -> List[str]:
        """An implementation may not strengthen a precondition or weaken a guarantee (9.5). Every claim of the interface
        method binds it (11.2), written or through a contract: a caller through the interface or `dynamic` relies on
        exactly those (D-61)."""
        problems = []
        if (isig.receiver or "") != (have.receiver or ""):
            problems.append(f"receiver `{have.receiver}` differs from `{isig.receiver}`")
        idecl0 = self.table.get(iface.name)
        imods, iforbids, iexcludes = self.claims(isig, idecl0.module if idecl0 is not None else None)
        hmods, hforbids, hexcludes = self.claims(have, d.module)
        explained: Set[str] = set()
        for claim, effects in (("pure", PURE_FORBIDS), ("noalloc", ("alloc",))):
            if claim in imods and claim not in hmods:
                problems.append(f"drops `{claim}`")
                explained |= set(effects)
        for weak, effect in (("try", "fail"), ("async", "suspend"), ("recovers", "recovers")):
            if weak in hmods and weak not in imods:
                problems.append(f"adds `{weak}`")
                explained.add(effect)
        for effect in sorted(iforbids - hforbids - explained):
            problems.append(f"does not rule out `{effect}`, which the interface's contract forbids")
        for a in sorted(iexcludes - hexcludes):
            problems.append(f"does not exclude capability {a}, which the interface's contract excludes")
        if len(isig.params) != len(have.params):
            problems.append("parameter count differs")
            return problems
        subst = {"Self": d.as_named(tuple(TypeParam(p) for p in d.type_params))}
        idecl = self.table.get(iface.name)
        if idecl is not None:
            for p, a in zip(idecl.type_params, iface.args):
                subst[p] = a
        for (pn, pt, pm), (hn, ht, hm) in zip(isig.params, have.params):
            pt2 = self.subst(pt, subst)
            if pm != hm:
                problems.append(f"parameter `{hn}` mode `{hm}` differs from `{pm}`")
            if not self.param_compatible(pt2, ht):
                problems.append(f"parameter `{hn}` narrows {pt2.show()} to {ht.show()}")
        rt = self.subst(isig.ret, subst)
        if not self.return_compatible(rt, have.ret):
            problems.append(f"return {have.ret.show()} widens {rt.show()}")
        return problems

    def subst(self, t: Type, m: Dict[str, Type]) -> Type:
        if isinstance(t, Named):
            if t.name in m and not t.args: return m[t.name]
            return Named(t.name, tuple(self.subst(a, m) for a in t.args), t.package, t.origin)
        if isinstance(t, TypeParam) and t.name in m: return m[t.name]
        if isinstance(t, OptionalT): return OptionalT(self.subst(t.inner, m))
        if isinstance(t, LoanT): return LoanT(self.subst(t.inner, m), t.mode, t.origin)
        if isinstance(t, TupleT): return TupleT(tuple(self.subst(x, m) for x in t.items))
        if isinstance(t, FnT): return FnT(tuple(self.subst(x, m) for x in t.params), self.subst(t.ret, m), t.kind, t.modifiers, "", t.dynamic, t.identity)
        if isinstance(t, RecordT): return RecordT(tuple((k, self.subst(x, m)) for k, x in t.fields))
        return t

    def param_compatible(self, iface_t: Type, impl_t: Type) -> bool:
        """The implementation's parameter type must accept everything the interface's does."""
        if isinstance(iface_t, Unknown) or isinstance(impl_t, Unknown): return True
        if isinstance(iface_t, Refined) and isinstance(impl_t, Refined) and iface_t.base == impl_t.base:
            lo_ok = impl_t.lo is None or (iface_t.lo is not None and impl_t.lo <= iface_t.lo)
            hi_ok = impl_t.hi is None or (iface_t.hi is not None and impl_t.hi >= iface_t.hi)
            return lo_ok and hi_ok
        if isinstance(iface_t, Refined) and isinstance(impl_t, Named): return iface_t.base == impl_t
        if isinstance(iface_t, Named) and isinstance(impl_t, Refined): return False
        return iface_t == impl_t

    def return_compatible(self, iface_t: Type, impl_t: Type) -> bool:
        if isinstance(iface_t, Unknown) or isinstance(impl_t, Unknown): return True
        if isinstance(iface_t, Refined) and isinstance(impl_t, Refined) and iface_t.base == impl_t.base:
            lo_ok = iface_t.lo is None or (impl_t.lo is not None and impl_t.lo >= iface_t.lo)
            hi_ok = iface_t.hi is None or (impl_t.hi is not None and impl_t.hi <= iface_t.hi)
            return lo_ok and hi_ok
        if isinstance(iface_t, Named) and isinstance(impl_t, Refined): return impl_t.base == iface_t
        if isinstance(iface_t, Refined) and isinstance(impl_t, Named): return False
        return iface_t == impl_t

    def check_interface(self, d: Decl):
        tok = self.tok_of(d)
        # requirement cycle
        seen, stack = set(), [d.name]
        def visit(n, path):
            idecl = self.table.get(n)
            if idecl is None or idecl.kind != "interface":
                return False
            for r in idecl.requires:
                if r.name == d.name:
                    self.rec("E-TYP-060", f"interface requirement cycle: {' requires '.join(path + [r.name])}", "LANG-09-03", tok, {"expected_type": "an acyclic requirement graph", "actual_type": " -> ".join(path + [r.name]), "primary_span": d.name})
                    return True
                if r.name not in seen:
                    seen.add(r.name)
                    if visit(r.name, path + [r.name]):
                        return True
            return False
        visit(d.name, [d.name])

    def check_coherence(self):
        seen: Dict[Tuple[str, str], str] = {}
        for iface, target, module, node in self.table.implement_blocks:
            tok = tokens(node)[0]
            idecl, tdecl = self.table.get(iface.name), self.table.get(target.name)
            local_iface = idecl is not None and idecl.package == self.package and idecl.module.split(".")[0] not in ("fortis",) and not idecl.module.startswith("fortis.")
            local_type = tdecl is not None and tdecl.package == self.package and not tdecl.module.startswith("fortis.")
            if not (local_iface or local_type):
                self.rec("E-TYP-100", f"`implement {iface.name} for {target.name}`: neither is declared in this package (coherence)", "LANG-06-10", tok, {"expected_type": "a local interface or type", "actual_type": f"{iface.name}/{target.name}", "primary_span": target.name})
            key = (iface.name, target.name)
            if key in seen or (tdecl is not None and (any(i.name == iface.name for i in tdecl.implements) or any(n == iface.name for n, _ in tdecl.derives))):
                self.rec("E-TYP-100", f"two implementations of {iface.name} for {target.name}", "LANG-06-10", tok, {"expected_type": "one implementation", "actual_type": "two", "primary_span": target.name})
            seen[key] = module
            if idecl is not None and idecl.kind == "interface" and tdecl is not None:
                methods = {}
                for m in [c for c in children(node) if name(c) == "implementMember"]:
                    head = find(m, "methodHead")
                    if head is not None:
                        sig = self.signature(head, True, find(m, "funcBody"))
                        methods[sig.name] = sig
                self.check_implements(tdecl, iface, methods, tok, via_block=True)


class Session:
    """The declaration tables of package modules for one check, built once per (package library, module) and dropped
    with the check, as fortis-types' `Session` is: a later check, whatever its libraries hold, builds its own."""
    def __init__(self):
        self.loaded: Dict[Tuple[int, str], Table] = {}
        self.loading: Dict[Tuple[int, str], Table] = {}    # the heads of the modules whose tables are being built
        # unannotated constant -> its initialiser's type, keyed by the declaring module and the name. An import
        # copies the declaration (its alias stays Unknown); the type is read from here (D-54.4).
        self.inferred_consts: Dict[Tuple[str, str], Type] = {}
        # (type identity, class, field, path) -> the one lambda that position holds (8.4). The identity is the
        # declaring package and module, so the class name keeps two classes in one module apart. Shared by every
        # module of the check and both typing passes: the second pass re-notes the same identity.
        self.field_closures: Dict[Tuple[str, str, str, str], str] = {}


def heads(mtree, module: str, package: str) -> Table:
    """A module's declarations as heads: kind, name, visibility, identity and lifetime parameter, with no types.

    Building a declaration reads only these of the declarations its types name (`named_of`, `loan_carrying_inner`),
    so a module of an import cycle that is met again while its own table is being built is seen as its heads. Every
    member's table is then the same whichever member the session enters first, and one module's import order never
    changes another module's records (D-38, D-35)."""
    t = Table()
    for d in tree.declarations(mtree):
        if d["name"] is None or d["kind"] in ("head", "test", "property", "implement"):
            continue
        h = Decl(d["kind"], d["name"], d["exported"], module, package, node=d["node"])
        lp = find(d["node"], "lifetimeParam")
        if lp is not None:
            h.lifetime = tokens(lp)[2].text
        t.decls[d["name"]] = h          # as in a library load, a later declaration of the name replaces an earlier one
    return t


def _loaded(sess: Session, lm, path: str) -> Table:
    """The declaration table of a package module, built once per (package library, module) in the check's session;
    while it is being built (an import cycle leads back to it), its heads."""
    key = (id(lm.library), path)
    if key in sess.loaded:
        return sess.loaded[key]
    if key in sess.loading:
        return sess.loading[key]
    sess.loading[key] = heads(lm.tree, path, lm.package)
    try:
        sub = DeclChecker(lm.library, lm.path, path, lm.package, sess)
        sub.load(lm.tree, is_library=True)
    finally:
        del sess.loading[key]
    sess.loaded[key] = sub.table
    return sub.table


def check_source(library: names.Library, src: str, file: str, package: str = "") -> Tuple[List[dict], DeclChecker]:
    t = library.rec.parse(lexer.lex(src), "module")
    c = DeclChecker(library, file, "main", package)
    c.load(t)
    c.check()
    return c.records, c
