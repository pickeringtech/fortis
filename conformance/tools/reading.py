"""Parse-tree shapes of the v0.0.2 reading release (register X-345) that the names layer, the reference checker and
the definitional interpreter must all recognise the same way:

- the entry `main`'s parameters, supplied by type (Language Reference 4.6): `param_findings`;
- the log statements `log.info(m)`, `log.warn(m)`, `log.error(m)` (Language Reference 13): `log_call`;
- the consuming position of a call in a test body, where `or fail` is not implied (Language Reference 14):
  `consuming_parents`."""
from __future__ import annotations

from typing import Callable, List, Optional, Tuple

from tree import children, direct_tokens, find, is_node, name, tokens

# the capability types of Language Reference 11.6, defined by the core and imported from fortis.host
CAPABILITY_TYPES = ("FileSystem", "Network", "Clock", "Random", "Environment", "Process", "Supervisor", "Console", "SecretAccess", "RawSql")
# the field of `Capabilities` that holds each capability (fortis.host)
CAPABILITY_FIELDS = {"FileSystem": "fileSystem", "Network": "network", "Clock": "clock", "Random": "random", "Environment": "environment",
                     "Process": "process", "Supervisor": "supervisor", "Console": "console", "SecretAccess": "secretAccess", "RawSql": "rawSql"}
ARGS = "List[String]"
LOG_LEVELS = ("info", "warn", "error")
ENTRY_FORM = "[export] [async] [try] [recovers] func main(...) Void, each parameter List[String] or a capability type"


def param_type_text(p) -> str:
    pt = find(p, "paramType")
    ty = find(pt, "type") if pt is not None else None
    return "".join(t.text for t in tokens(ty)) if ty is not None else ""


def param_findings(sig, origin_of: Callable[[str], Optional[str]]) -> List[Tuple[str, str, object, dict]]:
    """[(code, message, token, fields)] for the parameters of the entry `main` whose `funcSignature` is `sig` (4.6).

    Each parameter is `name: T` with no mode, default or `...`, where T is the prelude's `List[String]` or a
    capability type of fortis.host, each type at most once: `E-NAM-005` for any other parameter, `E-NAM-006` for
    a type already taken. `origin_of(n)` says where the module's name `n` comes from: "prelude", "module" (a
    declaration of the module), an import path such as "fortis.host", "alias:<path>" for a module alias, or None
    when nothing binds it (the unresolved name is reported by name resolution, not here). A `List[String]` whose
    `List` or `String` is not the prelude's keeps v0.0.1's finding, `E-NAM-001` (D-51.2)."""
    out: List[Tuple[str, str, object, dict]] = []
    pl = find(sig, "paramList") if sig is not None else None
    seen = {}
    candidates = [ARGS] + list(CAPABILITY_TYPES)
    for p in ([c for c in children(pl) if name(c) == "param"] if pl is not None else []):
        ts = [t for t in direct_tokens(p)]
        tok = ts[0] if ts else tokens(p)[0]
        pname = tok.text
        pt = find(p, "paramType")
        mode = next((t.text for t in direct_tokens(pt) if t.text in ("editable", "owned")), None) if pt is not None else None
        if find(p, "selfParam") is not None or find(p, "restParam") is not None or any(t.text == "..." for t in ts) \
                or find(p, "constExpr") is not None or mode is not None or pt is None:
            out.append(("E-NAM-005", f"main's parameter `{pname}` must be written `name: T`, with no mode, default or `...`: the runtime supplies it by its type (4.6)",
                        tok, {"name": pname, "candidates": candidates}))
            continue
        ty = find(pt, "type")
        tts = tokens(ty)
        words = [t.text for t in tts]
        text = "".join(words)
        kind = None
        if words == ["List", "[", "String", "]"]:
            shadowed = next((n for n in ("List", "String") if origin_of(n) not in (None, "prelude")), None)
            if shadowed:
                out.append(("E-NAM-001", f"main's `{shadowed}` is not the prelude {shadowed}, so the signature is not one of the forms of 4.6",
                            tok, {"name": "main", "candidates": [ENTRY_FORM]}))
                continue
            kind = ARGS
        elif len(tts) == 1 and tts[0].kind == "UPPER":
            o = origin_of(tts[0].text)
            if o is None:
                continue                                # an undeclared type: name resolution reports it
            if o == "fortis.host" and tts[0].text in CAPABILITY_TYPES:
                kind = tts[0].text
        elif len(tts) == 3 and tts[0].kind == "LOWER" and words[1] == "." and tts[2].kind == "UPPER":
            o = origin_of(tts[0].text)
            if o is None:
                continue
            if o == "alias:fortis.host" and tts[2].text in CAPABILITY_TYPES:
                kind = tts[2].text
        if kind is None:
            out.append(("E-NAM-005", f"main's parameter `{pname}` has type {text}, which the runtime cannot supply: a parameter of the entry `main` is "
                        f"List[String] or a capability of fortis.host ({', '.join(CAPABILITY_TYPES)}) (4.6)", tok, {"name": pname, "candidates": candidates}))
            continue
        if kind in seen:
            out.append(("E-NAM-006", f"main takes two parameters of type {kind}, `{seen[kind]}` and `{pname}`: the runtime supplies each type once (4.6)",
                        tok, {"name": pname, "candidates": [seen[kind]]}))
            continue
        seen[kind] = pname
    return out


