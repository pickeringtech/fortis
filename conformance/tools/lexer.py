"""Fortis lexer for the G0 conformance tooling.

Implements Language Reference 3.1-3.7 and the lexer notes at the end of fortis.ebnf. It is
context-free: identifier category by first character, keywords always keyword tokens (the
grammar admits them as names in member position through `memberName`), literals, the
refinement delimiters as distinct tokens, newline termination with trailing and leading
continuation tokens, body versus expression braces, and raw template regions opened by the
lexical shape `view [Name [(...)]] {` at the start of a statement.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import List, Optional, Tuple

KEYWORDS = set("""
func fn let mut const type alias class trivial enum error interface mixin implement implements
with derive requires forbids excludes contract aspect needs before after on metadata lifetime from
where export import as
pure noalloc async try recovers editable owned dynamic linear trusted once
copy move
if else while for in loop break continue return fail give recover match when otherwise is or catch
discard defer await scope
true false null present success failure self Self
capture record report observation required version encoding
test cases property forall fuzz bench expect
application context model component view explore server client hybrid
inject provide state action resource
""".split())

# Trailing tokens that continue the line (LR 3.2). The refinement delimiters are not among them.
TRAILING_CONTINUATION = {
    ",", "(", "[", "=", "=>", "+", "-", "*", "/", "%", "<<", ">>", "&", "|", "^", "&&", "||",
    "??", "==", "!=", "<", "<=", ">", ">=", "..", "..<", ".",
}
# Leading tokens that continue the previous line (LR 3.2).
LEADING_CONTINUATION = {".", "or", "catch", "??", "else", ")", "]", "with", "where", "implements", "derive", "from"}

# A "{" directly after one of these tokens opens an expression brace (LR 3.2, closed list).
EXPRESSION_BRACE_AFTER = {
    "=", "(", ",", ":", "[", "??", "return", "give", "fail", "recover",
    "+=", "-=", "*=", "/=", "%=", "==", "!=",
}

PUNCT = [
    "...", "..<", "..", "?.", "??", "=>", "==", "!=", "<=", ">=", "<<", ">>", "&&", "||",
    "+=", "-=", "*=", "/=", "%=",
    "(", ")", "[", "]", "{", "}", ",", ".", ":", "=", "+", "-", "*", "/", "%", "<", ">", "!",
    "&", "|", "^", "?", "@",
]

_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_DIGITS = r"[0-9](?:_?[0-9])*"
# Literal categories, LR 3.5 and the EBNF terminals; tried in this order.
_NUMBERS: List[Tuple[str, re.Pattern]] = [
    ("INT", re.compile(r"0[xX][0-9A-Fa-f](?:_?[0-9A-Fa-f])*|0[bB][01](?:_?[01])*|0[oO][0-7](?:_?[0-7])*")),
    ("DUR", re.compile(_DIGITS + r"(?:ns|us|ms|s|min|h)")),
    ("DEC", re.compile(_DIGITS + r"(?:\." + _DIGITS + r")?d")),
    ("FLOAT", re.compile(_DIGITS + r"(?:\." + _DIGITS + r")?[eE][+-]?" + _DIGITS + r"(?:f32)?|" + _DIGITS + r"\." + _DIGITS + r"(?:f32)?")),
    ("INT", re.compile(_DIGITS)),
]


class LexError(Exception):
    def __init__(self, message: str, line: int, col: int, code: str = "E-SYN-000"):
        super().__init__(f"{code}: {message} at {line}:{col}")
        self.line, self.col, self.code = line, col, code


@dataclass
class Token:
    kind: str   # NL LOWER UPPER KW INT FLOAT DEC DUR STR RAW BYTES TEMPLATE PUNCT REF_OPEN REF_CLOSE EOF
    text: str
    line: int
    col: int
    interps: List[Tuple[str, int, int]] = field(default_factory=list)   # STR: each ${...} source and position
    closes_type_args: bool = False

    def __repr__(self):
        return f"{self.kind}({self.text!r})@{self.line}:{self.col}"


def _scan_string(src: str, i: int, line: int, col: int, interps: Optional[list]) -> int:
    """Return the index just past a double-quoted string starting at src[i] == '"'. Handles
    escapes and ${ } interpolation (which may contain strings, raw strings and byte strings);
    records each interpolation's source in `interps` when given (None for byte strings)."""
    j = i + 1
    while j < len(src):
        c = src[j]
        if c == "\\":
            if j + 1 < len(src) and src[j + 1] == "\n":
                raise LexError("newline inside string literal", line, col)
            j += 2
            continue
        if c == '"':
            return j + 1
        if c == "\n":
            raise LexError("newline inside string literal", line, col)
        if c == "$" and src.startswith("${", j) and interps is not None:
            depth = 1
            start = j + 2
            j += 2
            while j < len(src) and depth:
                c = src[j]
                if c == "{":
                    depth += 1
                    j += 1
                elif c == "}":
                    depth -= 1
                    j += 1
                elif c == "b" and src.startswith('b"', j) and not (j > 0 and (src[j - 1].isalnum() or src[j - 1] == "_")):
                    j = _scan_string(src, j + 1, line, col, None)
                elif src.startswith('"""', j):
                    end = src.find('"""', j + 3)
                    if end < 0 or "\n" in src[j:end]:
                        raise LexError("unterminated raw string inside interpolation", line, col)
                    j = end + 3
                elif c == '"':
                    j = _scan_string(src, j, line, col, [])
                elif src.startswith("//", j):
                    raise LexError("comment inside an interpolation", line, col)
                elif c == "\n":
                    raise LexError("newline inside interpolation", line, col)
                else:
                    j += 1
            if depth:
                raise LexError("unterminated interpolation", line, col)
            interps.append((src[start:j - 1], line, col + (start - i)))
            continue
        j += 1
    raise LexError("unterminated string literal", line, col)


