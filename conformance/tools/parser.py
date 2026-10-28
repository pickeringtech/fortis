"""Grammar-driven recogniser for Fortis source, interpreting fortis.ebnf directly.

It is a memoised set-of-end-positions recogniser over the token stream from lexer.py, so
alternatives are tried exhaustively (no ordered choice) and ambiguity cannot hide a defect.
The special sequences of the grammar are mapped as follows:
  lowerIdent/upperIdent/INTEGER/FLOAT/DECIMAL/DURATION/STRING/TEMPLATE/RAWSTRING/BYTESTRING/NL
      -> token kinds from the lexer
  headBody   -> a balanced token sequence up to the matching "}"
The "<" and ">" of the refinement rule match only the lexer's REF_OPEN/REF_CLOSE tokens.
"""
from __future__ import annotations

import re
import sys
from typing import Dict, List, Set, Tuple

import ebnf
from ebnf import Alt, Lit, Opt, Ref, Rep, Seq, Special
from lexer import KEYWORDS, Token  # noqa: F401  (Token re-exported for the runner)

sys.setrecursionlimit(20000)

_SPECIAL_KINDS = {
    "lowerIdent": ("LOWER",), "upperIdent": ("UPPER",), "INTEGER": ("INT",), "FLOAT": ("FLOAT",),
    "DECIMAL": ("DEC",), "DURATION": ("DUR",), "STRING": ("STR",), "TEMPLATE": ("STR",),
    "RAWSTRING": ("RAW",), "BYTESTRING": ("BYTES",), "NL": ("NL",), "keywordName": ("KW",),
}
_PLACEHOLDER = re.compile(r"^\s*[a-z_][A-Za-z0-9_]*\s*$")   # TEMPLATE: "${field}" names a payload field


class ParseFailure(Exception):
    def __init__(self, token: Token, expected: Set[str]):
        self.token = token
        self.expected = expected
        exp = ", ".join(sorted(expected)[:12])
        super().__init__(f"unexpected {token.kind} {token.text!r} at {token.line}:{token.col}; expected one of: {exp}")


