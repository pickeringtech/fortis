"""Experimental lowering of the synchronous core to safe Rust (DEL-25; Language Reference 7.10, Appendix C).

`python3 -I compiler/experimental-lowering/lower.py FILE.ft OUT_DIR` checks the module with the reference
checker (a rejected program is never lowered), then writes a Cargo crate under OUT_DIR:
`Cargo.toml`, `src/main.rs` (or `src/lib.rs` when the module has no `main`) carrying
`#![forbid(unsafe_code)]`, depending on the audited runtime crate `fortis_rt`.

Every copy or reference-count operation the lowering emits carries an attribution comment
(`/* copy: … */`, `/* reference count: … */`) naming its source construct or lowering rule, so the copy
audit (`audit.py`) can show that none is a repair copy. The lowering is thin: Rust's own type and borrow
checker is the second check on every loan, move and closure shape (C.1 "Rust-checked" rows)."""
from __future__ import annotations

import os
import re
import sys
from typing import Any, Dict, List, Optional, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
import lexer  # noqa: E402
import names  # noqa: E402
import parser as fparser  # noqa: E402
import tree as tree_mod  # noqa: E402
from lexer import Token  # noqa: E402
from tree import children, direct_tokens, find, find_all, name, tokens  # noqa: E402
from checker import check  # noqa: E402
from checker.types import (DynamicT, FnT, LoanT, Named, OptionalT, Refined, TupleT, Type, TypeParam, Unknown)  # noqa: E402

