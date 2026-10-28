"""Package set loading and import resolution (Language Reference 4.1–4.3, 4.6; DEL-05).

`resolve.py --root DIR [--search DIR]` loads the root package, its declared dependencies (pinned
by version, found as `<name>@<version>` or `<name>` under the search directory), parses every
module, and checks: the longest-declared-prefix import split and the prefix-ambiguity rule
(E-NAM-010); module existence, `exports:` across packages, root modules; braced members exist and
are exported; duplicate and colliding imports; the entry module's `main` signature (4.6).
Diagnostics are records of class "Names and modules"."""
from __future__ import annotations

import os
import re
import sys
from typing import Dict, List, Optional, Tuple

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diagnostics  # noqa: E402
import elaboration  # noqa: E402
import parser       # noqa: E402
import project      # noqa: E402
import tree         # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CORE_MODULES = ["prelude", "text", "collections", "numeric", "time", "money", "host"]
# 4.6's entry forms: `async` and `try` as needed, and `recovers` when the program discharges a failure it did not report
# (10.4, 11.3; D-111, the user's decision of 26 Oct 2026), in the grammar's modifier order
MAIN_FORMS = {"", "try", "async", "async try", "recovers", "try recovers", "async recovers", "async try recovers"}
# `run` stops at the first syntax error, before import checks: an unparsed module has no declarations, so judging
# its exports would report members it might have declared. A one-file check that then sees no record for its file
# must not succeed (D-51.8). Both CLIs and packages_check emit this record on project.yaml instead.
NOT_NAME_CHECKED = "this module was not name-checked: another module of the package does not parse"
# 4.6's signature names these prelude types. A module-level declaration or import of the same spelling is not that type.
ENTRY_TYPES = ("List", "String", "Void")


