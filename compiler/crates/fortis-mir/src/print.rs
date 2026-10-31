//! The text form of the typed IR: one readable rendering per program, used for goldens and debugging.

use std::fmt::Write;

use crate::model::*;

pub fn program(p: &Program) -> String {
    let mut out = String::new();
    for t in &p.types {
        type_def(&mut out, t);
    }
    for c in &p.consts {
        let _ = writeln!(out, "const {}::{}: {} = {}", c.module, c.name, c.ty.show(), expr(None, &c.value));
    }
    for f in &p.functions {
        out.push_str(&function(f));
    }
    if let Some(e) = &p.entry {
        let _ = writeln!(out, "entry {e}");
    }
    for t in &p.tests {
        let _ = writeln!(out, "{} {} -> {}", t.kind, t.name, t.function);
    }
    for (t, r) in &p.unbuilt_tests {
        let _ = writeln!(out, "unbuilt test {t:?}: {r}");
    }
    out
}

fn type_def(out: &mut String, t: &TypeDef) {
    let tp = if t.type_params.is_empty() { String::new() } else { format!("[{}]", t.type_params.join(", ")) };
    let mut flags = vec![];
    if t.trivial {
        flags.push("trivial".to_string());
    }
    if t.copyable {
        flags.push("copyable".to_string());
    }
    if !t.implements.is_empty() {
        flags.push(format!("implements {}", t.implements.join(", ")));
    }
    if let Some(c) = &t.cleanup {
        flags.push(format!("cleanup {c}"));
    }
    let fl = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join("; ")) };
    match &t.kind {
        TypeKind::Class { fields } => {
            let _ = writeln!(out, "class {}::{}{tp}{fl} {{", t.module, t.name);
            for f in fields {
                let d = f.default.as_ref().map_or(String::new(), |e| format!(" = {}", expr(None, e)));
                let _ = writeln!(out, "  {}{}: {}{d}", if f.exported { "export " } else { "" }, f.name, f.ty.show());
            }
        }
        TypeKind::Enum { variants } => {
            let _ = writeln!(out, "enum {}::{}{tp}{fl} {{", t.module, t.name);
            for v in variants {
                let fs: Vec<String> = v.fields.iter().map(|(n, ty)| format!("{n}: {}", ty.show())).collect();
                let _ = writeln!(out, "  {}{}", v.name, if fs.is_empty() { String::new() } else { format!("({})", fs.join(", ")) });
            }
        }
        TypeKind::Newtype { repr } => {
            let _ = writeln!(out, "type {}::{}{fl} = {} {{", t.module, t.name, repr.show());
        }
        TypeKind::Error { code, payload, message } => {
            let _ = writeln!(out, "error {}::{}{fl} {{ code {code:?}; message {message:?}", t.module, t.name);
            for f in payload {
                let _ = writeln!(out, "  {}: {}", f.name, f.ty.show());
            }
        }
        TypeKind::Interface { methods } => {
            let ms: Vec<String> = methods.iter().map(|(m, mods)| if mods.is_empty() { m.clone() } else { format!("{m} [{}]", mods.join(" ")) }).collect();
            let _ = writeln!(out, "interface {}::{}{tp}{fl} {{ {} ", t.module, t.name, ms.join(", "));
        }
        TypeKind::Mixin { provides, requires } => {
            let _ = writeln!(out, "mixin {}::{}{fl} {{ provides {}; requires {}", t.module, t.name, provides.join(", "), requires.join(", "));
        }
        TypeKind::Observation { fields, version } => {
            let _ = writeln!(out, "observation {}::{}{fl} {{ version {version}", t.module, t.name);
            for (n, ty, req) in fields {
                let _ = writeln!(out, "  {n}: {} required {req}", ty.show());
            }
        }
    }
    for (m, f) in &t.methods {
        let _ = writeln!(out, "  method {m} -> {f}");
    }
    out.push_str("}\n");
}

pub fn function(f: &Function) -> String {
    let mut out = String::new();
    let ps: Vec<String> = f.params.iter().map(|p| format!("{} _{} {}: {}", p.mode.word(), p.local, p.name, f.local(p.local).ty.show())).collect();
    let mods = if f.modifiers.is_empty() { String::new() } else { format!(" [{}]", f.modifiers.join(" ")) };
    let _ = writeln!(out, "fn {}({}) {}{mods}  // line {}", f.name, ps.join(", "), f.ret.show(), f.line);
    if !f.effects.effects.is_empty() {
        let es: Vec<String> = f.effects.effects.iter().map(|(e, p)| format!("{e}:{p}")).collect();
        let _ = writeln!(out, "  effects {}", es.join(" "));
    }
    if let Some(o) = &f.observe {
        let _ = writeln!(out, "  observe {o}");
    }
    if let Some(c) = &f.closure {
        let cs: Vec<String> = c.captures.iter().map(|c| format!("{} _{} {}", c.mode.word(), c.local, c.name)).collect();
        let _ = writeln!(out, "  closure {} [{}]", c.kind, cs.join(", "));
    }
    for (i, l) in f.locals.iter().enumerate() {
        let _ = writeln!(out, "  local _{i} {}{}: {} ({:?}{})", if l.mutable { "mut " } else { "" }, l.name, l.ty.show(), l.kind, if l.boxed { ", boxed" } else { "" });
    }
    match &f.body {
        None => out.push_str("  trusted\n"),
        Some(b) => block(&mut out, Some(f), b, 1),
    }
    out
}