TRIVIAL = {"Int", "Bool", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float", "Float32", "Scalar", "Duration", "Must"}
EXPR_CHAIN = ("expr", "constExpr", "outcomeExpr", "coalesceExpr", "orExpr", "andExpr", "bitOrExpr", "bitXorExpr", "bitAndExpr",
              "equalityExpr", "relationalExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr", "asExpr", "unaryExpr", "exprStmt", "literal")


class LowerError(Exception):
    pass


class Binding:
    __slots__ = ("mode", "type", "mutable", "closure")

    def __init__(self, mode: str, t: Type, mutable: bool = False, closure: Optional[dict] = None):
        self.mode, self.type, self.mutable, self.closure = mode, t, mutable, closure   # mode: owned | ref | mutref


class Env:
    def __init__(self, parent: Optional["Env"] = None):
        self.parent, self.vars = parent, {}

    def lookup(self, n: str) -> Optional[Binding]:
        e = self
        while e is not None:
            if n in e.vars:
                return e.vars[n]
            e = e.parent
        return None

    def set(self, n: str, b: Binding):
        self.vars[n] = b


def strip(t: Type) -> Type:
    return t.inner if isinstance(t, LoanT) else t


def base(t: Type) -> Type:
    t = strip(t)
    while isinstance(t, Refined):
        t = t.base
    return t


def tname(t: Type) -> str:
    b = base(t)
    return b.name if isinstance(b, Named) else ""


TRIVIAL_NEWTYPES: set = set()


def is_trivial(t: Type) -> bool:
    b = base(t)
    return isinstance(b, Named) and (b.name in TRIVIAL or b.name in TRIVIAL_NEWTYPES) and not b.args


def rust_ident(n: str) -> str:
    return n + "_" if n in ("type", "match", "move", "ref", "self", "fn", "mod", "loop", "where", "in", "as", "use", "box", "impl", "trait", "struct", "enum", "let", "mut", "pub", "crate", "super", "dyn", "unsafe", "static", "const", "true", "false") else n


class Lowering:
    def __init__(self, src: str, path: str):
        self.src, self.path = src, path
        self.rec = fparser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
        self.lib = names.Library(self.rec)
        self.mtree = self.rec.parse(lexer.lex(src), "module")
        check.RECORD_TYPES = True
        self.records = [r for r in check.check_tree(self.lib, self.mtree, path) if r.get("code")]
        self.ty = check.LAST_TYPER
        self.dc = check.LAST_DC
        self.table = self.dc.table
        self.types: Dict[int, Type] = self.ty.node_types or {}
        self.out: List[str] = []
        self.label_counter = 0
        self.lifetime_classes = {d.name: d.lifetime for d in self.table.decls.values() if d.kind == "class" and d.lifetime}
        self.ret_try = False
        self.ret_type: Optional[Type] = None
        self.fn_name = "module"
        self.errors = {d.name: d for d in self.table.decls.values() if d.kind == "error" and d.module == self.dc.module_name}
        self.has_main = "main" in self.table.decls and self.table.decls["main"].kind == "func"
        for d in self.table.decls.values():
            if d.kind == "type" and d.alias is not None and is_trivial(d.alias):
                TRIVIAL_NEWTYPES.add(d.name)                 # a newtype over a trivial representation is Copy (6.9)
        self.copies: List[str] = []
        self.loop_labels: List[Optional[str]] = []

    # ---- entry -----------------------------------------------------------------------------
    def crate(self) -> Dict[str, str]:
        if self.records:
            raise LowerError("the checker rejected the program: " + ", ".join(sorted({r["code"] for r in self.records})))
        self.emit("#![forbid(unsafe_code)]")
        self.emit("#![allow(dead_code, non_snake_case, non_camel_case_types, unused_imports, unused_variables, unused_mut, unused_parens, unreachable_code, unused_labels, unused_braces, irrefutable_let_patterns, clippy::all)]")
        self.emit("use fortis_rt::*;")
        self.emit("use fortis_rt::int;")
        self.emit("use std::rc::Rc;")
        self.emit("#[global_allocator]")
        self.emit("static ALLOC: fortis_rt::probe::Counting = fortis_rt::probe::Counting;")
        for d in tree_mod.declarations(self.mtree):
            self.lower_decl(d)
        if self.has_main:
            self.emit("fn main() {")
            self.emit("    let args: Vec<String> = std::env::args().skip(1).collect();")
            msig = self.table.decls["main"].sig
            call = "fortis_main(&args)"
            if "try" not in msig.modifiers:
                call = f"{{ {call}; Ok(()) }}"
            self.emit(f"    let code = fortis_rt::run_main({call});")
            self.emit("    std::process::exit(code);")
            self.emit("}")
        body = "\n".join(self.out) + "\n"
        return {"src/main.rs" if self.has_main else "src/lib.rs": body}

    def emit(self, line: str):
        self.out.append(line)

    # ---- types -------------------------------------------------------------------------------
    def rty(self, t: Type, lifetime: str = "'_", param_mode: str = "") -> str:
        """The Rust type of a Fortis type in a given position."""
        if isinstance(t, LoanT):
            if tname(t.inner) in ("StringView", "ListView"):
                return self.rty(t.inner, lifetime)            # a view is already a reference (6.6)
            if tname(t.inner) in self.lifetime_classes:
                return self.rty(t.inner, lifetime)            # a lifetime-class value `from` a parameter is owned by its holder (7.7)
            inner = self.rty(t.inner, lifetime)
            mut = "mut " if t.mode == "editable" else ""
            return f"&{lifetime} {mut}{inner}"
        if isinstance(t, Refined):
            return self.rty(t.base, lifetime)
        if isinstance(t, OptionalT):
            return f"Option<{self.rty(t.inner, lifetime)}>"
        if isinstance(t, TupleT):
            return "(" + ", ".join(self.rty(x, lifetime) for x in t.items) + ",)" if len(t.items) == 1 else "(" + ", ".join(self.rty(x, lifetime) for x in t.items) + ")"
        if isinstance(t, FnT):
            kind = {"fn": "Fn", "fn mut": "FnMut", "fn once": "FnOnce"}[t.kind]
            bound = f" + {lifetime}" if lifetime != "'_" else ""
            return f"impl {kind}({', '.join(self.rty(p, lifetime) for p in t.params)}) -> {self.rty(t.ret, lifetime)}{bound}"
        if isinstance(t, TypeParam):
            return t.name
        if isinstance(t, Named):
            n, args = t.name, t.args
            if n in ("Int", "Int64"): return "i64"
            if n == "Bool": return "bool"
            if n == "String": return "String"
            if n == "StringView": return f"&{lifetime} str"
            if n == "Void": return "()"
            if n == "Never": return "!"
            if n == "List": return f"Vec<{self.rty(args[0], lifetime)}>"
            if n == "ListView": return f"&{lifetime} Vec<{self.rty(args[0], lifetime)}>"
            if n == "Outcome": return f"Outcome<{self.rty(args[0], lifetime)}>"
            if n == "Shared": return f"Rc<{self.rty(args[0], lifetime)}>"
            if n == "Weak": return f"std::rc::Weak<{self.rty(args[0], lifetime)}>"
            if n in ("Failure", "Report", "Must", "Console", "FileSystem", "Path", "Clock", "Network", "Random", "Environment", "Process", "Supervisor", "SecretAccess", "RawSql", "Capabilities", "Builder"):
                return n
            if n in self.lifetime_classes:
                return f"{rust_type(n)}<{lifetime}>"
            if args:
                return f"{rust_type(n)}<{', '.join(self.rty(a, lifetime) for a in args)}>"
            return rust_type(n)
        raise LowerError(f"no Rust type for {t.show() if hasattr(t, 'show') else t}")

    def param_rust(self, pname: str, pt: Type, mode: str, lifetime: str, generics: List[str]) -> Tuple[str, str]:
        """(declaration text, binding mode) for a parameter."""
        mode = mode.lstrip(".")
        s = strip(pt)
        if isinstance(s, FnT):
            g = f"F{len(generics)}"
            kind = {"fn": "Fn", "fn mut": "FnMut", "fn once": "FnOnce"}[s.kind]
            generics.append(f"{g}: {kind}({', '.join(self.rty(p, lifetime) for p in s.params)}) -> {self.rty(s.ret, lifetime)}")
            if mode == "editable":
                return f"{rust_ident(pname)}: &mut {g}", "mutref"
            return f"{rust_ident(pname)}: {g}", "owned"
        if isinstance(pt, LoanT):
            return f"{rust_ident(pname)}: {self.rty(pt, lifetime)}", "mutref" if pt.mode == "editable" else "ref"
        if mode == "editable":
            return f"{rust_ident(pname)}: &{lifetime} mut {self.rty(pt, lifetime)}", "mutref"
        if mode == "owned" or is_trivial(pt) or tname(pt) in ("StringView", "ListView"):
            return f"{rust_ident(pname)}: {self.rty(pt, lifetime)}", "owned" if tname(pt) != "ListView" else "ref"
        return f"{rust_ident(pname)}: &{lifetime} {self.rty(pt, lifetime)}", "ref"

    # ---- declarations ------------------------------------------------------------------------
    def lower_decl(self, d: dict):
        kind, nm, node = d["kind"], d["name"], d["node"]
        if kind == "func":
            self.lower_func(self.table.decls[nm].sig, nm, None, exported=d["exported"])
        elif kind == "class":
            self.lower_class(self.table.decls[nm])
        elif kind == "enum":
            self.lower_enum(self.table.decls[nm])
        elif kind == "type":
            decl = self.table.decls[nm]
            rep = decl.alias
            self.emit(f"#[derive(Clone, Copy, PartialEq, Debug)]\npub struct {nm}(pub {self.rty(rep)});")
            lo, hi = None, None
            if isinstance(rep, Refined) and rep.kind == "range":
                lo, hi = rep.lo, rep.hi
            self.emit(f"impl {nm} {{ pub fn validate(x: {self.rty(rep)}) -> Outcome<{nm}> {{ fortis_rt::validate_int(x, {self.opt(lo)}, {self.opt(hi)}, \"{nm}.validate\").map({nm}) }} }}")
            self.emit(f"impl fortis_rt::Show for {nm} {{ fn show(&self) -> String {{ self.0.show() }} }}")
            self.emit(f"impl PartialOrd for {nm} {{ fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {{ self.0.partial_cmp(&o.0) }} }}")
        elif kind == "error":
            pass                                             # errors lower to Failure::new(code, payload, origin) at the fail site
        elif kind in ("const",):
            decl = self.table.decls[nm]
            ce = find(node, "constExpr")
            self.emit(f"pub fn {rust_ident(nm)}() -> {self.rty(decl.alias)} {{ {self.expr(ce, Env())} }}")
        # interfaces, mixins, implements, contracts, aspects, observations, tests: no representation in this slice

    @staticmethod
    def opt(v) -> str:
        return "None" if v is None else f"Some({v})"

    def lower_class(self, decl):
        lt = "<'r>" if decl.lifetime else ""
        copyable = self.dc.is_copyable(Named(decl.name, (), decl.package))
        derives = ["Clone"] if copyable else []
        if any(i.name == "Equatable" for i in decl.implements) or any(x[0] == "Equatable" for x in decl.derives):
            derives.append("PartialEq")
        if derives:
            self.emit(f"#[derive({', '.join(derives)})]")
        self.emit(f"pub struct {rust_type(decl.name)}{lt} {{")
        for fname, (ft, exp, has_default) in decl.fields.items():
            flt = "'r" if decl.lifetime else "'_"
            self.emit(f"    pub {rust_ident(fname)}: {self.rty(ft, flt)},")
        self.emit("}")
        shown_fields = ", ".join("format!(\"" + f + ": {}\", self." + rust_ident(f) + ".show())" for f in decl.fields)
        self.emit("impl" + lt + " fortis_rt::Show for " + rust_type(decl.name) + lt + " { fn show(&self) -> String { format!(\"" + decl.name + "({})\", vec![" + shown_fields + "].join(\", \")) } }")
        self.emit(f"impl{lt} {rust_type(decl.name)}{lt} {{")
        for mname, sig in decl.methods.items():
            if sig.body is not None:
                self.lower_func(sig, mname, decl, exported=sig.exported)
        cl = find(decl.node, "cleanupDecl") if decl.node is not None else None
        if cl is not None:
            cenv = Env()
            cenv.set("self", Binding("mutref", Named(decl.name, (), decl.package)))
            self.lower_body_fn("cleanup", [], "&mut self", Named("Void"), False, find(cl, "block"), None, cenv, "cleanup", decl, lifetime="'r" if decl.lifetime else "'_")
        self.emit("}")
        if cl is not None:
            self.emit(f"impl{lt} Drop for {rust_type(decl.name)}{lt} {{ fn drop(&mut self) {{ self.cleanup(); }} }}")

    def lower_enum(self, decl):
        derives = ["Clone", "PartialEq"] if all(not p for p in decl.variants.values()) else ["Clone", "PartialEq"]
        self.emit(f"#[derive({', '.join(derives)})]")
        self.emit(f"pub enum {rust_type(decl.name)} {{")
        for vname, payload in decl.variants.items():
            if payload:
                self.emit(f"    {vname} {{ " + ", ".join(f"{rust_ident(f)}: {self.rty(ft)}" for f, ft in payload) + " },")
            else:
                self.emit(f"    {vname},")
        self.emit("}")
        arms = []
        for vname, payload in decl.variants.items():
            if payload:
                fields = ", ".join(rust_ident(f) for f, _ in payload)
                shown = " + \", \" + ".join("&format!(\"" + f + ": {}\", " + rust_ident(f) + ".show())" for f, _ in payload)
                arms.append(rust_type(decl.name) + "::" + vname + " { " + fields + " } => format!(\"" + vname + "({})\", String::new() + " + shown + ")")
            else:
                arms.append(f"{rust_type(decl.name)}::{vname} => \"{vname}\".to_string()")
        self.emit(f"impl fortis_rt::Show for {rust_type(decl.name)} {{ fn show(&self) -> String {{ match self {{ " + ", ".join(arms) + " } } }")
        self.emit(f"impl {rust_type(decl.name)} {{")
        for mname, sig in decl.methods.items():
            if sig.body is not None:
                self.lower_func(sig, mname, decl, exported=sig.exported)
        self.emit("}")

    def lower_func(self, sig, fname: str, owner, exported: bool):
        if sig.body is None:
            return
        lifetime = "'a"
        needs_lt = isinstance(sig.ret, LoanT) or (isinstance(strip(sig.ret), Named) and strip(sig.ret).name in ("StringView", "ListView")) or tname(sig.ret) in self.lifetime_classes or (isinstance(strip(sig.ret), FnT) and strip(sig.ret).origin)
        env = Env()
        generics: List[str] = []
        params = []
        receiver = None
        if sig.receiver is not None:
            receiver = {"self": "&self", "editable self": "&mut self", "owned self": "self"}[sig.receiver]
            if needs_lt and sig.receiver != "owned self":
                receiver = {"self": "&'a self", "editable self": "&'a mut self"}[sig.receiver]
            env.set("self", Binding({"self": "ref", "editable self": "mutref", "owned self": "owned"}[sig.receiver], Named(owner.name, (), owner.package)))
        for pn, pt, pm in sig.params:
            text, mode = self.param_rust(pn, pt, pm, lifetime if needs_lt else "'_", generics)
            params.append(text)
            env.set(pn, Binding(mode, pt, mutable=False))
        rname = "fortis_main" if fname == "main" and owner is None else rust_ident(fname)
        self.lower_body_fn(rname, params, receiver, sig.ret, "try" in sig.modifiers, sig.body, generics, env, fname, owner,
                           lifetime=lifetime if needs_lt else "'_", noalloc="noalloc" in sig.modifiers, pub=True)

    def lower_body_fn(self, rname, params, receiver, ret: Type, is_try: bool, body, generics, env: Env, fname: str, owner, lifetime="'_", noalloc=False, pub=False):
        lt_decl = []
        if lifetime == "'a":
            lt_decl.append("'a")
        gens = ", ".join(lt_decl + (generics or []))
        gens = f"<{gens}>" if gens else ""
        rt = self.rty(ret, lifetime) if ret is not None else "()"
        if is_try:
            rt = f"Outcome<{rt}>"
        ps = ([receiver] if receiver else []) + params
        indent = "    " if owner is not None else ""
        self.emit(f"{indent}{'pub ' if pub else ''}fn {rname}{gens}({', '.join(ps)}) -> {rt} {{")
        saved = (self.ret_try, self.ret_type, self.fn_name, self.loop_labels)
        self.ret_try, self.ret_type, self.fn_name, self.loop_labels = is_try, ret, fname, []
        inner_env = Env(env)
        blk = body if name(body) == "block" else (find(body, "block") if name(children(body)[0]) == "block" else None)
        if noalloc:
            self.emit(f"{indent}    fortis_rt::probe::noalloc(\"{fname}\", || {{")
        if blk is not None:
            lines = self.block_lines(blk, inner_env)
            for l in lines:
                self.emit(f"{indent}    {l}")
            if is_try and tname(ret) == "Void":
                self.emit(f"{indent}    Ok(())")
            elif not is_try and tname(ret) == "Void":
                pass
        else:
            e = find(body, "expr")
            code = self.expr(e, inner_env, want=ret)
            self.emit(f"{indent}    {self.wrap_ok(code, e)}")
        if noalloc:
            self.emit(f"{indent}    }})")
        self.emit(f"{indent}}}")
        self.ret_try, self.ret_type, self.fn_name, self.loop_labels = saved

    def wrap_ok(self, code: str, e) -> str:
        """A try function's result expression: `Ok(v)` unless it already is `return`/`fail`/`!`."""
        if not self.ret_try:
            return code
        t = self.types.get(id(e))
        if t is not None and tname(t) == "Never":
            return code
        return f"Ok({code})"

    # ---- blocks and statements -----------------------------------------------------------------
    def block_lines(self, blk, env: Env) -> List[str]:
        e = Env(env)
        lines: List[str] = []
        for st in [c for c in children(blk) if name(c) == "statement"]:
            lines.extend(self.statement(st, e))
        return lines

    def block_code(self, blk, env: Env, give_label: Optional[str] = None) -> str:
        """A block as a Rust block expression; `give` inside breaks out of the labelled block with its value."""
        lines = self.block_lines(blk, env)
        body = "\n".join("    " + l for l in lines)
        if give_label:
            return f"{give_label}: {{\n{body}\n}}"
        return "{\n" + body + "\n}"

    def new_label(self) -> str:
        self.label_counter += 1
        return f"'g{self.label_counter}"

    def statement(self, st, env: Env) -> List[str]:
        first = children(st)[0]
        n = name(first)
        if n is None:
            return self.bare_statement(st, env)
        if n == "letStmt":
            return self.let_stmt(first, env)
        if n == "assignStmt":
            return self.assign_stmt(first, env)
        if n == "ifStmt":
            return [self.if_stmt(first, env)]
        if n == "whileStmt":
            label = self.loop_label(first)
            cond = self.expr(find(first, "expr"), env)
            self.loop_labels.append(label)
            body = self.block_code(find(first, "block"), env)
            self.loop_labels.pop()
            return [f"{self.lbl(label)}while {cond} {body}"]
        if n == "loopStmt":
            label = self.loop_label(first)
            self.loop_labels.append(label)
            body = self.block_code(find(first, "block"), env)
            self.loop_labels.pop()
            return [f"{self.lbl(label)}loop {body}"]
        if n == "forStmt":
            return [self.for_stmt(first, env)]
        if n == "block":
            return [self.block_code(first, env)]
        if n == "exprStmt":
            e = children(first)[0]
            code = self.expr(e, env)
            return [f"{code};"]
        if n == "deferStmt":
            raise LowerError("defer is outside the synchronous slice")
        if n in ("scopeStmt", "captureStmt", "recordStmt", "expectStmt"):
            raise LowerError(f"{n} is outside the synchronous slice")
        raise LowerError(f"statement {n}")

    def loop_label(self, node) -> Optional[str]:
        lbl = find(node, "label")
        return direct_tokens(lbl)[0].text if lbl is not None else None

    @staticmethod
    def lbl(label: Optional[str]) -> str:
        return f"'{label}: " if label else ""

    def bare_statement(self, st, env: Env) -> List[str]:
        ts = direct_tokens(st)
        word = ts[0].text
        e = next((c for c in children(st) if name(c) == "expr"), None)
        if word == "return":
            if e is None:
                return ["return Ok(());" if self.ret_try else "return;"]
            code = self.expr(e, env, want=self.ret_type)
            return [f"return {self.wrap_ok(code, e)};"]
        if word == "give":
            return [f"break {self.give_label} {self.expr(e, env)};"]
        if word == "recover":
            rn = getattr(self, "recover_name", None)
            if rn:
                return [f"{{ let __v = {self.expr(e, env)}; {rn}.recover(); break {self.give_label} __v; }}"]
            return [f"break {self.give_label} {self.expr(e, env)};"]
        if word == "report":
            code = self.expr(e, env)
            if "as" in [t.text for t in ts]:
                nm = ts[[t.text for t in ts].index("as") + 1].text
                env.set(nm, Binding("owned", Named("Report")))
                return [f"let {rust_ident(nm)} = {code}.report(\"{self.fn_name}\");"]
            return [f"{code}.report(\"{self.fn_name}\");"]
        if word == "discard":
            return [f"let _ = {self.expr(e, env)};"]
        if word == "break":
            return [f"break '{ts[1].text};" if len(ts) > 1 else "break;"]
        if word == "continue":
            return [f"continue '{ts[1].text};" if len(ts) > 1 else "continue;"]
        if word == "fail":
            return [f"return Err({self.expr(e, env)});"]
        raise LowerError(f"statement {word}")

    def let_stmt(self, inner, env: Env) -> List[str]:
        ts = direct_tokens(inner)
        words = [t.text for t in ts]
        mutable = "mut" in words
        pat = next((c for c in children(inner) if name(c) == "pattern"), None)
        if pat is not None:
            pl = next((c for c in children(inner) if name(c) == "place"), None)
            src = self.place(pl, env, "move")
            pt = self.node_type(pl, env)
            code, _ = self.pattern(pat, pt, env, by_ref=False)
            return [f"let {code} = {src};"]
        nm = next(t for t in ts if t.kind == "LOWER")
        e = next((c for c in children(inner) if name(c) == "expr"), None)
        ann = next((c for c in children(inner) if name(c) == "type"), None)
        t = self.types.get(id(e))
        decl_t = self.dc.type_of(ann) if ann is not None else t
        mode = "owned"
        mk = self.marker_of(e)
        code = self.expr(e, env, want=decl_t)
        if mk == "alias":
            mode = "ref"
        elif mk == "editable":
            mode = "mutref"
        elif isinstance(t, LoanT):
            mode = "mutref" if t.mode == "editable" else "ref"
        elif t is not None and tname(t) in ("StringView", "ListView"):
            mode = "ref"
        lam = find(e, "lambda")
        closure = self.ty.closure_info.get(id(lam)) if lam is not None and self.is_direct_lambda(e) else None
        b = Binding(mode, decl_t if decl_t is not None else (t or Unknown()), mutable, closure)
        env.set(nm.text, b)
        tyann = ""
        if ann is not None and e is not None and not isinstance(strip(decl_t), FnT):
            tyann = f": {self.rty(decl_t)}"                 # an explicit Fortis annotation is carried to Rust
        return [f"let {'mut ' if mutable else ''}{rust_ident(nm.text)}{tyann} = {code};"]

    def needs_annotation(self, e) -> bool:
        txt = " ".join(t.text for t in tokens(e))
        return "List.default" in txt or txt == "null" or txt.startswith("List.of") is False and "default()" in txt

    def is_direct_lambda(self, e) -> bool:
        node = e
        while name(node) in EXPR_CHAIN and len(children(node)) == 1:
            node = children(node)[0]
        if name(node) == "postfixExpr" and len(children(node)) == 1:
            node = children(node)[0]
        if name(node) == "primaryExpr" and len(children(node)) == 1:
            node = children(node)[0]
        return name(node) == "lambda"

    def assign_stmt(self, inner, env: Env) -> List[str]:
        pl = find(inner, "place")
        op = next(t.text for t in direct_tokens(inner) if t.text in ("=", "+=", "-=", "*=", "/=", "%="))
        e = find(inner, "expr")
        target = self.place(pl, env, "write")
        pt = self.types.get(id(pl))
        code = self.expr(e, env, want=pt)
        return [f"{target} {op} {code};"]

    def if_stmt(self, node, env: Env) -> str:
        kids = children(node)
        out = []
        i = 0
        while i < len(kids):
            k = kids[i]
            if isinstance(k, Token) and k.text == "if":
                cond = self.expr(kids[i + 1], env)
                out.append(f"if {cond} {self.block_code(kids[i + 2], env)}")
                i += 3
            elif isinstance(k, Token) and k.text == "else":
                if name(kids[i + 1]) == "block":
                    out.append(f"else {self.block_code(kids[i + 1], env)}")
                    i += 2
                else:
                    out.append("else ")
                    i += 1
            else:
                i += 1
        return " ".join(out).replace("else  if", "else if")

    def for_stmt(self, node, env: Env) -> str:
        ts = direct_tokens(node)
        var = ts[[t.text for t in ts].index("for") + 1].text
        label = self.loop_label(node)
        src = next(c for c in children(node) if name(c) in ("rangeExpr", "expr", "place"))
        body_env = Env(env)
        if name(src) == "rangeExpr":
            kids = children(src)
            lo, hi = self.expr(kids[0], env), self.expr(kids[2], env)
            seq = f"{lo}..={hi}" if kids[1].text == ".." else f"{lo}..{hi}"
            body_env.set(var, Binding("owned", Named("Int")))
        else:
            mk = self.marker_of(src) if name(src) != "place" else None
            moving = any(isinstance(k, Token) and k.text == "move" for k in children(node))
            ct = self.node_type(src, env)
            elem = strip(ct).args[0] if isinstance(strip(ct), Named) and strip(ct).args else Unknown()
            if name(src) == "place":
                p = self.place(src, env, "move" if moving else "read")
                seq = p if moving else f"&{p}" if env.lookup(root_name(src)) and env.lookup(root_name(src)).mode == "owned" else f"{p}.iter()"
                body_env.set(var, Binding("owned" if moving else "ref", elem))
            else:
                seq = self.expr(src, env)
                if mk == "move":
                    body_env.set(var, Binding("owned", elem))
                else:
                    seq = f"({seq}).iter()" if not seq.startswith("&") else seq
                    body_env.set(var, Binding("ref", elem))
        self.loop_labels.append(label)
        body = self.block_code(find(node, "block"), body_env)
        self.loop_labels.pop()
        return f"{self.lbl(label)}for {rust_ident(var) if var != '_' else '_'} in {seq} {body}"

    # ---- places --------------------------------------------------------------------------------
    def place(self, pl, env: Env, how: str) -> str:
        """A place path as Rust: `how` is read | write | move | alias | editable | copy."""
        ts = tokens(pl)
        root = ts[0].text
        b = env.lookup(root)
        parts = [t for t in ts[1:] if t.text not in (".", "[", "]")]
        if root == "self":
            code = "self"
        else:
            code = rust_ident(root)
        path = code
        for t in parts:
            if t.kind == "INT":
                path += f".{t.text}"
            else:
                path += f".{rust_ident(t.text)}"
        if not parts and b is not None and b.mode in ("ref", "mutref") and is_trivial(b.type) and how in ("read", "write", "copy", "move"):
            return f"(*{path})" if how != "write" else f"*{path}"
        return path

    def node_type(self, node, env: Env) -> Optional[Type]:
        """The checker's recorded type of an expression node, or one derived from the lowering environment."""
        t = self.types.get(id(node))
        if t is not None:
            return t
        if isinstance(node, Token):
            if node.text == "self":
                b = env.lookup("self")
                return b.type if b else None
            b = env.lookup(node.text) if node.kind == "LOWER" else None
            return b.type if b else None
        kids = children(node)
        if name(node) in EXPR_CHAIN + ("postfixExpr", "primaryExpr") and len(kids) == 1:
            return self.node_type(kids[0], env)
        if name(node) == "literal":
            return self.node_type(kids[0], env)
        if name(node) == "place":
            return self.place_type(node, env)
        # fall back to the checker's inference with the lowering environment's bindings
        try:
            saved = self.ty.node_types
            self.ty.node_types = {}
            t = self.ty.infer(node, self.typer_env(env))
            self.types.update(self.ty.node_types)
            self.ty.node_types = saved
            return t
        except Exception:  # noqa: BLE001
            return None

    def place_type(self, pl, env: Env) -> Optional[Type]:
        ts = tokens(pl)
        b = env.lookup(ts[0].text)
        t = b.type if b else None
        for tk in ts[1:]:
            if tk.text in (".", "[", "]") or t is None:
                continue
            s = base(t)
            d = self.table.get(tname(s)) if isinstance(s, Named) else None
            if d is not None and tk.text in d.fields:
                t = d.fields[tk.text][0]
            elif tk.kind == "INT" and isinstance(s, TupleT):
                t = s.items[int(tk.text)]
            elif isinstance(s, Named) and s.name == "List" and tk.text not in ("count",):
                t = s.args[0] if s.args else None
            else:
                return None
        return t

    def marker_of(self, e) -> Optional[str]:
        node = e
        while name(node) in EXPR_CHAIN and len(children(node)) == 1:
            node = children(node)[0]
        if name(node) == "markerExpr":
            rest = children(node)[2:]
            if rest and isinstance(rest[0], Token) and rest[0].text == "(":
                return None                                   # a call marker (`editable f(…)`): the result is a value (8.4)
            return tokens(children(node)[0])[0].text
        return None

    # ---- expressions ---------------------------------------------------------------------------
    def expr(self, node, env: Env, want: Optional[Type] = None) -> str:
        if isinstance(node, Token):
            return self.literal(node, env, want)
        n = name(node)
        kids = children(node)
        if n in EXPR_CHAIN and len(kids) == 1:
            inner = self.expr(kids[0], env, want)
            if n == "expr":                                   # coerce once, at the expression's root
                return self.coerce(inner, self.node_type(kids[0], env), want, kids[0], env)
            return inner
        if n == "constTerm":
            return self.const_term(node, env, want)
        if n == "outcomeExpr":
            return self.outcome_expr(node, env, want)
        if n == "coalesceExpr":
            left = self.expr(kids[0], env)
            right = self.expr(kids[2], env, want=want)
            return f"({left}).unwrap_or_else(|| {right})"
        if n in ("orExpr", "andExpr"):
            op = " || " if n == "orExpr" else " && "
            return "(" + op.join(self.expr(k, env) for k in kids[::2]) + ")"
        if n in ("bitOrExpr", "bitXorExpr", "bitAndExpr", "shiftExpr", "additiveExpr", "multiplicativeExpr", "equalityExpr", "relationalExpr"):
            return self.chain(kids, env)
        if n == "unaryExpr":
            if isinstance(kids[0], Token):
                op = kids[0].text
                if op == "await":
                    raise LowerError("await is outside the synchronous slice")
                return f"({op}{self.expr(kids[1], env)})"
            return self.expr(kids[0], env, want)
        if n == "markerExpr":
            return self.marker_expr(node, env, want)
        if n == "postfixExpr":
            return self.postfix_expr(node, env, want)
        if n == "primaryExpr":
            return self.primary(node, env, want)
        if n == "lambda":
            return self.lambda_expr(node, env, want)
        if n == "matchExpr":
            return self.match_expr(node, env, want)
        if n == "ifExpr":
            return self.if_expr(node, env, want)
        if n == "tupleLit":
            return "(" + ", ".join(self.expr(k, env) for k in kids if name(k) == "expr") + ")"
        if n == "recordLit":
            raise LowerError("anonymous records are outside the synchronous slice")
        if n == "place":
            return self.place(node, env, "read")
        raise LowerError(f"expression {n}")

    def coerce(self, code: str, have: Optional[Type], want: Optional[Type], node, env: Env) -> str:
        """Bridge Fortis's implicit views and present-injection to Rust: literal → String, String → &str, T → Option<T>."""
        if want is None or have is None:
            return code
        hs, ws = strip(have), strip(want)
        if isinstance(ws, OptionalT) and not isinstance(hs, OptionalT) and tname(hs) != "Never" and not (isinstance(node, Token) and node.text == "null"):
            inner = self.coerce(code, have, ws.inner, node, env)
            if code != "None":
                return f"Some({inner})"
        if tname(ws) == "String" and (self.is_strlit(have) or tname(hs) == "StringView"):
            return f"{code}.to_string()"
        if tname(ws) == "StringView" and tname(hs) == "String" and not self.is_strlit(have):
            return f"&{code}" if not code.startswith("&") else code
        return code

    @staticmethod
    def is_strlit(t: Optional[Type]) -> bool:
        return t is not None and isinstance(base(t), Named) and base(t).name.startswith("#")

    def chain(self, kids, env: Env) -> str:
        out = self.expr(kids[0], env)
        for i in range(1, len(kids), 2):
            op = kids[i].text
            rhs = self.expr(kids[i + 1], env)
            lt = self.node_type(kids[0], env) if i == 1 else None
            if op in ("==", "!=") and (tname(lt) in ("String", "StringView") or self.is_strlit(lt)):
                out = f"({out}.to_string() {op} {rhs}.to_string())"
            elif op in ("==", "!=", "<", "<=", ">", ">="):
                out = f"({self.deref_if_ref(out, kids[0], env)} {op} {self.deref_if_ref(rhs, kids[i + 1], env)})"
            elif op == "/" and tname(lt) not in ("Float", "Float32"):
                out = f"fortis_rt::pdiv({out}, {rhs})"
            elif op == "%" and tname(lt) not in ("Float", "Float32"):
                out = f"fortis_rt::prem({out}, {rhs})"
            else:
                out = f"({out} {op} {rhs})"
        return out

    def deref_if_ref(self, code: str, node, env: Env) -> str:
        pl = self.direct_place(node)
        if pl is None:
            return code
        b = env.lookup(root_name(pl))
        if b is not None and b.mode in ("ref", "mutref") and not is_trivial(b.type) and len(tokens(pl)) == 1:
            return f"(*{code})"
        return code

    def outcome_expr(self, node, env: Env, want) -> str:
        kids = children(node)
        words = [k.text for k in kids if isinstance(k, Token)]
        inner = self.expr(kids[0], env)
        if words[:2] == ["or", "fail"]:
            return f"({inner})?"
        if words[:2] == ["or", "recover"]:
            rhs = self.expr(kids[-1], env, want=want)
            return f"(match {inner} {{ Ok(v) => v, Err(f) => {{ f.recover(); {rhs} }} }})"
        if words[0] == "catch":
            ident = next(k for k in kids if isinstance(k, Token) and k.kind == "LOWER")
            blk = find(node, "block")
            cenv = Env(env)
            cenv.set(ident.text, Binding("owned", Named("Failure")))
            consumed = self.consumes_name(blk, ident.text)
            label = self.new_label()
            saved = getattr(self, "give_label", None)
            saved_rec = getattr(self, "recover_name", None)
            self.give_label = label
            self.recover_name = None if consumed else rust_ident(ident.text)
            lines = self.block_lines(blk, cenv)
            self.give_label = saved
            self.recover_name = saved_rec
            body = "\n".join("        " + l for l in lines)
            return f"(match {inner} {{ Ok(v) => v, Err({rust_ident(ident.text)}) => {label}: {{\n{body}\n    }} }})"
        raise LowerError("outcome form")

    def consumes_name(self, blk, nm: str) -> bool:
        txt = " ".join(t.text for t in tokens(blk))
        return bool(re.search(rf"\b(report|fail move|move|annotate\(move)\s+{nm}\b", txt)) or f"report {nm}" in txt or f"move {nm}" in txt

    def marker_expr(self, node, env: Env, want) -> str:
        kids = children(node)
        marker = tokens(kids[0])[0].text
        pl = kids[1]
        rest = kids[2:]
        b = env.lookup(root_name(pl))
        direct = len([t for t in tokens(pl)]) == 1
        pts = tokens(pl)
        if rest and isinstance(rest[0], Token) and rest[0].text == "(" and len(pts) >= 3 and pts[-2].text == "." and not (b is not None and isinstance(strip(b.type), FnT) and len(pts) == 1):
            # `editable p.m(args)` / `move p.m(args)`: a method call whose receiver marker names the loan (7.5)
            mname = pts[-1].text
            recv_code = rust_ident(pts[0].text) if pts[0].text != "self" else "self"
            recv_t = b.type if b is not None else None
            for tk in pts[1:-2]:
                if tk.text == ".":
                    continue
                recv_code += f".{rust_ident(tk.text)}"
                d = self.table.get(tname(recv_t)) if recv_t is not None else None
                recv_t = d.fields[tk.text][0] if d is not None and tk.text in d.fields else None
            if b is not None and b.mode in ("ref", "mutref") and is_trivial(b.type) and len(pts) == 3:
                recv_code = f"(*{recv_code})"
            al = rest[1] if len(rest) > 1 and name(rest[1]) == "argList" else None
            v = self.method_call_named(recv_code, mname, al, env, recv_t, place_node=None, marker=marker)
            t = self.method_type(recv_t, mname)
            i = 2 if al is not None else 1
            while i < len(rest):
                k = rest[i]
                v = self.postfix(v, k, env, t) if not isinstance(k, Token) else v
                i += 1
            return v
        if rest and isinstance(rest[0], Token) and rest[0].text == "(" and b is not None and isinstance(strip(b.type), FnT):
            # a call marker (`editable f(…)`, `move f(…)`): the marker names the call form, not a loan of the binding (8.4)
            callee = f"(*{rust_ident(root_name(pl))})" if b.mode == "mutref" else rust_ident(root_name(pl))
            al = rest[1] if len(rest) > 1 and name(rest[1]) == "argList" else None
            return f"{callee}({self.call_args_closure(al, env, b)})"
        if marker == "copy":
            code = self.place(pl, env, "copy")
            pt = self.types.get(id(pl))
            self.copies.append(f"copy of `{self.place_text(pl)}` (source `copy`, 7.5)")
            v = f"{code}.clone() /* copy: source `copy` of `{self.place_text(pl)}` (7.5) */"
        elif marker == "move":
            v = self.place(pl, env, "move")
            if b is not None and b.mode in ("ref", "mutref") and not is_trivial(b.type) and not direct:
                v = self.place(pl, env, "move")
        elif marker == "alias":
            code = self.place(pl, env, "alias")
            pt = self.node_type(pl, env)
            if want is not None and tname(want) == "StringView" and tname(pt) == "String":
                v = f"&*{code}"                               # a String place viewed as StringView (6.6)
            elif b is not None and b.mode == "ref" and direct:
                v = code
            elif b is not None and b.mode == "mutref" and direct:
                v = f"&*{code}"
            else:
                v = f"&{code}"
        else:   # editable
            code = self.place(pl, env, "editable")
            v = f"&mut *{code}" if (b is not None and b.mode == "mutref" and direct) else f"&mut {code}"
        if rest and (name(rest[0]) == "methodCall" or (self.dot_member(rest[0]) is not None and len(rest) > 1 and self.is_call_postfix(rest[1]))) and marker in ("alias", "editable", "move"):
            v = self.place(pl, env, "read")               # the receiver marker names the loan; Rust takes it at the call
        t = self.node_type(pl, env)
        i = 0
        while i < len(rest):
            k = rest[i]
            nxt = rest[i + 1] if i + 1 < len(rest) else None
            member = self.dot_member(k) if not isinstance(k, Token) else None
            if member is not None and nxt is not None and self.is_call_postfix(nxt):
                al = next((c for c in children(nxt) if name(c) == "argList"), None)
                v = self.method_call_named(v, member, al, env, t, place_node=pl, marker=marker)
                t = self.method_type(t, member)
                i += 2
                continue
            if name(k) == "methodCall":
                v = self.method_call(v, k, env, t, place_node=pl, marker=marker)
                t = None
            elif name(k) == "postfix":
                v = self.postfix(v, k, env, t)
                t = self.postfix_type(t, k)
            elif isinstance(k, Token) and k.text == "(":
                al = rest[i + 1] if i + 1 < len(rest) and name(rest[i + 1]) == "argList" else None
                v = f"{v}({self.call_args_closure(al, env, b)})"
                i += 2 if al is not None else 1
            i += 1
        return v

    def place_text(self, pl) -> str:
        return "".join(t.text for t in tokens(pl))

    def postfix_expr(self, node, env: Env, want) -> str:
        kids = children(node)
        v = self.expr(kids[0], env, want if len(kids) == 1 else None)
        t = self.node_type(kids[0], env)
        pl = kids[0] if name(kids[0]) == "place" else None
        i = 1
        while i < len(kids):
            k = kids[i]
            nxt = kids[i + 1] if i + 1 < len(kids) else None
            member = self.dot_member(k)
            if member is not None and nxt is not None and self.is_call_postfix(nxt):
                al = next((c for c in children(nxt) if name(c) == "argList"), None)
                mt = self.postfix_type(t, k) if t is not None else None
                v = self.method_call_named(v, member, al, env, t, place_node=pl)
                t = self.types.get(id(nxt)) or self.method_type(t, member)
                i += 2
                pl = None
                continue
            v = self.postfix(v, k, env, t, place_node=pl)
            t = self.types.get(id(k)) or self.postfix_type(t, k)
            pl = None
            i += 1
        return v

    @staticmethod
    def dot_member(k) -> Optional[str]:
        kids = children(k)
        if name(k) == "postfix" and isinstance(kids[0], Token) and kids[0].text == "." and not (isinstance(kids[1], Token) and kids[1].kind == "INT"):
            return tokens(kids[1])[0].text
        return None

    @staticmethod
    def is_call_postfix(k) -> bool:
        kids = children(k)
        return name(k) == "postfix" and isinstance(kids[0], Token) and kids[0].text == "("

    def method_type(self, t: Optional[Type], mname: str) -> Optional[Type]:
        if t is None:
            return None
        s = base(t)
        d = self.table.get(tname(s)) if isinstance(s, Named) else None
        if d is not None and mname in d.methods:
            return d.methods[mname].ret
        if isinstance(s, Named) and s.name == "List":
            if mname in ("count", "countWhere"): return Named("Int")
            if mname in ("reversed", "concat", "filter"): return s
            if mname in ("get", "first", "pop"): return OptionalT(s.args[0]) if s.args else None
        if isinstance(s, Named) and s.name in ("String", "StringView") and mname in ("scalarCount", "byteCount"):
            return Named("Int")
        if isinstance(s, Named) and s.name == "Failure" and mname == "code":
            return Named("StringView")
        return None

    def postfix_type(self, t: Optional[Type], k) -> Optional[Type]:
        kids = children(k)
        first = kids[0]
        if t is None:
            return None
        s = base(t)
        if isinstance(first, Token) and first.text == "." and not (isinstance(kids[1], Token) and kids[1].kind == "INT"):
            fname = tokens(kids[1])[0].text
            d = self.table.get(tname(s)) if isinstance(s, Named) else None
            if d is not None and fname in d.fields:
                return d.fields[fname][0]
        if name(first) == "methodCall":
            mname = tokens(find(first, "memberName"))[0].text
            d = self.table.get(tname(s)) if isinstance(s, Named) else None
            if d is not None and mname in d.methods:
                return d.methods[mname].ret
            if isinstance(s, Named) and s.name == "List":
                if mname in ("count", "countWhere"): return Named("Int")
                if mname in ("reversed", "concat", "filter"): return s
                if mname in ("get", "first", "pop"): return OptionalT(s.args[0]) if s.args else None
            if isinstance(s, Named) and s.name in ("String", "StringView") and mname in ("scalarCount", "byteCount"):
                return Named("Int")
        return None

    def postfix(self, v: str, k, env: Env, recv_t, place_node=None) -> str:
        kids = children(k)
        first = kids[0]
        if isinstance(first, Token) and first.text == ".":
            second = kids[1]
            if isinstance(second, Token) and second.kind == "INT":
                return f"{v}.{second.text}"
            return f"{v}.{rust_ident(tokens(second)[0].text)}"
        if isinstance(first, Token) and first.text == "?.":
            return f"{v}.as_ref().map(|x| x.{rust_ident(tokens(kids[1])[0].text)})"
        if name(first) == "methodCall":
            return self.method_call(v, first, env, recv_t, place_node=place_node)
        if isinstance(first, Token) and first.text == "(":
            al = next((c for c in kids if name(c) == "argList"), None)
            b = env.lookup(v) if v.isidentifier() else None
            d = self.table.get(v) if (v.isidentifier() and b is None) else None
            if d is not None and d.kind == "func" and d.sig is not None:
                return self.with_hoist(f"{v}({self.call_args(al, env, d.sig)})")
            return f"{v}({self.call_args_closure(al, env, b)})"
        if isinstance(first, Token) and first.text == "[":
            return f"{v}[({self.expr(kids[1], env)}) as usize]"
        raise LowerError("postfix")

    def call_args_closure(self, al, env: Env, b: Optional[Binding]) -> str:
        """Arguments of a closure call: by the closure's parameter types (trivial by value, else by reference)."""
        args = [c for c in children(al) if name(c) == "arg"] if al is not None else []
        ft = strip(b.type) if b is not None else None
        out = []
        for i, a in enumerate(args):
            e = find(a, "expr")
            pt = ft.params[i] if isinstance(ft, FnT) and i < len(ft.params) else None
            out.append(self.arg_code(e, env, pt, "" if pt is None or is_trivial(pt) or tname(pt) == "StringView" else "loan"))
        return ", ".join(out)

    def arg_code(self, e, env: Env, pt: Optional[Type], mode: str) -> str:
        """An argument lowered for a parameter: loans of owned places become `&p`, trivial values pass by value."""
        mk = self.marker_of(e)
        code = self.expr(e, env, want=pt)
        at = self.node_type(e, env)
        if mk is not None:
            return code
        pl = self.direct_place(e)
        if pl is not None:
            b = env.lookup(root_name(pl))
            mode = mode.lstrip(".")
            if mode in ("", "loan") and pt is not None and not is_trivial(pt) and tname(pt) != "StringView":
                if b is not None and b.mode == "owned":
                    return f"&{code}"
                if b is not None and b.mode == "mutref":
                    return f"&*{code}"
                return code
            if mode == "editable":
                return code if (b is not None and b.mode == "mutref") else f"&mut {code}"
            if tname(pt) == "StringView" and tname(at) == "String":
                return f"&{code}" if (b is None or b.mode == "owned") else code
            if tname(pt) == "ListView" and tname(at) == "List":
                return f"&{code}" if (b is None or b.mode == "owned") else code
            return code
        if pt is not None and tname(pt) == "ListView" and (tname(at) == "List" or at is None or isinstance(at, Unknown)):
            return f"&({code})"                              # an expression yields an owned list; the view borrows it for the call
        if pt is not None and tname(pt) == "String" and self.is_strlit(at) and mode in ("", "loan"):
            return f"&({code})"                              # a literal for a String loan parameter: a temporary String
        if pt is not None and not is_trivial(pt) and tname(pt) not in ("StringView", "ListView") and mode in ("", "loan") and not isinstance(strip(pt), FnT):
            if at is not None and not isinstance(at, LoanT) and tname(at) not in ("StringView",) and not self.is_strlit(at):
                return f"&{code}"
        return code

    def direct_place(self, e):
        node = e
        while name(node) in EXPR_CHAIN and len(children(node)) == 1:
            node = children(node)[0]
        if name(node) == "postfixExpr" and len(children(node)) == 1:
            node = children(node)[0]
        if name(node) == "primaryExpr" and len(children(node)) == 1 and isinstance(children(node)[0], Token) and children(node)[0].kind == "LOWER":
            return node
        if name(node) == "place":
            return node
        return None

    def call_args(self, al, env: Env, sig, type_args: Dict[str, Type] = None) -> str:
        """Positional then named arguments in the signature's order; omitted defaults re-evaluated at the call (5.8)."""
        args = [c for c in children(al) if name(c) == "arg"] if al is not None else []
        params = [(pn, pt, pm) for pn, pt, pm in sig.params]
        positional, named = [], {}
        for a in args:
            ks = children(a)
            if isinstance(ks[0], Token) and ks[0].text == "...":
                raise LowerError("spread arguments are outside the synchronous slice")
            if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                named[ks[0].text] = ks[2]
            else:
                positional.append(ks[0])
        out = []
        hoisted: List[str] = []
        def value_arg(e):
            return self.direct_place(e) is None and self.marker_of(e) is None
        exprs_present = any(value_arg(e) for e in positional + list(named.values()))
        loans_present = any(not value_arg(e) for e in positional + list(named.values()))
        for i, (pn, pt, pm) in enumerate(params):
            if pm.startswith("..."):
                for e in positional[i:]:
                    out.append(self.arg_code(e, env, pt, pm[3:]))
                break
            if i < len(positional):
                e = positional[i]
            elif pn in named:
                e = named[pn]
            else:
                dnode = self.default_of(sig, pn)
                if dnode is None:
                    raise LowerError(f"missing argument {pn}")
                out.append(self.expr(dnode, Env(), want=pt))
                continue
            code = self.arg_code(e, env, pt, pm)
            if exprs_present and loans_present and value_arg(e):
                tmp = f"__a{len(hoisted)}"
                hoisted.append(f"let {tmp} = {code};")        # 5.7: values are evaluated in step 1; loans activate in step 2
                code = tmp
            out.append(code)
        self.pending_hoist = hoisted
        return ", ".join(out)

    pending_hoist: List[str] = []

    def with_hoist(self, call: str) -> str:
        h, self.pending_hoist = self.pending_hoist, []
        if not h:
            return call
        return "{ " + " ".join(h) + " " + call + " }"

    def default_of(self, sig, pn: str):
        head = sig.node
        if head is None:
            return None
        pl = find(find(head, "funcSignature"), "paramList")
        for p in [c for c in children(pl) if name(c) == "param"] if pl is not None else []:
            ts = direct_tokens(p)
            if ts and ts[0].kind == "LOWER" and ts[0].text == pn:
                return find(p, "constExpr")
        return None

    def method_call(self, v: str, mc, env: Env, recv_t: Optional[Type], place_node=None, marker=None) -> str:
        return self.method_call_named(v, tokens(find(mc, "memberName"))[0].text, find(mc, "argList"), env, recv_t, place_node, marker)

    def method_call_named(self, v: str, mname: str, al, env: Env, recv_t: Optional[Type], place_node=None, marker=None) -> str:
        args = [c for c in children(al) if name(c) == "arg"] if al is not None else []
        if recv_t is None and place_node is not None:
            recv_t = self.node_type(place_node, env)
        rt = strip(recv_t) if recv_t is not None else None
        tn = tname(rt) if rt is not None else ""
        pos = [find(a, "expr") for a in args]
        if tn == "List":
            elem = rt.args[0] if isinstance(rt, Named) and rt.args else Unknown()
            if mname == "count": return f"{v}.count()"
            if mname == "isEmpty": return f"{v}.isEmpty()"
            if mname == "push": return f"{v}.push({self.arg_code(pos[0], env, elem, 'owned')})"
            if mname == "pop": return f"{v}.pop()"
            if mname in ("get", "at"): return f"{v}.{mname}({self.expr(pos[0], env)}){'.copied()' if is_trivial(elem) else '.cloned() /* copy: List.get returns a view in Fortis; the element is copied here (lowering rule L1) */'}" if mname == "get" else f"{v}.at({self.expr(pos[0], env)}).map(|x| x.clone() /* copy: List.at element read (lowering rule L1) */)"
            if mname == "first": return f"{v}.first(){'.copied()' if is_trivial(elem) else '.cloned() /* copy: List.first element read (lowering rule L1) */'}"
            if mname == "reversed": return f"{v}.reversed()"
            if mname == "concat": return f"{v}.concat(&{self.expr(pos[0], env)})"
            if mname == "set": return f"{v}.set({self.expr(pos[0], env)}, {self.expr(pos[1], env)})"
            if mname in ("countWhere", "map", "filter"):
                cb = self.callback_for_list(pos[0], env, elem)
                return f"{v}.{mname}({cb})"
        if tn in ("String", "StringView") or self.is_strlit(rt):
            if mname in ("scalarCount", "byteCount", "isEmpty"):
                return f"{v}.{mname}()"
        if tn == "Shared":
            if mname == "retain":
                self.copies.append("Rc::clone for `retain` (7.8)")
                return f"Rc::clone(&{v}) /* reference count: `retain` (7.8) */"
            if mname == "downgrade": return f"Rc::downgrade(&{v})"
        if tn == "Weak" and mname == "upgrade":
            return f"{v}.upgrade()"
        if tn == "Builder":
            if mname == "finish": return f"{v}.finish()"
            return f"{v}.{mname}({', '.join(self.arg_code(e, env, Named('StringView'), '') for e in pos)})"
        if tn == "Console" and mname == "print":
            return f"{v}.print({self.arg_code(pos[0], env, Named('StringView'), '')})"
        if tn == "FileSystem" and mname == "readText":
            return f"{v}.readText({self.arg_code(pos[0], env, Named('Path'), '')})"
        if tn == "Failure" and mname == "code":
            return f"{v}.code()"
        d = self.table.get(tn) if tn else None
        if d is not None and mname in d.methods:
            sig = d.methods[mname]
            return self.with_hoist(f"{v}.{rust_ident(mname)}({self.call_args(al, env, sig)})")
        raise LowerError(f"method {tn}.{mname}")

    def callback_for_list(self, e, env: Env, elem: Type) -> str:
        """A list callback receives `&T`; a Fortis callback takes the element by loan (trivial: by value)."""
        if self.is_direct_lambda(e):
            lam = find(e, "lambda")
            return self.lambda_expr(lam, env, None, elem_ref=True)
        code = self.expr(e, env)
        return f"|x| {code}(*x)" if is_trivial(elem) else f"|x| {code}(x)"

    def literal(self, tok: Token, env: Env, want) -> str:
        if tok.kind == "INT":
            t = tok.text.replace("_", "")
            return str(int(t, 0) if t[:2].lower() in ("0x", "0b", "0o") else int(t)) + "i64"
        if tok.kind == "STR":
            return self.interpolate(tok, env, want)
        if tok.kind == "RAW":
            return "r#\"" + tok.text[3:-3] + "\"#"
        if tok.kind == "FLOAT":
            t = tok.text.replace("_", "")
            if t.endswith("f32"):
                return f"{t[:-3]}f32"
            return f"{t}f64" if "." in t or "e" in t.lower() else f"{t}.0f64"
        if tok.kind == "KW":
            if tok.text in ("true", "false"): return tok.text
            if tok.text == "null": return "None"
            if tok.text == "self": return "self"
        if tok.kind == "LOWER":
            b = env.lookup(tok.text)
            if b is None:
                d = self.table.get(tok.text)
                if d is not None and d.kind == "const":
                    return f"{rust_ident(tok.text)}()"
                if d is not None and d.kind == "func":
                    return rust_ident(tok.text)
                raise LowerError(f"unbound {tok.text}")
            if b.mode in ("ref", "mutref") and is_trivial(b.type):
                return f"(*{rust_ident(tok.text)})"
            return rust_ident(tok.text)
        raise LowerError(f"literal {tok.kind}")

    def interpolate(self, tok: Token, env: Env, want) -> str:
        body = tok.text[1:-1]
        if "${" not in body:
            text = self.unescape(body)
            lit = '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("{", "{{").replace("}", "}}") + '"'
            if "{" in text or "}" in text:
                lit = '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'
            if want is not None and tname(want) == "String":
                return f"{lit}.to_string()"
            return lit
        parts, args, i = [], [], 0
        while i < len(body):
            c = body[i]
            if c == "\\":
                parts.append({"n": "\\n", "t": "\\t", '"': '\\"', "\\": "\\\\", "$": "$"}.get(body[i + 1], body[i + 1]))
                i += 2
            elif body.startswith("${", i):
                depth, j = 1, i + 2
                while depth:
                    if body[j] == "{": depth += 1
                    elif body[j] == "}": depth -= 1
                    j += 1
                src = body[i + 2:j - 1]
                sub = self.rec.parse(lexer.lex(src), "expr")
                # the fragment was typed by the checker through the same parse; find its type by text
                code = self.expr_fragment(sub, env, tok)
                parts.append("{}")
                args.append(f"({code}).show()")
                i = j
            else:
                parts.append(c.replace("{", "{{").replace("}", "}}") if c in "{}" else c)
                i += 1
        code = f"format!(\"{''.join(parts)}\"{''.join(', ' + a for a in args)})"
        return f"&{code}" if want is not None and tname(want) == "StringView" else code

    def expr_fragment(self, sub, env: Env, tok: Token) -> str:
        """A `${…}` fragment: re-typed here since the checker's fragment parse is a different tree."""
        saved = self.types
        typer = self.ty
        typer.node_types = {}
        tenv = self.typer_env(env)
        try:
            typer.infer(sub, tenv)
        except Exception:  # noqa: BLE001
            pass
        self.types = dict(typer.node_types)       # only the fragment's own types: stale ids of discarded trees must not leak in
        code = self.expr(sub, env)
        self.types = saved
        return code

    def typer_env(self, env: Env):
        from checker.typer import Env as TEnv
        te = TEnv()
        e = env
        chain = []
        while e is not None:
            chain.append(e)
            e = e.parent
        for e in reversed(chain):
            for n, b in e.vars.items():
                te.set(n, b.type, mutable=b.mutable, kind="let")
        return te

    @staticmethod
    def unescape(body: str) -> str:
        out, i = [], 0
        while i < len(body):
            if body[i] == "\\":
                out.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\", "$": "$"}.get(body[i + 1], body[i + 1]))
                i += 2
            else:
                out.append(body[i])
                i += 1
        return "".join(out)

    def primary(self, node, env: Env, want) -> str:
        kids = children(node)
        first = kids[0]
        if isinstance(first, Token):
            if first.text == "(":
                return f"({self.expr(kids[1], env, want)})"
            if first.text == "present":
                return f"Some({self.expr(kids[2], env)})"
            if first.text == "fail":
                return f"return Err({self.expr(kids[1], env)})"
            return self.literal(first, env, want)
        n = name(first)
        if n == "typeRef":
            return self.type_member(kids, env, want)
        if n == "literal":
            return self.literal(children(first)[0], env, want)
        return self.expr(first, env, want)

    def type_member(self, kids, env: Env, want) -> str:
        tr = kids[0]
        trs = direct_tokens(tr)
        tn = trs[0].text if trs[0].kind == "UPPER" else trs[2].text
        member = None
        idx = 1
        if idx < len(kids) and isinstance(kids[idx], Token) and kids[idx].text == ".":
            member = kids[idx + 1].text if isinstance(kids[idx + 1], Token) else tokens(kids[idx + 1])[0].text
            idx += 2
        called = idx < len(kids) and isinstance(kids[idx], Token) and kids[idx].text == "("
        al = next((c for c in kids[idx:] if name(c) == "argList"), None) if called else None
        args = [find(a, "expr") for a in children(al) if name(a) == "arg"] if al is not None else []
        d = self.table.get(tn)
        if tn in ("Int", "List", "String", "Shared", "Outcome", "Must", "Host", "Builder", "Path") or tn.endswith("Builder"):
            d = None                                        # kernel intrinsics lower to the runtime, not to the prelude's signatures
        if d is not None and d.kind == "class" and member is None and called:
            return self.construct(d, al, env)
        if d is not None and d.kind == "error" and called:
            return self.failure(d, al, env)
        if d is not None and d.kind == "enum":
            if member in d.variants:
                payload = d.variants[member]
                if not payload:
                    return f"{rust_type(tn)}::{member}"
                named = {}
                for a in children(al) if al is not None else []:
                    if name(a) != "arg":
                        continue
                    ks = children(a)
                    if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                        named[ks[0].text] = ks[2]
                fields = ", ".join(f"{rust_ident(f)}: {self.expr(named[f], env, want=ft)}" for f, ft in payload)
                return f"{rust_type(tn)}::{member} {{ {fields} }}"
            if member in d.methods:
                return self.with_hoist(f"{rust_type(tn)}::{rust_ident(member)}({self.call_args(al, env, d.methods[member])})")
        if d is not None and d.kind == "type":
            if member == "validate":
                return f"{tn}::validate({self.expr(args[0], env)})"
            if member is None and called:
                return f"{tn}({self.expr(args[0], env)})"
        if d is not None and d.kind == "class" and member in d.methods:
            return self.with_hoist(f"{rust_type(tn)}::{rust_ident(member)}({self.call_args(al, env, d.methods[member])})")
        if tn == "Int":
            if member == "validate":
                lo, hi = self.refinement_bounds(tr)
                return f"fortis_rt::validate_int({self.expr(args[0], env)}, {self.opt(lo)}, {self.opt(hi)}, \"Int.validate\")"
            return f"int::{member}({', '.join(self.expr(a, env) for a in args)})"
        if tn == "List":
            if member == "default": return "Vec::new()"
            if member == "of":
                et = strip(want).args[0] if isinstance(strip(want), Named) and strip(want).args else None
                return "vec![" + ", ".join(self.arg_code(a, env, et, "owned") for a in args) + "]"
        if tn == "String":
            if member == "from": return f"({self.expr(args[0], env)}).show()"
            if member == "empty": return "String::new()"
        if tn == "Shared" and member == "create":
            self.copies.append("Rc::new for Shared.create (7.8)")
            return f"Rc::new({self.expr(args[0], env)}) /* reference count: Shared creation (7.8) */"
        if tn == "Outcome":
            if member == "collect": return f"fortis_rt::collect({self.expr(args[0], env)})"
            if member in ("success",): return f"Ok({self.expr(args[0], env)})"
            if member in ("failure",): return f"Err({self.expr(args[0], env)})"
        if tn == "Must":
            if member == "create": return "Must::create()"
            if member == "discharge": return f"Must::discharge({self.expr(args[0], env)})"
        if tn == "Host" and member == "capabilities":
            return "Host::capabilities()"
        if tn == "Builder" or tn.endswith("Builder"):
            if member == "create": return "Builder::create()"
        if tn == "Path" and member is None and called:
            return f"Path({self.expr(args[0], env)}.to_string())"
        if member is None and not called:
            return tn
        raise LowerError(f"type member {tn}.{member}")

    def refinement_bounds(self, tr) -> Tuple[Optional[int], Optional[int]]:
        ref = find(tr, "refinement")
        if ref is None:
            return None, None
        ts = [t for t in tokens(ref)]
        txt = "".join(t.text for t in ts)
        m = re.search(r"<(-?\d+)?(\.\.<?)(-?\d+)?>", txt.replace("_", ""))
        if not m:
            return None, None
        lo = int(m.group(1)) if m.group(1) else None
        hi = int(m.group(3)) if m.group(3) else None
        if hi is not None and m.group(2) == "..<":
            hi -= 1
        return lo, hi

    def construct(self, d, al, env: Env) -> str:
        named, positional = {}, []
        for a in children(al) if al is not None else []:
            if name(a) != "arg":
                continue
            ks = children(a)
            if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                named[ks[0].text] = ks[2]
            else:
                positional.append(ks[0])
        fields = []
        for i, (fname, (ft, exp, has_default)) in enumerate(d.fields.items()):
            if i < len(positional):
                e = positional[i]
            elif fname in named:
                e = named[fname]
            else:
                fd = next((m for m in find_all(d.node, "fieldDecl") if direct_tokens(m)[0].text == fname), None)
                ce = [c for c in children(fd) if name(c) == "constExpr"] if fd is not None else []
                if not ce:
                    raise LowerError(f"field {fname} not given")
                fields.append(f"{rust_ident(fname)}: {self.expr(ce[0], Env(), want=ft)}")
                continue
            mk = self.marker_of(e)
            code = self.expr(e, env, want=ft)
            if isinstance(ft, LoanT) and mk is None and tname(ft.inner) not in self.lifetime_classes:
                code = self.arg_code(e, env, ft.inner, "loan")
            fields.append(f"{rust_ident(fname)}: {code}")
        return f"{rust_type(d.name)} {{ {', '.join(fields)} }}"

    def failure(self, d, al, env: Env) -> str:
        named = {}
        pos = []
        for a in children(al) if al is not None else []:
            if name(a) != "arg":
                continue
            ks = children(a)
            if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                named[ks[0].text] = ks[2]
            else:
                pos.append(ks[0])
        items = list(d.payload.items())
        values = []
        for i, (fname, ft) in enumerate(items):
            e = pos[i] if i < len(pos) else named.get(fname)
            values.append((fname, ft, self.expr(e, env, want=ft) if e is not None else None))
        ints = [(f, c) for f, ft, c in values if is_trivial(ft) and tname(ft) == "Int"]
        if not values:
            payload = "Payload::None"
        elif len(values) == 1 and ints:
            payload = f"Payload::Int(\"{ints[0][0]}\", {ints[0][1]})"
        elif len(values) == 2 and len(ints) == 2:
            payload = f"Payload::Ints((\"{ints[0][0]}\", {ints[0][1]}), (\"{ints[1][0]}\", {ints[1][1]}))"
        elif len(values) == 1 and tname(values[0][1]) == "String":
            payload = f"Payload::Text(\"{values[0][0]}\", ({values[0][2]}).show())"
        else:
            payload = "Payload::Many(vec![" + ", ".join(f"(\"{f}\", ({c}).show())" for f, ft, c in values) + "])"
        return f"Failure::new(\"{d.code}\", {payload}, \"{self.fn_name}\")"

    def const_term(self, node, env: Env, want) -> str:
        kids = children(node)
        first = kids[0]
        if isinstance(first, Token):
            if first.text == "null": return "None"
            if first.text == "-": return f"(-{self.expr(kids[1], env)})"
            if first.text == "(": return f"({self.expr(kids[1], env, want)})"
            if first.kind == "LOWER": return self.literal(first, env, want)
        if name(first) == "literal":
            return self.literal(children(first)[0], env, want)
        if name(first) == "typeRef":
            return self.type_member(kids, env, want)
        raise LowerError("constant form")

    # ---- lambdas -------------------------------------------------------------------------------
    def lambda_expr(self, node, env: Env, want, elem_ref: bool = False) -> str:
        info = self.ty.closure_info.get(id(node), {"kind": "fn", "captures": []})
        lp = find(node, "lambdaParams")
        params = []
        cenv = Env(env)
        for p in ([c for c in children(lp) if name(c) == "lambdaParam"] if lp is not None else []):
            pn = direct_tokens(p)[0].text
            pt = find(p, "paramType")
            t = self.dc.type_of(find(pt, "type")) if pt is not None else Unknown()
            if elem_ref:
                params.append(f"{rust_ident(pn)}: &{self.rty(t)}")
                cenv.set(pn, Binding("ref", t))
            else:
                mode = next((x.text for x in direct_tokens(pt) if x.text in ("editable", "owned")), "") if pt is not None else ""
                text, bmode = self.param_rust(pn, t, mode, "'_", [])
                params.append(text if not text.startswith(rust_ident(pn) + ": F") else f"{rust_ident(pn)}: impl Fn()")
                cenv.set(pn, Binding(bmode, t))
        pre = []
        moving = False
        cl = find(node, "captureList")
        for cap in ([c for c in children(cl) if name(c) == "capture"] if cl is not None else []):
            ck = children(cap)
            if isinstance(ck[0], Token):
                nm = ck[0].text
                pre.append(f"let {rust_ident(nm)} = {self.place_value(nm, env)};")   # a trivial value, copied
                cenv.set(nm, Binding("owned", env.lookup(nm).type))
                moving = True
                continue
            marker = tokens(ck[0])[0].text
            nm = tokens(ck[1])[0].text
            b = env.lookup(nm)
            if marker == "alias":
                pre.append(f"let {rust_ident(nm)} = {'&' if b.mode == 'owned' else ''}{rust_ident(nm)};")
                cenv.set(nm, Binding("ref", b.type))
                moving = True
            elif marker == "editable":
                pre.append(f"let {rust_ident(nm)} = {'&mut ' if b.mode == 'owned' else ''}{rust_ident(nm)};")
                cenv.set(nm, Binding("mutref", b.type))
                moving = True
            elif marker == "copy":
                self.copies.append(f"copy capture of `{nm}` (8.3)")
                pre.append(f"let {rust_ident(nm)} = {rust_ident(nm)}.clone() /* copy: `copy` capture of `{nm}` (8.3) */;")
                cenv.set(nm, Binding("owned", b.type))
                moving = True
            else:
                pre.append(f"let {'mut ' if b.mutable else ''}{rust_ident(nm)} = {rust_ident(nm)};")
                cenv.set(nm, Binding("owned", b.type, mutable=b.mutable))
                moving = True
        rt_node = next((c for c in children(node) if name(c) == "type"), None)
        rt = self.dc.type_of(rt_node) if rt_node is not None else Named("Void")
        body = find(node, "funcBody")
        saved = (self.ret_try, self.ret_type, self.fn_name)
        mods = [t.text for t in tokens(find(node, "modifiers"))] if find(node, "modifiers") is not None else []
        self.ret_try, self.ret_type, self.fn_name = "try" in mods, rt, "lambda"
        blk = find(body, "block") if name(children(body)[0]) == "block" else None
        if blk is not None:
            lines = self.block_lines(blk, cenv)
            inner = "{\n" + "\n".join("    " + l for l in lines) + "\n}"
        else:
            e = find(body, "expr")
            inner = "{ " + self.wrap_ok(self.expr(e, cenv, want=rt), e) + " }"
        self.ret_try, self.ret_type, self.fn_name = saved
        rts = f" -> {'Outcome<' + self.rty(rt) + '>' if 'try' in mods else self.rty(rt)}"
        closure = f"{'move ' if moving else ''}|{', '.join(params)}|{rts} {inner}"
        if pre:
            return "{ " + " ".join(pre) + " " + closure + " }"
        return closure

    def place_value(self, nm: str, env: Env) -> str:
        b = env.lookup(nm)
        if b is not None and b.mode in ("ref", "mutref") and is_trivial(b.type):
            return f"*{rust_ident(nm)}"
        return rust_ident(nm)

    # ---- match and if expressions -----------------------------------------------------------------
    def match_expr(self, node, env: Env, want) -> str:
        kids = children(node)
        scrut = next(c for c in kids if name(c) in ("expr", "place"))
        moving = any(isinstance(k, Token) and k.text == "move" for k in kids[:3])
        st = self.node_type(scrut, env)
        by_ref = False
        self.scrutinee_owned = True
        if name(scrut) == "place":
            code = self.place(scrut, env, "move" if moving else "read")
            b = env.lookup(root_name(scrut))
            if not moving and b is not None and b.mode in ("ref", "mutref"):
                self.scrutinee_owned = False               # a loaned failure is inspected, not consumed (10.5)
        elif self.direct_place(scrut) is not None and not moving:
            code = self.expr(scrut, env)
            b = env.lookup(root_name(self.direct_place(scrut)))
            if b is not None and b.mode in ("ref", "mutref"):
                self.scrutinee_owned = False
            if not moving and not is_trivial(st) and tname(st) not in ("Outcome", "Failure") and not isinstance(strip(st), OptionalT):
                by_ref = True
                code = f"&{code}" if (b is None or b.mode == "owned" or len(tokens(scrut)) > 1) else code
        else:
            code = self.expr(scrut, env)
        arms = []
        for arm in [a for a in children(node) if name(a) == "matchArm"]:
            aenv = Env(env)
            pat = find(arm, "pattern")
            pcode, guard_extra = self.pattern(pat, st, aenv, by_ref=by_ref)
            ak = children(arm)
            guard = ""
            if any(isinstance(k, Token) and k.text == "when" for k in ak):
                g = next(c for c in ak if name(c) == "expr")
                guard = f" if {self.expr(g, aenv)}"
            if guard_extra:
                guard = f" if {guard_extra}" + (f" && ({guard[4:]})" if guard else "")
            body = ak[-1]
            if name(body) == "block":
                label = self.new_label()
                saved = getattr(self, "give_label", None)
                self.give_label = label
                lines = self.block_lines(body, aenv)
                self.give_label = saved
                pre = self.pattern_prelude(pat, st, aenv)
                bcode = f"{label}: {{\n" + "\n".join("        " + l for l in pre + lines) + "\n    }"
            else:
                pre = self.pattern_prelude(pat, st, aenv)
                inner = self.expr(body, aenv, want=want)
                bcode = ("{ " + " ".join(pre) + " " + inner + " }") if pre else inner
            arms.append(f"    {pcode}{guard} => {bcode},")
        return "(match " + code + " {\n" + "\n".join(arms) + "\n})"

    def pattern_prelude(self, pat, st, env: Env) -> List[str]:
        """Payload fields of a matched failure are read out of the bounded payload (10.5)."""
        return getattr(pat, "_prelude", []) if False else self._prelude_for.get(id(pat), [])

    _prelude_for: Dict[int, List[str]] = {}

    def pattern(self, pat, st: Optional[Type], env: Env, by_ref: bool) -> Tuple[str, str]:
        """(Rust pattern, extra guard). Failure patterns match on the code; their fields are bound in a prelude."""
        kids = children(pat)
        first = kids[0]
        s = strip(st) if st is not None else None
        if isinstance(first, Token):
            t = first.text
            if t == "_":
                return "_", ""
            if first.kind == "LOWER":
                env.set(t, Binding("ref" if by_ref else "owned", s if s is not None else Unknown()))
                return rust_ident(t), ""
            if t == "null":
                return "None", ""
            if t == "present":
                inner = s.inner if isinstance(s, OptionalT) else Unknown()
                sub, g = self.pattern(kids[2], inner, env, by_ref)
                return f"Some({sub})", g
            if t == "success":
                inner = s.args[0] if isinstance(s, Named) and s.args else Unknown()
                sub, g = self.pattern(kids[2], inner, env, by_ref)
                return f"Ok({sub})", g
            if t == "failure":
                sub, g = self.pattern(kids[2], Named("Failure"), env, False)
                return f"Err({sub})", g
            if t == "otherwise":
                if len(kids) >= 3 and isinstance(kids[2], Token):
                    env.set(kids[2].text, Binding("owned" if getattr(self, "scrutinee_owned", True) else "ref", Named("Failure")))
                    return rust_ident(kids[2].text), ""
                if not getattr(self, "scrutinee_owned", True):
                    return "_", ""
                self._prelude_for[id(pat)] = ["__f.recover();"]
                return "__f", ""
            if t == "is":
                ename = kids[1].text
                ed = self.table.get(ename)
                code = ed.code if ed is not None else ename
                ws = [k.text for k in kids if isinstance(k, Token)]
                bound = ws[ws.index("as") + 1] if "as" in ws else None
                var = rust_ident(bound) if bound else "__f"
                pre = []
                pf = find(pat, "patternFields")
                if pf is not None:
                    fields = [c for c in children(pf) if name(c) == "patternField"]
                    keys = list(ed.payload) if ed is not None else []
                    for i, f in enumerate(fields):
                        ks = children(f)
                        if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                            key, sub = ks[0].text, ks[2]
                        else:
                            key, sub = keys[i], ks[0]
                        subname = tokens(sub)[0].text
                        ft = ed.payload.get(key) if ed is not None else Unknown()
                        getter = "int" if is_trivial(ft) else "text"
                        pre.append(f"let {rust_ident(subname)} = {var}.payload.{getter}(\"{key}\");")
                        env.set(subname, Binding("owned", ft if ft is not None else Unknown()))
                if bound:
                    env.set(bound, Binding("owned" if getattr(self, "scrutinee_owned", True) else "ref", Named("Failure")))
                elif getattr(self, "scrutinee_owned", True):
                    pre.append(f"{var}.recover();")          # an is-arm without `as` discharges the failure (10.5)
                self._prelude_for[id(pat)] = pre
                return var, f"{var}.code() == \"{code}\""
            if first.kind == "UPPER":
                en = tname(s) if s is not None else ""
                ed = self.table.get(en)
                pf = find(pat, "patternFields")
                if ed is not None and ed.kind == "class":
                    fields = [c for c in children(pf) if name(c) == "patternField"] if pf is not None else []
                    parts = []
                    for f in fields:
                        ks = children(f)
                        key, sub = ks[0].text, ks[2]
                        subname = tokens(sub)[0].text
                        env.set(subname, Binding("owned", ed.fields[key][0] if key in ed.fields else Unknown()))
                        parts.append(f"{rust_ident(key)}: {rust_ident(subname)}")
                    return f"{rust_type(en)} {{ {', '.join(parts)}, .. }}", ""
                if ed is None or t not in ed.variants:
                    return f"{rust_type(en)}::{t}" if en else t, ""
                payload = dict(ed.variants[t])
                if pf is None:
                    return f"{rust_type(en)}::{t}" + (" { .. }" if payload else ""), ""
                fields = [c for c in children(pf) if name(c) == "patternField"]
                keys = list(payload)
                parts = []
                for i, f in enumerate(fields):
                    ks = children(f)
                    if isinstance(ks[0], Token) and ks[0].kind == "LOWER" and len(ks) > 1 and isinstance(ks[1], Token) and ks[1].text == ":":
                        key, sub = ks[0].text, ks[2]
                    else:
                        key, sub = keys[i], ks[0]
                    subcode, _ = self.pattern(sub, payload.get(key), env, by_ref)
                    parts.append(f"{rust_ident(key)}: {subcode}")
                return f"{rust_type(en)}::{t} {{ {', '.join(parts)}, .. }}", ""
            if t == "(":
                subs = [k for k in kids if name(k) == "pattern"]
                items = s.items if isinstance(s, TupleT) else [Unknown()] * len(subs)
                codes = [self.pattern(p, it, env, by_ref)[0] for p, it in zip(subs, items)]
                return "(" + ", ".join(codes) + ")", ""
        if name(first) == "literal":
            tok = children(first)[0]
            if tok.kind == "STR":
                return f"__s if __s.to_string() == {self.literal(tok, env, None)}", ""
            return self.literal(tok, env, None), ""
        raise LowerError("pattern")

    def if_expr(self, node, env: Env, want) -> str:
        kids = children(node)
        cond = self.expr(kids[1], env)
        def branch(b):
            if name(b) == "ifExpr":
                return self.if_expr(b, env, want)
            label = self.new_label()
            saved = getattr(self, "give_label", None)
            self.give_label = label
            code = self.block_code(b, env, give_label=label)
            self.give_label = saved
            return code
        return f"(if {cond} {branch(kids[2])} else {branch(kids[4])})"


def root_name(pl) -> str:
    return tokens(pl)[0].text


RUST_NAMES = {"Box", "Vec", "Option", "Result", "String", "Some", "None", "Ok", "Err", "Rc", "Cell", "Drop", "Default", "Clone", "Copy", "Fn", "Iterator", "Send", "Sync", "Path", "Failure", "Report", "Must", "Console", "Host", "Payload"}


def rust_type(n: str) -> str:
    return n + "_" if n in RUST_NAMES else n


def lower_file(path: str, out_dir: str) -> Lowering:
    src = open(path, encoding="utf-8").read()
    lw = Lowering(src, path)
    files = lw.crate()
    os.makedirs(os.path.join(out_dir, "src"), exist_ok=True)
    name_ = re.sub(r"[^a-z0-9_]", "_", os.path.basename(path)[:-3].lower())
    if name_[:1].isdigit():
        name_ = "f_" + name_
    rt = os.path.relpath(os.path.join(HERE, "runtime"), out_dir)
    cargo = f'[package]\nname = "{name_}"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nfortis_rt = {{ path = "{rt}" }}\n\n[profile.dev]\nopt-level = 0\n'
    if "src/lib.rs" in files:
        cargo += '\n[lib]\npath = "src/lib.rs"\n'
    open(os.path.join(out_dir, "Cargo.toml"), "w", encoding="utf-8").write(cargo)
    for f, body in files.items():
        p = os.path.join(out_dir, f)
        open(p, "w", encoding="utf-8").write(body)
        try:
            import subprocess
            subprocess.run(["rustfmt", "--edition", "2021", p], check=False, capture_output=True, timeout=30)   # formatting only; the audit reads the formatted file
        except (OSError, subprocess.TimeoutExpired):
            pass
    return lw


if __name__ == "__main__":
    lw = lower_file(sys.argv[1], sys.argv[2])
    print(f"lowered {sys.argv[1]} -> {sys.argv[2]} ({len(lw.copies)} attributed copies)")
