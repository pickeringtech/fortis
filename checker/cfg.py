"""Typed control-flow graph of a function body (DEL-12: Language Reference 5.1–5.4, 7.2 places).

`build(func_node)` turns a block-bodied function into basic blocks joined by labelled edges;
`dump(cfg)` renders the stable text form kept as golden files under conformance/cfg/golden, and
`parse(text)` reads that form back so a golden round-trips (dump(parse(dump(g))) == dump(g)).
Each block lists its statements in canonical token form and the places it reads or assigns."""
from __future__ import annotations

import os
import sys
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
from lexer import Token  # noqa: E402
from tree import children, name, direct_tokens, find, find_all, tokens  # noqa: E402


@dataclass
class Block:
    id: int
    stmts: List[str] = field(default_factory=list)
    places: List[str] = field(default_factory=list)
    edges: List[Tuple[str, int]] = field(default_factory=list)      # (label, target)


@dataclass
class CFG:
    name: str
    blocks: List[Block]
    entry: int = 0
    exit: int = -1


def canon(node) -> str:
    out = []
    for t in tokens(node):
        if t.kind == "NL":
            continue
        out.append(t.text)
    s = " ".join(out)
    for a, b in ((" ( ", "("), ("( ", "("), (" )", ")"), (" ,", ","), (" .", "."), (". ", "."), (" [", "["), ("[ ", "["), (" ]", "]"), ("{ ", "{"), (" }", "}")):
        s = s.replace(a, b)
    return s


def places_of(node) -> List[str]:
    out: List[str] = []
    for pl in find_all(node, "place"):
        p = canon(pl)
        if p not in out:
            out.append(p)
    for pe in find_all(node, "postfixExpr"):
        kids = children(pe)
        pk = children(kids[0])
        if len(pk) == 1 and isinstance(pk[0], Token) and pk[0].kind in ("LOWER", "KW") and pk[0].text not in ("null", "true", "false"):
            path = pk[0].text
            for pf in kids[1:]:
                k = children(pf)
                if k and isinstance(k[0], Token) and k[0].text == "." and len(k) > 1 and name(k[1]) == "memberName":
                    path += "." + tokens(k[1])[0].text
                else:
                    break
            if path not in out:
                out.append(path)
    return out


