"""The schema registry (SP-09 design 2): every `(schema, version)` the pipeline has seen declared by a program, with
each field's declared type, `required` clause and, for an enum type, its variants. Filled from programs' own
declarations, never from a plan. A later version never replaces an earlier one, and one `(schema, version)` declared
twice with different fields is refused: a changed schema needs a new version (Platform 6.1)."""
from __future__ import annotations

import json
from typing import Dict, List, Optional

from telemetry import schemas

NUMERIC = {"Int", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float", "Float32", "Float64", "Decimal"}


class RegistryError(Exception):
    pass


class FieldInfo:
    def __init__(self, type_: str, required: str, variants: Optional[List[str]]):
        self.type, self.required, self.variants = type_, required, variants

    def base(self) -> str:
        return self.type.split("<")[0].split("[")[0].rstrip("?")

    def kind(self) -> str:
        """`enum`, `numeric`, `string`, `bool` or `opaque`: what a predicate may compare it with."""
        if self.variants is not None:
            return "enum"
        b = self.base()
        if b in NUMERIC:
            return "numeric"
        if b == "String":
            return "string"
        if b == "Bool":
            return "bool"
        return "opaque"

    def to_json(self) -> dict:
        return {"type": self.type, "required": self.required, "variants": self.variants}


class Registry:
    def __init__(self):
        self.schemas: Dict[str, Dict[int, Dict[str, FieldInfo]]] = {}

    def add_program(self, paths: List[str]) -> List[str]:
        """Register every observation schema the program declares; returns the identities newly added."""
        added = []
        for s in schemas.read(paths).values():
            fields = {n: FieldInfo(f.type, f.required, f.variants) for n, f in s.fields.items()}
            have = self.schemas.get(s.name, {}).get(s.version)
            if have is not None:
                if {n: f.to_json() for n, f in have.items()} != {n: f.to_json() for n, f in fields.items()}:
                    raise RegistryError(f"{s.name}.v{s.version} is already registered with different fields; a changed schema needs a new version")
                continue
            self.schemas.setdefault(s.name, {})[s.version] = fields
            added.append(f"{s.name}.v{s.version}")
        return added

    def versions(self, name: str) -> List[int]:
        return sorted(self.schemas.get(name, {}))

    def latest(self, name: str) -> Optional[int]:
        vs = self.versions(name)
        return vs[-1] if vs else None

    def fields(self, name: str, version: int) -> Optional[Dict[str, FieldInfo]]:
        return self.schemas.get(name, {}).get(version)

    def to_json(self) -> dict:
        return {"schemas": {n: {str(v): {f: i.to_json() for f, i in fs.items()} for v, fs in vs.items()} for n, vs in sorted(self.schemas.items())}}

    @staticmethod
    def from_json(d: dict) -> "Registry":
        r = Registry()
        for n, vs in d.get("schemas", {}).items():
            for v, fs in vs.items():
                r.schemas.setdefault(n, {})[int(v)] = {f: FieldInfo(i["type"], i["required"], i["variants"]) for f, i in fs.items()}
        return r

    def save(self, path: str):
        with open(path, "w", encoding="utf-8") as f:
            json.dump(self.to_json(), f, sort_keys=True, indent=1)

    @staticmethod
    def load(path: str) -> "Registry":
        with open(path, encoding="utf-8") as f:
            return Registry.from_json(json.load(f))
