//! The canonical codec in generated code (Language Reference 6.10; DEL-39): a `rt::codec::Codec` implementation for
//! each Encodable user class and enum, and the static refinement descriptor (`rt::codec::Refine`) of each decoded
//! position, since refinements are erased from native types.

use fortis_mir::model::*;
use fortis_types::types::{strip, Type};

use crate::types::*;

fn bytes(t: &Type) -> bool {
    matches!(name_of(t), Some("Bytes") | Some("BytesView"))
}

const BUILTIN_CODEC: &[&str] = &[
    "Int", "Int8", "Int16", "Int32", "Int64", "UInt", "UInt8", "UInt16", "UInt32", "UInt64", "Byte", "Bool", "Float", "Float32", "Decimal", "String", "StringView", "Scalar", "Bytes",
    "BytesView", "Duration", "Timestamp",
];

/// Whether the native value of `t` implements `rt::codec::Codec` (a generic or lifetime-carrying type does not).
pub fn codec_able(t: &Type, prog: &Program, seen: &mut Vec<String>) -> bool {
    match strip(t) {
        Type::Optional(i) => codec_able(i, prog, seen),
        Type::Tuple(ts) => (2..=4).contains(&ts.len()) && ts.iter().all(|x| codec_able(x, prog, seen)),
        Type::Named(_) | Type::Refined { .. } => {
            let n = name_of(t).unwrap_or("");
            if BUILTIN_CODEC.contains(&n) {
                return true;
            }
            let args = args_of(t);
            match n {
                "List" | "ListView" | "Array" | "Set" => return args.first().map_or(false, |a| codec_able(a, prog, seen)),
                "Map" => return args.len() == 2 && args.iter().all(|a| codec_able(a, prog, seen)),
                _ => {}
            }
            if seen.iter().any(|s| s == n) {
                return true;
            }
            match prog.type_def(n) {
                Some(d) if d.type_params.is_empty() && d.lifetimes.is_empty() => {
                    seen.push(n.to_string());
                    let ok = match &d.kind {
                        TypeKind::Class { fields } => fields.iter().all(|f| codec_able(&f.ty, prog, seen)),
                        TypeKind::Enum { variants } => variants.iter().all(|v| v.fields.iter().all(|(_, ft)| codec_able(ft, prog, seen))),
                        TypeKind::Newtype { repr } => codec_able(repr, prog, seen),
                        _ => false,
                    };
                    seen.pop();
                    ok
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// The user and library types a program encodes or decodes: the types at its codec sites and, recursively, their
/// fields' and variants' types. Only these get a `Codec` impl (a class holding a capability or a `Must` is never
/// encoded, and needs none).
pub fn codec_types(prog: &Program) -> std::collections::BTreeSet<String> {
    let mut roots: Vec<Type> = vec![];
    fn walk_e(e: &Expr, roots: &mut Vec<Type>) {
        if let ExprKind::Codec { target, arg, .. } = &e.kind {
            roots.push(target.clone());
            roots.push(arg.ty.clone());
        }
    }
    for f in &prog.functions {
        if let Some(b) = &f.body {
            fortis_mir_walk(b, &mut |e| walk_e(e, &mut roots));
        }
    }
    let mut out = std::collections::BTreeSet::new();
    let mut work = roots;
    while let Some(t) = work.pop() {
        match strip(&t) {
            Type::Optional(i) => work.push((**i).clone()),
            Type::Tuple(ts) => work.extend(ts.iter().cloned()),
            Type::Record(fs) => work.extend(fs.iter().map(|(_, x)| x.clone())),
            Type::Named(_) | Type::Refined { .. } => {
                work.extend(args_of(&t));
                let n = name_of(&t).unwrap_or("").to_string();
                if let Some(d) = prog.type_def(&n) {
                    if out.insert(n) {
                        match &d.kind {
                            TypeKind::Class { fields } => work.extend(fields.iter().map(|f| f.ty.clone())),
                            TypeKind::Enum { variants } => work.extend(variants.iter().flat_map(|v| v.fields.iter().map(|(_, ft)| ft.clone()))),
                            TypeKind::Newtype { repr } => work.push(repr.clone()),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Every expression of a block, statements and nested blocks included.
fn fortis_mir_walk(b: &Block, f: &mut dyn FnMut(&Expr)) {
    fn stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
        match &s.kind {
            StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } => expr(init, f),
            StmtKind::Assign { value, .. } | StmtKind::Compound { value, .. } | StmtKind::Expr(value) | StmtKind::Discard(value) | StmtKind::Give(value) | StmtKind::Recover(value) | StmtKind::Fail(value) | StmtKind::Capture { value, .. } | StmtKind::Report { value, .. } => expr(value, f),
            StmtKind::Return(Some(e)) => expr(e, f),
            StmtKind::If { cond, then, els } => {
                expr(cond, f);
                fortis_mir_walk(then, f);
                if let Some(e) = els {
                    fortis_mir_walk(e, f);
                }
            }
            StmtKind::Loop { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => fortis_mir_walk(body, f),
            StmtKind::For { source, body, .. } => {
                match source {
                    ForSource::Range { lo, hi, .. } => {
                        expr(lo, f);
                        expr(hi, f);
                    }
                    ForSource::Items(e) => expr(e, f),
                }
                fortis_mir_walk(body, f);
            }
            StmtKind::Match { scrutinee, arms } => {
                expr(scrutinee, f);
                for a in arms {
                    match &a.body {
                        ArmBody::Expr(e) => expr(e, f),
                        ArmBody::Block(b) => fortis_mir_walk(b, f),
                    }
                }
            }
            StmtKind::Record { fields, .. } => fields.iter().for_each(|(_, e)| expr(e, f)),
            StmtKind::Expect { cond, .. } => expr(cond, f),
            _ => {}
        }
    }
    fn expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
        f(e);
        match &e.kind {
            ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
                expr(a, f);
                expr(b, f);
            }
            ExprKind::Unary(_, a) | ExprKind::Field(a, _) | ExprKind::TupleField(a, _) | ExprKind::OptField(a, _) | ExprKind::Present(a) | ExprKind::Success(a) | ExprKind::OrFail(a) | ExprKind::Fail(a) | ExprKind::Await(a) | ExprKind::Temp { value: a, .. } | ExprKind::Codec { arg: a, .. } => expr(a, f),
            ExprKind::Catch { value, handler, .. } => {
                expr(value, f);
                fortis_mir_walk(handler, f);
            }
            ExprKind::Call { callee, args } => {
                if let Callee::Value(v) = callee {
                    expr(v, f);
                }
                args.iter().for_each(|a| expr(&a.value, f));
            }
            ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::Record(fields) => fields.iter().for_each(|(_, x)| expr(x, f)),
            ExprKind::Tuple(es) => es.iter().for_each(|x| expr(x, f)),
            ExprKind::Interpolate(parts) => parts.iter().for_each(|p| {
                if let Part::Show(x) = p {
                    expr(x, f)
                }
            }),
            ExprKind::If { cond, then, els } => {
                expr(cond, f);
                fortis_mir_walk(then, f);
                fortis_mir_walk(els, f);
            }
            ExprKind::Match { scrutinee, arms } => {
                expr(scrutinee, f);
                for a in arms {
                    match &a.body {
                        ArmBody::Expr(e) => expr(e, f),
                        ArmBody::Block(b) => fortis_mir_walk(b, f),
                    }
                }
            }
            ExprKind::Block(b) => fortis_mir_walk(b, f),
            _ => {}
        }
    }
    for d in &b.defers {
        fortis_mir_walk(d, f);
    }
    for s in &b.stmts {
        stmt(s, f);
    }
}

fn bound(b: Option<i128>) -> String {
    b.map_or("None".into(), |v| format!("Some({v})"))
}

/// The refinement descriptor of a decoded position, as a constant expression.
pub fn refine(t: &Type, prog: &Program) -> String {
    let none = "rt::codec::Refine::NONE".to_string();
    match strip(t) {
        Type::Optional(i) => refine(i, prog),
        Type::Tuple(ts) => {
            let items: Vec<String> = ts.iter().map(|x| refine(x, prog)).collect();
            if items.iter().all(|i| *i == none) {
                none
            } else {
                format!("rt::codec::Refine {{ items: &[{}], ..rt::codec::Refine::NONE }}", items.join(", "))
            }
        }
        Type::Refined { base, lo, hi, kind } => {
            let inner = refine(&Type::Named(base.clone()), prog);
            let rest = if inner == none { "rt::codec::Refine::NONE".to_string() } else { inner.trim_start_matches("rt::codec::Refine ").trim_start_matches('{').trim_end_matches('}').trim().to_string() };
            let rest = if rest == "rt::codec::Refine::NONE" { rest } else { format!("rt::codec::Refine {{ {rest} }}") };
            match (base.name.as_str(), kind.as_str()) {
                ("String" | "StringView", _) => format!("rt::codec::Refine {{ scalars: Some(({}, {})), ..{rest} }}", bound(*lo), bound(*hi)),
                ("List" | "ListView" | "Set" | "Map" | "Array", _) => format!("rt::codec::Refine {{ length: Some(({}, {})), ..{rest} }}", bound(*lo), bound(*hi)),
                _ => format!("rt::codec::Refine {{ lo: {}, hi: {}, ..{rest} }}", bound(*lo), bound(*hi)),
            }
        }
        Type::Named(n) => {
            let args = &n.args;
            match n.name.as_str() {
                "List" | "ListView" | "Array" | "Set" => match args.first().map(|a| refine(a, prog)) {
                    Some(e) if e != none => format!("rt::codec::Refine {{ elem: Some(&{e}), ..rt::codec::Refine::NONE }}"),
                    _ => none,
                },
                "Map" => {
                    let k = args.first().map(|a| refine(a, prog)).unwrap_or_else(|| none.clone());
                    let v = args.get(1).map(|a| refine(a, prog)).unwrap_or_else(|| none.clone());
                    if k == none && v == none {
                        none
                    } else {
                        format!("rt::codec::Refine {{ elem: Some(&{k}), value: Some(&{v}), ..rt::codec::Refine::NONE }}")
                    }
                }
                other => match prog.type_def(other).map(|d| &d.kind) {
                    Some(TypeKind::Newtype { repr }) => refine(repr, prog),
                    _ => none,
                },
            }
        }
        _ => none,
    }
}

/// `impl rt::codec::Codec` for a user class or enum, or None when it is not Encodable natively. `default` lowers a
/// field's default value expression.
pub fn impl_for(prog: &Program, t: &TypeDef, default: &mut dyn FnMut(&Expr) -> String) -> Option<String> {
    if !codec_able(&fortis_types::types::named(&t.name), prog, &mut vec![]) {
        return None;
    }
    let name = ident(&t.name);
    let enc_field = |f: &str, ty: &Type, text: bool| -> String {
        match (bytes(ty), text) {
            (true, true) => format!("rt::codec::BytesCodec::enc_text(&{f}, out);"),
            (true, false) => format!("rt::codec::BytesCodec::enc_bin(&{f}, out);"),
            (false, true) => format!("rt::codec::Codec::enc_text(&{f}, out);"),
            (false, false) => format!("rt::codec::Codec::enc_bin(&{f}, out);"),
        }
    };
    let dec_text_field = |slot: &str, fname: &str, ty: &Type, dflt: Option<String>| -> String {
        if bytes(ty) {
            return format!("match {slot} {{ Some(j) => rt::codec::dec_bytes_text(j)?, None => return rt::codec::bad(\"missing field {fname:?}\") }}");
        }
        let rt_ty = rust_ty(ty, prog);
        let d = match dflt {
            Some(code) => format!("Some((|| -> {rt_ty} {{ {code} }}) as fn() -> {rt_ty})"),
            None => "None".into(),
        };
        format!("rt::codec::field::<{rt_ty}>({slot}, {fname:?}, &{}, {d})?", refine(ty, prog))
    };
    let dec_bin_field = |ty: &Type| -> String {
        if bytes(ty) {
            "rt::codec::dec_bytes_bin(b)?".to_string()
        } else {
            format!("<{} as rt::codec::Codec>::dec_bin(b, &{})?", rust_ty(ty, prog), refine(ty, prog))
        }
    };
    let body = match &t.kind {
        TypeKind::Class { fields } => {
            let names: Vec<String> = fields.iter().map(|f| format!("{:?}", f.name)).collect();
            let mut et = vec!["out.push('{');".to_string()];
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    et.push("out.push(',');".into());
                }
                et.push(format!("rt::codec::text_field_name({:?}, out);", f.name));
                et.push(enc_field(&format!("self.{}", ident(&f.name)), &f.ty, true));
            }
            et.push("out.push('}');".into());
            let mut eb = vec![format!("out.push(rt::codec::T_RECORD); rt::codec::varint(out, {});", fields.len())];
            for f in fields {
                eb.push(enc_field(&format!("self.{}", ident(&f.name)), &f.ty, false));
            }
            let dt: Vec<String> = fields.iter().enumerate().map(|(i, f)| format!("{}: {}", ident(&f.name), dec_text_field(&format!("__f[{i}]"), &f.name, &f.ty, f.default.as_ref().map(|e| default(e))))).collect();
            let db: Vec<String> = fields.iter().map(|f| format!("{}: {}", ident(&f.name), dec_bin_field(&f.ty))).collect();
            format!(
                "    fn enc_text(&self, out: &mut String) {{\n        {}\n    }}\n    fn enc_bin(&self, out: &mut Vec<u8>) {{\n        {}\n    }}\n    fn dec_text(j: &rt::codec::J, _r: &rt::codec::Refine) -> rt::codec::D<Self> {{\n        let __f = rt::codec::fields(j, &[{}])?;\n        Ok({name} {{ {} }})\n    }}\n    fn dec_bin(b: &mut rt::codec::Bin, _r: &rt::codec::Refine) -> rt::codec::D<Self> {{\n        rt::codec::record_header(b, {})?;\n        Ok({name} {{ {} }})\n    }}\n",
                et.join(" "),
                eb.join(" "),
                names.join(", "),
                dt.join(", "),
                fields.len(),
                db.join(", ")
            )
        }
        TypeKind::Enum { variants } => {
            let mut et = vec![];
            let mut eb = vec![];
            let mut dt = vec![];
            let mut db = vec![];
            for (vi, v) in variants.iter().enumerate() {
                let vn = ident(&v.name);
                let binds: Vec<String> = v.fields.iter().map(|(n, _)| ident(n)).collect();
                let pat = if v.fields.is_empty() { format!("{name}::{vn}") } else { format!("{name}::{vn} {{ {} }}", binds.join(", ")) };
                if v.fields.is_empty() {
                    et.push(format!("{pat} => rt::codec::json_string({:?}, out),", v.name));
                    dt.push(format!("({:?}, None) => Ok({pat}),", v.name));
                } else {
                    let mut parts = vec![format!("out.push('{{'); rt::codec::text_field_name({:?}, out); out.push('{{');", v.name)];
                    for (i, (n, ft)) in v.fields.iter().enumerate() {
                        if i > 0 {
                            parts.push("out.push(',');".into());
                        }
                        parts.push(format!("rt::codec::text_field_name({n:?}, out);"));
                        // a boxed field of a recursive enum encodes through the box (see `boxes_field`)
                        let acc = if crate::boxes_field(prog, &t.name, ft) { format!("(**{})", ident(n)) } else { format!("(*{})", ident(n)) };
                        parts.push(enc_field(&acc, ft, true));
                    }
                    parts.push("out.push_str(\"}}\");".into());
                    et.push(format!("{pat} => {{ {} }}", parts.join(" ")));
                    let names: Vec<String> = v.fields.iter().map(|(n, _)| format!("{n:?}")).collect();
                    let vals: Vec<String> = v.fields.iter().enumerate().map(|(i, (n, ft))| { let dv = dec_text_field(&format!("__f[{i}]"), n, ft, None); if crate::boxes_field(prog, &t.name, ft) { format!("{}: Box::new({})", ident(n), dv) } else { format!("{}: {}", ident(n), dv) } }).collect();
                    dt.push(format!("({:?}, Some(body)) => {{ let __f = rt::codec::fields(body, &[{}])?; Ok({name}::{vn} {{ {} }}) }}", v.name, names.join(", "), vals.join(", ")));
                }
                let mut bparts = vec![format!("out.push(rt::codec::T_VARIANT); rt::codec::varint(out, {vi}); rt::codec::varint(out, {});", v.fields.len())];
                for (n, ft) in &v.fields {
                    let acc = if crate::boxes_field(prog, &t.name, ft) { format!("(**{})", ident(n)) } else { format!("(*{})", ident(n)) };
                    bparts.push(enc_field(&acc, ft, false));
                }
                eb.push(format!("{pat} => {{ {} }}", bparts.join(" ")));
                let bvals: Vec<String> = v.fields.iter().map(|(n, ft)| { let dv = dec_bin_field(ft); if crate::boxes_field(prog, &t.name, ft) { format!("{}: Box::new({})", ident(n), dv) } else { format!("{}: {}", ident(n), dv) } }).collect();
                let built = if v.fields.is_empty() { format!("{name}::{vn}") } else { format!("{name}::{vn} {{ {} }}", bvals.join(", ")) };
                db.push(format!("{vi} => {{ if b.len()? != {} {{ return rt::codec::bad(\"wrong payload count\"); }} Ok({built}) }}", v.fields.len()));
            }
            let unreachable_arm = if variants.iter().any(|v| v.name.starts_with("__")) { "" } else { "" };
            format!(
                "    fn enc_text(&self, out: &mut String) {{\n        match self {{ {}{unreachable_arm} #[allow(unreachable_patterns)] _ => {{}} }}\n    }}\n    fn enc_bin(&self, out: &mut Vec<u8>) {{\n        match self {{ {} #[allow(unreachable_patterns)] _ => {{}} }}\n    }}\n    fn dec_text(j: &rt::codec::J, _r: &rt::codec::Refine) -> rt::codec::D<Self> {{\n        match rt::codec::variant(j)? {{ {} (v, _) => rt::codec::bad(format!(\"no variant {{v:?}}\")) }}\n    }}\n    fn dec_bin(b: &mut rt::codec::Bin, _r: &rt::codec::Refine) -> rt::codec::D<Self> {{\n        b.expect(rt::codec::T_VARIANT, \"a variant\")?;\n        match b.len()? {{ {} _ => rt::codec::bad(\"variant index out of range\") }}\n    }}\n",
                et.join(" "),
                eb.join(" "),
                dt.join(" "),
                db.join(" ")
            )
        }
        _ => return None,
    };
    Some(format!("impl rt::codec::Codec for {name} {{\n{body}}}\n\n"))
}
