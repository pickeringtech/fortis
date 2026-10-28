"""Parser generator for fortis.ebnf (DEL-02, LANG-03-03, Programme G0).

Reads the grammar, computes FIRST_k / FOLLOW_k sets for every decision point (alternative, option,
repetition) at the smallest k <= K_MAX that makes every decision deterministic, and emits a
standalone recursive-descent parser, conformance/generated/fortis_parser.py, whose decisions are
k-token lookahead tables over token classes only (no symbol table, no names). A decision point that
no k <= K_MAX separates is a *conflict*: it is listed in the lookahead report and the emitted parser
resolves it by ordered choice with local backtracking, counting every such resolution so that the
ambiguity corpus can show the count is zero. Terminal classes: a grammar literal matches a token's
text; lowerIdent/upperIdent/INTEGER/... match the lexer's kinds; "<" and ">" of a refinement are the
lexer's REF_OPEN/REF_CLOSE tokens; headBody is the lexer-level balanced scan (Language Reference 16).

  gen_parser.py [--kmax 3] [--out conformance/generated/fortis_parser.py] [--rust compiler/crates/fortis-syntax/src/generated.rs]"""
from __future__ import annotations

import hashlib
import json
import os
import sys
from typing import Dict, FrozenSet, List, Set, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import ebnf  # noqa: E402
from ebnf import Alt, Lit, Opt, Ref, Rep, Seq, Special  # noqa: E402

GRAMMAR = os.path.join(ROOT, "reference", "grammar", "fortis.ebnf")
OUT = os.path.join(ROOT, "conformance", "generated", "fortis_parser.py")
KIND_RULES = {"lowerIdent": "LOWER", "upperIdent": "UPPER", "INTEGER": "INT", "FLOAT": "FLOAT", "DECIMAL": "DEC", "DURATION": "DUR",
              "STRING": "STR", "TEMPLATE": "STR", "RAWSTRING": "RAW", "BYTESTRING": "BYTES", "NL": "NL", "keywordName": "KW"}
END = "$END"
Tup = Tuple[str, ...]

# Documented resolutions of the decisions no fixed lookahead separates (Language Reference 3.2, X-94).
# P1: lexer — a run of newlines and blank lines is one NL token (removes the NL-run conflicts).
# P2: greedy — an option or repetition whose body can start is entered/continued; this is the usual
#     "longest match" rule for operator chains, postfix chains, and an expression-bodied lambda or
#     match arm that extends to the end of the enclosing expression.
# P3: ordered choice — the alternatives are tried in grammar order and the first that parses wins;
#     the parser backtracks locally within the statement. The cases: a statement starting with a place
#     is an assignment when an assignment operator follows the place, else an expression; a `for`
#     header tries `move place`, then a range, then an expression; `let x = move p` is the plain let;
#     a parenthesised type is tried before a tuple type; a `.member(` postfix is a method call.
POLICIES = {
    ("*", "option"): "P2", ("*", "repetition"): "P2",
    ("statement", "alternative"): "P3", ("forStmt", "alternative"): "P3", ("letStmt", "alternative"): "P3",
    ("postfix", "alternative"): "P3", ("primaryExpr", "alternative"): "P3", ("type", "alternative"): "P3",
    ("namedType", "alternative"): "P3", ("matchArm", "alternative"): "P3", ("mixinMember", "alternative"): "P3",
}