class Recogniser:
    def __init__(self, rules: Dict[str, object]):
        self.rules = dict(rules)
        # In the refinement rule, "<" and ">" are the refinement delimiters, not comparison operators.
        self.rules["refinement"] = Seq((Special("REF_OPEN"), Ref("refineBody"), Special("REF_CLOSE")))
        self.toks: List[Token] = []
        self.memo: Dict[Tuple[str, int], Set[int]] = {}
        self.node_memo: Dict[Tuple[int, int], Set[int]] = {}
        self.furthest = 0
        self.expected_at_furthest: Set[str] = set()

    # -- terminal matching -------------------------------------------------
    def _terminal(self, pos: int, ok: bool, desc: str) -> Set[int]:
        if ok:
            if pos + 1 > self.furthest:
                self.furthest = pos + 1
                self.expected_at_furthest = set()
            return {pos + 1}
        if pos == self.furthest:
            self.expected_at_furthest.add(desc)
        elif pos > self.furthest:
            self.furthest = pos
            self.expected_at_furthest = {desc}
        return set()

    def _lit(self, node: Lit, pos: int) -> Set[int]:
        t = self.toks[pos]
        ok = t.kind not in ("STR", "RAW", "BYTES", "EOF", "REF_OPEN", "REF_CLOSE") and t.text == node.text
        return self._terminal(pos, ok, repr(node.text))

    def _special(self, node: Special, pos: int) -> Set[int]:
        name = node.text.split()[0] if node.text else ""
        if node.text in ("REF_OPEN", "REF_CLOSE"):
            return self._terminal(pos, self.toks[pos].kind == node.text, "<" if node.text == "REF_OPEN" else ">")
        raise AssertionError(f"unmapped special sequence: {node.text!r}")

    # -- rules ---------------------------------------------------------------
    def ends(self, node, pos: int) -> Set[int]:
        key = (id(node), pos)
        if key in self.node_memo:
            return self.node_memo[key]
        r = self._ends(node, pos)
        self.node_memo[key] = r
        return r

    def _ends(self, node, pos: int) -> Set[int]:
        if isinstance(node, Lit):
            return self._lit(node, pos)
        if isinstance(node, Ref):
            return self.rule(node.name, pos)
        if isinstance(node, Special):
            return self._special(node, pos)
        if isinstance(node, Opt):
            return {pos} | self.ends(node.body, pos)
        if isinstance(node, Rep):
            result = {pos}
            frontier = {pos}
            while frontier:
                new = set()
                for p in frontier:
                    new |= self.ends(node.body, p)
                frontier = new - result
                result |= new
            return result
        if isinstance(node, Seq):
            current = {pos}
            for item in node.items:
                nxt = set()
                for p in current:
                    nxt |= self.ends(item, p)
                current = nxt
                if not current:
                    break
            return current
        if isinstance(node, Alt):
            result = set()
            for opt in node.options:
                result |= self.ends(opt, pos)
            return result
        raise AssertionError(node)

    def rule(self, name: str, pos: int) -> Set[int]:
        key = (name, pos)
        if key in self.memo:
            return self.memo[key]
        kinds = _SPECIAL_KINDS.get(name)
        if kinds is not None:
            t = self.toks[pos]
            ok = t.kind in kinds
            if ok and name == "TEMPLATE":
                ok = all(_PLACEHOLDER.match(src) and src.strip() not in KEYWORDS for src, _, _ in t.interps)
                desc = "TEMPLATE (placeholders name payload fields: ${name})"
            else:
                desc = name
            r = self._terminal(pos, ok, desc)
        elif name == "headBody":
            r = self._head_body(pos)
        else:
            self.memo[key] = set()        # guard against left recursion
            r = self.ends(self.rules[name], pos)
        self.memo[key] = r
        return r

    def _head_body(self, pos: int) -> Set[int]:
        """A balanced token sequence up to the "}" that closes the head (LR 16): every closing
        delimiter must match the innermost open one."""
        closing = {"(": ")", "[": "]", "{": "}"}
        stack: List[str] = []
        i = pos
        while i < len(self.toks):
            t = self.toks[i]
            if t.kind == "EOF":
                return set()
            if t.kind == "REF_OPEN":
                stack.append("REF_CLOSE")
            elif t.kind == "REF_CLOSE":
                if not stack or stack.pop() != "REF_CLOSE":
                    return self._terminal(i, False, "matching refinement delimiter in head body")
            elif t.kind == "PUNCT" and t.text in closing:
                stack.append(closing[t.text])
            elif t.kind == "PUNCT" and t.text in (")", "]", "}"):
                if not stack:
                    if t.text == "}":
                        self.furthest = max(self.furthest, i)
                        return {i}
                    return self._terminal(i, False, "balanced head body")
                if stack.pop() != t.text:
                    return self._terminal(i, False, "matching delimiter in head body")
            i += 1
        return set()

    # -- entry points ------------------------------------------------------
    # -- parse tree --------------------------------------------------------
    # A tree is a tuple (ruleName, children); children are trees or Tokens. It is derived
    # from the memo tables after recognition, taking the first alternative that spans the
    # recognised range and, within a sequence or repetition, the longest span for each item
    # (greedy), so it is one derivation of a possibly ambiguous sentence. The ambiguity corpus
    # (DEL-02) is where the grammar is made unambiguous; until then this choice is documented.
    def derive(self, node, pos: int, end: int):
        if isinstance(node, (Lit, Special)):
            return [self.toks[pos]]
        if isinstance(node, Ref):
            name = node.name
            if name in _SPECIAL_KINDS:
                return [self.toks[pos]]
            if name == "headBody":
                return [(name, list(self.toks[pos:end]))]
            return [(name, self.derive(self.rules[name], pos, end))]
        if isinstance(node, Opt):
            if end == pos:
                return []
            return self.derive(node.body, pos, end)
        if isinstance(node, Rep):
            if end == pos:
                return []
            for p in sorted(self.ends(node.body, pos), reverse=True):     # longest item first (greedy, like the usual expression parse)
                if pos < p <= end and end in self.ends(node, p):
                    return self.derive(node.body, pos, p) + self.derive(node, p, end)
            raise AssertionError("no derivation for repetition")
        if isinstance(node, Seq):
            out = self._derive_seq(node.items, 0, pos, end)
            if out is None:
                raise AssertionError("no derivation for sequence")
            return out
        if isinstance(node, Alt):
            for opt in node.options:
                if end in self.ends(opt, pos):
                    return self.derive(opt, pos, end)
            raise AssertionError("no derivation for alternative")
        raise AssertionError(node)

    def _derive_seq(self, items, k: int, pos: int, end: int):
        if k == len(items):
            return [] if pos == end else None
        for p in sorted(self.ends(items[k], pos), reverse=True):     # longest first: `fail X()` binds the call to fail, a lambda takes its whole body
            if p > end:
                continue
            rest = self._derive_seq(items, k + 1, p, end)
            if rest is not None:
                return self.derive(items[k], pos, p) + rest
        return None

    def parse(self, toks: List[Token], start: str = "module"):
        """Recognise, then return one parse tree for the whole input (trailing NLs allowed
        for fragments)."""
        self.recognise(toks, start)
        end = len(toks) - 1
        ends = self.rule(start, 0)
        if end not in ends and start != "module":
            end = max(e for e in ends if all(t.kind == "NL" for t in toks[e:len(toks) - 1]))
        return (start, self.derive(self.rules[start] if start not in _SPECIAL_KINDS else Ref(start), 0, end))

    def recognise(self, toks: List[Token], start: str = "module") -> None:
        self.toks = toks
        self.memo = {}
        self.node_memo = {}
        self.furthest = 0
        self.expected_at_furthest = set()
        end = len(toks) - 1  # index of EOF
        ends = self.rule(start, 0)
        if end in ends:
            return
        if start != "module":
            # a fragment: trailing NLs before EOF are allowed (a file ends with an implicit NL)
            for e in ends:
                if all(t.kind == "NL" for t in toks[e:end]):
                    return
        pos = min(self.furthest, end)
        raise ParseFailure(toks[pos], self.expected_at_furthest)