fn ind(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn block(out: &mut String, f: Option<&Function>, b: &Block, depth: usize) {
    ind(out, depth);
    let ls: Vec<String> = b.locals.iter().map(|l| format!("_{l}")).collect();
    let _ = writeln!(out, "{{{}", if ls.is_empty() { String::new() } else { format!("  // declares {}", ls.join(" ")) });
    for d in &b.defers {
        ind(out, depth + 1);
        out.push_str("defer\n");
        block(out, f, d, depth + 2);
    }
    for s in &b.stmts {
        stmt(out, f, s, depth + 1);
    }
    ind(out, depth);
    out.push_str("}\n");
}

fn stmt(out: &mut String, f: Option<&Function>, s: &Stmt, depth: usize) {
    ind(out, depth);
    let e = |x: &Expr| expr(f, x);
    match &s.kind {
        StmtKind::Let { local, init } => {
            let _ = writeln!(out, "let _{local} = {}", e(init));
        }
        StmtKind::LetPattern { pattern: p, init } => {
            let _ = writeln!(out, "let {} = {}", pattern(p), e(init));
        }
        StmtKind::Assign { place: p, value, drop_old } => {
            let _ = writeln!(out, "{} = {}{}", place(p), e(value), if *drop_old { "  // drops the old value" } else { "" });
        }
        StmtKind::Compound { place: p, op, value } => {
            let _ = writeln!(out, "{} {}= {}", place(p), op.symbol(), e(value));
        }
        StmtKind::Expr(x) => {
            let _ = writeln!(out, "{}", e(x));
        }
        StmtKind::Discard(x) => {
            let _ = writeln!(out, "discard {}", e(x));
        }
        StmtKind::If { cond, then, els } => {
            let _ = writeln!(out, "if {}", e(cond));
            block(out, f, then, depth + 1);
            if let Some(b) = els {
                ind(out, depth);
                out.push_str("else\n");
                block(out, f, b, depth + 1);
            }
        }
        StmtKind::Loop { label, body } => {
            let _ = writeln!(out, "loop{}", label.as_ref().map_or(String::new(), |l| format!(" '{l}")));
            block(out, f, body, depth + 1);
        }
        StmtKind::For { label, binding, source, body } => {
            let src = match source {
                ForSource::Range { lo, hi, inclusive } => format!("{} {} {}", e(lo), if *inclusive { ".." } else { "..<" }, e(hi)),
                ForSource::Items(x) => format!("items({})", e(x)),
            };
            let b = binding.map_or("_".to_string(), |l| format!("_{l}"));
            let _ = writeln!(out, "for{} {b} in {src}", label.as_ref().map_or(String::new(), |l| format!(" '{l}")));
            block(out, f, body, depth + 1);
        }
        StmtKind::Break(l) => {
            let _ = writeln!(out, "break{}", l.as_ref().map_or(String::new(), |l| format!(" '{l}")));
        }
        StmtKind::Continue(l) => {
            let _ = writeln!(out, "continue{}", l.as_ref().map_or(String::new(), |l| format!(" '{l}")));
        }
        StmtKind::Return(x) => {
            let _ = writeln!(out, "return{}", x.as_ref().map_or(String::new(), |x| format!(" {}", e(x))));
        }
        StmtKind::Give(x) => {
            let _ = writeln!(out, "give {}", e(x));
        }
        StmtKind::Recover(x) => {
            let _ = writeln!(out, "recover {}", e(x));
        }
        StmtKind::Fail(x) => {
            let _ = writeln!(out, "fail {}", e(x));
        }
        StmtKind::Block(b) => {
            out.push_str("block\n");
            block(out, f, b, depth + 1);
        }
        StmtKind::Match { scrutinee, arms: a } => {
            let _ = writeln!(out, "match {}", e(scrutinee));
            arms(out, f, a, depth + 1);
        }
        StmtKind::Capture { field, value } => {
            let _ = writeln!(out, "capture {field} = {}", e(value));
        }
        StmtKind::Record { schema, fields } => {
            let fs: Vec<String> = fields.iter().map(|(n, x)| format!("{n}: {}", e(x))).collect();
            let _ = writeln!(out, "record {schema}({})", fs.join(", "));
        }
        StmtKind::Report { value, bind } => {
            let _ = writeln!(out, "report {}{}", e(value), bind.map_or(String::new(), |b| format!(" as _{b}")));
        }
        StmtKind::Expect { cond, evidence } => {
            let ev = evidence.as_ref().map_or(String::new(), |(a, op, b)| format!("  // evidence {} {op} {}", e(a), e(b)));
            let _ = writeln!(out, "expect {}{ev}", e(cond));
        }
        StmtKind::Defer(i) => {
            let _ = writeln!(out, "defer #{i}");
        }
        StmtKind::Scope { local, body } => {
            let _ = writeln!(out, "scope _{local}");
            block(out, f, body, depth + 1);
        }
        StmtKind::Drop(p) => {
            let _ = writeln!(out, "drop {}", place(p));
        }
    }
}

fn arms(out: &mut String, f: Option<&Function>, arms: &[Arm], depth: usize) {
    for a in arms {
        ind(out, depth);
        let g = a.guard.as_ref().map_or(String::new(), |g| format!(" when {}", expr(f, g)));
        match &a.body {
            ArmBody::Expr(x) => {
                let _ = writeln!(out, "{}{g} => {}", pattern(&a.pattern), expr(f, x));
            }
            ArmBody::Block(b) => {
                let _ = writeln!(out, "{}{g} =>", pattern(&a.pattern));
                block(out, f, b, depth + 1);
            }
        }
    }
}

pub fn place(p: &Place) -> String {
    let mut s = format!("_{}", p.local);
    for pr in &p.proj {
        match pr {
            Proj::Field(n) => {
                let _ = write!(s, ".{n}");
            }
            Proj::Tuple(i) => {
                let _ = write!(s, ".{i}");
            }
            Proj::Index(i) => {
                let _ = write!(s, "[{i}]");
            }
        }
    }
    s
}

pub fn pattern(p: &Pattern) -> String {
    let fields = |fs: &[(String, Pattern)]| fs.iter().map(|(n, p)| format!("{n}: {}", pattern(p))).collect::<Vec<_>>().join(", ");
    match p {
        Pattern::Wildcard => "_".into(),
        Pattern::Bind(l) => format!("_{l}"),
        Pattern::Literal(c) => konst(c),
        Pattern::Null => "null".into(),
        Pattern::Present(x) => format!("present({})", pattern(x)),
        Pattern::Success(x) => format!("success({})", pattern(x)),
        Pattern::Failure(x) => format!("failure({})", pattern(x)),
        Pattern::Variant { ty, variant, fields: fs } => format!("{ty}.{variant}({})", fields(fs)),
        Pattern::Class { ty, fields: fs } => format!("{ty}({})", fields(fs)),
        Pattern::Is { ty, fields: fs, bind } => format!("is {ty}({}){}", fields(fs), bind.map_or(String::new(), |b| format!(" as _{b}"))),
        Pattern::Tuple(ps) => format!("({})", ps.iter().map(pattern).collect::<Vec<_>>().join(", ")),
        Pattern::Record(fs) => format!("{{{}}}", fields(fs)),
        Pattern::Otherwise(b) => format!("otherwise{}", b.map_or(String::new(), |b| format!("(_{b})"))),
        Pattern::Range { lo, hi } => format!("{}..{}", lo.map_or(String::new(), |n| n.to_string()), hi.map_or(String::new(), |n| n.to_string())),
    }
}

pub fn konst(c: &Const) -> String {
    match c {
        Const::Void => "void".into(),
        Const::Bool(b) => b.to_string(),
        Const::Int(n) => n.to_string(),
        Const::Float(b) => format!("{:?}", f64::from_bits(*b)),
        Const::Float32(b) => format!("{:?}f32", f32::from_bits(*b)),
        Const::Decimal(d) => format!("{d}d"),
        Const::Duration(ns) => format!("{ns}ns"),
        Const::Str(s) => format!("{s:?}"),
        Const::Bytes(b) => format!("b{:?}", String::from_utf8_lossy(b)),
        Const::Null => "null".into(),
    }
}

fn args(f: Option<&Function>, args: &[Arg]) -> String {
    args.iter().map(|a| format!("{}{}{}", a.param, if a.mode == Mode::Read { ": ".to_string() } else { format!(": {} ", a.mode.word()) }, expr(f, &a.value))).collect::<Vec<_>>().join(", ")
}

pub fn expr(f: Option<&Function>, e: &Expr) -> String {
    let x = |e: &Expr| expr(f, e);
    let fields = |fs: &[(String, Expr)]| fs.iter().map(|(n, e)| format!("{n}: {}", expr(f, e))).collect::<Vec<_>>().join(", ");
    match &e.kind {
        ExprKind::Const(c) => konst(c),
        ExprKind::Place(p, u) => {
            if *u == Use::Read {
                place(p)
            } else {
                format!("{} {}", u.word(), place(p))
            }
        }
        ExprKind::Global(g) => format!("const {g}"),
        ExprKind::FuncRef(g) => format!("fn {g}"),
        ExprKind::Binary(op, a, b) => format!("({} {} {})", x(a), op.symbol(), x(b)),
        ExprKind::Codec { op, target, arg } => format!("codec {}[{}]({})", op.name(), target.show(), x(arg)),
        ExprKind::Unary(UnOp::Neg, a) => format!("-{}", x(a)),
        ExprKind::Unary(UnOp::Not, a) => format!("!{}", x(a)),
        ExprKind::And(a, b) => format!("({} && {})", x(a), x(b)),
        ExprKind::Or(a, b) => format!("({} || {})", x(a), x(b)),
        ExprKind::Call { callee, args: a } => {
            let c = match callee {
                Callee::Func(n) => n.clone(),
                Callee::Intrinsic(n) => format!("intrinsic {n}"),
                Callee::Virtual { method } => format!("virtual .{method}"),
                Callee::Value(v) => format!("call {}", x(v)),
            };
            format!("{c}({})", args(f, a))
        }
        ExprKind::Construct { ty, fields: fs } => format!("{ty} {{ {} }}", fields(fs)),
        ExprKind::Variant { ty, variant, fields: fs } => format!("{ty}.{variant}({})", fields(fs)),
        ExprKind::MakeFailure { ty, fields: fs } => format!("failure {ty}({})", fields(fs)),
        ExprKind::Tuple(es) => format!("({})", es.iter().map(x).collect::<Vec<_>>().join(", ")),
        ExprKind::Record(fs) => format!("{{ {} }}", fields(fs)),
        ExprKind::Closure { func, captures } => format!("closure {func} [{}]", captures.iter().map(x).collect::<Vec<_>>().join(", ")),
        ExprKind::Interpolate(parts) => {
            let ps: Vec<String> = parts.iter().map(|p| match p {
                Part::Lit(s) => format!("{s:?}"),
                Part::Show(e) => format!("show({})", x(e)),
            }).collect();
            format!("interpolate({})", ps.join(" ++ "))
        }
        ExprKind::Field(a, n) => format!("{}.{n}", x(a)),
        ExprKind::TupleField(a, i) => format!("{}.{i}", x(a)),
        ExprKind::Index(a, i) => format!("{}[{}]", x(a), x(i)),
        ExprKind::OptField(a, n) => format!("{}?.{n}", x(a)),
        ExprKind::Present(a) => format!("present({})", x(a)),
        ExprKind::Success(a) => format!("success({})", x(a)),
        ExprKind::Coalesce(a, b) => format!("({} ?? {})", x(a), x(b)),
        ExprKind::OrFail(a) => format!("({} or fail)", x(a)),
        ExprKind::OrRecover(a, b) => format!("({} or recover {})", x(a), x(b)),
        ExprKind::Catch { value, bind, handler } => {
            let mut s = String::new();
            block(&mut s, f, handler, 0);
            format!("({} catch _{bind} {})", x(value), s.trim_end().replace('\n', " "))
        }
        ExprKind::If { cond, then, els } => {
            let (mut a, mut b) = (String::new(), String::new());
            block(&mut a, f, then, 0);
            block(&mut b, f, els, 0);
            format!("if {} {} else {}", x(cond), a.trim_end().replace('\n', " "), b.trim_end().replace('\n', " "))
        }
        ExprKind::Match { scrutinee, arms: a } => {
            let mut s = String::new();
            arms(&mut s, f, a, 0);
            format!("match {} {{ {} }}", x(scrutinee), s.trim_end().replace('\n', "; "))
        }
        ExprKind::Block(b) => {
            let mut s = String::new();
            block(&mut s, f, b, 0);
            s.trim_end().replace('\n', " ")
        }
        ExprKind::Fail(a) => format!("fail {}", x(a)),
        ExprKind::Validate { value, lo, hi, length } => format!(
            "validate{}({}, {}..{})",
            if *length { " length" } else { "" },
            x(value),
            lo.map_or(String::new(), |v| v.to_string()),
            hi.map_or(String::new(), |v| v.to_string())
        ),
        ExprKind::Wrap { ty, value } => format!("{ty}({})", x(value)),
        ExprKind::Await(a) => format!("await {}", x(a)),
        ExprKind::ToDynamic { iface, value, borrowed } => format!("{}dynamic {iface}({})", if *borrowed { "alias " } else { "" }, x(value)),
        ExprKind::Temp { local, value } => format!("temp _{local}({})", x(value)),
    }
}