def _is_member_of_type(sig: List[Token]) -> bool:
    """True when the last token is an UPPER name reached through "." from a complete type
    reference (`E.A`, `E[Int].A`, `Int<0..9>.Max`): a variant or associated member, which is a
    value, so a following "<" is a comparison (lexer note 2)."""
    if len(sig) >= 3 and sig[-2].kind == "PUNCT" and sig[-2].text == ".":
        before = sig[-3]
        return before.kind in ("UPPER", "REF_CLOSE") or (before.kind == "PUNCT" and before.text == "]" and before.closes_type_args)
    return False


def _opens_template(out: List[Token], stack: List[str]) -> bool:
    """LR 16: a "{" opens a raw template region when its statement so far is exactly
    `[export] [server|client|hybrid] view`, `… view Name` or `… view Name( ... )`, with that
    statement starting at a logical statement boundary (LR 3.2)."""
    k = len(out) - 1
    had_params = False
    if k >= 0 and out[k].kind == "PUNCT" and out[k].text == ")":
        depth = 0
        while k >= 0:
            t = out[k]
            if t.kind == "PUNCT" and t.text == ")":
                depth += 1
            elif t.kind == "PUNCT" and t.text == "(":
                depth -= 1
                if depth == 0:
                    k -= 1
                    break
            k -= 1
        else:
            return False
        had_params = True
    if k >= 0 and out[k].kind in ("UPPER", "LOWER"):
        k -= 1
    elif had_params:
        return False                      # parentheses only after a name
    if not (k >= 0 and out[k].kind == "KW" and out[k].text == "view"):
        return False
    k -= 1
    if k >= 0 and out[k].kind == "KW" and out[k].text in ("server", "client", "hybrid"):
        k -= 1                            # at most one execution prefix
    if k >= 0 and out[k].kind == "KW" and out[k].text == "export":
        k -= 1                            # at most one export
    if k < 0:
        return True
    before = out[k]
    if before.kind == "PUNCT" and before.text == "{":
        return bool(stack) and stack[-1] == "body{"     # `{ statement }` on one line (LR 3.2)
    if before.kind != "NEWLINE":
        return False                      # after `}` or anything else a newline must separate items
    # the newline must end a statement: innermost delimiter is a body (or none) and the previous
    # significant token does not continue the line
    if stack and stack[-1] != "body{":
        return False
    j = k - 1
    while j >= 0 and out[j].kind == "NEWLINE":
        j -= 1
    return j < 0 or not (out[j].kind in ("PUNCT", "KW") and out[j].text in TRAILING_CONTINUATION)