def entry_param_kinds(sig) -> List[Tuple[str, str]]:
    """[(parameter name, ARGS or a capability type)] of an entry `main` the names layer accepted, in order."""
    out = []
    pl = find(sig, "paramList") if sig is not None else None
    for p in ([c for c in children(pl) if name(c) == "param"] if pl is not None else []):
        ts = direct_tokens(p)
        if not ts:
            continue
        ty = find(find(p, "paramType"), "type")
        words = [t.text for t in tokens(ty)] if ty is not None else []
        out.append((ts[0].text, ARGS if words[:1] == ["List"] else words[-1]))
    return out


def _single(node, rule: str):
    """Descend through rules with exactly one child until `rule`; None if any level has more than one child."""
    n = node
    while is_node(n) and name(n) != rule:
        kids = children(n)
        if len(kids) != 1:
            return None
        n = kids[0]
    return n if is_node(n) else None


def log_call(expr) -> Optional[Tuple[object, object, object]]:
    """(the `log` token, the level token, the argList node or None) when `expr` (an expr, exprStmt or statement node)
    has the shape of a log statement, `log.LEVEL(args)`; whether `log` names a binding is the caller's to decide (13).
    LEVEL is returned whatever it is, so the caller can reject another level."""
    if is_node(expr) and name(expr) == "statement":
        kids = children(expr)
        if len(kids) != 1 or name(kids[0]) != "exprStmt":
            return None
        expr = kids[0]
    pf = _single(expr, "postfixExpr")
    if pf is None:
        return None
    kids = children(pf)
    if len(kids) != 2 or name(kids[0]) != "primaryExpr" or name(kids[1]) != "postfix":
        return None
    pts = children(kids[0])
    if len(pts) != 1 or is_node(pts[0]) or pts[0].kind != "LOWER" or pts[0].text != "log":
        return None
    mc = children(kids[1])
    if len(mc) != 1 or name(mc[0]) != "methodCall":
        return None
    mts = direct_tokens(mc[0])
    level = next((t for t in tokens(find(mc[0], "memberName"))), None) if find(mc[0], "memberName") is not None else None
    if level is None or not mts or mts[0].text != ".":
        return None
    return pts[0], level, find(mc[0], "argList")


def call_name_token(expr):
    """The name token of the call an expression makes, when the whole expression is one call (an `@ignorable`
    statement, 5.5): `f(a)`, `x.m(a)`, `Type.m(a)`, `a.b().m()` and the marked `editable x.m(a)`; None otherwise."""
    n = expr
    while is_node(n) and name(n) not in ("markerExpr", "postfixExpr"):
        kids = children(n)
        if len(kids) != 1:
            return None
        n = kids[0]
    if not is_node(n):
        return None
    kids = children(n)
    if name(n) == "markerExpr":
        posts = [k for k in kids[2:] if name(k) == "postfix"]
        if posts:
            return _postfix_call_name(posts, None)
        if len(kids) > 2 and name(kids[2]) == "methodCall":
            return tokens(find(kids[2], "memberName"))[0]
        if len(kids) > 2 and not is_node(kids[2]) and kids[2].text == "(":
            pl = find(n, "place")
            members = [k for k in children(pl) if name(k) == "memberName"]
            return tokens(members[-1])[0] if members else direct_tokens(pl)[0]
        return None
    prim, posts = kids[0], kids[1:]
    if not posts:
        pts = children(prim)
        if pts and is_node(pts[0]) and name(pts[0]) == "typeRef" and any(not is_node(t) and t.text == "(" for t in pts):
            mn = next((c for c in children(prim) if name(c) == "memberName"), None)
            if mn is not None:
                return tokens(mn)[0]
            ups = [t for t in pts if not is_node(t) and t.kind == "UPPER"]
            return ups[-1] if ups else None
        return None
    return _postfix_call_name(posts, prim)


