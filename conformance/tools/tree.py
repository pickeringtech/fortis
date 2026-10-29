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


def lambda_body(node):
    """A lambda's body: its `funcBody`, or, for an expression body written without a return type (LR 8.2, X-345), the
    same shape built from its `=>` and expression, so every reader of a body reads both forms alike."""
    fb = child(node, "funcBody")
    if fb is not None:
        return fb
    kids = children(node)
    arrow = next((i for i, k in enumerate(kids) if isinstance(k, Token) and k.text == "=>"), None)
    return ("funcBody", kids[arrow:]) if arrow is not None else None


def pun_name(arg) -> Optional[Token]:
    """The name of an argument written as a bare local name (`wanted`), which a call reads as `wanted: wanted` where a
    positional argument cannot stand: after a named argument, and in a class's generated constructor (LR 5.8, X-345)."""
    ks = children(arg)
    if len(ks) != 1 or name(ks[0]) not in ("expr", "constExpr"):
        return None
    ts = tokens(ks[0])
    return ts[0] if len(ts) == 1 and ts[0].kind == "LOWER" else None


ARITH_RULES = ("additiveExpr", "multiplicativeExpr", "shiftExpr")
WRAPPERS = ("expr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr", "equalityExpr", "relationalExpr",
            "shiftExpr", "additiveExpr", "multiplicativeExpr", "unaryExpr", "postfixExpr")


def region_operators(node) -> set:
    """The operators of a checked region (6.4, X-345): the integer arithmetic an outcome operator's left operand is made
    of, through `+ - * / % << >>`, unary `-` and parentheses, and nothing else (not into a call's arguments, an index, a
    lambda or another operator). Empty when the left operand is not arithmetic. Positions are (line, column)."""
    out: set = set()

    def walk(n, top: bool) -> bool:
        if isinstance(n, Token):
            return False
        nm = name(n)
        kids = children(n)
        if nm in ARITH_RULES and len(kids) > 1:
            for i, k in enumerate(kids):
                if isinstance(k, Token):
                    out.add((k.line, k.col))
                else:
                    walk(k, False)
            return True
        if nm == "unaryExpr" and len(kids) == 2 and isinstance(kids[0], Token) and kids[0].text == "-":
            out.add((kids[0].line, kids[0].col))
            walk(kids[1], False)
            return True
        if nm == "primaryExpr" and len(kids) == 3 and isinstance(kids[0], Token) and kids[0].text == "(":
            return walk(kids[1], False)
        if nm == "postfixExpr" and len(kids) == 1:
            return walk(kids[0], top)
        if nm in WRAPPERS and len(kids) == 1:
            return walk(kids[0], top)
        return False

    walk(node, True)
    return out


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
