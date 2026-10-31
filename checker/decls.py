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
from dataclasses import dataclass, field, replace
from typing import Dict, List, Optional, Set, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
import diagnostics  # noqa: E402
import lexer  # noqa: E402
import names  # noqa: E402
from reading import CAPABILITY_TYPES  # noqa: E402
import tree  # noqa: E402
from lexer import Token  # noqa: E402
from tree import children, name, direct_tokens, find, find_all, tokens  # noqa: E402

from checker import conversions as conv_table  # noqa: E402
from checker.effects import AUTHORITIES, PURE_FORBIDS, ROOT, contract_contradictions, standard_packages  # noqa: E402
from checker.literals import assignable, is_strlit, known, strip, origin_clash  # noqa: E402
from checker.types import (BOOL, COPYABLE_CONTAINERS, COPYABLE_HEAP, FLOAT_TYPES, INT, INT_TYPES, LINEAR_BUILTINS,  # noqa: E402
                           NOT_COPYABLE, STRING, STRING_VIEW, TRIVIAL_BUILTINS, VOID, DynamicT, FnT, LoanT, Named, OptionalT, RecordT,
                           Refined, TupleT, Type, TypeParam, Unknown, base_name, diagnostic_types)

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
DERIVABLE = {"Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Codec", "Default", "From", "convert"}
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
    linear: Tuple[str, ...] = ()                    # its own type parameters the signature declares `T: linear` (6.12, 10.7)
    ignorable: Optional[Token] = None               # the `@ignorable` application on the declaration, if any (5.5, X-345)

    def show(self, types=None) -> str:
        types = types or [t.show() for _, t, _ in self.params] + [self.ret.show()]
        recv = (self.receiver + ", ") if self.receiver else ""
        ps = ", ".join(f"{n}: {(m + ' ') if m else ''}{t}" for (n, _, m), t in zip(self.params, types))
        return f"{' '.join(self.modifiers)} func {self.name}({recv}{ps}) {types[-1]}".strip()


def diagnostic_signatures(*sigs):
    types = diagnostic_types(*(t for sig in sigs for t in [*(t for _, t, _ in sig.params), sig.ret]))
    shown, offset = [], 0
    for sig in sigs:
        end = offset + len(sig.params) + 1
        shown.append(sig.show(types[offset:end]))
        offset = end
    return tuple(shown)


@dataclass
class Convert:
    """A `convert` member of a class or enum body (Language Reference 6.13, X-475, X-551): the head is the what, the
    body the how. `convert to T` converts a value of the declaring type into a `T`; `convert from S(s)` converts an
    `S` into a value of the declaring type. The member is checked as a method (`sig`: a receiver `self` for `to`, one
    parameter `s` for `from`, the return type the target or the declaring type), and `x as T` resolves to it by the
    pair (source, target), keyed by the whole type as `implement` coherence keys its pairs (9.5)."""
    direction: str                          # to | from
    other: Type                             # the nominal type the head names (the target, or the source)
    fallible: bool                          # `try`: the body may fail, and `x as T` is an `Outcome`
    owned: bool                             # `owned self` / `owned s`: the conversion consumes, `move x as T`
    exported: bool
    sig: Sig                                # the member as a method: its synthetic name keys the body's summary
    node: object = None                     # the convertDecl node
    derived: bool = False                    # explicit field-wise derivation (6.13, X-552)
    legacy: bool = False                     # a retired From implementation

    @property
    def label(self) -> str:
        return self.sig.name