def _raw_tokens(src: str) -> List[Token]:
    """Tokens with every physical newline as a NEWLINE token, before continuation handling."""
    if src.startswith("﻿"):
        raise LexError("byte-order mark", 1, 1)
    out: List[Token] = []
    sig: List[Token] = []          # significant (non-NEWLINE) tokens
    i, line, line_start = 0, 1, 0
    n = len(src)
    ref_depth = 0
    square_after_upper: List[bool] = []
    stack: List[str] = []          # open delimiters: "(" "[" "expr{" "body{" (for the template-region shape)

    def prev() -> Optional[Token]:
        return sig[-1] if sig else None

    def push(tok: Token):
        out.append(tok)
        sig.append(tok)

    while i < n:
        c = src[i]
        col = i - line_start + 1
        if c == "\n":
            out.append(Token("NEWLINE", "\n", line, col))
            i += 1
            line += 1
            line_start = i
            continue
        if c in " \t\r":
            i += 1
            continue
        if src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j
            continue
        if c == ";":
            raise LexError("';' is not a token", line, col, "E-SYN-001")
        if src.startswith('"""', i):
            j = src.find('"""', i + 3)
            if j < 0:
                raise LexError("unterminated raw string", line, col)
            text = src[i:j + 3]
            push(Token("RAW", text, line, col))
            line += text.count("\n")
            if "\n" in text:
                line_start = i + text.rfind("\n") + 1
            i = j + 3
            continue
        if c == '"':
            interps: list = []
            j = _scan_string(src, i, line, col, interps)
            push(Token("STR", src[i:j], line, col, interps))
            i = j
            continue
        if c == "b" and i + 1 < n and src[i + 1] == '"':
            j = _scan_string(src, i + 1, line, col, None)
            text = src[i:j]
            if not text.isascii():
                raise LexError("non-ASCII character in byte string", line, col)
            push(Token("BYTES", text, line, col))
            i = j
            continue
        m = _IDENT.match(src, i)
        if m:
            text = m.group(0)
            kind = "KW" if text in KEYWORDS else ("UPPER" if text[0].isupper() else "LOWER")
            push(Token(kind, text, line, col))
            i = m.end()
            continue
        if c.isdigit():
            for kind, pattern in _NUMBERS:
                m = pattern.match(src, i)
                if m and not (m.end() < n and (src[m.end()].isalnum() or src[m.end()] == "_")):
                    push(Token(kind, m.group(0), line, col))
                    i = m.end()
                    break
            else:
                raise LexError("malformed numeric literal", line, col)
            continue
        if c == "{" and _opens_template(out, stack):
            depth, j = 1, i + 1
            while j < n and depth:
                if src[j] == "{":
                    depth += 1
                elif src[j] == "}":
                    depth -= 1
                j += 1
            if depth:
                raise LexError("unterminated view body", line, col)
            push(Token("PUNCT", "{", line, col))
            text = src[i + 1:j - 1]
            push(Token("TEMPLATE", text, line, col + 1))
            # the "}" below closes it; nothing is pushed on the delimiter stack for a raw region
            line += text.count("\n")
            if "\n" in text:
                line_start = i + 1 + text.rfind("\n") + 1
            push(Token("PUNCT", "}", line, j - line_start))
            i = j
            continue
        for p_text in PUNCT:
            if src.startswith(p_text, i):
                kind = "PUNCT"
                p = prev()
                if p_text == "<" and p is not None and (
                    (p.kind == "UPPER" and not _is_member_of_type(sig))
                    or (p.text == "]" and p.kind == "PUNCT" and p.closes_type_args)
                ):
                    kind = "REF_OPEN"
                    ref_depth += 1
                elif p_text == ">" and ref_depth > 0:
                    kind = "REF_CLOSE"
                    ref_depth -= 1
                elif p_text == ">=" and ref_depth > 0:
                    push(Token("REF_CLOSE", ">", line, col))
                    ref_depth -= 1
                    push(Token("PUNCT", "=", line, col + 1))
                    i += 2
                    break
                elif p_text == ">>" and ref_depth > 0:
                    # a refinement closes before the shift operator can form: Int<0..Int<1..2>>
                    push(Token("REF_CLOSE", ">", line, col))
                    ref_depth -= 1
                    i += 1
                    break
                tok = Token(kind, p_text, line, col)
                if p_text == "[":
                    square_after_upper.append(p is not None and p.kind == "UPPER")
                elif p_text == "]":
                    tok.closes_type_args = square_after_upper.pop() if square_after_upper else False
                if p_text in ("(", "["):
                    stack.append(p_text)
                elif p_text == "{":
                    is_expr = p is not None and p.kind in ("PUNCT", "KW") and p.text in EXPRESSION_BRACE_AFTER
                    stack.append("expr{" if is_expr else "body{")
                elif p_text in (")", "]", "}") and stack:
                    stack.pop()
                push(tok)
                i += len(p_text)
                break
        else:
            raise LexError(f"unexpected character {c!r}", line, col)
    out.append(Token("NEWLINE", "\n", line, i - line_start + 1))
    return out