class Generator:
    def __init__(self, rules: Dict[str, object], kmax: int):
        self.rules = dict(rules)
        self.rules["refinement"] = Seq((Special("REF_OPEN"), Ref("refineBody"), Special("REF_CLOSE")))
        self.kmax = kmax
        self.k = 1
        self.first: Dict[str, Set[Tup]] = {}
        self.follow: Dict[str, Set[Tup]] = {}
        self.conflicts: List[dict] = []
        self.decisions = 0

    # ---- terminal classes -------------------------------------------------------------
    @staticmethod
    def term(node) -> str:
        if isinstance(node, Lit):
            return node.text
        if isinstance(node, Special):
            return "KIND:" + node.text
        raise AssertionError(node)

    def is_terminal_rule(self, name: str) -> bool:
        return name in KIND_RULES or name == "headBody"

    # ---- FIRST_k with an explicit tail ------------------------------------------------
    def cat(self, prefix: Tup, tails: Set[Tup]) -> Set[Tup]:
        k = self.k
        if len(prefix) >= k:
            return {prefix[:k]}
        return {(prefix + t)[:k] for t in tails}

    def firstk(self, node, tails: Set[Tup]) -> Set[Tup]:
        return self.prune(self._firstk(node, tails))

    @staticmethod
    def prune(s: Set[Tup]) -> Set[Tup]:
        """Policy P1: the lexer never emits two NL tokens in a row, so such prefixes cannot occur."""
        return {t for t in s if not any(t[i] == "KIND:NL" and t[i + 1] == "KIND:NL" for i in range(len(t) - 1))}

    def _firstk(self, node, tails: Set[Tup]) -> Set[Tup]:
        """Prefixes (length <= k) of node followed by any of `tails`; END inside a tuple marks the end of input."""
        if isinstance(node, Lit) or isinstance(node, Special):
            return self.cat((self.term(node),), tails)
        if isinstance(node, Ref):
            if node.name == "headBody":
                return {tuple(["$ANY"] * self.k)}    # an opaque head body: any tokens (lexer-level balanced scan)
            if self.is_terminal_rule(node.name):
                key = "KIND:" + KIND_RULES.get(node.name, node.name)
                return self.cat((key,), tails)
            out: Set[Tup] = set()
            for p in self.first.get(node.name, set()):
                if END in p:
                    i = p.index(END)
                    out |= self.cat(p[:i], tails)
                else:
                    out.add(p)
            return out
        if isinstance(node, Opt):
            return set(tails) | self.firstk(node.body, tails)
        if isinstance(node, Rep):
            acc = set(tails)
            while True:
                new = acc | self.firstk(node.body, acc)
                if new == acc:
                    return acc
                acc = new
        if isinstance(node, Seq):
            t = tails
            for item in reversed(node.items):
                t = self.firstk(item, t)
            return t
        if isinstance(node, Alt):
            out = set()
            for o in node.options:
                out |= self.firstk(o, tails)
            return out
        raise AssertionError(node)

    def compute_first(self):
        self.first = {r: set() for r in self.rules if not self.is_terminal_rule(r)}
        changed = True
        while changed:
            changed = False
            for r, body in self.rules.items():
                if self.is_terminal_rule(r):
                    continue
                new = self.firstk(body, {(END,)})
                if new != self.first[r]:
                    self.first[r] = new
                    changed = True

    def compute_follow(self, start: str):
        self.follow = {r: set() for r in self.rules}
        self.follow[start] = {("KIND:EOF",)}
        changed = True
        while changed:
            changed = False
            for r, body in self.rules.items():
                if self.is_terminal_rule(r):
                    continue
                for ref, tail_nodes in self.occurrences(body):
                    tails = self.firstk(Seq(tuple(tail_nodes)), self.expand_follow(r)) if tail_nodes else self.expand_follow(r)
                    before = len(self.follow[ref])
                    self.follow[ref] |= tails
                    if len(self.follow[ref]) != before:
                        changed = True

    def expand_follow(self, r: str) -> Set[Tup]:
        return self.follow.get(r) or {("KIND:EOF",)}

    def occurrences(self, node, after: Tuple = ()):
        """(rule referenced, nodes that follow it inside the enclosing structure) for every Ref in node."""
        out = []
        if isinstance(node, Ref):
            if not self.is_terminal_rule(node.name):
                out.append((node.name, list(after)))
        elif isinstance(node, (Opt,)):
            out += self.occurrences(node.body, after)
        elif isinstance(node, Rep):
            out += self.occurrences(node.body, (node,) + after)
        elif isinstance(node, Seq):
            for i, item in enumerate(node.items):
                out += self.occurrences(item, tuple(node.items[i + 1:]) + after)
        elif isinstance(node, Alt):
            for o in node.options:
                out += self.occurrences(o, after)
        return out

    # ---- decisions ----------------------------------------------------------------------
    def decide(self, rule: str, sets: List[Set[Tup]], where: str) -> Tuple[List[Set[Tup]], bool]:
        """Branch lookahead sets; conflict when two branches share a k-prefix. A conflict covered by a
        documented policy (POLICIES) is recorded as resolved; any other is an undocumented conflict."""
        self.decisions += 1
        conflict = False
        for i in range(len(sets)):
            for j in range(i + 1, len(sets)):
                shared = sets[i] & sets[j]
                if shared:
                    conflict = True
                    policy = POLICIES.get((rule, where)) or (POLICIES.get(("*", where)) if where in ("option", "repetition") else None)
                    self.conflicts.append({"rule": rule, "where": where, "branches": [i, j], "shared": sorted(" ".join(s) for s in shared)[:6],
                                           "policy": policy})
        return sets, conflict

    # ---- emission -------------------------------------------------------------------------
    def emit(self, start: str) -> str:
        self.lines: List[str] = []
        self.tables: List[Set[Tup]] = []
        self.conflicts = []
        self.decisions = 0
        self.compute_first()
        self.compute_follow(start)
        body_lines = []
        for r, body in self.rules.items():
            if self.is_terminal_rule(r):
                continue
            self.cur = []
            self.indent = 2
            self.rule_name = r
            self.gen(body, self.expand_follow(r))
            body_lines.append(f"    def r_{r}(self):")
            body_lines.append(f"        node = [{r!r}, []]")
            body_lines.append("        self.stack.append(node)")
            body_lines += self.cur if self.cur else ["        pass"]
            body_lines.append("        self.stack.pop()")
            body_lines.append("        return self.attach(node)")
            body_lines.append("")
        tables = "\n".join(f"T{i} = {self.trie(t)!r}" for i, t in enumerate(self.tables))
        header = f'''"""GENERATED by conformance/tools/gen_parser.py from reference/grammar/fortis.ebnf
(sha256 {hashlib.sha256(open(GRAMMAR, "rb").read()).hexdigest()}); do not edit. Lookahead k = {self.k}; decisions {self.decisions};
conflicts {len(self.conflicts)} (resolved by ordered choice with local backtracking, counted in `fallbacks`).
Every decision consults only token classes (literal text or lexer kind), never a name."""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tools"))
import parser as _ref  # noqa: E402  (the lexer-level head-body scan only)
from lexer import Token  # noqa: E402

K = {self.k}
KIND_ONLY = ("STR", "RAW", "BYTES", "EOF", "REF_OPEN", "REF_CLOSE", "NL")


class ParseError(Exception):
    def __init__(self, tok, expected):
        super().__init__(f"unexpected {{tok.kind}} {{tok.text!r}} at {{tok.line}}:{{tok.col}}; expected one of: {{', '.join(sorted(expected))[:120]}}")
        self.tok, self.expected = tok, expected


def keys(tok):
    """The terminal classes a token can match: its kind, and its text when it is a punctuation or keyword token."""
    ks = ["KIND:" + tok.kind]
    if tok.kind not in KIND_ONLY:
        ks.append(tok.text)
    if tok.kind == "KW":
        ks.append(tok.text)
    return ks


def matches(trie, toks, pos):
    """Does some k-prefix of toks[pos:] lie in the trie (a nested dict ending in None)?"""
    if trie is None:
        return True
    if not trie:
        return False
    if pos >= len(toks):
        return "KIND:EOF" in trie and matches(trie["KIND:EOF"], toks, pos + 1)
    for k in keys(toks[pos]) + ["$ANY"]:
        sub = trie.get(k, False)
        if sub is not False and matches(sub, toks, pos + 1):
            return True
    return False


class Parser:
    def __init__(self, toks):
        self.toks = toks
        self.pos = 0
        self.fallbacks = 0          # undocumented conflicts resolved by backtracking (must be 0)
        self.ordered = 0            # documented ordered-choice resolutions (policy P3)
        self.stack = []
        self.furthest = 0
        self.expected = set()

    def attach(self, node):
        if self.stack:
            self.stack[-1][1].append((node[0], node[1]))
        return (node[0], node[1])

    def tok(self):
        return self.toks[self.pos] if self.pos < len(self.toks) else self.toks[-1]

    def fail(self, expected):
        t = self.tok()
        if self.pos > self.furthest:
            self.furthest, self.expected = self.pos, set(expected)
        elif self.pos == self.furthest:
            self.expected |= set(expected)
        raise ParseError(t, expected)

    def lit(self, text):
        t = self.tok()
        if t.kind in ("STR", "RAW", "BYTES", "EOF", "REF_OPEN", "REF_CLOSE") or t.text != text:
            self.fail({{repr(text)}})
        self.pos += 1
        if self.stack:
            self.stack[-1][1].append(t)
        return t

    def kind(self, k, desc):
        t = self.tok()
        if t.kind != k:
            self.fail({{desc}})
        if desc == "TEMPLATE" and not all(_ref._PLACEHOLDER.match(src) and src.strip() not in _ref.KEYWORDS for src, _, _ in t.interps):
            self.fail({{"TEMPLATE (placeholders name payload fields)"}})
        self.pos += 1
        if self.stack:
            self.stack[-1][1].append(t)
        return t

    def head_body(self):
        rec = _ref.Recogniser(_ref.ebnf.load(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "reference", "grammar", "fortis.ebnf"))[0])
        rec.toks = self.toks
        ends = rec._head_body(self.pos)
        if not ends:
            self.fail({{"balanced head body"}})
        end = max(ends)
        if self.stack:
            self.stack[-1][1].append(("headBody", list(self.toks[self.pos:end])))   # the same node the recogniser derives
        self.pos = end

    def choose(self, tries, exclusive, documented=False):
        """Ordered choice over (table, thunk) pairs; exclusive=True means the tables are disjoint (LL(k))."""
        cands = [(i, th) for i, (tb, th) in enumerate(tries) if matches(tb, self.toks, self.pos)]
        if not cands:
            self.fail({{list(tb)[0] if isinstance(tb, dict) and tb else "?" for tb, _ in tries}})
        if exclusive or len(cands) == 1:
            return cands[0][1]()
        if documented:
            self.ordered += 1
        else:
            self.fallbacks += 1
        save, depth = self.pos, len(self.stack)
        kept = len(self.stack[-1][1]) if self.stack else 0
        last = None
        for i, th in cands:
            try:
                return th()
            except ParseError as e:
                last = e
                self.pos = save
                del self.stack[depth:]               # rules that raised left their nodes on the stack
                if self.stack:
                    del self.stack[-1][1][kept:]
        raise last

    def parse(self, start="module"):
        node = [start, []]
        self.stack.append(node)
        try:
            getattr(self, "r_" + start)()
            self.stack.pop()
            while self.tok().kind == "NL":         # a fragment parsed from a non-module start may end with the file's final NL
                self.pos += 1
            if self.tok().kind != "EOF":
                self.fail({{"end of input"}})
        except ParseError:
            raise ParseError(self.toks[min(self.furthest, len(self.toks) - 1)], self.expected)    # the furthest failure, not the last branch tried
        return node[1][0]


{tables}


class FortisParser(Parser):
'''
        return header + "\n".join(body_lines)

    def trie(self, s: Set[Tup]):
        root: dict = {}
        for t in s:
            d = root
            for i, x in enumerate(t):
                if i == len(t) - 1:
                    d[x] = None
                else:
                    d = d.setdefault(x, {})
        return root

    def line(self, s: str):
        self.cur.append("    " * self.indent + s)

    def table(self, s: Set[Tup]) -> str:
        self.tables.append(s)
        return f"T{len(self.tables) - 1}"

    def gen(self, node, tails: Set[Tup]):
        if isinstance(node, Lit):
            self.line(f"self.lit({node.text!r})")
        elif isinstance(node, Special):
            self.line(f"self.kind({node.text!r}, {('<' if node.text == 'REF_OPEN' else '>')!r})")
        elif isinstance(node, Ref):
            if node.name == "headBody":
                self.line("self.head_body()")
            elif node.name in KIND_RULES:
                self.line(f"self.kind({KIND_RULES[node.name]!r}, {node.name!r})")
            else:
                self.line(f"self.r_{node.name}()")
        elif isinstance(node, Seq):
            t = tails
            tails_after = []
            for item in reversed(node.items):
                tails_after.append(t)
                t = self.firstk(item, t)
            tails_after.reverse()
            for item, tl in zip(node.items, tails_after):
                self.gen(item, tl)
        elif isinstance(node, Opt):
            enter = self.firstk(node.body, tails)
            sets, conflict = self.decide(self.rule_name, [enter, set(tails)], "option")
            self.line(f"if matches({self.table(enter)}, self.toks, self.pos):{'' if not conflict else '  # conflict: greedy (documented policy P2)'}")
            self.indent += 1
            self.gen(node.body, tails)
            self.indent -= 1
        elif isinstance(node, Rep):
            again = self.firstk(node.body, self.firstk(node, tails))
            sets, conflict = self.decide(self.rule_name, [again, set(tails)], "repetition")
            self.line(f"while matches({self.table(again)}, self.toks, self.pos):{'' if not conflict else '  # conflict: greedy (documented policy P2)'}")
            self.indent += 1
            self.line("_p = self.pos")
            self.gen(node.body, self.firstk(node, tails))
            self.line("if self.pos == _p: break")
            self.indent -= 1
        elif isinstance(node, Alt):
            sets = [self.firstk(o, tails) for o in node.options]
            sets, conflict = self.decide(self.rule_name, sets, "alternative")
            names = []
            for i, o in enumerate(node.options):
                fn = f"_alt{len(self.tables)}_{i}"
                names.append((self.table(sets[i]), fn))
                self.line(f"def {fn}():")
                self.indent += 1
                before = len(self.cur)
                self.gen(o, tails)
                if len(self.cur) == before:
                    self.line("pass")
                self.indent -= 1
            documented = bool(conflict and POLICIES.get((self.rule_name, "alternative")))
            self.line(f"self.choose([{', '.join(f'({t}, {fn})' for t, fn in names)}], {not conflict}, {documented})")
        else:
            raise AssertionError(node)


    # ---- Rust emission (DEL-31: the production frontend's syntax crate) ----------------------
    def emit_rust(self, start: str) -> str:
        """The same decisions as `emit`, as a Rust module for `compiler/crates/fortis-syntax`: one method per rule and
        per alternative branch, tables as a static trie arena keyed by interned terminal classes."""
        self.tables = []
        self.conflicts = []
        self.decisions = 0
        self.compute_first()
        self.compute_follow(start)
        self.rust_methods: List[Tuple[str, List[str]]] = []
        self.alt_counter = 0
        rule_names = [r for r in self.rules if not self.is_terminal_rule(r)]
        self.rule_ids = {r: i for i, r in enumerate(rule_names)}
        for r in rule_names:
            self.cur = []
            self.indent = 2
            self.rule_name = r
            self.gen_rust(self.rules[r], self.expand_follow(r))
            body = [f"        self.enter({self.rule_ids[r]});"] + self.cur + ["        self.leave();", "        Ok(())"]
            self.rust_methods.append((f"r_{r}", body))
        # intern keys and lay the tries out as an arena
        keys: Dict[str, int] = {}
        nodes: List[Tuple[int, int]] = []
        edges: List[Tuple[int, int]] = []
        def key(k: str) -> int:
            if k not in keys:
                keys[k] = len(keys)
            return keys[k]
        shared: Dict[Tuple, int] = {}
        def lay(trie: dict) -> int:
            """Bottom-up, with identical subtries shared (hash-consing): the arena is a DAG."""
            items = tuple(sorted((key(k), 0xFFFFFFFF if sub is None else lay(sub)) for k, sub in trie.items()))
            if items not in shared:
                shared[items] = len(nodes)
                nodes.append((len(edges), len(items)))
                edges.extend(items)
            return shared[items]
        key("KIND:EOF"); key("$ANY")
        roots = [lay(self.trie(t)) for t in self.tables]
        g_sha = hashlib.sha256(open(GRAMMAR, "rb").read()).hexdigest()
        # the grammar itself, for the recogniser that locates syntax errors (conformance/tools/parser.py)
        garena: List[str] = []
        def gnode(node) -> int:
            if isinstance(node, Lit):
                garena.append(f"G::Lit({json.dumps(node.text)})")
            elif isinstance(node, Special):
                garena.append(f"G::Special({json.dumps(node.text)})")
            elif isinstance(node, Ref):
                garena.append(f"G::Ref({json.dumps(node.name)})")
            elif isinstance(node, Opt):
                c = gnode(node.body)
                garena.append(f"G::Opt({c})")
            elif isinstance(node, Rep):
                c = gnode(node.body)
                garena.append(f"G::Rep({c})")
            elif isinstance(node, Seq):
                cs = [gnode(x) for x in node.items]
                garena.append("G::Seq(&[" + ", ".join(map(str, cs)) + "])")
            elif isinstance(node, Alt):
                cs = [gnode(x) for x in node.options]
                garena.append("G::Alt(&[" + ", ".join(map(str, cs)) + "])")
            else:
                raise AssertionError(node)
            return len(garena) - 1
        bodies = [(r, gnode(body)) for r, body in self.rules.items()]
        out = [f"// GENERATED by conformance/tools/gen_parser.py --rust from reference/grammar/fortis.ebnf",
               f"// (sha256 {g_sha}); do not edit. Lookahead k = {self.k}; decisions {self.decisions}; conflicts {len(self.conflicts)}",
               "// (resolved by ordered choice with local backtracking, counted in `fallbacks` / `ordered`).",
               "// Every decision consults only token classes (literal text or lexer kind), never a name.",
               "#![allow(non_snake_case, clippy::all)]",
               "use crate::grammar::G;",
               "use crate::runtime::{AltFn, PResult, Parser};",
               "",
               f"pub const K: usize = {self.k};",
               f"pub const GRAMMAR_SHA256: &str = \"{g_sha}\";",
               "pub static RULE_NAMES: &[&str] = &[" + ", ".join(json.dumps(r) for r in rule_names) + "];",
               "pub static KEYS: &[&str] = &[" + ", ".join(json.dumps(k) for k in keys) + "];",
               "pub static TRIE_NODES: &[(u32, u32)] = &[" + ", ".join(f"({a}, {b})" for a, b in nodes) + "];",
               "pub static TRIE_EDGES: &[(u16, u32)] = &[" + ", ".join(f"({a}, {b})" for a, b in edges) + "];",
               "pub static TABLES: &[u32] = &[" + ", ".join(str(r) for r in roots) + "];",
               "pub static GRAMMAR: &[G] = &[" + ", ".join(garena) + "];",
               "pub static RULE_BODIES: &[(&str, u32)] = &[" + ", ".join(f"({json.dumps(r)}, {i})" for r, i in bodies) + "];",
               "",
               "pub fn rule_id(name: &str) -> Option<u16> {",
               "    RULE_NAMES.iter().position(|r| *r == name).map(|i| i as u16)",
               "}",
               "",
               "pub fn dispatch(p: &mut Parser, rule: u16) -> PResult {",
               "    match rule {"]
        out += [f"        {self.rule_ids[r]} => p.r_{r}()," for r in rule_names]
        out += ["        _ => unreachable!(\"no such rule\"),", "    }", "}", "", "impl<'a> Parser<'a> {"]
        for name, body in self.rust_methods:
            out.append(f"    pub fn {name}(&mut self) -> PResult {{")
            out += body
            out.append("    }")
        out += ["}", ""]
        return "\n".join(out)

    def rline(self, s: str):
        self.cur.append("    " * self.indent + s)

    def gen_rust(self, node, tails: Set[Tup]):
        if isinstance(node, Lit):
            self.rline(f"self.lit({json.dumps(node.text)})?;")
        elif isinstance(node, Special):
            self.rline(f"self.kind({json.dumps(node.text)}, {json.dumps('<' if node.text == 'REF_OPEN' else '>')})?;")
        elif isinstance(node, Ref):
            if node.name == "headBody":
                self.rline("self.head_body()?;")
            elif node.name in KIND_RULES:
                self.rline(f"self.kind({json.dumps(KIND_RULES[node.name])}, {json.dumps(node.name)})?;")
            else:
                self.rline(f"self.r_{node.name}()?;")
        elif isinstance(node, Seq):
            t = tails
            tails_after = []
            for item in reversed(node.items):
                tails_after.append(t)
                t = self.firstk(item, t)
            tails_after.reverse()
            for item, tl in zip(node.items, tails_after):
                self.gen_rust(item, tl)
        elif isinstance(node, Opt):
            enter = self.firstk(node.body, tails)
            self.decide(self.rule_name, [enter, set(tails)], "option")
            self.rline(f"if self.matches({self.table(enter)[1:]}) {{")
            self.indent += 1
            self.gen_rust(node.body, tails)
            self.indent -= 1
            self.rline("}")
        elif isinstance(node, Rep):
            again = self.firstk(node.body, self.firstk(node, tails))
            self.decide(self.rule_name, [again, set(tails)], "repetition")
            self.rline(f"while self.matches({self.table(again)[1:]}) {{")
            self.indent += 1
            self.rline("let p0 = self.pos;")
            self.gen_rust(node.body, self.firstk(node, tails))
            self.rline("if self.pos == p0 { break; }")
            self.indent -= 1
            self.rline("}")
        elif isinstance(node, Alt):
            sets = [self.firstk(o, tails) for o in node.options]
            sets, conflict = self.decide(self.rule_name, sets, "alternative")
            self.alt_counter += 1
            n = self.alt_counter
            names = []
            for i, o in enumerate(node.options):
                fn = f"alt{n}_{i}"
                names.append((self.table(sets[i]), fn))
                saved_cur, saved_indent = self.cur, self.indent
                self.cur, self.indent = [], 2
                self.gen_rust(o, tails)
                self.rust_methods.append((fn, self.cur + ["        Ok(())"]))
                self.cur, self.indent = saved_cur, saved_indent
            documented = bool(conflict and POLICIES.get((self.rule_name, "alternative")))
            branches = ", ".join(f"({t[1:]}, Self::{fn} as AltFn<'a>)" for t, fn in names)
            self.rline(f"self.choose(&[{branches}], {str(not conflict).lower()}, {str(documented).lower()})?;")
        else:
            raise AssertionError(node)

