"""Name resolution over parse trees (Language Reference 4.3–4.5; DEL-06/DEL-07).

Scopes: the implicit prelude (reference/library/prelude.ft), the module's declarations and imports,
then lexical scopes for parameters, `let` bindings, `for` variables, `scope` names, `catch`
bindings, `report ... as` receipts, pattern bindings and lambda parameters/captures. Every root
identifier in expression, place and type position must resolve (E-NAM-001); shadowing a visible
binding is E-NAM-003; `Alias.member` and `Type.member` are checked against the imported module or
the known class's associated functions when the left side is known.
"""
from __future__ import annotations

import os
import sys
from typing import Dict, List, Optional, Set

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diagnostics  # noqa: E402
import lexer        # noqa: E402
import reading      # noqa: E402
import tree         # noqa: E402
from tree import children, name, tokens, direct_tokens, find, find_all  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
LIB = os.path.join(ROOT, "reference", "library")
CORE_MODULES = ["prelude", "text", "collections", "numeric", "time", "money", "host"]
# the entry point's modifiers (LR 4.6): the four forms, each of which may also carry `recovers` (X-155); the
# grammar's modifier order is async, try, recovers
ENTRY_FORMS = {"", "async", "try", "async try", "recovers", "async recovers", "try recovers", "async try recovers"}
ENTRY_SIGNATURE = "( args : List [ String ] ) Void"
# the declaration heads whose kit this toolchain does not install (LR 16, Platform 1.2; X-231): head word ->
# (kit, the package that defines it). The Dependency injection kit's `context` is installed (fortis-di).
UNINSTALLED_HEADS = {
    "application": ("Runtime host", "fortis.runtime"),
    "model": ("Data", "fortis.data"),
    "explore": ("Developer tooling", ""),
    "view": ("Web UI", "fortis.web.ssr"),
    "component": ("Web UI", "fortis.web.components"),
}


class LibraryModule:
    """Declarations of a signature module: name -> {kind, members (associated functions, fields, variants), exported}."""
    def __init__(self, path: str, rec, tree_=None, package: str = "fortis"):
        self.path = path
        self.package = package
        if tree_ is None:
            src = open(path, encoding="utf-8").read()
            tree_ = rec.parse(lexer.lex(src), "module")
        self.tree = tree_
        self.decls: Dict[str, dict] = {}
        for d in tree.declarations(self.tree):
            if d["name"]:
                self.decls[d["name"]] = {"kind": d["kind"], "exported": d["exported"], "members": members_of(d["node"]), "node": d["node"]}


def exported(lm: LibraryModule) -> List[str]:
    """The candidates of a member a module does not declare: the members another module can name, its exported ones."""
    return sorted(k for k, d in lm.decls.items() if d["exported"])


def members_of(node) -> Set[str]:
    """Associated functions, methods, fields, variants and constants declared directly in a class/enum/interface/mixin."""
    out: Set[str] = set()
    for member_rule in ("classMember", "enumMember", "interfaceMember", "mixinMember", "implementMember"):
        for m in children(node):
            if name(m) != member_rule:
                continue
            head = find(m, "methodHead")
            if head is not None:
                mn = find(head, "memberName")
                out.add(tokens(mn)[0].text)
                continue
            v = find(m, "variant")
            if v is not None:
                out.add(direct_tokens(v)[0].text)
                continue
            fd = find(m, "fieldDecl")
            if fd is not None:
                out.add(direct_tokens(fd)[0].text)
                continue
            cd = find(m, "constDecl")
            if cd is not None:
                out.add(direct_tokens(cd)[1].text)
    # `derive Default` generates the associated `default()` (6.10, 9.2; D-170)
    kids = children(node)
    for i, k in enumerate(kids):
        if isinstance(k, lexer.Token) and k.text == "derive" and i + 1 < len(kids) and not isinstance(kids[i + 1], lexer.Token):
            if any(t.kind == "UPPER" and t.text == "Default" for t in direct_tokens(kids[i + 1])):
                out.add("default")
            if any(t.text in ("From", "convert") for t in direct_tokens(kids[i + 1])):
                out.add("from")
    return out


class Library:
    def __init__(self, rec):
        self.rec = rec
        self.modules: Dict[str, LibraryModule] = {}
        for m in CORE_MODULES:
            p = os.path.join(LIB, m + ".ft")
            if os.path.exists(p):
                self.modules["fortis." + m] = LibraryModule(p, rec)
        for sub in ("kits", "examples"):              # kit signature fixtures; the reference examples' illustrative dependencies
            d = os.path.join(LIB, sub)
            if os.path.isdir(d):
                for f in sorted(os.listdir(d)):
                    if f.endswith(".ft"):
                        self.modules[f[:-3]] = LibraryModule(os.path.join(d, f), rec, package=("fortis" if sub == "kits" else f[:-3].rsplit(".", 1)[0] + "@0.0.0"))
        self.prelude = self.modules.get("fortis.prelude")


class Scope:
    def __init__(self, parent: Optional["Scope"] = None):
        self.parent = parent
        self.names: Dict[str, dict] = {}

    def lookup(self, n: str) -> Optional[dict]:
        s: Optional[Scope] = self
        while s is not None:
            if n in s.names:
                return s.names[n]
            s = s.parent
        return None

    def declare(self, n: str, info: dict):
        self.names[n] = info