def lex(src: str) -> List[Token]:
    """Tokens with NL emitted only where a newline ends a statement (LR 3.2)."""
    raw = _raw_tokens(src)
    out: List[Token] = []
    stack: List[str] = []    # open delimiters: "(" "[" "expr{" "body{"
    k = 0
    while k < len(raw):
        t = raw[k]
        if t.kind == "NEWLINE":
            j = k + 1
            while j < len(raw) and raw[j].kind == "NEWLINE":
                j += 1
            nxt = raw[j] if j < len(raw) else None
            prv = out[-1] if out else None
            suppressed = False
            if stack and stack[-1] in ("(", "[", "expr{"):
                suppressed = True
            elif prv is not None and prv.kind in ("PUNCT", "KW") and prv.text in TRAILING_CONTINUATION:
                suppressed = True
            elif nxt is not None and nxt.kind in ("PUNCT", "KW") and nxt.text in LEADING_CONTINUATION:
                suppressed = True
            if not suppressed and prv is not None and prv.kind != "NL":
                out.append(Token("NL", "\n", t.line, t.col))
            k = j
            continue
        if t.kind == "PUNCT":
            if t.text in ("(", "["):
                stack.append(t.text)
            elif t.text == "{":
                prv = out[-1] if out else None
                is_expr = prv is not None and prv.kind in ("PUNCT", "KW") and prv.text in EXPRESSION_BRACE_AFTER
                stack.append("expr{" if is_expr else "body{")
            elif t.text in (")", "]", "}"):
                if stack:
                    stack.pop()
        out.append(t)
        k += 1
    if out and out[-1].kind != "NL":
        out.append(Token("NL", "\n", raw[-1].line, raw[-1].col))
    out.append(Token("EOF", "", raw[-1].line, raw[-1].col))
    return out


if __name__ == "__main__":
    import sys
    for tok in lex(open(sys.argv[1], encoding="utf-8").read()):
        print(tok)