def main(argv):
    kmax = int(argv[argv.index("--kmax") + 1]) if "--kmax" in argv else 3
    out = argv[argv.index("--out") + 1] if "--out" in argv else OUT
    rules, start = ebnf.load(GRAMMAR)
    start = "module"
    g = Generator(rules, kmax)
    report = {"grammar_sha256": hashlib.sha256(open(GRAMMAR, "rb").read()).hexdigest(), "kmax": kmax, "runs": []}
    chosen = None
    for k in range(1, kmax + 1):
        g.k = k
        src = g.emit(start)
        undocumented = [c for c in g.conflicts if not c["policy"]]
        report["runs"].append({"k": k, "decisions": g.decisions, "conflicts": len(g.conflicts), "undocumented": len(undocumented), "conflict_list": g.conflicts})
        print(f"k={k}: {g.decisions} decisions, {len(g.conflicts)} conflicts ({len(undocumented)} without a documented policy)")
        if chosen is None and not undocumented:
            chosen = (k, src)
    if chosen is None:
        g.k = kmax
        src = g.emit(start)
        chosen = (kmax, src)
    k, src = chosen
    report["lookahead"] = k
    report["confirmed"] = report["runs"][k - 1]["undocumented"] == 0
    os.makedirs(os.path.dirname(out), exist_ok=True)
    open(out, "w", encoding="utf-8").write(src)
    if "--rust" in argv:
        rust_out = argv[argv.index("--rust") + 1]
        g.k = k
        open(rust_out, "w", encoding="utf-8").write(g.emit_rust(start))
        print(f"emitted {os.path.relpath(rust_out, ROOT)} (Rust) with k={k}")
    json.dump(report, open(os.path.join(os.path.dirname(out), "lookahead.json"), "w", encoding="utf-8"), indent=1)
    print(f"emitted {os.path.relpath(out, ROOT)} with k={k} ({report['runs'][k - 1]['conflicts']} conflicts, {report['runs'][k - 1]['undocumented']} undocumented)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
