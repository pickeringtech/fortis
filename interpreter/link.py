"""Linking a package program for the kernel (Language Reference 4.1-4.5, LANG-04-04; X-136).

The kernel runs one table of declarations. A name declared by two modules (two packages' `Item`, two versions of
one package, two modules' private `parse`) is entered under a program-unique key, `name@module`, where a module of a
package linked at two versions carries its version (`ver.greet.greet@2.0.0`). Every other name keeps its own
spelling, so a program without collisions links exactly as before.

Each module gets a scope: the key of every name it may write, its own declarations and its imports' members, and
`alias.name` for each name of a module it imports whole. Type names are upper case and name nothing else, so the
linker renames them in the module's tree; functions and constants share lower case with locals and keep their
spelling in the tree, and the kernel resolves them through the scope of the module whose code is running.
"""
from __future__ import annotations

import os
import sys
from typing import Dict, List, Optional, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "conformance", "tools"))

import elaboration  # noqa: E402
import lexer  # noqa: E402
import tree  # noqa: E402
from tree import tokens  # noqa: E402

# declarations a module's code may name; tests, properties and implement blocks have no name to collide
NAMED = ("class", "enum", "error", "type", "observation", "interface", "mixin", "func", "const")


class LinkedModule:
    def __init__(self, key: str, file: str, tree_, package):
        self.key, self.file, self.tree, self.package = key, file, tree_, package
        self.scope: Dict[str, str] = {}       # name or alias.name -> program key
        self.aliases: Dict[str, str] = {}     # import alias -> module key


class Link:
    def __init__(self):
        self.modules: Dict[str, LinkedModule] = {}
        self.colliding: set = set()

    def key(self, module_key: str, name: str) -> str:
        return f"{name}@{module_key}" if name in self.colliding else name


def link(ps, rec, entry_module: str) -> Tuple[Link, LinkedModule]:
    """Every module of the package set, freshly parsed, with its key, scope and renamed type names."""
    counts: Dict[str, int] = {}
    for p in ps.packages.values():
        counts[p.name] = counts.get(p.name, 0) + 1
    out = Link()
    by_package: Dict[int, Dict[str, LinkedModule]] = {}
    entry: Optional[LinkedModule] = None
    declared: Dict[str, List[str]] = {}
    for p in ps.packages.values():
        for mpath, m in p.modules.items():
            key = f"{p.name}.{mpath}" + (f"@{p.version}" if counts[p.name] > 1 else "")
            src = open(m.file, encoding="utf-8").read()
            if elaboration.uses_di(src):     # run as checked: elaborated (D-90; fortis-di `source`)
                src = elaboration.elaborate(os.path.relpath(m.file, elaboration.ROOT))[0]
            t = rec.parse(lexer.lex(src), "module")
            lm = LinkedModule(key, m.file, t, p)
            out.modules[key] = lm
            by_package.setdefault(id(p), {})[mpath] = lm
            if p is ps.root and mpath == entry_module:
                entry = lm
            for d in tree.declarations(t):
                if d["kind"] in NAMED and d["name"]:
                    declared.setdefault(d["name"], []).append(key)
    out.colliding = {n for n, ks in declared.items() if len(ks) > 1}
    names_of = {k: [d["name"] for d in tree.declarations(lm.tree) if d["kind"] in NAMED and d["name"]] for k, lm in out.modules.items()}
    for lm in out.modules.values():
        for n in names_of[lm.key]:
            lm.scope[n] = out.key(lm.key, n)
        visible = ps.visible_packages(lm.package)
        for imp in tree.imports(lm.tree):
            pkg_name, rest = ps.split(lm.package, imp["path"])
            if pkg_name is None or pkg_name == "fortis" or pkg_name not in visible:
                continue
            target_pkg = visible[pkg_name]
            mpath = ".".join(rest) if rest else target_pkg.root
            target = by_package.get(id(target_pkg), {}).get(mpath)
            if target is None:
                continue
            if imp["members"]:
                for n in imp["members"]:
                    lm.scope.setdefault(n, out.key(target.key, n))
            else:
                alias = imp["alias"] or imp["path"][-1]
                lm.aliases[alias] = target.key
                for n in names_of[target.key]:
                    lm.scope[f"{alias}.{n}"] = out.key(target.key, n)
        rename_types(lm, rec)
    assert entry is not None, f"no module {entry_module} in {ps.root.name}"
    return out, entry


def renamed(lm: LinkedModule, ts, i) -> Optional[str]:
    """The program key of the upper-case token `ts[i]`, when it differs from its spelling: `alias.Type` by the
    alias's module; any other member after `.` (an enum's variant, a type's constant) is not a module-level name."""
    t = ts[i]
    if i >= 1 and ts[i - 1].text == ".":
        alias = ts[i - 2].text if i >= 2 and ts[i - 2].kind == "LOWER" else None
        k = lm.scope.get(f"{alias}.{t.text}") if alias in lm.aliases else None
    else:
        k = lm.scope.get(t.text)
    return k if k is not None and k != t.text else None


def rename_types(lm: LinkedModule, rec):
    """Upper-case names that collide take their program key, in the tree and in each string's `${...}`: a segment is
    parsed here, renamed like the tree, and kept on its token for the kernel (a key is not lexically a name, so the
    segment's text cannot carry it)."""
    ts = tokens(lm.tree)
    for i, t in enumerate(ts):
        if t.kind == "UPPER":
            k = renamed(lm, ts, i)
            if k is not None:
                t.text = k
        elif t.kind == "STR" and "${" in t.text:
            trees = {}
            for src in segments(t.text[1:-1]):
                node = rec.parse(lexer.lex(src), "expr")
                sts = tokens(node)
                for n in range(len(sts)):
                    if sts[n].kind == "UPPER":
                        k = renamed(lm, sts, n)
                        if k is not None:
                            sts[n].text = k
                trees[src] = node
            t.interp_trees = trees


def segments(body: str):
    """The source of each `${...}` of a string's body, scanned as the kernel's interpolation scans it."""
    i = 0
    while i < len(body):
        if body[i] == "\\":
            i += 2
        elif body.startswith("${", i):
            depth, j = 1, i + 2
            while depth and j < len(body):
                depth += {"{": 1, "}": -1}.get(body[j], 0)
                j += 1
            yield body[i + 2:j - 1]
            i = j
        else:
            i += 1