@dataclass
class Decl:
    kind: str                                 # class | enum | type | interface | mixin | error | func | const
    name: str
    exported: bool
    module: str
    package: str = ""
    core_home: str = ""                   # prelude copy of a type one other standard module declares: that module
    type_params: Tuple[str, ...] = ()
    linear: Tuple[str, ...] = ()
    fields: Dict[str, Tuple[Type, bool, bool]] = field(default_factory=dict)   # name -> (type, exported, has_default)
    methods: Dict[str, Sig] = field(default_factory=dict)
    converts: List[Convert] = field(default_factory=list)   # the declared conversions of a class or enum (6.13)
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
        one name are different types. A single-file program has its own identity too (D-592)."""
        return "" if standard(self.package) or standard(self.module) else f"{self.package or '<single>'}/{self.module}"

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
        # implement blocks whose target is not a named type (`implement I for Int?`): rejected in 1.0 (E-TYP-171)
        self.other_implement_targets: List[Tuple[Type, str, object]] = []      # (target, module, node)
        self.module_of: Dict[str, str] = {}

    def get(self, n: str) -> Optional[Decl]:
        return self.decls.get(n)


def ignorable_use(member) -> Optional[Token]:
    """The name token of an `@ignorable` application among a class member's aspect uses (5.5, 11.8; X-345)."""
    for u in children(member):
        if name(u) == "aspectUse" and direct_tokens(u)[1].text == "ignorable":
            return direct_tokens(u)[1]
    return None


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
        self.linear_params: Set[str] = set()             # the body being typed declares these of its own type parameters `T: linear` (6.12, 10.7)
        # heads of this module's own declarations, consulted by `named_of` only while the module is loading and only
        # for a name the table does not already hold, so a forward reference is stamped with this module's identity
        # (LANG-04-04) without being inserted early (that would trip E-TYP-070)
        self.pending_heads: Dict[str, Decl] = {}
        self.prelude_decls: Dict[str, Decl] = {}
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
                self.prelude_decls = dict(self.table.decls)
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
        # `@ignorable` is the core aspect only where the name resolves to the prelude's: a module that declares or
        # imports an aspect `ignorable` applies its own, which exempts no result (5.5, 11.8; D-314)
        core_ignorable = module == "fortis.prelude" or ("ignorable" not in local_aspects and not any(
            ".".join(imp["path"]) != "fortis.prelude" and "ignorable" in (imp["members"] or []) for imp in tree.imports(mtree)))
        for d in tree.declarations(mtree):
            if d["kind"] in ("head", "test", "property", "implement"):
                if d["kind"] == "implement":
                    # a package module loaded as a library keeps its blocks too, so its importers see them (D-132)
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
            if not core_ignorable:
                for sig in ([decl.sig] if decl.sig is not None else []) + list(decl.methods.values()):
                    sig.ignorable = None
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
        # the declaration's own type parameters are in its head (a direct child); a generic method's are its own
        tp = next((c for c in children(node) if name(c) == "typeParams"), None)
        if tp is not None and name(children(node)[0] if children(node) else None) != "funcHead":
            decl.type_params = tuple(t.text for t in tokens(tp) if t.kind == "UPPER")
        self.type_params = set(decl.type_params)
        wc = next((c for c in children(node) if name(c) in ("whereClause", "linearWhereClause")), None)
        if wc is not None:
            decl.where = self.where_of(wc)
            decl.linear = tuple(self.linear_of(wc)) + tuple(tokens(c)[0].text for c in find_all(wc, "linearConstraint"))
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
                    if fname in decl.fields or fname in decl.methods or fname in decl.constants:
                        self.rec("E-TYP-070", f"member `{fname}` of {nm} is declared twice", "LANG-09-01", ft[0], {"expected_type": "one member", "actual_type": "two", "primary_span": fname})
                    decl.fields[fname] = (self.type_of(find(fd, "type")), exp, any(name(c) == "constExpr" for c in children(fd)))
                    continue
                cd = find(m, "constDecl")
                if cd is not None and find(m, "methodHead") is None:
                    ct = direct_tokens(cd)
                    cname = ct[1].text
                    if cname in decl.constants or cname in decl.fields or cname in decl.methods:
                        self.rec("E-TYP-070", f"member `{cname}` of {nm} is declared twice", "LANG-09-01", ct[1], {"expected_type": "one member", "actual_type": "two", "primary_span": cname})
                    decl.constants[ct[1].text] = self.type_of(find(cd, "type")) if find(cd, "type") else Unknown("untyped const")
                    continue
                cl = find(m, "cleanupDecl")
                if cl is not None:
                    decl.has_cleanup = True
                    continue
                cv = find(m, "convertDecl")
                if cv is not None:
                    decl.converts.append(self.convert_of(cv, exp, decl))
                    continue
                head = find(m, "methodHead")
                if head is not None:
                    sig = self.signature(head, exp, find(m, "funcBody"))
                    sig.ignorable = ignorable_use(m)
                    if sig.name in decl.methods or sig.name in decl.fields or sig.name in decl.constants:
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
                elif head is not None:
                    sig = self.signature(head, any(isinstance(c, Token) and c.text == "export" for c in children(m)), find(m, "funcBody"))
                    decl.methods[sig.name] = sig
        elif kind == "type":
            decl.alias = self.type_of(find(node, "type"))
            if isinstance(decl.alias, Refined) and not decl.alias.base.package and decl.alias.base.name in FLOAT_TYPES:
                self.rec("E-TYP-020", f"float refinement in `type {nm}`", "LANG-06-05", d["token"], {"expected_type": "an integer, text, byte or collection base", "actual_type": decl.alias.show(), "primary_span": nm})
        elif kind == "error":
            ts = direct_tokens(node)
            decl.code = next(t.text for t in ts if t.kind == "STR")[1:-1]
            fl = find(node, "fieldList")
            if fl is not None:
                decl.payload = dict(self.field_list(fl))
        elif kind == "func":
            decl.sig = self.signature(find(node, "funcHead"), d["exported"], find(node, "funcBody"))
            decl.sig.ignorable = next((direct_tokens(u)[1] for u in d.get("aspects", []) if direct_tokens(u)[1].text == "ignorable"), None)
            if nm == "main" and self.entry_module is not None and module == self.entry_module:
                # the entry `main` owns the capabilities the runtime supplies to its parameters (4.6, X-345, Q7)
                decl.sig.params = [(pn, pt, "owned" if pm == "" and isinstance(pt, Named) and not pt.package and pt.name in CAPABILITY_TYPES else pm)
                                   for pn, pt, pm in decl.sig.params]
        elif kind == "const":
            t = tree.child(node, "type")
            decl.alias = self.type_of(t) if t is not None else Unknown("inferred const")
        self.type_params = set()
        return decl

    def load_implement(self, node, module):
        ts = [c for c in children(node) if name(c) == "type"]
        iface, target = self.type_of(ts[0]), self.type_of(ts[1])
        if isinstance(iface, Named) and isinstance(target, Named):
            self.table.implement_blocks.append((iface, target, module, node))
        elif isinstance(iface, Named) and not isinstance(target, Unknown):
            self.table.other_implement_targets.append((target, module, node))

    def upper_list(self, ul) -> List[Tuple[str, Tuple[Type, ...]]]:
        out = []
        kids = children(ul)
        i = 0
        while i < len(kids):
            k = kids[i]
            if isinstance(k, Token) and k.text == "convert":
                out.append(("convert", (self.type_of_ref(kids[i + 2]),)))
                i += 2
            elif isinstance(k, Token) and k.kind == "UPPER":
                args: Tuple[Type, ...] = ()
                if i + 1 < len(kids) and name(kids[i + 1]) == "typeArgs":
                    args = tuple(self.type_of(a) for a in find_all(kids[i + 1], "type") if a in children(find(kids[i + 1], "typeArg")) or True)
                    args = tuple(self.type_arg(a) for a in children(kids[i + 1]) if name(a) == "typeArg")
                    i += 1
                spelling = k.text
                owner = self.pending_heads.get(spelling) or self.table.get(spelling)
                if owner is not None and spelling in DERIVABLE:
                    spelling = spelling if standard(owner.module) else spelling + " (not prelude)"
                out.append((spelling, args))
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

    def linear_of(self, wc) -> List[str]:
        """The type parameters a `where` clause declares `X: linear` (6.12, 10.7): a constraint with no interface type."""
        return [direct_tokens(c)[0].text for c in find_all(wc, "constraint") if find(c, "type") is None]

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
        rt = find(sig, "returnType")
        ret = self.type_of(find(rt, "type")) if rt is not None or sig is None else VOID   # no return type: Void (8.1, X-345)
        self.type_params = saved
        wc = find(sig, "whereClause")
        where = tuple(self.where_of(wc)) if wc is not None else ()
        linear = tuple(self.linear_of(wc)) if wc is not None else ()
        return Sig(fname, params, ret, receiver, modifiers, contracts, exported, head, body, tparams, where, linear)

    def convert_of(self, node, exported, decl) -> Convert:
        """A `convert` member as the method signature its body is checked with (6.13, X-551): `convert to T` has the
        receiver `self` and returns `T`; `convert from S(s)` has the one parameter `s: S` and returns the declaring
        type. The synthetic name (`convert:to:T`) is one no spelled member can hold, since `:` is no identifier."""
        kids = children(node)
        tr_i = next(i for i, c in enumerate(kids) if name(c) == "typeRef")
        direction = kids[tr_i - 1].text                       # the `to`/`from` token directly before the type
        fallible = any(isinstance(c, Token) and c.text == "try" for c in kids[:tr_i])
        owned = any(isinstance(c, Token) and c.text == "owned" for c in kids[tr_i + 1:])
        other = self.type_of_ref(kids[tr_i])                  # the head names a type reference, as `as` does
        pname = "self"
        if direction == "from":
            pname = next(t.text for t in kids[tr_i + 1:] if isinstance(t, Token) and t.kind == "LOWER")
        body = find(node, "funcBody")
        modifiers = ("try",) if fallible else ()
        qual = (other.package or other.origin) if isinstance(other, Named) else ""
        synth = f"convert:{direction}:{other.show()}" + (f"@{qual}" if qual else "")
        if isinstance(other, Named) and other.args:
            at = tokens(node)[0]
            synth += f":{at.line}:{at.col}"
        if direction == "to":
            sig = Sig(synth, [], other, "owned self" if owned else "self", modifiers, (), exported, node, body)
        else:
            sig = Sig(synth, [(pname, other, "owned" if owned else "")], decl.as_named(), None, modifiers, (), exported, node, body)
        return Convert(direction, other, fallible, owned, exported, sig, node)

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
            if any(name(c) == "refinement" for c in children(tr)):
                shown = "".join(t.text for t in tokens(tr))
                self.rec("E-TYP-020", f"`{shown}` is not a refinement of {nm}: a type parameter has no refinement", "LANG-06-02", base_tok, {"expected_type": "a refinement 6.3 lists", "actual_type": shown, "primary_span": shown})
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
            lo, hi, bound_why = self.range_of(rg, nm)
            declared = d is not None and d.kind != "unknown" and not (d.module or "").startswith("fortis")   # a program's own type
            why = self.refinement_why(nm, kind, lo, hi, bound_why, declared)
            if why is not None:
                # not a refinement of this base (6.3, X-510): the type is read as its base, so one record names the form
                shown = "".join(t.text + (" " if t.text in (":", ",") else "") for t in tokens(tr))
                self.rec("E-TYP-020", f"`{shown}` is not a refinement of {nm}: {why}", "LANG-06-02", base_tok, {"expected_type": "a refinement 6.3 lists", "actual_type": shown, "primary_span": shown})
                return named
            return Refined(named, lo, hi, kind)
        return named

    # the bases each measure refines, and the measures' own range (6.3, X-510): a count is never negative
    REFINEMENT_BASES = {"range": frozenset(INT_TYPES), "scalars": frozenset({"String", "StringView"}), "length": frozenset({"Bytes", "BytesView", "List", "ListView"})}

    def refinement_why(self, nm: str, kind: str, lo: Optional[int], hi: Optional[int], bound_why: Optional[str], declared: bool) -> Optional[str]:
        """Why `nm<kind: lo..hi>` is not a refinement 6.3 lists, or None when it is (X-510)."""
        bases = self.REFINEMENT_BASES.get(kind)
        if bases is None:
            return f"`{kind}` is not a measure; a refinement is a range of an integer type, `scalars` of text or `length` of bytes or a list"
        if nm not in bases or declared:
            if kind == "range":
                return f"only an integer type has a range refinement" if nm in self.REFINEMENT_BASES["scalars"] | self.REFINEMENT_BASES["length"] else f"{nm} is not a built-in integer, text, byte or list type"
            return f"`{kind}` measures {'text' if kind == 'scalars' else 'bytes or a list'}, not {nm}"
        if bound_why is not None:
            return bound_why
        full = INT_TYPES[nm] if kind == "range" else (0, 2 ** 63 - 1)
        if lo is not None and hi is not None and lo > hi:
            return "the range is empty"
        for b in (lo, hi):
            if b is not None and not (full[0] <= b <= full[1]):
                return f"{b} is outside {nm}'s own range [{full[0]}, {full[1]}]" if kind == "range" else f"a {kind} count is not {b}"
        return None

    def range_of(self, rg, base: str) -> Tuple[Optional[int], Optional[int], Optional[str]]:
        """(lo, hi, why): the folded bounds of a refinement of `base` or of a range pattern over `base`, and why a
        bound could not be folded (6.3, 9.8, X-510, X-512, X-517): it is not an integer literal or a `const` of the
        base type resolved at compile time. `hi` is stored inclusive, so `lo..<hi` folds to hi - 1; a leading `..`
        or `..<` (a range pattern's one-sided exclusive form, 9.8) leaves `lo` open."""
        kids = children(rg)
        lo = hi = None
        why = None
        if not (isinstance(kids[0], Token) and kids[0].text in ("..", "..<")):
            lo, why = self.bound_of(kids[0], base)
            rest = kids[1:]
        else:
            rest = kids
        if len(rest) >= 2 and name(rest[1]) == "constExpr":
            hi, hi_why = self.bound_of(rest[1], base)
            why = why if why is not None else hi_why
            if isinstance(rest[0], Token) and rest[0].text == "..<" and hi is not None:
                hi -= 1
        return lo, hi, why

    def bound_of(self, node, base: str) -> Tuple[Optional[int], Optional[str]]:
        """(value, why): a bound's compile-time value, or why it has none (6.3, X-512)."""
        shown = "".join(t.text for t in tokens(node))
        bases: Set[str] = set()
        v = self.const_int(node, (), bases)
        if v is None:
            return None, f"the bound `{shown}` is not an integer literal or a `const` resolved at compile time"
        other = next((b for b in sorted(bases) if b != base), None)
        if other is not None:
            return None, f"the bound `{shown}` has type {other}, not {base}: a `const` bound has the base type"
        return v, None

    def const_int(self, node, seen: Tuple[str, ...] = (), bases: Optional[Set[str]] = None) -> Optional[int]:
        """The compile-time value of a constant expression (6.3, X-512): an integer literal; a `const` (possibly
        module-qualified) whose initialiser folds the same way, its declared type's base noted in `bases`; `-x`;
        parentheses; and `+ - * / %` over them, products first, division truncating toward zero and the remainder taking
        the dividend's sign (6.4). Anything else, a divisor of zero and a constant that names itself have no value."""
        ts = tokens(node)
        if len(ts) == 1 and ts[0].kind == "INT":
            return int(ts[0].text.replace("_", ""), 0)
        if len(ts) == 2 and ts[0].text == "-" and ts[1].kind == "INT":
            return -int(ts[1].text.replace("_", ""), 0)
        n = name(node)
        kids = children(node)
        if n == "constExpr":
            if len(kids) == 1:
                return self.const_int(kids[0], seen, bases)
            terms: List[int] = []
            ops: List[str] = []
            for k in kids:
                if isinstance(k, Token):
                    ops.append(k.text)
                    continue
                v = self.const_int(k, seen, bases)
                if v is None:
                    return None
                terms.append(v)
            vals, sums = [terms[0]], []
            for op, t in zip(ops, terms[1:]):
                if op in ("+", "-"):
                    sums.append(op)
                    vals.append(t)
                    continue
                a = vals.pop()
                if op == "*":
                    vals.append(a * t)
                    continue
                if t == 0:
                    return None
                q = abs(a) // abs(t)
                q = q if (a >= 0) == (t >= 0) else -q
                vals.append(q if op == "/" else a - q * t)
            total = vals[0]
            for op, v in zip(sums, vals[1:]):
                total = total + v if op == "+" else total - v
            return total
        if n == "constTerm" and kids:
            if isinstance(kids[0], Token) and kids[0].text == "-" and len(kids) == 2:
                v = self.const_int(kids[1], seen, bases)
                return None if v is None else -v
            if isinstance(kids[0], Token) and kids[0].text == "(" and len(kids) >= 2:
                return self.const_int(kids[1], seen, bases)
            if isinstance(kids[0], Token) and kids[0].kind == "LOWER":
                return self.const_value("".join(t.text for t in ts), seen, bases)
            if name(kids[0]) == "typeRef" and len(kids) == 3 and name(kids[2]) == "memberName":
                # `Type.member`, an associated constant of a class (9.3), folded from its own initialiser
                trs = direct_tokens(kids[0])
                key = trs[0].text + "." + trs[2].text if trs[0].kind == "LOWER" and len(trs) > 2 else trs[0].text
                return self.const_value(key + "." + tokens(kids[2])[0].text, seen, bases)
        if len(kids) == 1 and not isinstance(kids[0], Token):
            return self.const_int(kids[0], seen, bases)
        return None

    def const_value(self, key: str, seen: Tuple[str, ...], bases: Optional[Set[str]]) -> Optional[int]:
        """The value of the `const` named `key` (a declaration of this module, or `alias.name` of an import), folded
        from its initialiser; its declared type's base, or `Int` for an unannotated constant (4.2), is noted."""
        if key in seen:
            return None
        d = self.table.decls.get(key) or self.pending_heads.get(key)
        cnode = d.node if d is not None and d.kind == "const" else None
        if d is None and "." in key:
            # `Type.member`: the `const` member of a class or interface declared here or imported (9.3)
            tname, member = key.rsplit(".", 1)
            td = self.table.decls.get(tname) or self.pending_heads.get(tname)
            if td is not None and td.kind in ("class", "interface") and td.node is not None:
                cnode = next((cd for cd in find_all(td.node, "constDecl") if direct_tokens(cd)[1].text == member), None)
        if cnode is None:
            return None
        ce = next((c for c in children(cnode) if name(c) == "constExpr"), None)
        if ce is None:
            return None
        v = self.const_int(ce, seen + (key,), bases)
        if v is None:
            return None
        if bases is not None:
            t = next((c for c in children(cnode) if name(c) == "type"), None)
            tts = [x for x in tokens(t) if x.kind == "UPPER"] if t is not None else []
            bases.add(tts[0].text if tts else "Int")
        return v

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
        return FnT(params, ret, kind, modifiers, origin, "dynamic" in words, modes=tuple(next((t.text for t in direct_tokens(pt) if t.text in ("editable", "owned")), "") for pt in kids if name(pt) == "paramType"))

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

        core = getattr(self, "prelude_decls", {}).get(name)
        if core is not None and (core.module == module or core.core_home == module):
            return core
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
        core = getattr(self, "prelude_decls", {}).get(t.name)
        if core is not None and core.kind in self._TYPE_KINDS:
            return core
        table = home if home is not None else self.table
        d = table.get(t.name)
        if d is not None and d.kind != "unknown" and not d.type_identity:
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

    def view_newtype(self, t: Type) -> bool:
        """A direct newtype of ListView keeps its representation's loans (6.9, D-555)."""
        d = self.resolve_type(t.inner if isinstance(t, LoanT) else t)
        if d is None or d.kind != "type" or d.alias is None:
            return False
        rd = self.resolve_type(d.alias)
        return rd is not None and standard(rd.module) and rd.name == "ListView"

    def loan_carrying_inner(self, t: Type) -> bool:
        if self.view_newtype(t):
            return True
        s = t.inner if isinstance(t, OptionalT) else t
        if isinstance(s, Named):
            if not s.package and s.name in ("StringView", "BytesView", "ListView", "ListCursor"):
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
            if not t.package and t.name in TRIVIAL_BUILTINS: return True
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
        if self._linear(t, (), home): return False
        if self._trivial(t, seen, home if home is not None else self.table): return True
        if isinstance(t, OptionalT): return self._copyable(t.inner, seen, home)
        if isinstance(t, TupleT): return all(self._copyable(x, seen, home) for x in t.items)
        if isinstance(t, RecordT): return all(self._copyable(x, seen, home) for _, x in t.fields)
        if isinstance(t, Refined): return self._copyable(t.base, seen, home)
        if isinstance(t, TypeParam): return "Copyable" in self.tparam_bounds.get(t.name, ())
        if isinstance(t, (FnT, LoanT, DynamicT, Unknown)): return False
        if isinstance(t, Named):
            if not t.package and t.name in NOT_COPYABLE: return False
            if not t.package and t.name in COPYABLE_HEAP: return True
            if not t.package and t.name in COPYABLE_CONTAINERS: return all(self._copyable(a, seen, home) for a in t.args if not (isinstance(a, Named) and a.name.startswith("#")))
            d = self.resolve_type(t, home)
            key = self._fact_key(t, d)
            if d is None or key in seen: return False
            nxt, seen2 = self._next_home(d, home), seen + (key,)
            if d.kind == "type": return self._copyable(d.alias, seen2, nxt)
            if d.kind == "enum": return all(self._copyable(ft, seen2, nxt) for v in d.variants.values() for _, ft in v)
            if d.kind == "class":
                if (not d.type_identity and d.name == "Capabilities") or self.implements_core_authority(d): return False
                return all(self._copyable(ft, seen2, nxt) for ft, _, _ in d.fields.values())
        return False

    def is_linear(self, t: Type, seen=()) -> bool:
        return self._linear(t, seen, None)

    def _linear(self, t: Type, seen, home: Optional[Table]) -> bool:
        if isinstance(t, TypeParam): return t.name in self.linear_params   # `T: linear` holds while the declaring body is typed (6.12)
        if isinstance(t, OptionalT): return self._linear(t.inner, seen, home)
        if isinstance(t, TupleT): return any(self._linear(x, seen, home) for x in t.items)
        if isinstance(t, RecordT): return any(self._linear(x, seen, home) for _, x in t.fields)
        if isinstance(t, Refined): return self._linear(t.base, seen, home)
        if isinstance(t, FnT): return t.kind == "fn once"
        if isinstance(t, Named):
            if not t.package and t.name in LINEAR_BUILTINS: return True
            if not t.package and t.name in COPYABLE_CONTAINERS: return any(self._linear(a, seen, home) for a in t.args)
            d = self.resolve_type(t, home)
            key = self._fact_key(t, d)
            if d is None or key in seen: return False
            if any(tp in d.linear and self._linear(arg, seen, home) for tp, arg in zip(d.type_params, t.args)):
                return True
            nxt, seen2 = self._next_home(d, home), seen + (key,)
            sub = dict(zip(d.type_params, t.args))
            if d.kind == "type": return self._linear(self.subst(d.alias, sub), seen2, nxt)
            if d.kind == "enum": return any(self._linear(self.subst(ft, sub), seen2, nxt) for v in d.variants.values() for _, ft in v)
            if d.kind == "class": return any(self._linear(self.subst(ft, sub), seen2, nxt) for ft, _, _ in d.fields.values())
        return False

    def linear_instantiations(self, t: Type):
        """Invalid user generic type arguments, including nested and inferred instantiations (6.12)."""
        if isinstance(t, Named):
            d = self.resolve_type(t, None)
            intrinsic = d is not None and self.std_module_name(d.module) and t.name in (COPYABLE_CONTAINERS | NOT_COPYABLE | LINEAR_BUILTINS | {"Iterator", "Consumer", "ListCursor", "MapCursor", "SetCursor"})
            if d is not None and d.kind in ("class", "enum", "type") and not intrinsic:
                for tp, arg in zip(d.type_params, t.args):
                    if tp not in d.linear and self.is_linear(arg):
                        yield t, tp, arg
            for arg in t.args:
                yield from self.linear_instantiations(arg)
        elif isinstance(t, (OptionalT, LoanT)):
            yield from self.linear_instantiations(t.inner)
        elif isinstance(t, Refined):
            yield from self.linear_instantiations(t.base)
        elif isinstance(t, TupleT):
            for arg in t.items: yield from self.linear_instantiations(arg)
        elif isinstance(t, RecordT):
            for _, arg in t.fields: yield from self.linear_instantiations(arg)
        elif isinstance(t, FnT):
            for arg in (*t.params, t.ret): yield from self.linear_instantiations(arg)

    def check_linear_instantiation(self, t: Type, tok):
        for instance, tp, arg in self.linear_instantiations(t):
            self.rec("E-OBL-003", f"{instance.show()} cannot be instantiated with the linear type {arg.show()} for `{tp}` without declaring `{tp}: linear` (6.12)", "LANG-10-02", tok,
                     {"obligation_origin": f"{tok.line}:{tok.col}", "unconsumed_path": tp})

    def is_linear_apart_from_params(self, t: Type) -> bool:
        """Whether a type is linear whatever its type parameters are (6.12, X-519): a `Future[T]`, a `Task[T]`, a
        `fn once`. The `T: linear` declarations of the body being typed are not consulted: this judges a declaration's
        own shape, where every type parameter is non-linear, so a caller's own bound never answers for a callee's."""
        saved, self.linear_params = self.linear_params, set()
        try:
            return self._linear(t, (), None)
        finally:
            self.linear_params = saved

    # ---- implementation lookups --------------------------------------------------
    # the prelude's containers whose default no backend builds yet (6.10; D-242, X-345)
    NO_DEFAULT_CONTAINERS = ("Array", "FixedList", "FixedMap", "OrderedMap")

    def derivable_default(self, t: Type, seen=(), home: Optional[Table] = None) -> bool:
        """Whether `derive Default` can build a field of this type that has no initialiser (6.10; D-242): a type with a
        default that every backend builds. A type parameter has none, since a class has no `where` clause to require
        one; nor have the prelude's `Array`, `FixedList`, `FixedMap` and `OrderedMap`, known by their declaration
        and not their spelling, so a user's class named `Array` is judged as itself. A newtype is unfolded to its
        representation, at any depth. Elsewhere `implements` keeps treating a type parameter as implementing every
        interface."""
        if isinstance(t, TypeParam):
            return False
        if isinstance(t, Refined):
            if not ((t.lo is None or t.lo <= 0) and (t.hi is None or t.hi >= 0)):
                return False
            return self.derivable_default(t.base, seen, home)
        if isinstance(t, Named):
            d = self.resolve_type(t, home)
            if d is not None and d.kind != "unknown":
                key = self._fact_key(t, d)
                if key in seen:
                    return False
                prelude = d.module == "fortis.prelude" and not d.type_identity
                if prelude and d.name in self.NO_DEFAULT_CONTAINERS:
                    return False
                if d.kind == "type":
                    return d.alias is not None and self.derivable_default(d.alias, seen + (key,), self._next_home(d, home))
                if not prelude and (t.name in COPYABLE_CONTAINERS or t.name in BUILTIN_IMPLEMENTS):
                    # a declaration spelled as a built-in type is judged by what it declares (a user's `class Array`)
                    return (any(i.interface_key() == "Default" for i in d.implements) or any(n == "Default" for n, _ in d.derives)
                            or self._block_implements(t, "Default"))
        return self._implements(t, "Default", (), home)

    def default_cycle(self, t: Type, path: Tuple[Tuple[Tuple[str, str], str], ...], home: Optional[Table] = None) -> Optional[List[str]]:
        """The declarations a derived default would build again before it finished (6.10; D-335, D-336), or None. `path`
        holds the classes and newtypes whose defaults are being built, each as ((module, name), name): a newtype is
        unfolded to its representation and a class that derives Default to its fields without an initialiser. Reaching
        one already on the path is the cycle, named from that declaration back to itself. `T?` defaults to null, and a
        class that implements Default gives its own `default()`, so either ends the walk."""
        if isinstance(t, Refined):
            return self.default_cycle(t.base, path, home)
        if not isinstance(t, Named):
            return None
        d = self.resolve_type(t, home)
        if d is None or d.kind == "unknown" or (d.module == "fortis.prelude" and not d.type_identity):
            return None
        key = (d.module, d.name)
        keys = [k for k, _ in path]
        if key in keys:
            return [n for _, n in path[keys.index(key):]] + [d.name]
        nxt, here = self._next_home(d, home), path + ((key, d.name),)
        if d.kind == "type":
            return self.default_cycle(d.alias, here, nxt) if d.alias is not None else None
        if d.kind == "class" and any(n == "Default" for n, _ in d.derives):
            for ft, _, has_default in d.fields.values():
                if not has_default and (cycle := self.default_cycle(ft, here, nxt)) is not None:
                    return cycle
        return None

    def interface_decl(self, key: str):
        name, _, package = key.partition("@")
        return self.resolve_type(Named(name, package=package))

    def implements(self, t: Type, iface: str, seen=()) -> bool:
        """Does type t implement interface iface (declared, derived, built-in, bundle or structural)?"""
        return self._implements(t, iface, seen, None)

    def _block_implements(self, t: Type, iface: str) -> bool:
        """An `implement` block in this module applies to `t` only when it names that same declaration.

        A block whose target has no identity matches only a core type that has none.
        A block for this module's `Cee` does not make another module's `Cee` implement the interface (D-130).
        Origin distinguishes the standard modules whose types share the empty core package identity."""
        if not isinstance(t, Named):
            return False
        for i, tt, _, _ in self.table.implement_blocks:
            if i.interface_key() != iface or tt.name != t.name or tt.origin != t.origin:
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
        if isinstance(t, Refined):
            # a refined type has its representation's default only when its refinement admits it (6.10): 0, or empty
            if iface == "Default" and not ((t.lo is None or t.lo <= 0) and (t.hi is None or t.hi >= 0)):
                return False
            return self._implements(t.base, iface, seen, home)
        if isinstance(t, OptionalT):
            if iface == "Default": return True      # `T?` defaults to null (6.10)
            if iface in ("Encodable", "Decodable") and isinstance(t.inner, OptionalT): return False   # `T??`: present(null) has no canonical form (6.10)
            return iface in ("Equatable", "Hashable", "Encodable", "Decodable", "Displayable") and self._implements(t.inner, iface, seen, home)
        if isinstance(t, TupleT): return iface in ("Equatable", "Hashable", "Comparable", "Encodable", "Decodable") and all(self._implements(x, iface, seen, home) for x in t.items)
        if isinstance(t, RecordT): return iface in ("Equatable", "Hashable", "Encodable", "Decodable") and all(self._implements(x, iface, seen, home) for _, x in t.fields)
        if isinstance(t, TypeParam): return True   # bounds are checked at use (DEL-12)
        if isinstance(t, Unknown): return True
        if not isinstance(t, Named): return False
        if not t.package and t.name in BUILTIN_IMPLEMENTS and iface in BUILTIN_IMPLEMENTS[t.name]: return True
        if not t.package and t.name in COPYABLE_CONTAINERS and iface in ("Equatable", "Hashable", "Encodable", "Decodable", "Displayable", "Default"):
            if iface == "Default": return True
            return all(self._implements(a, iface, seen, home) for a in t.args if not (isinstance(a, Named) and a.name.startswith("#")))
        if not t.package and t.name in ("Shared", "Weak", "Mutex") and iface == "Shareable": return True
        d = self.resolve_type(t, home)
        key = (self._fact_key(t, d)[0], t.name, iface)
        if d is None or key in seen: return False
        seen = seen + (key,)
        nxt = self._next_home(d, home)
        # the interface name is the one the question was asked with: a name of the module doing the checking
        idecl = self.interface_decl(iface)
        if d.kind == "type":
            if iface in ("Encodable", "Decodable", "Codec", "Equatable", "Hashable", "Comparable", "Displayable", "Default"):
                return self._implements(d.alias, iface, seen, nxt)
        if any(i.interface_key() == iface for i in d.implements): return True
        if any(n == iface for n, _ in d.derives): return True
        if iface in ("Encodable", "Decodable") and any(n == "Codec" for n, _ in d.derives): return True
        if self._block_implements(t, iface): return True
        # bundle: an interface with requirements and no members of its own
        if idecl is not None and idecl.kind == "interface" and not idecl.methods and not idecl.constants and idecl.requires:
            return all(self._implements(t, r.interface_key(), seen, home) for r in idecl.requires)
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
            if not t.package and t.name in ("StringView", "BytesView", "ListView", "MutexGuard", "Future", "Task"): return False   # 12.7: loans, views, guards, futures, tasks
            if not t.package and t.name in COPYABLE_HEAP: return True
            if not t.package and t.name in COPYABLE_CONTAINERS: return all(self._shareable(a, seen, home) for a in t.args if not (isinstance(a, Named) and a.name.startswith("#")))
            if not t.package and t.name in ("Shared", "Weak", "Mutex"): return all(self._shareable(a, seen, home) for a in t.args)
            if not t.package and t.name in ("AtomicInt", "AtomicBool"): return True
            d = self.resolve_type(t, home)
            key = self._fact_key(t, d)
            if d is None or key in seen: return False
            nxt, seen2 = self._next_home(d, home), seen + (key,)
            if d.kind == "class":
                if d.lifetime: return False
                if self.implements_core_authority(d): return True
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
                    if isinstance(ft, Named) and not ft.package and ft.name == "String":
                        self.rec("E-OBS-012", f"observation {d.name}: String field `{fname}` must declare a scalar bound (`String<scalars: 0..n>`) so the frame stays bounded (13)", "LANG-13-03", self.tok_of(d), {"frame_field": fname, "exit_path": "declaration"})
                    if known(ft) and not self.encodable(ft):
                        self.rec("E-TYP-090", f"observation {d.name}: field `{fname}: {ft.show()}` is not Encodable; secrets, authorities, loans, resources, closures, futures and tasks cannot be captured (13)", "LANG-13-01", self.tok_of(d), {"expected_type": "Encodable", "actual_type": ft.show(), "primary_span": fname})
            if d.kind in ("class", "enum"):
                self.check_type_decl(d)
            elif d.kind == "interface":
                self.check_interface(d)
            self.check_value_types(d)
            if d.kind == "contract":
                self.check_contract_decl(d)
            self.check_trusted(d)
        self.check_trusted_blocks()
        self.check_coherence()
        self.check_conversions()
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
                d = self.resolve_type(x)
                if d is not None and d.kind == "interface":
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

    @staticmethod
    def mentions_self(t: Type) -> bool:
        """Does a signature type name `Self` (an interface's own type) anywhere?"""
        if isinstance(t, (Named, TypeParam)) and t.name == "Self":
            return True
        if isinstance(t, Named):
            return any(DeclChecker.mentions_self(a) for a in t.args)
        if isinstance(t, (OptionalT, LoanT, DynamicT)):
            return DeclChecker.mentions_self(t.inner if not isinstance(t, DynamicT) else t.iface)
        if isinstance(t, TupleT):
            return any(DeclChecker.mentions_self(x) for x in t.items)
        if isinstance(t, RecordT):
            return any(DeclChecker.mentions_self(x) for _, x in t.fields)
        if isinstance(t, FnT):
            return any(DeclChecker.mentions_self(x) for x in t.params) or DeclChecker.mentions_self(t.ret)
        return False

    def dynamic_ineligible(self, idecl: Decl) -> Optional[str]:
        """Why an interface is not dynamic-eligible (9.5), or None: a generic method, or a method whose parameters or
        result name `Self` beyond its receiver (the other value's type could not be known through a `dynamic`). An
        associated function or constant is not reached through a `dynamic` value and does not count."""
        for mname, sig in idecl.methods.items():
            if sig.receiver is None:
                continue
            if sig.type_params:
                return f"its method `{mname}` is generic"
            if any(self.mentions_self(pt) for _, pt, _ in sig.params) or self.mentions_self(sig.ret):
                return f"its method `{mname}` takes or returns `Self`"
        return None

    def key_problems(self, t: Type, bounds=None, unfixed=False) -> List[Tuple[str, str, str]]:
        """D-549: validate hash-container types, including nested and instantiated types.

        Declaration checks supply their own bounds. During inference an unsolved call variable is deferred;
        a parameter of the enclosing body is checked against that body's bounds, never assumed Hashable.
        """
        bounds = self.tparam_bounds if bounds is None else bounds
        out = []

        def grants(have, want, seen=()):
            if have == want:
                return True
            if have in seen:
                return False
            d = self.interface_decl(have)
            needs = [r.interface_key() for r in d.requires] if d is not None else IFACE_REQUIRES.get(have, [])
            return any(grants(r, want, seen + (have,)) for r in needs)

        def holds(x, iface, seen=(), home=None):
            if isinstance(x, Unknown):
                return True
            if isinstance(x, TypeParam):
                return (unfixed and x.name not in self.type_params) or any(grants(b, iface) for b in bounds.get(x.name, ()))
            if isinstance(x, Refined):
                return holds(x.base, iface, seen, home)
            if isinstance(x, OptionalT):
                return holds(x.inner, iface, seen, home)
            if isinstance(x, (TupleT, RecordT)):
                return False  # structural conveniences do not satisfy interfaces (6.9)
            if not isinstance(x, Named):
                return False
            d = self.resolve_type(x, home)
            core = d is None or standard(d.module)
            if core and x.name in COPYABLE_CONTAINERS:
                return all(holds(a, iface, seen, home) for a in x.args if not (isinstance(a, Named) and a.name.startswith("#")))
            key = (d.module if d else "", x.show(), iface)
            if key in seen:
                return d is not None and any(n == iface for n, _ in d.derives)
            if d is not None and d.kind == "type":
                return holds(d.alias, iface, seen + (key,), self._next_home(d, home))
            if d is not None and not core:
                # A user declaration named Int/Set is not a built-in implementation.
                if not (any(i.interface_key() == iface for i in d.implements) or any(n == iface for n, _ in d.derives) or self._block_implements(x, iface)):
                    return False
                if any(n == iface for n, _ in d.derives):
                    sub = dict(zip(d.type_params, x.args))
                    fields = [f for f, _, _ in d.fields.values()] + [f for v in d.variants.values() for _, f in v]
                    return all(holds(self.subst(f, sub), iface, seen + (key,), self._next_home(d, home)) for f in fields)
                return True
            return self._implements(x, iface, (), home)

        def require(container, key):
            if not all(holds(key, i) for i in ("Hashable", "Equatable")):
                shown = diagnostic_types(container, key, Named("Hashable"), Named("Equatable"))
                item = (shown[0], shown[1], f"{shown[2]} and {shown[3]}")
                if item not in out:
                    out.append(item)

        def walk(x):
            if isinstance(x, Named):
                if x.name in ("Set", "Map") and x.args:
                    d = self.resolve_type(x)
                    if d is None or standard(d.module):
                        require(x, x.args[0])
                for a in x.args:
                    walk(a)
            elif isinstance(x, (OptionalT, LoanT)):
                walk(x.inner)
            elif isinstance(x, Refined):
                walk(x.base)
            elif isinstance(x, DynamicT):
                for a in x.iface.args:
                    walk(a)
            elif isinstance(x, TupleT):
                for a in x.items:
                    walk(a)
            elif isinstance(x, RecordT):
                for _, a in x.fields:
                    walk(a)
            elif isinstance(x, FnT):
                for a in x.params:
                    walk(a)
                walk(x.ret)
        walk(t)
        return out

    def check_keys(self, t: Type, tok, bounds=None, unfixed=False):
        for container, key, required in self.key_problems(t, bounds, unfixed):
            self.rec("E-TYP-090", f"{container} needs a key that implements {required}; {key} does not (6.7)",
                     "LANG-06-06", tok, {"expected_type": required, "actual_type": key, "primary_span": container})

    def dynamic_problems(self, t: Type) -> List[Tuple[str, str, str, str]]:
        """Every `dynamic I` written inside `t` that 9.5 refuses: (code, rule, the interface shown, why). `E-TYP-172`
        when `I` is not dynamic-eligible; `E-TYP-090` when `I`'s `where` bound does not hold for the written arguments
        (checked at every use, D-69)."""
        out: List[Tuple[str, str, str, str]] = []

        def walk(x):
            if isinstance(x, DynamicT):
                i = x.iface
                d = self.resolve_type(i)
                if d is not None and d.kind == "interface":
                    why = self.dynamic_ineligible(d)
                    if why is not None:
                        out.append(("E-TYP-172", "LANG-09-07", x.show(), f"`{i.name}` is not dynamic-eligible: {why} (9.5); take the value as a type parameter bounded by {i.name}"))
                    sub = dict(zip(d.type_params, i.args))
                    for tp, bound in list(d.where or []):
                        arg = sub.get(tp)
                        if arg is None or isinstance(arg, (TypeParam, Unknown)):
                            continue
                        if not self.implements(arg, bound.interface_key()):
                            display_0, display_1, display_2 = diagnostic_types(x, bound, arg)
                            out.append(("E-TYP-090", "LANG-06-10", display_0, f"`{display_0}` needs `{tp}` to implement {display_1}; {display_2} does not (9.5)"))
                for a in i.args:
                    walk(a)
            elif isinstance(x, Named):
                for a in x.args:
                    walk(a)
            elif isinstance(x, (OptionalT, LoanT)):
                walk(x.inner)
            elif isinstance(x, TupleT):
                for y in x.items:
                    walk(y)
            elif isinstance(x, RecordT):
                for _, y in x.fields:
                    walk(y)
            elif isinstance(x, FnT):
                for p in x.params:
                    walk(p)
                walk(x.ret)
        walk(t)
        return out

    def check_contract_decl(self, d: Decl):
        """A contract's items must agree (11.5, E-EFF-050): `pure` with `requires capability` (a pure function receives no
        authority), a modifier that permits an effect beside `forbids` of that effect (`async` suspend, `try` fail,
        `recovers` recovers), and one authority both required and excluded. The type a `capability` clause names is an
        Authority type (11.6), or the clause is E-TYP-040: a type in scope by its declaration, a name not in scope by the
        core capabilities' names, as an authority parameter is judged."""
        tok = self.tok_of(d)
        for kind, _, _, claim, effect in contract_contradictions([(d.name, d.items)], skip_own=False):
            if kind == "pure":
                msg = f"contract {d.name} is `pure` and requires capability {claim}: a pure function receives no authority, so the contract contradicts itself (11.5)"
            elif kind == "forbids":
                msg = f"contract {d.name} is `{claim}`, which permits `{effect}`, and forbids `{effect}`: the contract contradicts itself (11.5)"
            else:
                msg = f"contract {d.name} both requires and excludes capability {claim}: the contract contradicts itself (11.5)"
            self.rec("E-EFF-050", msg, "LANG-11-06", tok, {"required_contract": d.name, "violating_effect": effect, "witness_path": claim})
        for it in [c for c in children(d.node) if name(c) == "contractItem"]:
            ts = tokens(it)
            if ts[0].text not in ("requires", "excludes"):
                continue
            a = ts[-1]
            ad = self.table.get(a.text)
            if ad is None or ad.kind == "unknown":
                ok = a.text in AUTHORITIES
            else:
                ok = ad.kind in ("class", "enum", "type") and self.implements_core_authority(ad)
            if not ok:
                self.rec("E-TYP-040", f"contract {d.name} names capability {a.text}, which is not an Authority type (11.5, 11.6)", "LANG-11-06", a,
                         {"expected_type": "an Authority type", "actual_type": a.text, "primary_span": a.text})

    def check_value_types(self, d: Decl):
        """A field, variant payload, parameter or result whose type is an interface itself is refused at its declaration
        (D-21): an interface is used through `dynamic I`, `Shared[dynamic I]` or a type parameter bounded by it (9.5)."""
        bounds = {p: {b.interface_key() for q, b in d.where if q == p} for p in d.type_params}
        def refuse(t: Type, what: str, tok):
            self.check_linear_instantiation(t, tok)
            self.check_keys(t, tok, bounds)
            for i in self.bare_interfaces(t):
                self.rec("E-TYP-040", f"{what} has the interface type {i}, which no value has: write `dynamic {i}`, `Shared[dynamic {i}]` or a type parameter bounded by {i} (9.5)",
                         "LANG-06-01", tok, {"expected_type": f"dynamic {i}", "actual_type": i, "primary_span": what})
            for code, rule, shown, why in self.dynamic_problems(t):
                self.rec(code, f"{what}: {why}", rule, tok, {"expected_type": "a dynamic-eligible interface whose bounds hold", "actual_type": shown, "primary_span": what})
        saved_linear, self.linear_params = self.linear_params, set(d.linear)
        tok = self.tok_of(d)
        if d.alias is not None:
            refuse(d.alias, f"type of {d.name}", tok)
        for cname, ct in d.constants.items():
            refuse(ct, f"constant `{cname}` of {d.name}", tok)
        for fname, (ft, _, _) in d.fields.items():
            refuse(ft, f"field `{fname}` of {d.name}", tok)
        for vname, payload in d.variants.items():
            for fname, ft in payload:
                refuse(ft, f"field `{fname}` of variant {d.name}.{vname}", tok)
        sigs = [(d.name, d.sig)] if d.kind == "func" and d.sig is not None else [(f"{d.name}.{m}", s) for m, s in d.methods.items()]
        for label, sig in sigs:
            self.linear_params = set(d.linear) | set(sig.linear)
            bounds = {p: {b.interface_key() for q, b in list(d.where) + list(sig.where) if q == p} for p in set(d.type_params) | set(sig.type_params)}
            at = tokens(sig.node)[0] if sig.node is not None else tok
            for pn, pt, _ in sig.params:
                refuse(pt, f"parameter `{pn}` of {label}", at)
            refuse(sig.ret, f"the result of {label}", at)
        if d.module == self.module_name and d.kind in ("func", "class", "enum", "interface"):
            for q, n, qtok in self.shadowed_qualified_interfaces(d.node):
                self.rec("E-TYP-040", f"`{q}` is the interface {n}, which no value has: write `dynamic {q}`, `Shared[dynamic {q}]` or a type parameter bounded by it (9.5)", "LANG-06-01", qtok, {"expected_type": f"dynamic {q}", "actual_type": n, "primary_span": q})
        self.linear_params = saved_linear

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
            if any(i.interface_key() == iface for i in d.implements):
                self.rec("E-MIX-001", f"{d.name} both derives and implements `{iface}`", "LANG-09-04", tok, {"expected_type": "derive or implement", "actual_type": "both", "primary_span": iface})
            for hand in sorted(self.BUILTIN_MEMBERS.get(iface, set()) & set(d.methods)):
                self.rec("E-MIX-001", f"{d.name} derives {iface} and also declares `{hand}`, which the derivation would generate", "LANG-09-04", tok, {"expected_type": "one source of the member", "actual_type": "derived and declared", "primary_span": hand})
            if iface in ("From", "convert"):
                if not args:
                    self.rec("E-TYP-090", "derive From needs a source type", "LANG-06-07", tok, {"expected_type": "From[S]", "actual_type": "From", "primary_span": d.name})
                elif iface == "convert" and d.kind == "enum":
                    names = [n.translate(str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")) for n in d.variants]
                    if (not self.same_type(args[0], STRING) or not self.is_core_type(args[0]) or d.type_params
                            or any(d.variants.values()) or len(names) != len(set(names))):
                        self.rec("E-TYP-090", "enum text derivation needs String, a non-generic payload-free enum and names distinct ignoring ASCII case", "LANG-06-07", tok, {"expected_type": "a non-generic payload-free enum with distinct ASCII names", "actual_type": d.name, "primary_span": d.name})
                else:
                    self.check_derive_from(d, args[0], tok)
                continue
            if iface == "Default":
                if d.kind == "enum":
                    # an enum has no fields to build its default from, and no variant is declared the default (6.10; D-242)
                    self.rec("E-TYP-090", f"derive Default: enum {d.name} has no default to derive, since nothing chooses its variant; implement Default and return the variant", "LANG-06-07", tok,
                             {"expected_type": "a class whose fields have defaults", "actual_type": "an enum", "primary_span": d.name})
                    continue
                for fname, (ft, _, has_default) in d.fields.items():
                    cycle = None if has_default else self.default_cycle(ft, (((d.module, d.name), d.name),))
                    if cycle is not None:
                        # a default that holds itself has no end: no backend could build it (6.10; D-335, D-336)
                        chain = " -> ".join(cycle)
                        self.rec("E-TYP-090", f"derive Default: field `{fname}: {ft.show()}` builds the default of {cycle[0]} again before it is finished ({chain}); give the field an initialiser or an optional type", "LANG-06-07", tok,
                                 {"expected_type": "a default that does not contain itself", "actual_type": chain, "primary_span": fname})
                    elif not has_default and not self.derivable_default(ft):
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
                            display_0, display_1 = diagnostic_types(ft, Named(need))
                            self.rec("E-TYP-090", f"derive {display_1}: field `{fname}: {display_0}` is not {display_1}", "LANG-06-07", tok, {"expected_type": display_1, "actual_type": display_0, "primary_span": fname})
                    for vname, payload in d.variants.items():
                        for fname, ft in payload:
                            if not self.encodable(ft):
                                display_0, display_1 = diagnostic_types(ft, Named(need))
                                self.rec("E-TYP-090", f"derive {display_1}: payload `{vname}.{fname}: {display_0}` is not {display_1}", "LANG-06-07", tok, {"expected_type": display_1, "actual_type": display_0, "primary_span": fname})
                else:
                    for fname, ft in [(f, t) for f, (t, _, _) in d.fields.items()] + [(f"{v}.{f}", t) for v, p in d.variants.items() for f, t in p]:
                        if not self.implements(ft, need):
                            display_0, display_1 = diagnostic_types(ft, Named(need))
                            self.rec("E-TYP-090", f"derive {display_1}: field `{fname}: {display_0}` does not implement {display_1}", "LANG-06-07", tok, {"expected_type": display_1, "actual_type": display_0, "primary_span": fname})
                    if need in IFACE_REQUIRES:
                        for req in IFACE_REQUIRES[need]:
                            if not (req in derived or self.implements(self_t, req)):
                                self.rec("E-TYP-080", f"derive {need} requires {req}, which {d.name} neither derives nor implements", "LANG-09-03", tok, {"expected_type": req, "actual_type": d.name, "primary_span": d.name})
        # implements: completeness, compatibility, requires
        # a mixin's method may satisfy an interface member (9.6): the class's own methods first, then its mixins'
        members: Dict[str, Sig] = dict(d.methods)
        for mx in d.mixins:
            md = self.resolve_type(mx)
            if md is not None and md.kind == "mixin":
                for mname, msig in md.methods.items():
                    if not mname.startswith("requires:") and mname not in members:
                        members[mname] = msig
        for iface in d.implements:
            self.check_implements(d, iface, members, tok)
        # mixins: requires satisfied, conflicts
        provided: Dict[str, str] = {}
        for mx in d.mixins:
            md = self.resolve_type(mx)
            if md is None or md.kind != "mixin":
                self.rec("E-TYP-080", f"`{mx.name}` is not a mixin", "LANG-09-04", tok, {"expected_type": "a mixin", "actual_type": mx.name, "primary_span": d.name})
                continue
            for req in md.requires:
                mname = req.name.split(":", 1)[1] if req.name.startswith("__method__:") else req.name
                have = d.methods.get(mname)
                if have is None:
                    if mname not in self.interface_members(d):
                        self.rec("E-TYP-080", f"mixin {mx.name} requires method `{mname}`, which {d.name} lacks", "LANG-09-04", tok, {"expected_type": mname, "actual_type": "absent", "primary_span": d.name})
                    continue
                # a requirement is a signature, compared as an interface member's is (9.6)
                rsig = md.methods.get("requires:" + mname)
                if rsig is not None:
                    for p in self.compatible(rsig, have, mx, d):
                        expected, actual = diagnostic_signatures(rsig, have)
                        self.rec("E-TYP-080", f"{d.name}.{mname} does not satisfy mixin {mx.name}'s requirement `{expected}`: {p}", "LANG-09-04", tok, {"expected_type": expected, "actual_type": actual, "primary_span": mname})
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
            idecl = self.interface_decl(iface)
            if idecl is not None:
                out |= set(idecl.methods) | set(idecl.constants)
        for iface in d.implements:
            out |= self.BUILTIN_MEMBERS.get(iface.interface_key(), set())
        return out

    def encodable(self, t: Type) -> bool:
        if isinstance(t, Named) and not t.package and t.name in NOT_ENCODABLE: return False
        if isinstance(t, (LoanT, FnT, DynamicT)): return False
        if isinstance(t, Named):
            d = self.resolve_type(t)
            if d is not None and d.kind == "class" and self.implements_core_authority(d):
                return False
        return self.implements(t, "Encodable")

    def check_derive_from(self, d: Decl, source: Type, tok):
        sd = self.resolve_type(source)
        if self.same_type(source, STRING) and self.is_core_type(source):
            self.rec("E-TYP-090", "derive convert from String requires a payload-free enum", "LANG-06-07", tok, {"expected_type": "an enum", "actual_type": d.kind, "primary_span": d.name})
            return
        if d.kind != "class":
            self.rec("E-TYP-090", "derive convert from requires a class target", "LANG-06-07", tok, {"expected_type": "a class", "actual_type": d.kind, "primary_span": d.name})
            return
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
            st = self.subst(st, self.type_substitution(sd, source))
            if not exp and sd.module != d.module:
                self.rec("E-TYP-090", f"derive From[{source.show()}]: field `{fname}` of the source is not visible", "LANG-06-07", tok, {"expected_type": "an exported field", "actual_type": fname, "primary_span": fname})
            if not (self.same_type(st, ft) or self.total_conversion(st, ft, d.module)):
                display_0, display_1, display_2 = diagnostic_types(source, st, ft)
                self.rec("E-TYP-090", f"derive From[{display_0}]: field `{fname}` has type {display_1}, needs {display_2}", "LANG-06-07", tok, {"expected_type": display_2, "actual_type": display_1, "primary_span": fname})
            saved_bounds = self.tparam_bounds
            self.tparam_bounds = {p: {b.interface_key() for q, b in d.where if q == p} for p in d.type_params}
            copyable = self.is_copyable(st)
            self.tparam_bounds = saved_bounds
            if not copyable:
                self.rec("E-TYP-090", f"derive From[{source.show()}]: field `{fname}: {st.show()}` is not Copyable", "LANG-06-07", tok, {"expected_type": "Copyable", "actual_type": st.show(), "primary_span": fname})

    def same_type(self, a: Type, b: Type) -> bool:
        return (a == b and not origin_clash(a, b)) or isinstance(a, Unknown) or isinstance(b, Unknown)

    def check_implements(self, d: Decl, iface: Named, methods: Dict[str, Sig], tok, via_block=False):
        idecl = self.resolve_type(iface)
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
                expected, actual = diagnostic_signatures(isig, have)
                self.rec("E-TYP-080", f"{d.name}.{mname} does not satisfy {iface.name}.{mname}: {p}", "LANG-09-03", tok, {"expected_type": expected, "actual_type": actual, "primary_span": mname})
        for cname in idecl.constants:
            if cname not in d.constants:
                self.rec("E-TYP-080", f"{d.name} implements {iface.name} but lacks constant `{cname}`", "LANG-09-03", tok, {"expected_type": cname, "actual_type": "absent", "primary_span": cname})
        isub = dict(zip(idecl.type_params, iface.args))
        for req in idecl.requires:
            want = self.subst(req, isub)
            if not self.requirement_met(d, self_t, idecl, want):
                self.rec("E-TYP-080", f"{d.name} implements {iface.show()}, which requires {want.show()}; {d.name} does not implement it", "LANG-09-03", tok, {"expected_type": want.show(), "actual_type": d.name, "primary_span": d.name})
        # the interface's own `where` holds for the arguments the implementation gives it (9.5; D-69)
        for tp, bound in list(idecl.where or []):
            arg = isub.get(tp)
            if arg is None or isinstance(arg, Unknown):
                continue
            if isinstance(arg, TypeParam):
                if not any(p == arg.name and b == bound for p, b in list(d.where or [])):
                    display_0, display_1, display_2 = diagnostic_types(iface, bound, arg)
                    self.rec("E-TYP-080", f"{d.name} implements {display_0}, whose `where` needs `{tp}` to implement {display_1}; {d.name}'s own `where` does not grant it for {display_2}", "LANG-09-03", tok, {"expected_type": display_1, "actual_type": display_2, "primary_span": d.name})
            elif not self.implements(arg, bound.interface_key()):
                display_0, display_1, display_2 = diagnostic_types(iface, bound, arg)
                self.rec("E-TYP-080", f"{d.name} implements {display_0}, whose `where` needs `{tp}` to implement {display_1}; {display_2} does not (9.5)", "LANG-09-03", tok, {"expected_type": display_1, "actual_type": display_2, "primary_span": d.name})

    def decl_in_module(self, module: str, name: str) -> Optional[Decl]:
        """The declaration `name` reaches in `module`'s own table: this table for this module, the loaded table of a
        package module, and this table for a standard module loaded into it."""
        if module == self.module_name:
            d = self.table.get(name)
        else:
            table = next((sub for (_, path), sub in list(self.session.loaded.items()) if path == module), None)
            d = table.get(name) if table is not None else None
            if d is None:
                d = self.table.get(name)
        return d if d is not None and d.kind != "unknown" else None

    def requirement_met(self, d: Decl, self_t: Type, idecl: Decl, want: Named) -> bool:
        """Does `d` implement what `idecl` requires (9.5): the declaration the requirement names as the interface's own
        module sees it, with the type arguments the requirement states (D-67; 4.4 makes other modules' and other
        instantiations' interfaces distinct). A core, structural or bundle requirement, or one met by a derivation or
        an `implement` block, is judged by name."""
        if not self.implements(self_t, want.interface_key()):
            return False
        rd = self.resolve_type(want)
        if rd is None or rd.kind != "interface" or self.std_module_name(rd.module):
            return True
        listed = [i for i in d.implements if i.interface_key() == want.interface_key()]
        if not listed:
            return True
        for i in listed:
            ld = self.resolve_type(i)
            if ld is not None and (ld.module, ld.name) != (rd.module, rd.name):
                continue
            if want.args and i.args != want.args:
                continue
            return True
        return False

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
        idecl0 = self.resolve_type(iface)
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
        idecl = self.resolve_type(iface)
        if idecl is not None:
            for p, a in zip(idecl.type_params, iface.args):
                subst[p] = a
        for (pn, pt, pm), (hn, ht, hm) in zip(isig.params, have.params):
            pt2 = self.subst(pt, subst)
            if pm != hm:
                problems.append(f"parameter `{hn}` mode `{hm}` differs from `{pm}`")
            if not self.param_compatible(pt2, ht):
                display_0, display_1 = diagnostic_types(pt2, ht)
                problems.append(f"parameter `{hn}` narrows {display_0} to {display_1}")
        rt = self.subst(isig.ret, subst)
        if not self.return_compatible(rt, have.ret):
            display_0, display_1 = diagnostic_types(have.ret, rt)
            problems.append(f"return {display_0} widens {display_1}")
        return problems

    def subst(self, t: Type, m: Dict[str, Type]) -> Type:
        if isinstance(t, Named):
            if t.name in m and not t.args: return m[t.name]
            return Named(t.name, tuple(self.subst(a, m) for a in t.args), t.package, t.origin)
        if isinstance(t, TypeParam) and t.name in m: return m[t.name]
        if isinstance(t, OptionalT): return OptionalT(self.subst(t.inner, m))
        if isinstance(t, LoanT): return LoanT(self.subst(t.inner, m), t.mode, t.origin)
        if isinstance(t, TupleT): return TupleT(tuple(self.subst(x, m) for x in t.items))
        if isinstance(t, FnT): return FnT(tuple(self.subst(x, m) for x in t.params), self.subst(t.ret, m), t.kind, t.modifiers, t.origin, t.dynamic, t.identity, t.modes, t.capture_origins)
        if isinstance(t, RecordT): return RecordT(tuple((k, self.subst(x, m)) for k, x in t.fields))
        return t

    def param_compatible(self, iface_t: Type, impl_t: Type) -> bool:
        """The implementation's parameter type must accept everything the interface's does."""
        if isinstance(iface_t, Unknown) or isinstance(impl_t, Unknown): return True
        if origin_clash(iface_t, impl_t): return False
        if isinstance(iface_t, Refined) and isinstance(impl_t, Refined) and iface_t.base == impl_t.base:
            lo_ok = impl_t.lo is None or (iface_t.lo is not None and impl_t.lo <= iface_t.lo)
            hi_ok = impl_t.hi is None or (iface_t.hi is not None and impl_t.hi >= iface_t.hi)
            return lo_ok and hi_ok
        if isinstance(iface_t, Refined) and isinstance(impl_t, Named): return iface_t.base == impl_t
        if isinstance(iface_t, Named) and isinstance(impl_t, Refined): return False
        return iface_t == impl_t

    def return_compatible(self, iface_t: Type, impl_t: Type) -> bool:
        if isinstance(iface_t, Unknown) or isinstance(impl_t, Unknown): return True
        if origin_clash(iface_t, impl_t): return False
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
            idecl = self.interface_decl(n)
            if idecl is None or idecl.kind != "interface":
                return False
            for r in idecl.requires:
                if r.interface_key() == d.as_named().interface_key():
                    self.rec("E-TYP-060", f"interface requirement cycle: {' requires '.join(path + [r.name])}", "LANG-09-03", tok, {"expected_type": "an acyclic requirement graph", "actual_type": " -> ".join(path + [r.name]), "primary_span": d.name})
                    return True
                if r.interface_key() not in seen:
                    seen.add(r.interface_key())
                    if visit(r.interface_key(), path + [r.name]):
                        return True
            return False
        visit(d.as_named().interface_key(), [d.name])

    @staticmethod
    def target_spelling(node) -> Optional[str]:
        """The name an `implement` block's target is written with (`money.Currency`, `Tag`), or None for a form that
        is not a plain named type."""
        ts = [c for c in children(node) if name(c) == "type"]
        kids = children(ts[1]) if len(ts) >= 2 else []
        nk = children(kids[0]) if kids and name(kids[0]) == "namedType" else []
        if nk and name(nk[0]) == "typeRef":
            dts = direct_tokens(nk[0])
            return dts[0].text + "." + dts[2].text if dts[0].kind == "LOWER" and len(dts) >= 3 else dts[0].text
        return None

    @staticmethod
    def plain_target_spelling(node) -> Optional[str]:
        """The spelling of a block's target when it is a plain name (`Box`, `shapes.Box`): no type arguments, not
        optional."""
        ts = [c for c in children(node) if name(c) == "type"]
        if len(ts) < 2:
            return None
        kids = children(ts[1])
        if len(kids) != 1 or name(kids[0]) != "namedType":
            return None
        nk = children(kids[0])
        if len(nk) != 1 or name(nk[0]) != "typeRef" or any(name(c) == "typeArgs" for c in children(nk[0])):
            return None
        return DeclChecker.target_spelling(node)

    def implement_target_decl(self, node, target: Type) -> Optional[Decl]:
        """The declaration an `implement` block's target names, found by the block's own spelling: `money.Currency`
        through a module alias is that module's `Currency`, never a program class that happens to share the name
        (the canonical `Named` carries only the name, and a core type's identity is as empty as a single-file program's)."""
        spelled = self.target_spelling(node)
        d = self.table.decls.get(spelled) if spelled is not None else None
        if d is not None and d.kind != "unknown":
            return d
        return self.table.get(target.name) if isinstance(target, Named) else None

    def sibling_block_entries(self):
        """The `implement` blocks of this package's other modules (read from their trees: a module loaded as a library
        keeps no blocks), each with the declaration its plainly named target resolves to in its own module (aliases are
        per module), so a clash between modules that do not import each other is seen."""
        cached = getattr(self, "_sibling_entries", None)
        if cached is not None:
            return cached
        out = []
        if self.package:
            seen = set()
            for key, lm in sorted(self.lib.modules.items(), key=lambda kv: (-len(kv[0]), kv[0])):
                if id(lm) in seen:
                    continue
                seen.add(id(lm))
                if getattr(lm, "library", None) is None or getattr(lm, "package", None) != self.package or key == self.module_name:
                    continue
                sub = _loaded(self.session, lm, key)
                for dd in tree.declarations(lm.tree):
                    if dd["kind"] != "implement":
                        continue
                    sp = self.plain_target_spelling(dd["node"])
                    d = sub.get(sp) if sp is not None else None
                    out.append((key, dd["node"], d if d is not None and d.kind != "unknown" else None))
        self._sibling_entries = out
        return out

    def mixin_provided(self, tdecl: Decl) -> List[Tuple[str, str]]:
        """The methods the mixins of a class provide (with bodies), with the providing mixin, as the class's module sees
        them."""
        out: List[Tuple[str, str]] = []
        home = None
        for mx in tdecl.mixins:
            md = self.resolve_type(mx)
            if (md is None or md.kind != "mixin") and tdecl.module != self.module_name:
                if home is None:
                    lm = self.lib.modules.get(tdecl.module)
                    home = _loaded(self.session, lm, tdecl.module) if lm is not None and getattr(lm, "library", None) is not None else False
                md = home.get(mx.name) if home else None
            if md is None or md.kind != "mixin":
                continue
            for mname in md.methods:
                if not mname.startswith("requires:"):
                    out.append((mname, mx.name))
        return out

    @staticmethod
    def implement_target_excluded(tdecl: Decl) -> Optional[str]:
        """Why a declaration cannot be an `implement` block's target in Language 1.0 (X-180), or None when it can: the
        target is a non-generic class or enum the program declares. The two extensions of the user's option B, a
        newtype and a generic class or enum, stand by the user's decision of 29 Oct 2026; the newtype exclusion may be
        relaxed for a block inside the declaring module if a newtype comes to expose its representation there (Q2), so
        the whole test lives here, in one place."""
        core = tdecl.module == "fortis" or tdecl.module.startswith("fortis.")
        if core:
            return "a core library type"
        if tdecl.kind in ("class", "enum"):
            return f"a generic {tdecl.kind}" if tdecl.type_params else None
        return "a newtype" if tdecl.kind == "type" else f"a declaration of kind `{tdecl.kind}`"

    def check_implement_targets(self):
        """Language 1.0 (X-180): an `implement` block's target is a non-generic class or enum the program declares, and
        the block is that type listing the interface with the block's methods. Core types, newtypes and generic types
        have no run-time identity of their own (or of their instantiation) in 1.0's backends, so a block for one could
        not be honoured by `==`, interpolation, the codec or dispatch through a type parameter (E-TYP-171). A block
        method the target already has (its own, or one its mixins provide), or that another block for it adds in any
        module of the package (the earlier in module, line and column order keeps it), is a member declared twice
        (E-TYP-070). A later release lifts the limit."""
        blocks = [(t, m, n) for _, t, m, n in self.table.implement_blocks] + list(self.table.other_implement_targets)
        blocks = sorted([b for b in blocks if b[1] == self.module_name], key=lambda b: (tokens(b[2])[0].line, tokens(b[2])[0].col))
        found = []
        targets: Dict[str, Decl] = {}
        for target, module, node in blocks:
            ts = [c for c in children(node) if name(c) == "type"]
            tok = tokens(ts[1])[0]
            iface = tokens(ts[0])[0].text if ts else ""
            shown = target.show()
            spelled = self.target_spelling(node)
            if isinstance(target, Named) and not target.args and spelled is not None and "." in spelled:
                shown = spelled                                  # a module alias's type, as the block writes it
            if isinstance(target, Named):
                tdecl = self.implement_target_decl(node, target)
                if tdecl is None or tdecl.kind == "unknown":
                    continue                                     # an unresolved name is reported as such
                what = self.implement_target_excluded(tdecl)
                if what is None:
                    targets.setdefault(f"{tdecl.module}.{tdecl.name}", tdecl)
                    continue
            else:
                what = "not a class or enum"
            found.append((tok.line, tok.col, "E-TYP-171",
                          f"`implement {iface} for {shown}`: in Language 1.0 an `implement` block's target must be a non-generic class or enum the "
                          f"program declares, and `{shown}` is {what} (X-180); implementations for such targets come in a "
                          f"later release. Wrap the value in a class the program declares, or list the interface in its `implements`",
                          "LANG-06-13", tok, {"expected_type": "a non-generic class or enum the program declares", "actual_type": shown, "primary_span": shown}))
        # member clashes, target by target, over every block of the package for it
        for key, tdecl in targets.items():
            entries = []
            for _, target, module, node in self.table.implement_blocks:
                if module == self.module_name and isinstance(target, Named):
                    d = self.implement_target_decl(node, target)
                    if d is not None and f"{d.module}.{d.name}" == key:
                        entries.append((module, tokens(node)[0].line, tokens(node)[0].col, node))
            for module, node, d in self.sibling_block_entries():
                if d is not None and f"{d.module}.{d.name}" == key:
                    entries.append((module, tokens(node)[0].line, tokens(node)[0].col, node))
            entries.sort(key=lambda e: (e[0], e[1], e[2]))
            have: Dict[str, Tuple[str, str, str]] = {}
            for m in list(tdecl.fields) + list(tdecl.methods):
                have.setdefault(m, ("own", "", ""))
            for m, mx in self.mixin_provided(tdecl):
                have.setdefault(m, ("mixin", mx, ""))
            for module, _, _, node in entries:
                ts = [c for c in children(node) if name(c) == "type"]
                iface = tokens(ts[0])[0].text if ts else ""
                bound = next((i for i, _, m, n in self.table.implement_blocks if m == module and n is node), None)
                if bound is not None and (any(i == bound for i in tdecl.implements) or any(n == bound.interface_key() for n, _ in tdecl.derives)):
                    continue                                 # the whole block overlaps an implementation: E-TYP-100 alone
                spelled = self.target_spelling(node)
                shown = spelled if spelled is not None and "." in spelled else tdecl.name
                for mm in [c for c in children(node) if name(c) == "implementMember"]:
                    head = find(mm, "methodHead")
                    if head is None:
                        continue
                    mtok = tokens(find(head, "memberName"))[0]
                    src = have.get(mtok.text)
                    if src is not None and module == self.module_name:
                        if src[0] == "own":
                            how = f"a member {shown} already has"
                        elif src[0] == "mixin":
                            how = f"a member {shown}'s mixin {src[1]} provides"
                        else:
                            how = f"a member that `implement {src[1]} for {shown}` also adds" + (f" (module {src[2]})" if src[2] != self.module_name else "")
                        found.append((mtok.line, mtok.col, "E-TYP-070",
                                      f"member `{mtok.text}` of {shown} is declared twice: `implement {iface} for {shown}` adds {how} "
                                      f"(an `implement` block is its target listing the interface, X-180)",
                                      "LANG-09-01", mtok, {"expected_type": "one member", "actual_type": "two", "primary_span": mtok.text}))
                    if src is None:
                        have[mtok.text] = ("block", iface, module)
        for _, _, code, msg, rule, tok, fields in sorted(found, key=lambda x: (x[0], x[1])):
            self.rec(code, msg, rule, tok, fields)

    @staticmethod
    def whole_type_key(tdecl: Optional[Decl], t: Named) -> str:
        """A named type's identity for a pairing rule: the declaration it names (module and name) with its type
        arguments as written, so `Box[Int]` and `Box[String]` are two types and two modules' `Box` are two types. The
        coherence of `implement` blocks keys its (interface, target) pairs by it (9.5), and a conversion pair is
        keyed by it too (6.13, X-551)."""
        base = f"{tdecl.type_identity or tdecl.core_home or tdecl.module}.{tdecl.name}" if tdecl is not None else f"{t.package or t.origin}.{t.name}"
        return base + repr(t.args)

    def is_core_type(self, t: Type) -> bool:
        t = strip(t)
        if isinstance(t, Refined): t = t.base
        return isinstance(t, Named) and not t.package

    def named_decl(self, t: Type) -> Optional[Decl]:
        """Resolve every nominal identity, including two modules' same-named declarations."""
        return self.resolve_type(t) if isinstance(t, Named) else None

    def all_conversions(self, d: Decl):
        """The explicit members and the retired From implementations, keyed by their actual interface identity."""
        out = list(d.converts)
        for c in d.converts:
            if c.direction == "from" and self.is_core_type(c.other) and c.other in (STRING, STRING_VIEW):
                out.append(replace(c, other=STRING_VIEW if c.other == STRING else STRING))
        for iface, args in d.derives:
            if iface in ("From", "convert") and len(args) == 1:
                if iface == "convert" and d.kind == "enum" and self.same_type(args[0], STRING) and self.is_core_type(args[0]):
                    for source in (STRING, STRING_VIEW):
                        sig = Sig("derive:from:" + source.show(), [("source", source, "")], d.as_named(), None, ("try",), (), d.exported, d.node)
                        out.append(Convert("from", source, True, False, d.exported, sig, d.node, True, True))
                    continue
                source = args[0]
                qual = (source.package or source.origin) if isinstance(source, Named) else ""
                sig = Sig("derive:from:" + source.show() + ("@" + qual if qual else ""), [("source", source, "")], d.as_named(tuple(TypeParam(p) for p in d.type_params)), None, (), (), d.exported, d.node)
                out.append(Convert("from", source, False, False, d.exported, sig, d.node, True, True))
        for iface in d.implements:
            idecl = self.resolve_type(iface)
            if iface.name == "From" and len(iface.args) == 1 and idecl is not None and standard(idecl.module):
                sig = d.methods.get("from")
                if sig is not None:
                    out.append(Convert("from", iface.args[0], False, False, sig.exported, sig, sig.node, False, True))
        for iface, target, module, node in self.table.implement_blocks:
            idecl = self.resolve_type(iface)
            if iface.name != "From" or len(iface.args) != 1 or idecl is None or not standard(idecl.module) or self.named_decl(target) is not d:
                continue
            for member in children(node):
                if name(member) != "implementMember":
                    continue
                head = find(member, "methodHead")
                if head is not None and tokens(find(head, "memberName"))[0].text == "from":
                    sig = self.signature(head, True, find(member, "funcBody"))
                    out.append(Convert("from", iface.args[0], False, False, True, sig, head, False, True))
        return out

    def type_substitution(self, d: Decl, t: Type):
        return dict(zip(d.type_params, t.args)) if isinstance(t, Named) else {}

    def instantiated_conversions(self, d: Decl, t: Type):
        """Legacy generic implementations retain ordinary written type arguments (X-555)."""
        subst = self.type_substitution(d, t)
        return [replace(c, other=self.subst(c.other, subst), sig=replace(c.sig,
                    params=[(n, self.subst(pt, subst), mode) for n, pt, mode in c.sig.params],
                    ret=self.subst(c.sig.ret, subst))) for c in self.all_conversions(d)]

    def newtype_conversion(self, src: Type, dst: Type):
        """The one representation step (6.9, 6.13, X-552): (declaration, representation, wrapping, proved).
        Match the representation's base by identity before considering its range. Never search through another
        newtype or a numeric conversion. The proof is the construction judgement of X-511."""
        s, d = strip(src), strip(dst)
        if isinstance(s, LoanT):
            s = strip(s.inner)
        # Unknown is compatible for recovery, but is never a representation identity or a proof (D-505).
        if not known(s) or not known(d):
            return None
        sd, dd = self.resolve_type(s), self.resolve_type(d)
        if sd is not None and sd.kind == "type" and sd.alias is not None:
            rep = strip(sd.alias)
            base = rep.base if isinstance(rep, Refined) else rep
            if self.same_type(d, rep) or self.same_type(d, base):
                return sd, rep, False, True
        if dd is not None and dd.kind == "type" and dd.alias is not None:
            rep = strip(dd.alias)
            base = rep.base if isinstance(rep, Refined) else rep
            sb = s.base if isinstance(s, Refined) else s
            literal = is_strlit(s) and base in (STRING, STRING_VIEW)
            if self.same_type(sb, base) or literal:
                proved = assignable(self, s, rep)[0] is True
                if isinstance(s, Refined) and isinstance(rep, Refined) and s.kind == rep.kind and self.same_type(s.base, rep.base):
                    proved = ((rep.lo is None or (s.lo is not None and s.lo >= rep.lo))
                              and (rep.hi is None or (s.hi is not None and s.hi <= rep.hi)))
                return dd, rep, True, proved
        return None

    def total_conversion(self, source: Type, target: Type, module: str) -> bool:
        """Exactly one total borrowed field conversion, never a search or a bare From interface test."""
        nc = self.newtype_conversion(source, target)
        if nc is not None:
            decl, rep, wrapping, proved = nc
            return self.is_trivial(rep) and proved and (wrapping or decl.module == module)
        sb, tb = conv_table.base_of(source), conv_table.base_of(target)
        if sb and tb and self.is_core_type(source) and self.is_core_type(target):
            row = conv_table.resolve(sb, tb)
            if row is not None:
                return not row.fallible and not isinstance(target, Refined)
        found = []
        source = source.base if isinstance(source, Refined) else source
        for t, direction, other in ((source, "to", target), (target, "from", source)):
            decl = self.resolve_type(t)
            if decl is not None and decl.kind in ("class", "enum"):
                found += [c for c in self.instantiated_conversions(decl, t) if c.direction == direction and self.same_type(c.other, other) and (c.exported or decl.module == module)]
        return len(found) == 1 and not found[0].fallible and not found[0].owned

    def check_conversions(self):
        """The declared conversions of every visible class and enum (6.13, X-551): the forms a member may take, and
        one conversion per pair. A pair is keyed by the whole type, as `implement` coherence keys its pairs (9.5);
        a member of an imported module counts for the overlap and is reported by its own module (as D-132)."""
        seen: Dict[Tuple[str, str], Tuple[str, str]] = {}
        visited: Set[int] = set()
        for d in list(self.table.decls.values()):
            if d.kind not in ("class", "enum") or id(d) in visited:
                continue
            visited.add(id(d))
            for cv in self.all_conversions(d):
                tok = tokens(cv.node)[0]
                local = d.module == self.module_name
                src = cv.other if cv.direction == "from" else d.as_named()
                dst = d.as_named() if cv.direction == "from" else cv.other
                key = (self.whole_type_key(self.named_decl(src) if isinstance(src, Named) else None, src) if isinstance(src, Named) else src.show(),
                       self.whole_type_key(self.named_decl(dst) if isinstance(dst, Named) else None, dst) if isinstance(dst, Named) else dst.show())
                if local:
                    if d.type_params and not cv.legacy:
                        self.rec("E-TYP-184", f"`convert` member of a generic {d.kind}: {d.name} has type parameters, and a conversion is a pair of whole types (6.13, X-551)", "LANG-06-15", tok, {"expected_type": "a non-generic class or enum", "actual_type": f"a generic {d.kind}", "primary_span": d.name})
                    if not isinstance(cv.other, Named) and not cv.legacy:
                        self.rec("E-TYP-184", f"`convert {cv.direction} {cv.other.show()}`: the other type of a conversion is a nominal type, never {cv.other.show()} (a refinement target stays the built-in `validate` row, 6.13)", "LANG-06-15", tok, {"expected_type": "a nominal type", "actual_type": cv.other.show(), "primary_span": cv.other.show()})
                    elif self.named_decl(cv.other) is d or key[0] == key[1]:
                        self.rec("E-TYP-183", f"`convert {cv.direction} {cv.other.show()}` of {d.name}: the pair is the identity, which 6.13 already decides: a conversion converts nothing here", "LANG-06-15", tok, {"expected_type": "a different type", "actual_type": d.name, "primary_span": cv.other.show()})
                    elif self.newtype_conversion(src, dst) is not None:
                        self.rec("E-TYP-183", f"`convert {cv.direction} {cv.other.show()}` of {d.name}: the pair ({src.show()}, {dst.show()}) is a built-in newtype representation conversion (6.13)", "LANG-06-15", tok, {"expected_type": "a pair the table does not decide", "actual_type": f"{src.show()} to {dst.show()}", "primary_span": cv.other.show()})
                    elif (isinstance(src, Named) and isinstance(dst, Named) and not src.args and not dst.args
                          and self.is_core_type(src) and self.is_core_type(dst)
                          and (conv_table.same_builtin(src.name, dst.name) or conv_table.resolve(src.name, dst.name) is not None)):
                        self.rec("E-TYP-183", f"`convert {cv.direction} {cv.other.show()}` of {d.name}: the pair ({src.show()}, {dst.show()}) is a row of the built-in table, which needs no declaration (6.13)", "LANG-06-15", tok, {"expected_type": "a pair the table does not decide", "actual_type": f"{src.show()} to {dst.show()}", "primary_span": cv.other.show()})
                    if key in seen:
                        first = seen[key]
                        self.rec("E-TYP-182", f"two conversions are declared for the pair ({src.show()}, {dst.show()}): this one, and one {first[0]} declares (module {first[1]}): one conversion per pair (6.13)", "LANG-06-15", tok, {"expected_type": "one conversion", "actual_type": "two", "primary_span": cv.other.show()})
                seen.setdefault(key, (d.name, d.module))

    def check_coherence(self):
        self.check_implement_targets()
        seen: Dict[Tuple[str, str], str] = {}
        for iface, target, module, node in self.table.implement_blocks:
            tok = tokens(node)[0]
            idecl, tdecl = self.resolve_type(iface), self.implement_target_decl(node, target)
            # one implementation per (interface, target type): the target's declaration with its type arguments, so
            # `Box[Int]` and `Box[String]` are distinct targets (9.5)
            key = (self.whole_type_key(idecl, iface), self.whole_type_key(tdecl, target))
            if module != self.module_name:
                # an imported module's block (D-132): it counts for overlap; that module reports its own records
                seen.setdefault(key, module)
                continue
            local_iface = idecl is not None and idecl.package == self.package and idecl.module.split(".")[0] not in ("fortis",) and not idecl.module.startswith("fortis.")
            local_type = tdecl is not None and tdecl.package == self.package and not tdecl.module.startswith("fortis.")
            if not (local_iface or local_type):
                self.rec("E-TYP-100", f"`implement {iface.name} for {target.name}`: neither is declared in this package (coherence)", "LANG-06-10", tok, {"expected_type": "a local interface or type", "actual_type": f"{iface.name}/{target.name}", "primary_span": target.name})
            if key in seen or (tdecl is not None and (any(i == iface for i in tdecl.implements) or any(n == iface.interface_key() for n, _ in tdecl.derives))):
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
