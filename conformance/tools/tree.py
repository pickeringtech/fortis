"""Parse-tree helpers over the (ruleName, children) trees of parser.py."""
from __future__ import annotations

from typing import Iterator, List, Optional

from lexer import Token


def is_node(x) -> bool:
    return isinstance(x, tuple)


def name(node) -> Optional[str]:
    return node[0] if is_node(node) else None


def children(node) -> list:
    return node[1] if is_node(node) else []


def walk(node) -> Iterator:
    """Pre-order over nodes (not tokens)."""
    if is_node(node):
        yield node
        for c in node[1]:
            yield from walk(c)


def find_all(node, rule: str) -> List:
    return [n for n in walk(node) if n[0] == rule]


def child(node, rule: str):
    """The first direct child node of `rule`: unlike `find`, never one inside a nested construct (a lambda's own
    parameters, not those of a lambda in its body: D-34)."""
    return next((c for c in children(node) if is_node(c) and c[0] == rule), None)


def find(node, rule: str):
    for n in walk(node):
        if n[0] == rule:
            return n
    return None


def tokens(node) -> List[Token]:
    if isinstance(node, Token):
        return [node]
    out: List[Token] = []
    for c in children(node):
        out.extend(tokens(c))
    return out


def direct_tokens(node) -> List[Token]:
    return [c for c in children(node) if isinstance(c, Token)]


def first_token(node) -> Optional[Token]:
    ts = tokens(node)
    return ts[0] if ts else None


DECL_KINDS = {"constDecl": "const", "typeDecl": "type", "classDecl": "class", "enumDecl": "enum", "errorDecl": "error",
              "interfaceDecl": "interface", "implementDecl": "implement", "mixinDecl": "mixin", "contractDecl": "contract",
              "aspectDecl": "aspect", "observationDecl": "observation", "funcDecl": "func", "testDecl": "test",
              "propertyDecl": "property", "headBlock": "head"}


def declarations(module_tree) -> List[dict]:
    """[{kind, name, exported, node, token}] for every top-level declaration of a module tree."""
    out = []
    for decl in children(module_tree):
        if name(decl) != "declaration":
            continue
        exported = any(isinstance(c, Token) and c.text == "export" for c in children(decl))
        inner = next((c for c in children(decl) if is_node(c) and c[0] in DECL_KINDS), None)
        if inner is None:
            continue
        kind = DECL_KINDS[inner[0]]
        nm = None
        if kind == "func":
            head = find(inner, "funcHead")
            ts = direct_tokens(head)
            idx = next(i for i, t in enumerate(ts) if t.text == "func")
            nm = ts[idx + 1]
        elif kind == "implement":
            nm = None
        elif kind in ("test", "property"):
            nm = direct_tokens(inner)[1]
        elif kind == "head":
            ts = direct_tokens(inner)
            nm = next((t for t in ts if t.kind in ("UPPER", "LOWER")), None)
        else:
            ts = direct_tokens(inner)
            nm = next((t for t in ts if t.kind in ("UPPER", "LOWER")), None)
        uses = [c for c in children(decl) if is_node(c) and c[0] == "aspectUse"]
        out.append({"kind": kind, "name": nm.text if nm else None, "exported": exported, "node": inner, "token": nm, "aspects": uses})
    return out


def imports(module_tree) -> List[dict]:
    """[{path: [segments], alias, members: [names] or None, token}]"""
    out = []
    for imp in children(module_tree):
        if name(imp) != "import":
            continue
        dotted = find(imp, "dottedPath")
        segs = [t.text for t in tokens(dotted) if t.text != "."]
        ts = direct_tokens(imp)
        alias = None
        members = None
        if any(t.text == "as" for t in ts):
            alias = ts[[t.text for t in ts].index("as") + 1].text
        il = find(imp, "identList")
        if il is not None:
            members = [t.text for t in tokens(il) if t.text != ","]
        out.append({"path": segs, "alias": alias, "members": members, "token": tokens(dotted)[0]})
    return out