class Resolver:
    def __init__(self, library: Library, file: str, entry: bool = False):
        self.lib = library
        self.file = file
        # `entry`: the module is checked on its own, as the entry module of an implicit one-module application
        # (`fortis check|run FILE.ft`); a module of a package is judged by the manifest's `entry` instead
        # (resolve.py `check_entry`), so the package pipelines construct the resolver without it (D-51.2).
        self.entry = entry
        self.records: List[dict] = []

    def rec(self, code, msg, rule, tok, fields):
        self.records.append(diagnostics.make(code, msg, rule, self.file, tok.line, tok.col, fields))

    def bind_import(self, bound: Dict[str, str], n: str, path: str, tok):
        """An import binds `n` (LR 4.3): a name an earlier import has bound is rejected, as the package resolver
        rejects it (resolve.py `check_module`); a later local declaration that collides is the declarations' check."""
        if n in bound:
            # the candidate is the first binding: a later import of the same name does not replace it (LR 4.3)
            self.rec("E-NAM-001", f"import binds {n} twice or collides with a declaration", "LANG-04-03", tok, {"name": n, "candidates": [bound[n]]})
            return
        bound[n] = path

    def check_entry(self):
        """A module checked on its own is the entry module of an implicit one-module application (`fortis run
        FILE.ft` runs its `main`): the `main` it declares, exported or not (X-345), must have one of the four forms
        of LR 4.6, each of which may also carry `recovers` (X-155), and each parameter must be one the runtime
        supplies by type (`E-NAM-005`, `E-NAM-006`). A module of a package is judged by the manifest's `entry`
        instead (resolve.py `check_entry`); a module with no `main` function and no exported `main` is a library
        module (D-51.2)."""
        if not self.entry:
            return
        # the function named main is the entry point (LR 4.6), `export` implied. A module that has none, and exports
        # no other declaration named main, is a library module. An exported non-function main is not: the entry
        # must be a function of 4.6.
        mains = [d for d in self.decls if d["name"] == "main" and (d["kind"] == "func" or d["exported"])]
        if not mains:
            return
        main = next((d for d in mains if d["kind"] == "func"), mains[0])
        if main["kind"] != "func":
            self.rec("E-NAM-001", f"exported main is a {main['kind']}, not one of the forms of 4.6", "LANG-04-06", main["token"],
                     {"name": "main", "candidates": [reading.ENTRY_FORM]})
            return
        head = find(main["node"], "funcHead")
        mods = find(head, "modifiers")
        words = " ".join(t.text for t in tokens(mods)) if mods else ""
        sig = find(head, "funcSignature")
        rt = find(sig, "returnType")
        ret = " ".join(t.text for t in tokens(rt)) if rt is not None else ""     # none: `Void` left out (R6, X-345)
        if words not in ENTRY_FORMS or ret not in ("Void", "") or find(sig, "typeParams") is not None or find(sig, "whereClause") is not None:
            text = " ".join(t.text for t in tokens(sig))
            self.rec("E-NAM-001", f"main has signature `{words} func main{text}`, not one of the forms of 4.6", "LANG-04-06", main["token"],
                     {"name": "main", "candidates": [reading.ENTRY_FORM]})
            return
        # the token text can match while `Void` names a module-level declaration or import. 4.6's forms use the
        # prelude's Void, and its `List[String]` the prelude's List and String (D-51.2)
        shadowed = self.entry_type_shadowed(("Void",))
        if shadowed:
            self.rec("E-NAM-001", f"main's `{shadowed}` is not the prelude {shadowed}, so the signature is not one of the forms of 4.6", "LANG-04-06", main["token"],
                     {"name": "main", "candidates": [reading.ENTRY_FORM]})
            return
        for code, msg, tok, fields in reading.param_findings(sig, self.entry_origin):
            self.rec(code, msg, "LANG-04-06", tok, fields)

    def entry_origin(self, n: str) -> Optional[str]:
        """Where the module's name `n` comes from, for 4.6's parameter types (reading.param_findings)."""
        info = self.module_scope.names.get(n)
        if info is None:
            return None
        if info.get("kind") == "module" and "path" in info:
            return "alias:" + info["path"]
        return info.get("origin")

    def entry_type_shadowed(self, names=("List", "String", "Void")) -> Optional[str]:
        """The first of 4.6's `List`, `String` and `Void` that a module-level binding has replaced. An unbound name
        is the prelude's when the prelude is loaded, and a missing prelude is not reported here."""
        for n in names:
            info = self.module_scope.names.get(n)
            if info is not None and info.get("origin") != "prelude":
                return n
        return None

    # ---- module level -----------------------------------------------------
    def resolve_module(self, mtree):
        mod = Scope()
        self.module_scope = mod
        self.enum_variants: Set[str] = set()
        # prelude
        if self.lib.prelude:
            for n, d in self.lib.prelude.decls.items():
                mod.declare(n, {"kind": d["kind"], "members": d["members"], "origin": "prelude"})
                if d["kind"] == "enum":
                    self.enum_variants |= d["members"]
        # imports
        bound: Dict[str, str] = {}           # the names the imports bind: importing one name twice is an error (LR 4.3, D-51.4)
        for imp in tree.imports(mtree):
            path = ".".join(imp["path"])
            libmod = self.lib.modules.get(path)
            if imp["members"] is None:
                alias = imp["alias"] or imp["path"][-1]
                self.bind_import(bound, alias, path, imp["token"])
                mod.declare(alias, {"kind": "module", "module": libmod, "path": path})
            else:
                for m in imp["members"]:
                    if libmod is not None and m not in libmod.decls:
                        self.rec("E-NAM-001", f"{path} declares no {m}", "LANG-04-03", imp["token"], {"name": m, "candidates": sorted(libmod.decls)})
                    self.bind_import(bound, m, path, imp["token"])
                    d = libmod.decls.get(m) if libmod else None
                    mod.declare(m, {"kind": d["kind"] if d else "imported", "members": d["members"] if d else set(), "origin": path})
                    if d and d["kind"] == "enum":
                        self.enum_variants |= d["members"]
        # declarations
        self.decls = tree.declarations(mtree)
        for d in self.decls:
            if d["name"]:
                if d["name"] in mod.names and mod.names[d["name"]].get("origin") not in ("prelude",):
                    self.rec("E-NAM-001", f"{d['name']} is declared twice or collides with an import", "LANG-04-03", d["token"], {"name": d["name"], "candidates": []})
                mod.declare(d["name"], {"kind": d["kind"], "members": members_of(d["node"]), "origin": "module", "node": d["node"]})
                if d["kind"] == "enum":
                    self.enum_variants |= members_of(d["node"])
        self.check_entry()
        for d in self.decls:
            self.resolve_decl(d)
        return self.records

    def resolve_decl(self, d):
        node = d["node"]
        kind = d["kind"]
        scope = Scope(self.module_scope)
        for tp in find_all(node, "typeParams"):
            for t in tokens(tp):
                if t.kind == "UPPER":
                    scope.declare(t.text, {"kind": "typeparam"})
        lp = find(node, "lifetimeParam")
        if lp is not None:
            scope.declare(tokens(lp)[2].text, {"kind": "lifetime"})
        if kind == "head":
            self.check_head(node)
            return                                  # opaque body, kit-validated (LR 16)
        if kind in ("class", "enum", "interface", "mixin", "implement"):
            self.check_types_in(node, scope, skip_rules={"classMember", "enumMember", "interfaceMember", "mixinMember", "implementMember"})
            for mem_rule in ("classMember", "enumMember", "interfaceMember", "mixinMember", "implementMember"):
                for m in children(node):
                    if name(m) == mem_rule:
                        self.resolve_member(m, scope)
            return
        if kind == "func":
            self.resolve_callable(find(node, "funcHead"), find(node, "funcBody"), scope, is_method=False)
            return
        if kind == "aspect":
            self.resolve_aspect(node, scope)
            return
        if kind in ("test", "property"):
            s2 = Scope(scope)
            ts = direct_tokens(node)
            if "as" in [t.text for t in ts]:
                s2.declare(ts[[t.text for t in ts].index("as") + 1].text, {"kind": "row"})
            pl = find(node, "paramList")
            if pl is not None:
                self.declare_params(pl, s2)
            for b in [c for c in children(node) if name(c) == "block"]:
                self.resolve_block(b, s2)
            for cr in find_all(node, "caseRow"):
                self.resolve_expr(find(cr, "recordLit"), s2)
            return
        # const, type, error, contract, observation
        self.check_types_beside(node, scope)
        for e in [c for c in children(node) if name(c) in ("constExpr", "expr")]:
            self.resolve_expr(e, scope)
        if kind == "error":
            self.check_error_message(node)

    def check_error_message(self, node):
        """An error's `message` is a template whose `${name}` may name only the payload's fields (LR 10.3; D-340):
        any other name is undeclared there, and the rendered message would show it unfilled."""
        ts = direct_tokens(node)
        tok = next((ts[i + 2] for i in range(len(ts) - 2) if ts[i].text == "message" and ts[i + 1].text == ":" and ts[i + 2].kind == "STR"), None)
        if tok is None:
            return
        fl = find(node, "fieldList")
        payload = [t.text for t in direct_tokens(fl) if t.kind == "LOWER"] if fl is not None else []
        rest = tok.text[1:-1]
        while "${" in rest:
            after = rest[rest.index("${") + 2:]
            end = after.find("}")
            if end < 0:
                break
            nm = after[:end]
            rest = after[end + 1:]
            if nm not in payload:
                name_ = next((t for t in ts if t.kind == "UPPER"), None)
                self.rec("E-NAM-001", f"the message of error {name_.text if name_ else ''} names `${{{nm}}}`, which is not a payload field: a message template may name only the payload's fields (10.3)", "LANG-04-05", tok,
                         {"name": nm, "candidates": payload})

    def check_head(self, node):
        """A head whose kit this toolchain does not install cannot be validated or elaborated, so it is rejected,
        naming the kit (LR 16, E-NAM-034; D-90, X-231). `context` is the Dependency injection kit's, which
        the toolchain elaborates before the check (fortis-di)."""
        hw = find(node, "headWord")
        word = tokens(hw)[0] if hw is not None else None
        prefix = next((t for t in direct_tokens(node) if t.text in ("server", "client", "hybrid")), None)
        if word is not None and word.text == "context" and prefix is not None:
            # the installed kit defines `context` without a prefix, and elaborates only that form
            self.rec("E-NAM-034", f"`{prefix.text} context` is a head form no installed kit defines: the Dependency injection kit's `context` takes "
                     f"no `server`, `client` or `hybrid` prefix, so the declaration can be neither validated nor elaborated (LR 16)", "PROG-DEL-04", word,
                     {"name": f"{prefix.text} context", "candidates": ["context"]})
            return
        if word is None or word.text not in UNINSTALLED_HEADS:
            return
        kit, package = UNINSTALLED_HEADS[word.text]
        where = f" (`{package}`)" if package else ""
        self.rec("E-NAM-034", f"`{word.text}` is a head of the {kit} kit{where}, which this toolchain does not install: the declaration can be "
                 f"neither validated nor elaborated, so it is not accepted (LR 16)", "PROG-DEL-04", word, {"name": word.text, "candidates": [package] if package else []})

    def resolve_member(self, m, scope):
        s = Scope(scope)
        head = find(m, "methodHead")
        if head is not None:
            body = find(m, "funcBody")
            self.resolve_callable(head, body, s, is_method=True)
            return
        cv = find(m, "convertDecl")
        if cv is not None:
            # a `convert` member (6.13, X-551): its head type resolves as a signature's; its body resolves as a
            # method body: `convert to T` has `self`, `convert from S(s)` binds the one parameter
            kids = children(cv)
            tr_i = next(i for i, c in enumerate(kids) if tree.is_node(c) and name(c) == "typeRef")
            self.check_types_in(kids[tr_i], s)
            if kids[tr_i - 1].text == "to":
                s.declare("self", {"kind": "self"})
            else:
                pname = next(t.text for t in kids[tr_i + 1:] if not tree.is_node(t) and t.kind == "LOWER")
                s.declare(pname, {"kind": "param"})
            body = find(cv, "funcBody")
            if body is not None:
                b = children(body)[0] if children(body) and name(children(body)[0]) == "block" else None
                if b is not None:
                    self.resolve_block(b, s)
                else:
                    e = find(body, "expr")
                    if e is not None:
                        self.resolve_expr(e, s)
            return
        v = find(m, "variant")
        if v is not None:
            self.check_types_in(v, s)
            return
        for fd in [c for c in children(m) if name(c) == "fieldDecl"]:
            self.check_types_beside(fd, s)
            ce = tree.child(fd, "constExpr")
            if ce is not None:
                self.resolve_expr(ce, s)
        cd = find(m, "constDecl")
        if cd is not None:
            self.check_types_beside(cd, s)
            self.resolve_expr(tree.child(cd, "constExpr"), s)
        cl = find(m, "cleanupDecl")
        if cl is not None:
            s.declare("self", {"kind": "self"})
            self.resolve_block(find(cl, "block"), s)

    def resolve_aspect(self, node, scope):
        s = Scope(scope)
        ap = find(node, "aspectParams")
        if ap is not None:
            self.declare_params(ap, s)
        for part in find_all(node, "aspectPart"):
            ts = direct_tokens(part)
            words = [t.text for t in ts]
            s2 = Scope(s)
            if words and words[0] == "needs" and len(ts) > 1 and ts[1].kind == "LOWER":
                s.declare(ts[1].text, {"kind": "needs"})
                self.check_types_in(part, s)
            elif words and words[0] == "after":
                s2.declare(ts[2].text, {"kind": "binding"})
            elif words and words[0] == "on":
                s2.declare(ts[3].text, {"kind": "binding"})
            for b in [c for c in children(part) if name(c) == "block"]:
                self.resolve_block(b, s2)
        # specialisation: aspectDecl "=" lowerIdent ... "(" argList ")"
        for al in [c for c in children(node) if name(c) == "argList"]:
            self.resolve_expr(al, s)

    def declare_params(self, pl, scope):
        for p in find_all(pl, "param") + find_all(pl, "lambdaParam"):
            rp = find(p, "restParam")
            if rp is not None:
                scope.declare(tokens(rp)[1].text, {"kind": "param"})      # an aspect's `...rest`
                continue
            ts = [t for t in direct_tokens(p) if t.text != "..."]
            if ts and ts[0].kind == "LOWER":
                if name(p) == "lambdaParam":
                    self.declare_binding(ts[0], scope, "param")     # a lambda parameter is a binding inside the enclosing scope (4.5)
                else:
                    scope.declare(ts[0].text, {"kind": "param"})
            if find(p, "selfParam") is not None:
                scope.declare("self", {"kind": "self"})
            self.check_types_beside(p, scope)
            ce = tree.child(p, "constExpr")
            if ce is not None:
                self.resolve_expr(ce, scope)

    def resolve_callable(self, head, body, scope, is_method):
        s = Scope(scope)
        for tp in find_all(head, "typeParams"):
            for t in tokens(tp):
                if t.kind == "UPPER":
                    s.declare(t.text, {"kind": "typeparam"})
        sig = find(head, "funcSignature")
        pl = find(sig, "paramList") if sig else None
        if is_method:
            s.declare("self", {"kind": "self"})
        if pl is not None:
            self.declare_params(pl, s)
        mods = find(head, "modifiers")
        for t in (tokens(mods) if mods is not None else []):
            if t.kind == "UPPER" and scope.lookup(t.text) is None:     # a named contract in the modifier position (11.5)
                self.rec("E-NAM-001", f"contract `{t.text}` is not declared", "LANG-04-05", t, {"name": t.text, "candidates": []})
        if sig is not None:
            rt = find(sig, "returnType")
            if rt is not None:
                self.check_types_in(rt, s)
            wc = find(sig, "whereClause")
            if wc is not None:
                self.check_types_in(wc, s)
        if body is not None:
            b = children(body)[0] if body is not None and children(body) and name(children(body)[0]) == "block" else None   # the body itself, not a lambda block inside an expression body
            if b is not None:
                self.resolve_block(b, s)
            else:
                e = find(body, "expr")
                if e is not None:
                    self.resolve_expr(e, s)

    # ---- statements -------------------------------------------------------
    def resolve_block(self, block, scope):
        s = Scope(scope)
        for st in [c for c in children(block) if name(c) == "statement"]:
            self.resolve_statement(st, s)

    def declare_binding(self, tok, scope, kind="let"):
        if scope.lookup(tok.text) is not None and scope.lookup(tok.text).get("kind") not in ("prelude",):
            existing = scope.lookup(tok.text)
            if existing.get("origin") != "prelude":
                self.rec("E-NAM-003", f"`{tok.text}` shadows a visible binding", "LANG-04-05", tok, {"name": tok.text, "candidates": [existing.get("kind", "?")]})
        scope.declare(tok.text, {"kind": kind})

    def resolve_statement(self, st, scope):
        inner = children(st)[0]
        n = name(inner)
        if n is None:
            # "return" [expr] | "give" expr | "recover" expr | "report" expr ["as" ident] | "discard" expr | "break"/"continue" [label]
            for c in children(st):
                if name(c) == "expr":
                    self.resolve_expr(c, scope)
            ts = direct_tokens(st)
            words = [t.text for t in ts]
            if words and words[0] == "report" and "as" in words:
                self.declare_binding(ts[words.index("as") + 1], scope, "receipt")
            return
        if n == "letStmt":
            ts = direct_tokens(inner)
            pat = next((c for c in children(inner) if name(c) == "pattern"), None)
            for e in [c for c in children(inner) if name(c) in ("expr", "type")]:
                if name(e) == "expr":
                    self.resolve_expr(e, scope)
                else:
                    self.check_types_in(e, scope)
            pl = next((c for c in children(inner) if name(c) == "place"), None)
            if pl is not None:
                self.resolve_place(pl, scope)
            if pat is not None:
                for b in self.pattern_bindings(pat, scope):
                    self.declare_binding(b, scope)
            else:
                nm = next(t for t in ts if t.kind == "LOWER")
                self.declare_binding(nm, scope)
        elif n == "assignStmt":
            self.resolve_place(find(inner, "place"), scope)
            self.resolve_expr(find(inner, "expr"), scope)
        elif n == "ifStmt":
            for c in children(inner):
                if name(c) == "expr":
                    self.resolve_expr(c, scope)
                elif name(c) == "block":
                    self.resolve_block(c, scope)
        elif n in ("whileStmt", "loopStmt", "deferStmt"):
            e = find(inner, "expr") if n == "whileStmt" else None
            if e is not None:
                self.resolve_expr(e, scope)
            self.resolve_block(find(inner, "block"), scope)
        elif n == "forStmt":
            ts = direct_tokens(inner)
            var = ts[[t.text for t in ts].index("for") + 1]
            for c in children(inner):
                if name(c) in ("expr", "rangeExpr"):
                    self.resolve_expr(c, scope)
                elif name(c) == "place":
                    self.resolve_place(c, scope)
            s2 = Scope(scope)
            if var.text != "_":
                self.declare_binding(var, s2, "for")
            self.resolve_block(find(inner, "block"), s2)
        elif n == "scopeStmt":
            s2 = Scope(scope)
            s2.declare(direct_tokens(inner)[1].text, {"kind": "scope"})
            self.resolve_block(find(inner, "block"), s2)
        elif n == "captureStmt":
            self.resolve_expr(find(inner, "expr"), scope)      # the field name is an observation field, not a binding
        elif n == "recordStmt":
            al = find(inner, "argList")
            if al is not None:
                self.resolve_expr(al, scope)
        elif n == "block":
            self.resolve_block(inner, scope)
        else:
            # return/give/recover/report/discard/expect/exprStmt/break/continue
            lc = reading.log_call(inner) if n == "exprStmt" else None
            if lc is not None and scope.lookup("log") is None:
                # a log statement (LR 13): `log` names nothing here, so it is the statement, not a name
                _, level, al = lc
                if level.text not in reading.LOG_LEVELS:
                    self.rec("E-NAM-001", f"`log.{level.text}` is not a log statement: the levels are info, warn and error (13)", "LANG-13-04", level,
                             {"name": f"log.{level.text}", "candidates": [f"log.{x}" for x in reading.LOG_LEVELS]})
                if al is not None:
                    self.resolve_expr(al, scope)
            elif n == "exprStmt":
                self.resolve_expr(inner, scope)
            else:
                for c in children(inner):
                    if name(c) == "expr":
                        self.resolve_expr(c, scope)

    def pattern_bindings(self, pat, scope) -> List:
        out = []
        ts = direct_tokens(pat)
        words = [t.text for t in ts]
        if words and words[0] in ("present", "success", "failure", "("):
            pass
        if "as" in words:
            out.append(ts[words.index("as") + 1])
        if words and words[0] == "otherwise" and len(ts) >= 3:
            out.append(ts[2])
        if len(ts) == 1 and ts[0].kind == "LOWER" and ts[0].text != "_":
            out.append(ts[0])
        pfs = tree.child(pat, "patternFields")
        for pf in [c for c in children(pfs) if name(c) == "patternField"] if pfs is not None else []:   # its own: a nested pattern's are its sub-pattern's
            sub = find(pf, "pattern")
            if sub is not None:
                out += self.pattern_bindings(sub, scope)
        for sub in [c for c in children(pat) if name(c) == "pattern"]:
            out += self.pattern_bindings(sub, scope)
        # variant patterns: `Variant` upper names are variants, checked loosely
        return out

    # ---- expressions ---------------------------------------------------------
    def resolve_place(self, place, scope):
        ts = direct_tokens(place)
        root = ts[0]
        if root.text == "self":
            if scope.lookup("self") is None:
                self.rec("E-NAM-001", "`self` outside a method", "LANG-04-05", root, {"name": "self", "candidates": []})
            return
        info = self.check_root(root, scope)
        kids = children(place)
        if len(kids) > 2 and isinstance(kids[1], lexer.Token) and kids[1].text == "." and name(kids[2]) == "memberName":
            self.check_module_member(info, tokens(kids[2])[0])
        else:      # the alias alone or indexed: a module is not a value
            self.module_value(info, root)

    def module_value(self, info, tok):
        """A module alias standing alone in a place or a constant expression: a module is not a value (4.5; D-36), as
        the typer reports it in an expression."""
        if info is not None and info.get("kind") == "module":
            self.rec("E-NAM-001", f"`{tok.text}` names the module {info['path']}, which is not a value: name a member as `{tok.text}.member`", "LANG-04-03", tok,
                     {"name": tok.text, "candidates": exported(info["module"]) if info.get("module") is not None else []})

    def check_module_member(self, info, tok):
        """`alias.member` in a type, a constant expression or a place: the aliased module must declare the member and
        export it (D-36; an expression's is the typer's, which knows whether a local shadows the alias)."""
        if info is None or info.get("kind") != "module" or info.get("module") is None:
            return
        d = info["module"].decls.get(tok.text)
        if d is None:
            self.rec("E-NAM-001", f"{info['path']} declares no {tok.text}", "LANG-04-03", tok, {"name": tok.text, "candidates": exported(info["module"])})
        elif not d["exported"]:
            self.rec("E-NAM-004", f"{info['path']} does not export `{tok.text}`", "LANG-04-05", tok, {"name": tok.text, "candidates": exported(info["module"])})

    def check_root(self, tok, scope, members: Optional[List] = None):
        info = scope.lookup(tok.text)
        if info is None:
            outer = self.lambda_outer(scope)
            if outer is not None:
                nearest = scope
                while getattr(nearest, "lambda_outer", None) is None:
                    nearest = nearest.parent
                if getattr(nearest, "infer_captures", False):
                    return self.check_root(tok, outer, members)
            if outer is not None and outer.lookup(tok.text) is not None:
                self.rec("E-FUN-011", f"`{tok.text}` is not in the lambda's capture list", "LANG-08-03", tok, {"violation": "uncaptured name", "capture_span": tok.text, "use_span": tok.text})
                return None
            cands = sorted(k for k in self.module_scope.names if k.lower().startswith(tok.text[:1].lower()))[:6]
            self.rec("E-NAM-001", f"`{tok.text}` is not declared", "LANG-04-05", tok, {"name": tok.text, "candidates": cands})
            return None
        return info

    def module_level(self, scope, n: str) -> bool:
        """`n`, looked up from `scope`, is a module-level name: a declaration, an import, a module alias or the prelude."""
        s = scope
        while s is not None:
            if n in s.names:
                return s is self.module_scope
            s = s.parent
        return False

    def lambda_outer(self, scope):
        """The scope enclosing the nearest lambda, if the lookup happened inside one."""
        s = scope
        while s is not None:
            if getattr(s, "lambda_outer", None) is not None:
                return s.lambda_outer
            s = s.parent
        return None

    def check_types_in(self, node, scope, skip_rules: Set[str] = frozenset()):
        """Every typeRef root (and origin) under node must resolve to a type-like name."""
        for tr in self.typerefs(node, skip_rules):
            self.check_typeref(tr, scope)
        self.check_origins(node, scope, skip_rules)

    def check_types_beside(self, node, scope):
        """The types of a declaration with an initialiser (a field, a constant, a parameter): every child but its own
        `constExpr`, which `resolve_expr` reads as a value, so a bare variant there takes the declared type (9.7)."""
        for c in children(node):
            if tree.is_node(c) and name(c) != "constExpr":
                self.check_types_in(c, scope)

    def check_typeref(self, tr, scope):
        """One typeRef's own root: `[alias "."] UpperIdent` (its type arguments are typeRefs of their own)."""
        ts = direct_tokens(tr)
        # [lowerIdent "."] upperIdent
        if len(ts) >= 2 and ts[0].kind == "LOWER" and ts[1].text == ".":
            mod = scope.lookup(ts[0].text)
            if mod is None:
                self.rec("E-NAM-001", f"module alias `{ts[0].text}` is not imported", "LANG-04-03", ts[0], {"name": ts[0].text, "candidates": []})
            else:
                self.check_module_member(mod, ts[2])
            return
        up = next((t for t in ts if t.kind == "UPPER"), None)
        if up is not None and up.text != "Self":
            info = scope.lookup(up.text)
            if info is None:
                self.rec("E-NAM-001", f"type `{up.text}` is not declared", "LANG-04-05", up, {"name": up.text, "candidates": []})

    def check_origins(self, node, scope, skip_rules: Set[str] = frozenset()):
        for org in self.find_all_skipping(node, "origin", skip_rules):
            t = tokens(org)[0]
            if t.kind == "LOWER" and t.text != "static" and scope.lookup(t.text) is None:
                self.rec("E-NAM-001", f"loan origin `{t.text}` is not a parameter or lifetime", "LANG-07-06", t, {"name": t.text, "candidates": []})

    def find_all_skipping(self, node, rule, skip_rules):
        out = []
        def go(n):
            if not tree.is_node(n):
                return
            if n[0] in skip_rules and n is not node:
                return
            if n[0] == rule:
                out.append(n)
            for c in n[1]:
                go(c)
        go(node)
        return out

    def typerefs(self, node, skip_rules):
        return self.find_all_skipping(node, "typeRef", skip_rules)

    def interpolation_holes(self, tok):
        """The expressions of a string literal's `${…}` holes (3.6), each parsed on its own."""
        body, i, out = tok.text[1:-1], 0, []
        while i < len(body):
            if body[i] == "\\":
                i += 2
                continue
            if body.startswith("${", i):
                depth, j = 1, i + 2
                while j < len(body) and depth:
                    depth += {"{": 1, "}": -1}.get(body[j], 0)
                    j += 1
                try:
                    out.append(self.lib.rec.parse(lexer.lex(body[i + 2:j - 1]), "expr"))
                except Exception:  # noqa: BLE001  (a malformed hole is the lexer's and parser's finding)
                    pass
                i = j
            else:
                i += 1
        return out

    def resolve_expr(self, node, scope):
        """Walk an expression; check roots; handle lambdas, matches, if-expressions, blocks."""
        if isinstance(node, lexer.Token) and node.kind == "STR" and "${" in node.text:
            for hole in self.interpolation_holes(node):     # names inside interpolations resolve like any other (3.6)
                before = len(self.records)
                self.resolve_expr(hole, scope)
                for r in self.records[before:]:
                    r["line"], r["column"] = node.line, node.col
            return
        if node is None or isinstance(node, lexer.Token):
            return
        n = name(node)
        if n == "lambda":
            s = Scope(self.module_scope)          # names not in the capture list are inaccessible (8.3)
            s.infer_captures = tree.child(node, "captureList") is None
            s.lambda_outer = scope                # type: ignore[attr-defined]
            cl = tree.child(node, "captureList")       # the lambda's own, not a nested lambda's (D-34)
            if cl is not None:
                for cap in find_all(cl, "capture"):
                    pl = find(cap, "place")
                    t = direct_tokens(pl)[0] if pl is not None else direct_tokens(cap)[0]
                    if self.module_level(scope, t.text):
                        # an entry names a local or parameter; a module-level name is not a capture, and the body
                        # names it directly (8.3, X-230), so it is not bound here (and this is its one record)
                        self.rec("E-FUN-011", f"`{t.text}` is a module-level name, not a local or parameter: it is not a capture (8.3)", "LANG-08-03", t,
                                 {"violation": "capture of a module-level name", "capture_span": t.text, "use_span": t.text})
                        continue
                    if pl is not None:
                        self.resolve_place(pl, scope)
                    else:
                        self.check_root(t, scope)
                    s.declare(t.text, {"kind": "capture"})
            lp = tree.child(node, "lambdaParams")
            if lp is not None:
                self.declare_params(lp, s)
            rt = next((c for c in children(node) if name(c) == "type"), None)
            if rt is not None:
                self.check_types_in(rt, s)
            body = tree.lambda_body(node)
            if body is not None:
                b = children(body)[0] if body is not None and children(body) and name(children(body)[0]) == "block" else None   # the body itself, not a lambda block inside an expression body
                if b is not None:
                    self.resolve_block(b, s)
                else:
                    self.resolve_expr(find(body, "expr"), s)
            return
        if n == "matchExpr":
            for c in children(node):
                if name(c) == "expr":
                    self.resolve_expr(c, scope)
                elif name(c) == "place":
                    self.resolve_place(c, scope)
            for arm in [_a for _a in children(node) if name(_a) == "matchArm"]:
                s = Scope(scope)
                seen = set()
                for pat in [c for c in children(arm) if name(c) == "pattern"]:    # the alternatives of `A | B` (9.8)
                    for b in self.pattern_bindings(pat, s):
                        if b.text not in seen:
                            seen.add(b.text)
                            self.declare_binding(b, s, "pattern")
                for c in children(arm):
                    if name(c) == "expr":
                        self.resolve_expr(c, s)
                    elif name(c) == "block":
                        self.resolve_block(c, s)
            return
        if n == "ifExpr":
            for c in children(node):
                if name(c) == "expr":
                    self.resolve_expr(c, scope)
                elif name(c) == "block":
                    self.resolve_block(c, scope)
                elif name(c) == "ifExpr":
                    self.resolve_expr(c, scope)
            return
        if n == "block":
            self.resolve_block(node, scope)
            return
        if n == "outcomeExpr":
            # coalesceExpr [ "or" "fail" | "or" "recover" outcomeExpr | "catch" lowerIdent block ]
            kids = children(node)
            self.resolve_expr(kids[0], scope)
            words = [k.text for k in kids if isinstance(k, lexer.Token)]
            if "catch" in words:
                s = Scope(scope)
                ident = next(k for k in kids if isinstance(k, lexer.Token) and k.kind == "LOWER")
                self.declare_binding(ident, s, "catch")
                self.resolve_block(find(node, "block"), s)
            else:
                for k in kids[1:]:
                    self.resolve_expr(k, scope)
            return
        if n == "primaryExpr":
            ts = direct_tokens(node)
            if ts and ts[0].kind == "LOWER":
                info = self.check_root(ts[0], scope)
                return
            tr = find(node, "typeRef")
            if tr is not None and children(node)[0] is tr:
                trs = direct_tokens(tr)
                up = next((t for t in trs if t.kind == "UPPER"), None)
                if len(trs) >= 2 and trs[0].kind == "LOWER" and trs[1].text == ".":
                    self.check_typeref(tr, scope)      # the head only: its type arguments follow, its arguments below
                elif up is not None:
                    info = scope.lookup(up.text)
                    member = None
                    kids_ = children(node)
                    for i, k in enumerate(kids_):
                        if isinstance(k, lexer.Token) and k.text == "." and i + 1 < len(kids_):
                            nxt = kids_[i + 1]
                            member = nxt if isinstance(nxt, lexer.Token) else tokens(nxt)[0]
                            break
                    if info is None:
                        # a bare enum variant in a position where the enum is known (LR 9.7)
                        if member is None and up.text in self.enum_variants:
                            pass
                        else:
                            self.rec("E-NAM-001", f"`{up.text}` is not declared", "LANG-04-05", up, {"name": up.text, "candidates": sorted(v for v in self.enum_variants if v.startswith(up.text[:1]))[:6]})
                    elif member is not None and info.get("members") and member.text not in info["members"] and info.get("kind") in ("class", "enum") and info.get("origin") in ("prelude", "module") or (member is not None and info.get("members") and member.text not in info["members"] and str(info.get("origin", "")).startswith("fortis.")):
                        # Eligibility and identity are checked by the typer: `from` may be supplied by an
                        # implement block outside the declaration's own member list (6.13, X-552).
                        if member.text not in ("validate", "fromText", "fromBytes", "from"):
                            self.rec("E-NAM-001", f"`{up.text}` has no member `{member.text}`", "LANG-04-05", member, {"name": f"{up.text}.{member.text}", "candidates": sorted(info["members"])[:8]})
                # type arguments and refinements inside the typeRef, each once
                for sub in find_all(tr, "typeRef"):
                    if sub is not tr:
                        self.check_typeref(sub, scope)
                self.check_origins(tr, scope)
                for c in children(node):
                    if name(c) == "argList":
                        self.resolve_expr(c, scope)
                return
        if n == "typeRef":
            self.check_types_in(node, scope)
            return
        if n == "place":
            self.resolve_place(node, scope)
            return
        if n == "arg":
            # named argument label is not a name reference
            for c in children(node):
                if name(c) in ("expr", "place"):
                    self.resolve_expr(c, scope) if name(c) == "expr" else self.resolve_place(c, scope)
            return
        if n == "recordLit":
            for c in children(node):
                if name(c) in ("expr", "constExpr"):
                    self.resolve_expr(c, scope)
            return
        if n == "postfix":
            # ".member" is not a root; indexing expression and call arguments are
            for c in children(node):
                if name(c) in ("expr", "argList", "methodCall"):
                    self.resolve_expr(c, scope)
            return
        if n == "methodCall":
            al = find(node, "argList")
            if al is not None:
                self.resolve_expr(al, scope)
            return
        if n == "constTerm":
            ts = direct_tokens(node)
            tr = children(node)[0] if children(node) else None
            trs = direct_tokens(tr) if name(tr) == "typeRef" else []
            if trs and trs[0].kind == "UPPER" and "." not in [t.text for t in ts] and trs[0].text in self.enum_variants and scope.lookup(trs[0].text) is None:
                # a bare enum variant, in a field, constant or parameter default whose declared type the typer checks it
                # against (LR 9.7), as in an expression
                for sub in find_all(tr, "typeRef"):
                    if sub is not tr:
                        self.check_typeref(sub, scope)
                self.check_origins(tr, scope)
                for c in children(node)[1:]:
                    self.resolve_expr(c, scope)
                return
            if ts and ts[0].kind == "LOWER" and not (len(ts) > 1 and ts[1].text == "."):
                self.module_value(self.check_root(ts[0], scope), ts[0])
                return
            if ts and ts[0].kind == "LOWER":
                info = self.check_root(ts[0], scope)
                mn = next((c for c in children(node) if name(c) == "memberName"), None)
                if mn is not None:
                    self.check_module_member(info, tokens(mn)[0])
        for c in children(node):
            self.resolve_expr(c, scope)


def resolve_source(library: Library, src: str, file: str, start: str = "module"):
    toks = lexer.lex(src)
    t = library.rec.parse(toks, start)
    r = Resolver(library, file, entry=True)     # a source resolved on its own is a standalone module (D-51.2)
    if start == "module":
        return r.resolve_module(t)
    # a statement fragment: resolve as a block in a module with the prelude only
    r.module_scope = Scope()
    r.enum_variants = set()
    if library.prelude:
        for n, d in library.prelude.decls.items():
            r.module_scope.declare(n, {"kind": d["kind"], "members": d["members"], "origin": "prelude"})
    r.decls = []
    r.resolve_block(t, Scope(r.module_scope))
    return r.records
