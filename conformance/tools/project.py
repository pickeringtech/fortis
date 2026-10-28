"""project.yaml reader and package loader (Language Reference 4.1–4.3, 4.6).

The manifest is a small YAML subset: `key: value`, `key: [a, b]`, and one nested map
(`dependencies:` with `  name: "version"` lines). Values may be quoted."""
from __future__ import annotations

import os
import re
from typing import Dict, List, Optional

import elaboration
import lexer
import tree


MANIFEST_KEYS = {"schema", "name", "version", "exports", "dependencies", "entry", "root", "kits", "targets"}


class ManifestError(Exception):
    pass


def _unquote(v: str) -> str:
    v = v.strip()
    if len(v) >= 2 and v[0] == v[-1] and v[0] in "\"'":
        return v[1:-1]
    return v


def read_manifest(path: str) -> dict:
    out: Dict[str, object] = {}
    current: Optional[str] = None
    for raw in open(path, encoding="utf-8"):
        line = raw.rstrip("\n")
        if not line.strip() or line.strip().startswith("#"):
            continue
        if line.startswith("  ") and current is not None:
            k, _, v = line.strip().partition(":")
            out[current][k.strip()] = _unquote(v)        # type: ignore[index]
            continue
        current = None
        k, _, v = line.partition(":")
        k = k.strip()
        v = v.strip()
        if v == "":
            out[k] = {}
            current = k
        elif v.startswith("["):
            out[k] = [_unquote(x) for x in v.strip("[]").split(",") if x.strip()]
        else:
            out[k] = _unquote(v)
    for req in ("name", "version"):
        if req not in out:
            raise ManifestError(f"{path}: manifest lacks {req}")
    for k in out:
        if k not in MANIFEST_KEYS:
            raise ManifestError(f"{path}: `{k}` is not a key of fortis.project/v1; configuration cannot add or remove checks, contracts or predicates (PROG-FND-04)")
    out.setdefault("exports", [])
    out.setdefault("dependencies", {})
    return out


class Module:
    def __init__(self, package: "Package", path: str, file: str):
        self.package = package
        self.path = path            # dotted module path within the package
        self.file = file
        self.tree = None
        self.decls: List[dict] = []
        self.imports: List[dict] = []
        self.error: Optional[str] = None
        self.di_records: List[dict] = []        # the Dependency injection kit's findings (elaboration.report)
        self.origins: Optional[List[int]] = None
        self.source_lines = 0

    def parse(self, rec):
        src = open(self.file, encoding="utf-8").read()
        self.source_lines = len(src.split("\n"))
        if elaboration.uses_di(src):
            # read as the production frontend reads it: elaborated before the check (fortis-di `source`; D-90)
            src, self.di_records, self.origins = elaboration.elaborate(os.path.relpath(self.file, elaboration.ROOT))
        try:
            self.tree = rec.parse(lexer.lex(src), "module")
        except Exception as e:      # LexError / ParseFailure
            self.error = str(e)
            return
        self.decls = tree.declarations(self.tree)
        self.imports = tree.imports(self.tree)


class Package:
    def __init__(self, directory: str):
        self.dir = directory
        self.manifest = read_manifest(os.path.join(directory, "project.yaml"))
        self.name: str = self.manifest["name"]            # type: ignore[assignment]
        self.version: str = self.manifest["version"]      # type: ignore[assignment]
        self.exports: List[str] = list(self.manifest["exports"])   # type: ignore[arg-type]
        self.entry: Optional[str] = self.manifest.get("entry")     # type: ignore[assignment]
        self.root: Optional[str] = self.manifest.get("root")       # type: ignore[assignment]
        self.dependencies: Dict[str, str] = dict(self.manifest["dependencies"])  # type: ignore[arg-type]
        self.modules: Dict[str, Module] = {}
        src = os.path.join(directory, "src")
        for base, _, files in os.walk(src):
            for f in sorted(files):
                if f.endswith(".ft"):
                    rel = os.path.relpath(os.path.join(base, f), src)[:-3].replace(os.sep, ".")
                    self.modules[rel] = Module(self, rel, os.path.join(base, f))

    @property
    def identity(self) -> str:
        return f"{self.name}@{self.version}"

    def parse_all(self, rec):
        for m in self.modules.values():
            m.parse(rec)


def find_package_dir(search: str, name: str, version: Optional[str]) -> Optional[str]:
    if version is not None:
        cand = os.path.join(search, f"{name}@{version}")
        if os.path.isdir(cand):
            return cand
    cand = os.path.join(search, name)
    if os.path.isdir(cand):
        return cand
    return None