def _postfix_call_name(posts, prim):
    last = children(posts[-1])
    if last and name(last[0]) == "methodCall":
        return tokens(find(last[0], "memberName"))[0]
    if last and not is_node(last[0]) and last[0].text == "(":
        if len(posts) >= 2:
            prev = children(posts[-2])
            if prev and not is_node(prev[0]) and prev[0].text == "." and len(prev) == 2:
                return tokens(prev[1])[0]
            return None
        if prim is not None:
            pts = children(prim)
            if len(pts) == 1 and not is_node(pts[0]) and pts[0].kind == "LOWER":
                return pts[0]
    return None


def primary_call_token(prim):
    """The name token of the call a primary makes on its own, a type-rooted call (`Code.validate(s)`, `Name.parse(s)`,
    `Point(x: 1)`), or None (fortis-names `primary_call_token`)."""
    pts = children(prim) if is_node(prim) else []
    if pts and is_node(pts[0]) and name(pts[0]) == "typeRef" and any(not is_node(t) and t.text == "(" for t in pts):
        mn = next((c for c in pts if name(c) == "memberName"), None)
        if mn is not None:
            return tokens(mn)[0]
        ups = [t for t in pts if not is_node(t) and t.kind == "UPPER"]
        return ups[-1] if ups else None
    return None


def postfix_call_token(posts, i, prim):
    """The name token of the call the postfix `posts[i]` makes (a method call, or `(args)` after `.name` or after a
    plain name), or None when that postfix is not a call: a call that is the receiver of the next postfix (14)."""
    if i < 0 or i >= len(posts):
        return None
    return _postfix_call_name(posts[: i + 1], prim)


def _bare_name(node):
    """The name an operand is when it is just a binding (`o`, `(o)`, `move o`), or None."""
    n = node
    while is_node(n):
        k = children(n)
        if name(n) == "markerExpr":
            if len(k) == 2 and tokens(k[0])[0].text == "move":
                pts = tokens(k[1])
                return pts[0].text if len(pts) == 1 and pts[0].kind == "LOWER" else None
            return None
        if name(n) == "place":
            pts = tokens(n)
            return pts[0].text if len(pts) == 1 and pts[0].kind == "LOWER" else None
        if name(n) == "primaryExpr":
            if len(k) == 1 and not is_node(k[0]) and k[0].kind == "LOWER":
                return k[0].text
            if len(k) == 3 and not is_node(k[0]) and k[0].text == "(":
                n = k[1]
                continue
            return None
        if len(k) != 1:
            return None
        n = k[0]
    return None


def outcome_lets(block):
    """The initialisers of the `let` bindings of a test body that keep their outcome (14, X-345): a binding the body
    later consumes as an outcome (the operand of `or` or `catch`, or a `match` scrutinee), or one annotated
    `Outcome[...]`, binds the call's outcome, as in v0.0.1; every other binding of a fallible call binds its success
    value. Lambdas are their own functions and are not searched."""
    consumed, lets = set(), []

    def walk(n):
        if not is_node(n) or name(n) == "lambda":
            return
        rule, k = name(n), children(n)
        if rule == "outcomeExpr" and len(k) > 1:
            b = _bare_name(k[0])
            if b:
                consumed.add(b)
        elif rule == "matchExpr":
            s = next((c for c in k if name(c) in ("expr", "place")), None)
            b = _bare_name(s) if s is not None else None
            if b:
                consumed.add(b)
        elif rule == "letStmt" and not any(name(c) == "pattern" for c in k):
            nm = next((t for t in direct_tokens(n) if t.kind == "LOWER"), None)
            init = next((c for c in k if name(c) == "expr"), None)
            annot = next((c for c in k if name(c) == "type"), None)
            if nm is not None and init is not None:
                lets.append((nm.text, init, annot))
        for c in k:
            walk(c)

    walk(block)
    return [init for nm, init, annot in lets if nm in consumed or (annot is not None and tokens(annot)[0].text == "Outcome")]