class PackageSet:
    def __init__(self, root_dir: str, search: str, rec):
        self.rec = rec
        self.records: List[dict] = []
        self.root = project.Package(root_dir)
        self.packages: Dict[str, project.Package] = {self.root.name: self.root}
        self.versions: Dict[str, str] = {self.root.name: self.root.version}
        self.identities = {self.root.identity}
        todo = [self.root]
        while todo:
            pkg = todo.pop()
            for dep, ver in pkg.dependencies.items():
                if dep == "fortis":
                    continue
                d = project.find_package_dir(search, dep, ver)
                if d is None:
                    self.record("E-NAM-001", f"dependency {dep}@{ver} of {pkg.identity} not found", "LANG-04-03", os.path.join(pkg.dir, "project.yaml"), 1, 1, {"name": dep, "candidates": []})
                    continue
                p = project.Package(d)
                key = f"{p.name}@{p.version}" if p.name in self.packages and self.packages[p.name].version != p.version else p.name
                if key not in self.packages:
                    self.packages[key] = p
                    self.identities.add(p.identity)
                    todo.append(p)
        for p in self.packages.values():
            p.parse_all(rec)

    def record(self, code, msg, rule, file, line, col, fields):
        self.records.append(diagnostics.make(code, msg, rule, os.path.relpath(file, ROOT), line, col, fields))

    # ---- the import split ------------------------------------------------
    def visible_packages(self, pkg: project.Package) -> Dict[str, project.Package]:
        """Packages the import split may name from `pkg`: itself, its declared dependencies (pinned
        version), and the core package `fortis` (modelled as a package with the seven modules)."""
        out = {pkg.name: pkg}
        for dep, ver in pkg.dependencies.items():
            if dep == "fortis":
                continue
            for p in self.packages.values():
                if p.name == dep and p.version == ver:
                    out[dep] = p
        return out

    def check_prefix_ambiguity(self, pkg: project.Package):
        names = sorted(set(list(self.visible_packages(pkg)) + ["fortis"]))
        for a in names:
            for b in names:
                if b != a and b.startswith(a + "."):
                    rest = b[len(a) + 1:]
                    mods = CORE_MODULES if a == "fortis" else list(self.visible_packages(pkg)[a].modules)
                    if rest in mods or any(m.startswith(rest + ".") for m in mods):
                        self.record("E-NAM-010", f"package {b} is also module path {rest} of package {a}: the import split is ambiguous", "LANG-04-03",
                                    os.path.join(pkg.dir, "project.yaml"), 1, 1, {"name": b, "candidates": [a, b]})

    def split(self, pkg: project.Package, segs: List[str]) -> Tuple[Optional[str], List[str]]:
        visible = list(self.visible_packages(pkg)) + ["fortis"]
        best = None
        for i in range(len(segs), 0, -1):
            cand = ".".join(segs[:i])
            if cand in visible:
                best = (cand, segs[i:])
                break
        return best if best else (None, segs)

    def check_module(self, mod: project.Module):
        pkg = mod.package
        seen_names = {d["name"] for d in mod.decls if d["name"]}
        bound: Dict[str, str] = {}
        for imp in mod.imports:
            tok = imp["token"]
            pkg_name, rest = self.split(pkg, imp["path"])
            if pkg_name is None:
                self.record("E-NAM-001", f"import {'.'.join(imp['path'])} names no declared package", "LANG-04-03", mod.file, tok.line, tok.col,
                            {"name": ".".join(imp["path"]), "candidates": sorted(self.visible_packages(pkg))})
                continue
            if pkg_name == "fortis":
                if len(rest) != 1 or rest[0] not in CORE_MODULES:
                    self.record("E-NAM-001", f"fortis has no module {'.'.join(rest)}", "LANG-04-03", mod.file, tok.line, tok.col, {"name": ".".join(rest), "candidates": CORE_MODULES})
                    continue
                target = None
                modname = rest[0]
            else:
                target_pkg = self.visible_packages(pkg)[pkg_name]
                modname = ".".join(rest) if rest else (target_pkg.root or "")
                if not modname:
                    self.record("E-NAM-001", f"package {pkg_name} has no root module; name a module", "LANG-04-03", mod.file, tok.line, tok.col, {"name": pkg_name, "candidates": sorted(target_pkg.modules)})
                    continue
                target = target_pkg.modules.get(modname)
                if target is None:
                    self.record("E-NAM-001", f"package {pkg_name} has no module {modname}", "LANG-04-03", mod.file, tok.line, tok.col, {"name": modname, "candidates": sorted(target_pkg.modules)})
                    continue
                if target_pkg is not pkg and modname not in target_pkg.exports:
                    self.record("E-NAM-001", f"module {modname} of {pkg_name} is not in its exports", "LANG-04-03", mod.file, tok.line, tok.col, {"name": modname, "candidates": sorted(target_pkg.exports)})
                    continue
            # bindings
            if imp["members"] is None:
                alias = imp["alias"] or imp["path"][-1]
                names = [alias]
            else:
                names = imp["members"]
                if target is not None:
                    exported = {d["name"] for d in target.decls if d["exported"] and d["name"]}
                    for m in imp["members"]:
                        if m not in exported:
                            self.record("E-NAM-001", f"{pkg_name}.{modname} exports no {m}", "LANG-04-04", mod.file, tok.line, tok.col, {"name": m, "candidates": sorted(exported)})
            for n in names:
                if n in bound or n in seen_names:
                    # the candidate is the first binding: a later import of the same name does not replace it (LR 4.3)
                    self.record("E-NAM-001", f"import binds {n} twice or collides with a declaration", "LANG-04-03", mod.file, tok.line, tok.col, {"name": n, "candidates": [bound.get(n, "declaration")]})
                if n not in bound:
                    bound[n] = ".".join(imp["path"])

    def check_entry(self):
        pkg = self.root
        if not pkg.entry:
            return
        mod = pkg.modules.get(pkg.entry)
        if mod is None:
            self.record("E-NAM-001", f"entry module {pkg.entry} does not exist", "LANG-04-06", os.path.join(pkg.dir, "project.yaml"), 1, 1, {"name": pkg.entry, "candidates": sorted(pkg.modules)})
            return
        mains = [d for d in mod.decls if d["kind"] == "func" and d["name"] == "main"]
        if not mains or not mains[0]["exported"]:
            self.record("E-NAM-001", f"entry module {pkg.entry} exports no main", "LANG-04-06", mod.file, 1, 1, {"name": "main", "candidates": [d["name"] for d in mod.decls if d["name"]]})
            return
        head = tree.find(mains[0]["node"], "funcHead")
        mods = tree.find(head, "modifiers")
        words = " ".join(t.text for t in tree.tokens(mods)) if mods else ""
        sig = tree.find(head, "funcSignature")
        text = " ".join(t.text for t in tree.tokens(sig))
        tok = mains[0]["token"]
        form = "export [async] [try] [recovers] func main(args: List[String]) Void"
        if words not in MAIN_FORMS or text not in ("( args : List [ String ] ) Void",):
            self.record("E-NAM-001", f"main has signature `{words} func main{text}`, not one of the forms of 4.6", "LANG-04-06", mod.file, tok.line, tok.col,
                        {"name": "main", "candidates": [form]})
            return
        # the token text can match while `List`, `String` or `Void` names a declaration or import of this module.
        # 4.6's forms use the prelude types. The package resolver has no scope origins, so a module-level binding of
        # the name is the shadow (D-51.2)
        shadowed = entry_type_shadow(mod)
        if shadowed:
            self.record("E-NAM-001", f"main's `{shadowed}` is not the prelude {shadowed}, so the signature is not one of the forms of 4.6", "LANG-04-06", mod.file, tok.line, tok.col,
                        {"name": "main", "candidates": [form]})

    def run(self):
        for p in self.packages.values():
            for m in p.modules.values():
                if m.error:
                    at = re.search(r"at (\d+):(\d+)", m.error)      # the parser names the position of the offending token
                    self.record("E-SYN-000", m.error, "LANG-03-02", m.file, int(at.group(1)) if at else 1, int(at.group(2)) if at else 1, {"expected_tokens": [], "found_token": ""})
        self.check_prefix_ambiguity(self.root)
        if self.records:
            # an ambiguous dependency set is rejected as a whole (4.3); a module that does not parse is reported by its
            # syntax error, unless its elaboration failed, as fortis-di reports it (D-90)
            return elaboration.report(self.records, [m for p in self.packages.values() for m in p.modules.values()])
        for p in self.packages.values():
            for m in p.modules.values():
                if m.tree is not None:
                    self.check_module(m)
        self.check_entry()
        if self.records:   # with the Dependency injection kit's findings, as fortis-di reports them (D-90)
            return elaboration.report(self.records, [m for p in self.packages.values() for m in p.modules.values()])
        return self.records


def entry_type_shadow(mod) -> Optional[str]:
    """The first of 4.6's `List`, `String` and `Void` bound by a declaration or an import of `mod`."""
    bound = {d["name"] for d in mod.decls if d.get("name")}
    for imp in mod.imports:
        if imp["members"] is None:
            bound.add(imp["alias"] or imp["path"][-1])
        else:
            bound.update(imp["members"])
    for n in ENTRY_TYPES:
        if n in bound:
            return n
    return None


def main(argv):
    root = None
    search = os.path.join(ROOT, "conformance", "packages")
    fmt = None
    for a in argv:
        if a.startswith("--root="):
            root = a.split("=", 1)[1]
        elif a.startswith("--search="):
            search = a.split("=", 1)[1]
        elif a.startswith("--format="):
            fmt = a.split("=", 1)[1]
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    ps = PackageSet(root, search, rec)
    records = ps.run()
    diagnostics.emit(records, sys.stdout, fmt)
    if not records:
        print(f"resolved: {', '.join(sorted(ps.identities))}; modules: {sum(len(p.modules) for p in ps.packages.values())}", file=sys.stderr)
    return 1 if records else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