def load_grammar(path: str) -> Recogniser:
    rules, _ = ebnf.load(path)
    return Recogniser(rules)


# ---------------------------------------------------------------- E-SYN-004 on the tree
_OPERATOR_LEVELS = {"outcomeExpr": "outcome", "coalesceExpr": "coalesce", "orExpr": "logical", "andExpr": "logical"}
_STOP = {"primaryExpr", "block", "argList", "headBody"}


def _families(node, found: Dict[str, Token], root: bool = True):
    """Collect the operator families used at the top level of one expression: an outcomeExpr
    with an operator, a coalesceExpr with `??`, an orExpr/andExpr with more than one operand.
    Does not descend into primaryExpr (parentheses, literals, nested expressions), blocks,
    argument lists or a nested expr (an index); each of those is checked on its own."""
    if isinstance(node, Token):
        return
    name, children = node
    if name in _STOP or (name == "expr" and not root):
        return
    fam = _OPERATOR_LEVELS.get(name)
    if fam:
        ops = [c for c in children if isinstance(c, Token) and (
            (fam == "outcome" and c.text in ("or", "catch")) or (fam == "coalesce" and c.text == "??")
            or (fam == "logical" and c.text in ("||", "&&")))]
        if ops and fam not in found:
            found[fam] = ops[0]
    for c in children:
        _families(c, found, False)


def first_token(tree):
    if isinstance(tree, Token):
        return tree
    for c in tree[1]:
        t = first_token(c)
        if t is not None:
            return t
    return None


def check_operator_mixing(tree):
    """LR 5.6: outcome operators, `??` and `||`/`&&` may not be mixed in one unparenthesised
    expression. Returns (second family's operator token, first token of the expression) or None."""
    if isinstance(tree, Token):
        return None
    name, children = tree
    if name == "expr":
        found: Dict[str, Token] = {}
        _families(tree, found)
        if len(found) > 1:
            toks = sorted(found.values(), key=lambda t: (t.line, t.col))
            return toks[1], first_token(tree)
    for c in children:
        bad = check_operator_mixing(c)
        if bad is not None:
            return bad
    return None


def find_suspend_or_outcome(tree):
    """LANG-03-06 for interpolations: an `await` or an outcome operator anywhere in the parsed
    expression (keyword-named members such as `c.await()` are memberName nodes and do not count).
    Returns the offending token or None."""
    if isinstance(tree, Token):
        return None
    name, children = tree
    if name == "lambda":
        return None                   # a closure body is not evaluated by the interpolation (LR 8.2)
    if name == "unaryExpr" and children and isinstance(children[0], Token) and children[0].kind == "KW" and children[0].text == "await":
        return children[0]
    if name == "outcomeExpr" and len(children) > 1:
        return next(c for c in children if isinstance(c, Token))
    for c in children:
        bad = find_suspend_or_outcome(c)
        if bad is not None:
            return bad
    return None
