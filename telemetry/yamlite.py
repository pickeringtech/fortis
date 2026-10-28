"""A YAML subset for telemetry plans, with the line of every value (diagnostics are located at the plan site).

Block mappings (`key: value`, `key:` then an indented block), block sequences (`- item`), flow sequences
(`[a, b]`), scalars (plain, single- or double-quoted; integers, `true`/`false`, `null`) and `#` comments. Anything
else (anchors, tags, multi-line scalars, flow mappings) is refused with its line, not guessed at.
"""
from __future__ import annotations

import re
from decimal import Decimal
from typing import Any, List, Optional, Tuple


class PlanSyntax(Exception):
    def __init__(self, message: str, line: int):
        super().__init__(f"line {line}: {message}")
        self.line = line


class Node:
    """A value with the 1-based line it was written on; a mapping also has the line of each of its keys."""
    __slots__ = ("value", "line", "keys")

    def __init__(self, value: Any, line: int, keys: Optional[dict] = None):
        self.value, self.line, self.keys = value, line, keys or {}

    def key_line(self, k: str) -> int:
        return self.keys.get(k, self.line)

    def plain(self):
        """The value without line information."""
        if isinstance(self.value, dict):
            return {k: v.plain() for k, v in self.value.items()}
        if isinstance(self.value, list):
            return [v.plain() for v in self.value]
        return self.value


def _quoted_end(s: str, i: int) -> int:
    """The index just past the quoted scalar that starts at s[i]: a double-quoted one ends at an unescaped `"`, a
    single-quoted one at a `'` not doubled; -1 when it does not end."""
    q, j = s[i], i + 1
    while j < len(s):
        if q == '"' and s[j] == "\\":
            j += 2
            continue
        if s[j] == q:
            if q == "'" and j + 1 < len(s) and s[j + 1] == "'":
                j += 2
                continue
            return j + 1
        j += 1
    return -1


def _strip_comment(s: str) -> str:
    i = 0
    while i < len(s):
        if s[i] in "\"'" and (i == 0 or s[i - 1] in " :[,-"):
            e = _quoted_end(s, i)
            if e < 0:
                return s.rstrip()
            i = e
            continue
        if s[i] == "#" and (i == 0 or s[i - 1] == " "):
            return s[:i].rstrip()
        i += 1
    return s.rstrip()


ESCAPES = {"\\": "\\", '"': '"', "/": "/", "n": "\n", "t": "\t", "r": "\r"}


