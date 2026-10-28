"""Loader for reference/grammar/fortis.ebnf.

Parses the ISO 14977 subset the Fortis grammar uses (juxtaposition for concatenation,
"x" literals, [x] optional, {x} repetition, (x) grouping, x | y alternatives,
? special sequence ?, (* comment *)) into a rule table, and checks it for undefined
and unreachable nonterminals.
"""
from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from typing import List, Union

# ---------------------------------------------------------------- AST

@dataclass(frozen=True)
class Lit:
    text: str

@dataclass(frozen=True)
class Ref:
    name: str

@dataclass(frozen=True)
class Special:
    text: str

@dataclass(frozen=True)
class Opt:
    body: "Node"

@dataclass(frozen=True)
class Rep:
    body: "Node"

@dataclass(frozen=True)
class Seq:
    items: tuple

@dataclass(frozen=True)
class Alt:
    options: tuple

Node = Union[Lit, Ref, Special, Opt, Rep, Seq, Alt]

# ---------------------------------------------------------------- tokeniser

_TOKEN = re.compile(
    r"""
    (?P<comment>\(\*.*?\*\))
  | (?P<ws>\s+)
  | (?P<special>\?[^?]*\?)
  | (?P<string>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')
  | (?P<ident>[A-Za-z_][A-Za-z0-9_]*)
  | (?P<punct>=|;|\||\[|\]|\{|\}|\(|\)|,)
    """,
    re.VERBOSE | re.DOTALL,
)


def _skip_comment(text: str, pos: int) -> int:
    """Skip a (* ... *) comment starting at pos; comments nest."""
    depth = 0
    i = pos
    while i < len(text):
        if text.startswith("(*", i):
            depth += 1
            i += 2
        elif text.startswith("*)", i):
            depth -= 1
            i += 2
            if depth == 0:
                return i
        else:
            i += 1
    raise SyntaxError("ebnf: unterminated comment")


def _tokens(text: str):
    pos = 0
    while pos < len(text):
        if text.startswith("(*", pos):
            pos = _skip_comment(text, pos)
            continue
        m = _TOKEN.match(text, pos)
        if not m:
            line = text.count("\n", 0, pos) + 1
            raise SyntaxError(f"ebnf: unexpected character {text[pos]!r} at line {line}")
        pos = m.end()
        kind = m.lastgroup
        if kind in ("comment", "ws"):
            continue
        yield kind, m.group(kind), text.count("\n", 0, m.start()) + 1


class _Parser:
    def __init__(self, text: str):
        self.toks = list(_tokens(text))
        self.i = 0

    def peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else ("eof", "", -1)

    def take(self, kind=None, value=None):
        k, v, line = self.peek()
        if kind and k != kind or value is not None and v != value:
            raise SyntaxError(f"ebnf: expected {value or kind}, got {v!r} at line {line}")
        self.i += 1
        return v

    def rules(self):
        out = {}
        order = []
        while self.peek()[0] != "eof":
            name = self.take("ident")
            self.take("punct", "=")
            body = self.alt()
            self.take("punct", ";")
            if name in out:
                raise SyntaxError(f"ebnf: rule {name} defined twice")
            out[name] = body
            order.append(name)
        return out, order

    def alt(self):
        opts = [self.seq()]
        while self.peek()[1] == "|":
            self.take()
            opts.append(self.seq())
        return opts[0] if len(opts) == 1 else Alt(tuple(opts))

    def seq(self):
        items = []
        while True:
            k, v, _ = self.peek()
            if k == "string":
                self.take()
                items.append(Lit(bytes(v[1:-1], "utf-8").decode("unicode_escape")))
            elif k == "ident":
                self.take()
                items.append(Ref(v))
            elif k == "special":
                self.take()
                items.append(Special(v[1:-1].strip()))
            elif v == "[":
                self.take(); items.append(Opt(self.alt())); self.take("punct", "]")
            elif v == "{":
                self.take(); items.append(Rep(self.alt())); self.take("punct", "}")
            elif v == "(":
                self.take(); items.append(self.alt()); self.take("punct", ")")
            else:
                break
        if not items:
            raise SyntaxError(f"ebnf: empty sequence near line {self.peek()[2]}")
        return items[0] if len(items) == 1 else Seq(tuple(items))


def load(path: str):
    with open(path, encoding="utf-8") as f:
        rules, order = _Parser(f.read()).rules()
    return rules, order


def refs(node: Node, acc=None):
    acc = set() if acc is None else acc
    if isinstance(node, Ref):
        acc.add(node.name)
    elif isinstance(node, (Opt, Rep)):
        refs(node.body, acc)
    elif isinstance(node, Seq):
        for n in node.items:
            refs(n, acc)
    elif isinstance(node, Alt):
        for n in node.options:
            refs(n, acc)
    return acc


def check(rules: dict, start: str = "module"):
    """Return (undefined, unreachable) nonterminal name lists."""
    undefined = set()
    for name, body in rules.items():
        undefined |= {r for r in refs(body) if r not in rules}
    seen, todo = set(), [start]
    while todo:
        n = todo.pop()
        if n in seen or n not in rules:
            continue
        seen.add(n)
        todo.extend(refs(rules[n]))
    unreachable = [n for n in rules if n not in seen]
    return sorted(undefined), unreachable


if __name__ == "__main__":
    path = sys.argv[1] if len(sys.argv) > 1 else "reference/grammar/fortis.ebnf"
    rules, order = load(path)
    undefined, unreachable = check(rules)
    print(f"{len(rules)} rules")
    print("undefined:", undefined or "none")
    print("unreachable:", unreachable or "none")
    sys.exit(1 if undefined else 0)