class Builder:
    def __init__(self, fname: str):
        self.cfg = CFG(fname, [])
        self.loops: List[Tuple[Optional[str], int, int]] = []       # (label, continue target, break target)

    def new(self) -> Block:
        b = Block(len(self.cfg.blocks))
        self.cfg.blocks.append(b)
        return b

    def edge(self, src: Block, label: str, dst: Block):
        src.edges.append((label, dst.id))

    def build(self, body_block) -> CFG:
        entry = self.new()
        exit_ = self.new()
        self.cfg.exit = exit_.id
        end = self.block(body_block, entry, exit_)
        if end is not None:
            self.edge(end, "fallthrough", exit_)
        return self.cfg

    def block(self, blk, cur: Block, exit_: Block) -> Optional[Block]:
        """Lay out a block's statements starting in `cur`; return the open block at the end, or None if closed."""
        for st in [c for c in children(blk) if name(c) == "statement"]:
            if cur is None:
                cur = self.new()               # unreachable code still gets a block
            cur = self.statement(st, cur, exit_)
        return cur

    def statement(self, st, cur: Block, exit_: Block) -> Optional[Block]:
        inner = children(st)[0]
        n = name(inner)
        if n is None:
            w = direct_tokens(st)[0].text
            cur.stmts.append(canon(st))
            cur.places += [p for p in places_of(st) if p not in cur.places]
            if w in ("return", "fail"):
                self.edge(cur, w, self.cfg.blocks[exit_.id])
                return None
            if w in ("give", "recover"):
                return cur
            if w in ("break", "continue"):
                ts = direct_tokens(st)
                label = ts[1].text if len(ts) > 1 else None
                for lab, cont, brk in reversed(self.loops):
                    if label is None or lab == label:
                        self.edge(cur, w, self.cfg.blocks[brk if w == "break" else cont])
                        return None
                return None
            return cur
        if n == "ifStmt":
            conds = [c for c in children(inner) if name(c) == "expr"]
            blocks = [c for c in children(inner) if name(c) == "block"]
            has_else = len(blocks) > len(conds)
            join = self.new()
            test = cur
            for i, (cond, blk) in enumerate(zip(conds, blocks)):
                test.stmts.append("if " + canon(cond))
                test.places += [p for p in places_of(cond) if p not in test.places]
                then = self.new()
                self.edge(test, "true", then)
                end = self.block(blk, then, exit_)
                if end is not None:
                    self.edge(end, "next", join)
                if i + 1 < len(conds):
                    nxt = self.new()
                    self.edge(test, "false", nxt)
                    test = nxt
                elif has_else:
                    els = self.new()
                    self.edge(test, "false", els)
                    end = self.block(blocks[-1], els, exit_)
                    if end is not None:
                        self.edge(end, "next", join)
                else:
                    self.edge(test, "false", join)
            return join
        if n in ("whileStmt", "loopStmt", "forStmt"):
            lab = find(inner, "label")
            label = tokens(lab)[0].text if lab is not None else None
            head = self.new()
            self.edge(cur, "loop", head)
            body = self.new()
            after = self.new()
            if n == "whileStmt":
                cond = find(inner, "expr")
                head.stmts.append("while " + canon(cond))
                head.places += places_of(cond)
                self.edge(head, "true", body)
                self.edge(head, "false", after)
            elif n == "forStmt":
                ts = direct_tokens(inner)
                var = ts[[t.text for t in ts].index("for") + 1].text
                src = next(c for c in children(inner) if name(c) in ("expr", "rangeExpr", "place"))
                head.stmts.append(f"for {var} in {canon(src)}")
                head.places += places_of(src)
                self.edge(head, "next", body)
                self.edge(head, "done", after)
            else:
                head.stmts.append("loop")
                self.edge(head, "again", body)
            self.loops.append((label, head.id, after.id))
            end = self.block(find(inner, "block"), body, exit_)
            self.loops.pop()
            if end is not None:
                self.edge(end, "back", head)
            return after
        if n == "block":
            inner_b = self.new()
            self.edge(cur, "enter", inner_b)
            end = self.block(inner, inner_b, exit_)
            if end is None:
                return None
            out = self.new()
            self.edge(end, "leave", out)
            return out
        if n == "exprStmt" and tokens(inner) and tokens(inner)[0].text == "match":
            m = find(inner, "matchExpr")
            scrut = next(c for c in children(m) if name(c) in ("expr", "place"))
            cur.stmts.append("match " + canon(scrut))
            cur.places += [p for p in places_of(scrut) if p not in cur.places]
            join = self.new()
            any_open = False
            for arm in [_a for _a in children(m) if name(_a) == "matchArm"]:
                pats = [c for c in children(arm) if name(c) == "pattern"]       # the alternatives of `A | B` (9.8)
                ab = self.new()
                self.edge(cur, " | ".join(canon(p) for p in pats), ab)
                body = [c for c in children(arm) if name(c) in ("expr", "block")][-1]
                if name(body) == "block":
                    end = self.block(body, ab, exit_)
                else:
                    ab.stmts.append(canon(body))
                    ab.places += places_of(body)
                    end = None if tokens(body)[0].text == "fail" else ab
                    if end is None:
                        self.edge(ab, "fail", exit_)
                if end is not None:
                    self.edge(end, "next", join)
                    any_open = True
            return join if any_open else None
        cur.stmts.append(canon(inner))
        cur.places += [p for p in places_of(inner) if p not in cur.places]
        return cur


def build(func_node, fname: str) -> Optional[CFG]:
    body = find(func_node, "funcBody")
    blk = find(body, "block") if body is not None else None
    if blk is None:
        return None
    return Builder(fname).build(blk)


def dump(cfg: CFG) -> str:
    lines = [f"cfg {cfg.name} entry={cfg.entry} exit={cfg.exit}"]
    for b in cfg.blocks:
        lines.append(f"B{b.id}:")
        for s in b.stmts:
            lines.append(f"  stmt {s}")
        if b.places:
            lines.append("  places " + ", ".join(b.places))
        for label, dst in b.edges:
            lines.append(f"  -> B{dst} [{label}]")
    return "\n".join(lines) + "\n"


def parse(text: str) -> CFG:
    lines = text.rstrip("\n").split("\n")
    head = lines[0].split()
    cfg = CFG(head[1], [], int(head[2].split("=")[1]), int(head[3].split("=")[1]))
    cur: Optional[Block] = None
    for ln in lines[1:]:
        if ln.startswith("B") and ln.endswith(":"):
            cur = Block(int(ln[1:-1]))
            cfg.blocks.append(cur)
        elif ln.startswith("  stmt "):
            cur.stmts.append(ln[7:])
        elif ln.startswith("  places "):
            cur.places = ln[9:].split(", ")
        elif ln.startswith("  -> B"):
            rest = ln[6:]
            dst, label = rest.split(" [", 1)
            cur.edges.append((label[:-1], int(dst)))
    return cfg