def _unquote(t: str, line: int) -> str:
    if t[0] == "'":
        return t[1:-1].replace("''", "'")
    out, i, body = [], 0, t[1:-1]
    while i < len(body):
        c = body[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        e = body[i + 1: i + 2]
        if e in ESCAPES:
            out.append(ESCAPES[e])
            i += 2
        elif e == "u" and len(body) >= i + 6 and all(x in "0123456789abcdefABCDEF" for x in body[i + 2:i + 6]):
            cp = int(body[i + 2:i + 6], 16)
            if 0xD800 <= cp <= 0xDFFF:
                raise PlanSyntax(f"`\\u{body[i + 2:i + 6]}` is a surrogate, not a Unicode scalar value", line)
            out.append(chr(cp))
            i += 6
        else:
            raise PlanSyntax(f"the escape `\\{e}` is outside the plan's YAML subset", line)
    return "".join(out)


def _scalar(text: str, line: int) -> Node:
    t = text.strip()
    if t.startswith("[") :
        if not t.endswith("]"):
            raise PlanSyntax("an unclosed flow sequence", line)
        inner = t[1:-1].strip()
        return Node([_scalar(x, line) for x in _split_flow(inner, line)] if inner else [], line)
    if t[:1] in "{&*!|>":
        raise PlanSyntax(f"`{t[:1]}` is outside the plan's YAML subset", line)
    if t[:1] in "\"'":
        if _quoted_end(t, 0) != len(t):
            raise PlanSyntax("a quoted scalar that does not end where the value does", line)
        return Node(_unquote(t, line), line)
    if t in ("true", "false"):
        return Node(t == "true", line)
    if t in ("null", "~", ""):
        return Node(None, line)
    try:
        return Node(int(t), line)
    except ValueError:
        pass
    if re.fullmatch(r"[-+]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?", t):
        return Node(Decimal(t), line)               # exact: a float would change 9007199254740993.0
    return Node(t, line)


def _split_flow(inner: str, line: int) -> List[str]:
    parts, depth, cur, i = [], 0, [], 0
    while i < len(inner):
        c = inner[i]
        if c in "\"'":
            e = _quoted_end(inner, i)
            if e < 0:
                raise PlanSyntax("an unclosed quoted scalar", line)
            cur.append(inner[i:e])
            i = e
            continue
        i += 1
        if c == "[":
            depth += 1
        elif c == "]":
            depth -= 1
        elif c == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
            continue
        cur.append(c)
    parts.append("".join(cur))
    return [p for p in (x.strip() for x in parts) if p]


def loads(text: str) -> Node:
    lines: List[Tuple[int, int, str]] = []          # (line number, indent, content)
    for n, raw in enumerate(text.split("\n"), 1):
        if "\t" in raw[: len(raw) - len(raw.lstrip())]:
            raise PlanSyntax("a tab in indentation", n)
        body = _strip_comment(raw)
        if body.strip():
            lines.append((n, len(body) - len(body.lstrip()), body.strip()))
    pos = 0

    def block(indent: int) -> Node:
        nonlocal pos
        if pos >= len(lines):
            return Node(None, lines[-1][0] if lines else 1)
        n, ind, content = lines[pos]
        if content.startswith("- ") or content == "-":
            items: List[Node] = []
            first = n
            while pos < len(lines) and lines[pos][1] == ind and (lines[pos][2].startswith("- ") or lines[pos][2] == "-"):
                n, _, content = lines[pos]
                rest = content[1:].strip()
                pos += 1
                if not rest:
                    items.append(block(ind + 1) if pos < len(lines) and lines[pos][1] > ind else Node(None, n))
                elif ":" in rest and not rest.startswith(("\"", "'", "[")) and _key_split(rest, n):
                    # `- key: value` opens a mapping whose further keys are indented past the dash
                    k, v = _key_split(rest, n)
                    m = {k: (_scalar(v, n) if v.strip() else (block(ind + 2) if pos < len(lines) and lines[pos][1] > ind else Node(None, n)))}
                    kl = {k: n}
                    while pos < len(lines) and lines[pos][1] == ind + 2 and not lines[pos][2].startswith("-"):
                        mapping_entry(m, ind + 2, kl)
                    items.append(Node(m, n, kl))
                else:
                    items.append(_scalar(rest, n))
            return Node(items, first)
        m: dict = {}
        kl: dict = {}
        first = n
        while pos < len(lines) and lines[pos][1] == ind:
            if lines[pos][2].startswith("- "):
                raise PlanSyntax("a sequence item where a mapping key is expected", lines[pos][0])
            mapping_entry(m, ind, kl)
        if pos < len(lines) and lines[pos][1] > ind:
            raise PlanSyntax("unexpected indentation", lines[pos][0])
        return Node(m, first, kl)

    def mapping_entry(m: dict, ind: int, kl: dict):
        nonlocal pos
        n, _, content = lines[pos]
        kv = _key_split(content, n)
        if kv is None:
            raise PlanSyntax(f"`{content}` is not `key: value`", n)
        k, v = kv
        if k in m:
            raise PlanSyntax(f"the key `{k}` is written twice", n)
        pos += 1
        kl[k] = n
        if v.strip():
            m[k] = _scalar(v, n)
        elif pos < len(lines) and lines[pos][1] > ind:
            m[k] = block(lines[pos][1])
        else:
            m[k] = Node(None, n)

    root = block(lines[0][1] if lines else 0)
    if pos < len(lines):
        raise PlanSyntax("unexpected indentation", lines[pos][0])
    return root


def _key_split(content: str, line: int = 0):
    """(key, rest) for `key: rest`; a quoted key is decoded as a quoted value is."""
    i = 0
    while i < len(content):
        c = content[i]
        if c in "\"'":
            e = _quoted_end(content, i)
            if e < 0:
                return None
            i = e
            continue
        if c == ":" and (i + 1 == len(content) or content[i + 1] == " "):
            raw = content[:i].strip()
            if raw[:1] in "\"'":
                if _quoted_end(raw, 0) != len(raw):
                    return None
                raw = _unquote(raw, line)
            return (raw, content[i + 1:]) if raw else None
        i += 1
    return None
