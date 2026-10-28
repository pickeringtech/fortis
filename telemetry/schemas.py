"""The observation schemas of a Fortis program, read from its own declarations with the reference parser: each
field's type and `required` clause, the schema's `version` (1 when absent) and, for a field of an enum type, the
enum's variants."""
from __future__ import annotations

import os
import sys
from typing import Dict, List, Optional

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))

import lexer  # noqa: E402
import parser  # noqa: E402
import tree  # noqa: E402


class Field:
    def __init__(self, name: str, type_: str, required: str):
        self.name, self.type, self.required = name, type_, required
        self.variants: Optional[List[str]] = None

    def base(self) -> str:
        return self.type.split("<")[0].split("[")[0].rstrip("?")


class Schema:
    def __init__(self, name: str):
        self.name, self.version, self.fields = name, 1, {}

    def identity(self) -> str:
        return f"{self.name}.v{self.version}"


def read(paths: List[str]) -> Dict[str, Schema]:
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    schemas: Dict[str, Schema] = {}
    enums: Dict[str, List[str]] = {}
    for p in paths:
        t = rec.parse(lexer.lex(open(p, encoding="utf-8").read()), "module")
        for d in tree.declarations(t):
            if d["kind"] == "enum":
                enums[d["name"]] = [tree.tokens(c)[0].text for c in tree.children(d["node"]) if tree.is_node(c) and tree.name(c) == "enumMember" and tree.tokens(c)[0].kind == "UPPER"]
            if d["kind"] != "observation":
                continue
            s = Schema(d["name"])
            for item in [c for c in tree.children(d["node"]) if tree.is_node(c) and tree.name(c) == "observationItem"]:
                ws = [x.text for x in tree.tokens(item)]
                if ws[0] == "version":
                    s.version = int(ws[2])
                elif ws[0] == "encoding":
                    continue
                elif len(ws) > 2 and ws[1] == ":":
                    req = ws[ws.index("required") + 2] if "required" in ws else "none"
                    type_ = "".join(ws[2:ws.index("required")] if "required" in ws else ws[2:])
                    s.fields[ws[0]] = Field(ws[0], type_, req)
            schemas[s.name] = s
    for s in schemas.values():
        for f in s.fields.values():
            if f.base() in enums:
                f.variants = enums[f.base()]
    return schemas
