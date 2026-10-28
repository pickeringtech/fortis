//! The JSON form of the typed IR: lossless in both directions, so a validator fixture can be a stored program and a
//! corruption an edit of it. Every node is an object whose `k` names its kind.

use std::collections::BTreeMap;

use fortis_diag::Json;
use fortis_types::types::{NamedT, Type};

use crate::model::*;

type R<T> = Result<T, String>;

fn obj(kind: &str, mut fields: Vec<(&str, Json)>) -> Json {
    fields.push(("k", Json::str(kind)));
    Json::obj(fields)
}
fn arr<T, F: Fn(&T) -> Json>(xs: &[T], f: F) -> Json {
    Json::Arr(xs.iter().map(f).collect())
}
fn opt<T, F: Fn(&T) -> Json>(x: &Option<T>, f: F) -> Json {
    x.as_ref().map_or(Json::Null, f)
}
fn int(n: impl Into<i64>) -> Json {
    Json::Int(n.into())
}
fn s(x: &str) -> Json {
    Json::str(x)
}
fn strs(xs: &[String]) -> Json {
    Json::strs(xs.iter().cloned())
}

fn get<'j>(j: &'j Json, key: &str) -> R<&'j Json> {
    j.get(key).ok_or_else(|| format!("missing `{key}` in {}", short(j)))
}
fn short(j: &Json) -> String {
    let t = j.to_line();
    if t.len() > 120 {
        format!("{}…", &t[..t.char_indices().take_while(|(i, _)| *i < 120).last().map_or(0, |(i, c)| i + c.len_utf8())])
    } else {
        t
    }
}
fn kind(j: &Json) -> R<&str> {
    get(j, "k")?.as_str().ok_or_else(|| "`k` is not a string".to_string())
}
fn gs(j: &Json, key: &str) -> R<String> {
    get(j, key)?.as_str().map(|x| x.to_string()).ok_or_else(|| format!("`{key}` is not a string"))
}
fn gi(j: &Json, key: &str) -> R<i64> {
    get(j, key)?.as_i64().ok_or_else(|| format!("`{key}` is not an integer"))
}
fn gb(j: &Json, key: &str) -> R<bool> {
    get(j, key)?.as_bool().ok_or_else(|| format!("`{key}` is not a boolean"))
}
fn ga<'j>(j: &'j Json, key: &str) -> R<&'j Vec<Json>> {
    get(j, key)?.as_arr().ok_or_else(|| format!("`{key}` is not an array"))
}
fn gstrs(j: &Json, key: &str) -> R<Vec<String>> {
    ga(j, key)?.iter().map(|x| x.as_str().map(|s| s.to_string()).ok_or_else(|| format!("`{key}` holds a non-string"))).collect()
}
fn gopt<T, F: Fn(&Json) -> R<T>>(j: &Json, key: &str, f: F) -> R<Option<T>> {
    match j.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(x) => f(x).map(Some),
    }
}
fn gvec<T, F: Fn(&Json) -> R<T>>(j: &Json, key: &str, f: F) -> R<Vec<T>> {
    ga(j, key)?.iter().map(f).collect()
}
fn u32_of(j: &Json, key: &str) -> R<u32> {
    let n = gi(j, key)?;
    u32::try_from(n).map_err(|_| format!("`{key}` = {n} is out of range"))
}

// ---- types ---------------------------------------------------------------------------------------------------

pub fn ty(t: &Type) -> Json {
    match t {
        Type::Named(n) => {
            let mut fields = vec![("name", s(&n.name)), ("args", arr(&n.args, ty)), ("package", s(&n.package))];
            if !n.origin.is_empty() {
                fields.push(("origin", s(&n.origin)));
            }
            obj("named", fields)
        }
        Type::Refined { base, lo, hi, kind: k } => obj(
            "refined",
            vec![
                ("base", ty(&Type::Named(base.clone()))),
                ("lo", opt(lo, |v| s(&v.to_string()))),
                ("hi", opt(hi, |v| s(&v.to_string()))),
                ("kind", s(k)),
            ],
        ),
        Type::Optional(i) => obj("optional", vec![("inner", ty(i))]),
        Type::Tuple(ts) => obj("tuple", vec![("items", arr(ts, ty))]),
        Type::Record(fs) => obj("record", vec![("fields", Json::Arr(fs.iter().map(|(n, t)| Json::Arr(vec![s(n), ty(t)])).collect()))]),
        Type::Fn { params, ret, kind: k, modifiers, origin, dynamic, identity } => obj(
            "fn",
            vec![
                ("params", arr(params, ty)),
                ("ret", ty(ret)),
                ("kind", s(k)),
                ("modifiers", strs(modifiers)),
                ("origin", s(origin)),
                ("dynamic", Json::Bool(*dynamic)),
                ("identity", s(identity)),
            ],
        ),
        Type::Loan { inner, mode, origin } => obj("loan", vec![("inner", ty(inner)), ("mode", s(mode)), ("origin", s(origin))]),
        Type::Dynamic(n) => obj("dynamic", vec![("iface", ty(&Type::Named(n.clone())))]),
        Type::TypeParam(p) => obj("param", vec![("name", s(p))]),
        Type::Unknown(w) => obj("unknown", vec![("why", s(w))]),
    }
}

fn named_of(j: &Json) -> R<NamedT> {
    match de_ty(j)? {
        Type::Named(n) => Ok(n),
        other => Err(format!("expected a named type, got {}", other.show())),
    }
}

pub fn de_ty(j: &Json) -> R<Type> {
    let big = |key: &str| -> R<Option<fortis_types::types::Int>> {
        gopt(j, key, |v| v.as_str().ok_or("bound is not a string".to_string())?.parse::<fortis_types::types::Int>().map_err(|e| e.to_string()))
    };
    Ok(match kind(j)? {
        "named" => Type::Named(NamedT {
            name: gs(j, "name")?,
            args: gvec(j, "args", de_ty)?,
            package: gs(j, "package")?,
            origin: j.get("origin").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        }),
        "refined" => Type::Refined { base: named_of(get(j, "base")?)?, lo: big("lo")?, hi: big("hi")?, kind: gs(j, "kind")? },
        "optional" => Type::Optional(Box::new(de_ty(get(j, "inner")?)?)),
        "tuple" => Type::Tuple(gvec(j, "items", de_ty)?),
        "record" => Type::Record(gvec(j, "fields", |p| {
            let a = p.as_arr().ok_or("record field is not a pair")?;
            Ok((a.first().and_then(|x| x.as_str()).ok_or("record field name")?.to_string(), de_ty(a.get(1).ok_or("record field type")?)?))
        })?),
        "fn" => Type::Fn {
            params: gvec(j, "params", de_ty)?,
            ret: Box::new(de_ty(get(j, "ret")?)?),
            kind: gs(j, "kind")?,
            modifiers: gstrs(j, "modifiers")?,
            origin: gs(j, "origin")?,
            dynamic: j.get("dynamic").and_then(|v| v.as_bool()).unwrap_or(false),
            identity: j.get("identity").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        },
        "loan" => Type::Loan { inner: Box::new(de_ty(get(j, "inner")?)?), mode: gs(j, "mode")?, origin: gs(j, "origin")? },
        "dynamic" => Type::Dynamic(named_of(get(j, "iface")?)?),
        "param" => Type::TypeParam(gs(j, "name")?),
        "unknown" => Type::Unknown(gs(j, "why")?),
        k => return Err(format!("unknown type kind `{k}`")),
    })
}

// ---- program -------------------------------------------------------------------------------------------------

pub fn program(p: &Program) -> Json {
    obj(
        "program",
        vec![
            ("types", arr(&p.types, type_def)),
            ("functions", arr(&p.functions, function)),
            ("consts", arr(&p.consts, |c| obj("const", vec![("name", s(&c.name)), ("module", s(&c.module)), ("ty", ty(&c.ty)), ("value", expr(&c.value))]))),
            ("entry", opt(&p.entry, |e| s(e))),
            (
                "tests",
                arr(&p.tests, |t| {
                    obj(
                        "test",
                        vec![
                            ("name", s(&t.name)),
                            ("title", s(&t.title)),
                            ("kind", s(&t.kind)),
                            ("function", s(&t.function)),
                            ("cases", named_exprs(&t.cases)),
                            ("generators", Json::Arr(t.generators.iter().map(|(n, ty_)| Json::Arr(vec![s(n), ty(ty_)])).collect())),
                        ],
                    )
                }),
            ),
            ("unbuilt_tests", Json::Arr(p.unbuilt_tests.iter().map(|(t, r)| Json::Arr(vec![s(t), s(r)])).collect())),
        ],
    )
}

pub fn de_program(j: &Json) -> R<Program> {
    Ok(Program {
        types: gvec(j, "types", de_type_def)?,
        functions: gvec(j, "functions", de_function)?,
        consts: gvec(j, "consts", |c| Ok(ConstDef { name: gs(c, "name")?, module: gs(c, "module")?, ty: de_ty(get(c, "ty")?)?, value: de_expr(get(c, "value")?)? }))?,
        entry: gopt(j, "entry", |e| e.as_str().map(|x| x.to_string()).ok_or("entry".to_string()))?,
        tests: gvec(j, "tests", |t| {
            Ok(TestDef {
                name: gs(t, "name")?,
                title: gs(t, "title")?,
                kind: gs(t, "kind")?,
                function: gs(t, "function")?,
                cases: de_named_exprs(get(t, "cases")?)?,
                generators: gvec(t, "generators", name_ty)?,
            })
        })?,
        unbuilt_tests: match get(j, "unbuilt_tests") {
            Ok(Json::Arr(xs)) => xs
                .iter()
                .map(|x| match x {
                    Json::Arr(p) if p.len() == 2 => Ok((p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string())),
                    _ => Err("unbuilt test".to_string()),
                })
                .collect::<R<Vec<_>>>()?,
            _ => vec![],
        },
    })
}

fn field_def(f: &FieldDef) -> Json {
    obj("field", vec![("name", s(&f.name)), ("ty", ty(&f.ty)), ("exported", Json::Bool(f.exported)), ("default", opt(&f.default, expr))])
}
fn de_field_def(j: &Json) -> R<FieldDef> {
    Ok(FieldDef { name: gs(j, "name")?, ty: de_ty(get(j, "ty")?)?, exported: gb(j, "exported")?, default: gopt(j, "default", de_expr)? })
}

fn type_def(t: &TypeDef) -> Json {
    let k = match &t.kind {
        TypeKind::Class { fields } => obj("class", vec![("fields", arr(fields, field_def))]),
        TypeKind::Enum { variants } => obj(
            "enum",
            vec![("variants", arr(variants, |v| obj("variant", vec![("name", s(&v.name)), ("fields", Json::Arr(v.fields.iter().map(|(n, t)| Json::Arr(vec![s(n), ty(t)])).collect()))])))],
        ),
        TypeKind::Newtype { repr } => obj("newtype", vec![("repr", ty(repr))]),
        TypeKind::Error { code, payload, message } => obj("error", vec![("code", s(code)), ("payload", arr(payload, field_def)), ("message", s(message))]),
        TypeKind::Interface { methods } => obj("interface", vec![("methods", Json::Arr(methods.iter().map(|(m, mods)| Json::Arr(vec![s(m), strs(mods)])).collect()))]),
        TypeKind::Mixin { provides, requires } => obj("mixin", vec![("provides", strs(provides)), ("requires", strs(requires))]),
        TypeKind::Observation { fields, version } => obj("observation", vec![("fields", Json::Arr(fields.iter().map(|(n, t, r)| Json::Arr(vec![s(n), ty(t), s(r)])).collect())), ("version", int(*version))]),
    };
    obj(
        "type",
        vec![
            ("name", s(&t.name)),
            ("module", s(&t.module)),
            ("type_params", strs(&t.type_params)),
            ("def", k),
            ("trivial", Json::Bool(t.trivial)),
            ("copyable", Json::Bool(t.copyable)),
            ("implements", strs(&t.implements)),
            ("mixins", strs(&t.mixins)),
            ("lifetimes", strs(&t.lifetimes)),
            ("cleanup", opt(&t.cleanup, |c| s(c))),
            ("methods", Json::Arr(t.methods.iter().map(|(m, f)| Json::Arr(vec![s(m), s(f)])).collect())),
        ],
    )
}

fn pair_str(p: &Json) -> R<(String, String)> {
    let a = p.as_arr().ok_or("expected a pair")?;
    Ok((a.first().and_then(|x| x.as_str()).ok_or("pair[0]")?.to_string(), a.get(1).and_then(|x| x.as_str()).ok_or("pair[1]")?.to_string()))
}
fn name_ty(p: &Json) -> R<(String, Type)> {
    let a = p.as_arr().ok_or("expected a (name, type) pair")?;
    Ok((a.first().and_then(|x| x.as_str()).ok_or("name")?.to_string(), de_ty(a.get(1).ok_or("type")?)?))
}

fn de_type_def(j: &Json) -> R<TypeDef> {
    let d = get(j, "def")?;
    let kind = match kind(d)? {
        "class" => TypeKind::Class { fields: gvec(d, "fields", de_field_def)? },
        "enum" => TypeKind::Enum { variants: gvec(d, "variants", |v| Ok(VariantDef { name: gs(v, "name")?, fields: gvec(v, "fields", name_ty)? }))? },
        "newtype" => TypeKind::Newtype { repr: de_ty(get(d, "repr")?)? },
        "error" => TypeKind::Error { code: gs(d, "code")?, payload: gvec(d, "payload", de_field_def)?, message: gs(d, "message")? },
        "interface" => TypeKind::Interface {
            methods: gvec(d, "methods", |p| {
                let a = p.as_arr().ok_or("interface method")?;
                let mods = a.get(1).and_then(|x| x.as_arr()).ok_or("interface method modifiers")?.iter().map(|x| x.as_str().map(|s| s.to_string()).ok_or("modifier".to_string())).collect::<R<Vec<_>>>()?;
                Ok((a.first().and_then(|x| x.as_str()).ok_or("interface method name")?.to_string(), mods))
            })?,
        },
        "mixin" => TypeKind::Mixin { provides: gstrs(d, "provides")?, requires: gstrs(d, "requires")? },
        "observation" => TypeKind::Observation {
            fields: gvec(d, "fields", |p| {
                let a = p.as_arr().ok_or("observation field")?;
                Ok((a.first().and_then(|x| x.as_str()).ok_or("name")?.to_string(), de_ty(a.get(1).ok_or("type")?)?, a.get(2).and_then(|x| x.as_str()).ok_or("required")?.to_string()))
            })?,
            version: gi(d, "version").map_or(1, |v| v as u32),
        },
        k => return Err(format!("unknown type definition `{k}`")),
    };
    Ok(TypeDef {
        name: gs(j, "name")?,
        module: gs(j, "module")?,
        type_params: gstrs(j, "type_params")?,
        kind,
        trivial: gb(j, "trivial")?,
        copyable: gb(j, "copyable")?,
        implements: gstrs(j, "implements")?,
        mixins: gstrs(j, "mixins")?,
        lifetimes: gstrs(j, "lifetimes")?,
        cleanup: gopt(j, "cleanup", |c| c.as_str().map(|x| x.to_string()).ok_or("cleanup".to_string()))?,
        methods: gvec(j, "methods", pair_str)?,
    })
}

fn mode(m: Mode) -> Json {
    s(m.word())
}
fn de_mode(j: &Json) -> R<Mode> {
    Ok(match j.as_str().ok_or("mode is not a string")? {
        "read" => Mode::Read,
        "editable" => Mode::Editable,
        "owned" => Mode::Owned,
        m => return Err(format!("unknown mode `{m}`")),
    })
}

fn local_kind(k: LocalKind) -> &'static str {
    match k {
        LocalKind::Param => "param",
        LocalKind::SelfParam => "self",
        LocalKind::Let => "let",
        LocalKind::Binding => "binding",
        LocalKind::Capture => "capture",
        LocalKind::Temp => "temp",
    }
}
fn de_local_kind(x: &str) -> R<LocalKind> {
    Ok(match x {
        "param" => LocalKind::Param,
        "self" => LocalKind::SelfParam,
        "let" => LocalKind::Let,
        "binding" => LocalKind::Binding,
        "capture" => LocalKind::Capture,
        "temp" => LocalKind::Temp,
        k => return Err(format!("unknown local kind `{k}`")),
    })
}
fn capture_mode(m: CaptureMode) -> Json {
    s(m.word())
}
fn de_capture_mode(x: &str) -> R<CaptureMode> {
    Ok(match x {
        "value" => CaptureMode::Value,
        "copy" => CaptureMode::Copy,
        "move" => CaptureMode::Move,
        "alias" => CaptureMode::Alias,
        "editable" => CaptureMode::Editable,
        "implicit" => CaptureMode::Implicit,
        k => return Err(format!("unknown capture mode `{k}`")),
    })
}

pub fn function(f: &Function) -> Json {
    obj(
        "function",
        vec![
            ("name", s(&f.name)),
            ("label", s(&f.label)),
            ("module", s(&f.module)),
            ("line", int(f.line)),
            ("params", arr(&f.params, |p| obj("param", vec![("local", int(p.local)), ("name", s(&p.name)), ("mode", mode(p.mode)), ("default", opt(&p.default, expr))]))),
            ("ret", ty(&f.ret)),
            ("modifiers", strs(&f.modifiers)),
            (
                "locals",
                arr(&f.locals, |l| obj("local", vec![("name", s(&l.name)), ("ty", ty(&l.ty)), ("mutable", Json::Bool(l.mutable)), ("kind", s(local_kind(l.kind))), ("boxed", Json::Bool(l.boxed))])),
            ),
            ("body", opt(&f.body, block)),
            (
                "effects",
                Json::obj([
                    ("effects", Json::Arr(f.effects.effects.iter().map(|(e, p)| Json::Arr(vec![s(e), s(p)])).collect())),
                    ("footprint", strs(&f.effects.footprint)),
                    ("authorities", strs(&f.effects.authorities)),
                ]),
            ),
            ("observe", opt(&f.observe, |o| s(o))),
            (
                "closure",
                opt(&f.closure, |c| {
                    obj("closure", vec![("kind", s(&c.kind)), ("captures", arr(&c.captures, |c| obj("capture", vec![("local", int(c.local)), ("name", s(&c.name)), ("mode", capture_mode(c.mode))])))])
                }),
            ),
        ],
    )
}

pub fn de_function(j: &Json) -> R<Function> {
    let e = get(j, "effects")?;
    Ok(Function {
        name: gs(j, "name")?,
        label: gs(j, "label")?,
        module: gs(j, "module")?,
        line: u32_of(j, "line")?,
        params: gvec(j, "params", |p| Ok(Param { local: u32_of(p, "local")?, name: gs(p, "name")?, mode: de_mode(get(p, "mode")?)?, default: gopt(p, "default", de_expr)? }))?,
        ret: de_ty(get(j, "ret")?)?,
        modifiers: gstrs(j, "modifiers")?,
        locals: gvec(j, "locals", |l| {
            Ok(LocalDecl { name: gs(l, "name")?, ty: de_ty(get(l, "ty")?)?, mutable: gb(l, "mutable")?, kind: de_local_kind(&gs(l, "kind")?)?, boxed: gb(l, "boxed")? })
        })?,
        body: gopt(j, "body", de_block)?,
        effects: Effects { effects: gvec(e, "effects", pair_str)?, footprint: gstrs(e, "footprint")?, authorities: gstrs(e, "authorities")? },
        observe: gopt(j, "observe", |o| o.as_str().map(|x| x.to_string()).ok_or("observe".to_string()))?,
        closure: gopt(j, "closure", |c| {
            Ok(ClosureDef {
                kind: gs(c, "kind")?,
                captures: gvec(c, "captures", |x| Ok(CaptureDef { local: u32_of(x, "local")?, name: gs(x, "name")?, mode: de_capture_mode(&gs(x, "mode")?)? }))?,
            })
        })?,
    })
}

// ---- blocks, statements, patterns ----------------------------------------------------------------------------

pub fn block(b: &Block) -> Json {
    obj("block", vec![("stmts", arr(&b.stmts, stmt)), ("locals", Json::Arr(b.locals.iter().map(|l| int(*l)).collect())), ("line", int(b.line)), ("defers", arr(&b.defers, block))])
}
pub fn de_block(j: &Json) -> R<Block> {
    Ok(Block {
        stmts: gvec(j, "stmts", de_stmt)?,
        locals: ga(j, "locals")?.iter().map(|x| x.as_i64().and_then(|n| u32::try_from(n).ok()).ok_or("block local".to_string())).collect::<R<_>>()?,
        line: u32_of(j, "line")?,
        defers: gvec(j, "defers", de_block)?,
    })
}

fn label(l: &Option<String>) -> Json {
    opt(l, |x| s(x))
}
fn de_label(j: &Json) -> R<Option<String>> {
    gopt(j, "label", |x| x.as_str().map(|s| s.to_string()).ok_or("label".to_string()))
}

pub fn stmt(st: &Stmt) -> Json {
    let mut j = stmt_kind(st);
    if !st.temps.is_empty() {
        // only a statement with temporaries carries the key: stored IR from before them still decodes (7.9)
        if let Json::Obj(m) = &mut j {
            m.insert("temps".into(), arr(&st.temps, |t| int(*t)));
        }
    }
    j
}

fn stmt_kind(st: &Stmt) -> Json {
    let line = ("line", int(st.line));
    match &st.kind {
        StmtKind::Let { local, init } => obj("let", vec![line, ("local", int(*local)), ("init", expr(init))]),
        StmtKind::LetPattern { pattern: p, init } => obj("letpattern", vec![line, ("pattern", pattern(p)), ("init", expr(init))]),
        StmtKind::Assign { place: p, value, drop_old } => obj("assign", vec![line, ("place", place(p)), ("value", expr(value)), ("drop_old", Json::Bool(*drop_old))]),
        StmtKind::Compound { place: p, op, value } => obj("compound", vec![line, ("place", place(p)), ("op", s(op.symbol())), ("value", expr(value))]),
        StmtKind::Expr(e) => obj("expr", vec![line, ("value", expr(e))]),
        StmtKind::Discard(e) => obj("discard", vec![line, ("value", expr(e))]),
        StmtKind::If { cond, then, els } => obj("if", vec![line, ("cond", expr(cond)), ("then", block(then)), ("else", opt(els, block))]),
        StmtKind::Loop { label: l, body } => obj("loop", vec![line, ("label", label(l)), ("body", block(body))]),
        StmtKind::For { label: l, binding, source, body } => {
            let src = match source {
                ForSource::Range { lo, hi, inclusive } => obj("range", vec![("lo", expr(lo)), ("hi", expr(hi)), ("inclusive", Json::Bool(*inclusive))]),
                ForSource::Items(e) => obj("items", vec![("value", expr(e))]),
            };
            obj("for", vec![line, ("label", label(l)), ("binding", opt(binding, |b| int(*b))), ("source", src), ("body", block(body))])
        }
        StmtKind::Break(l) => obj("break", vec![line, ("label", label(l))]),
        StmtKind::Continue(l) => obj("continue", vec![line, ("label", label(l))]),
        StmtKind::Return(e) => obj("return", vec![line, ("value", opt(e, expr))]),
        StmtKind::Give(e) => obj("give", vec![line, ("value", expr(e))]),
        StmtKind::Recover(e) => obj("recover", vec![line, ("value", expr(e))]),
        StmtKind::Fail(e) => obj("fail", vec![line, ("value", expr(e))]),
        StmtKind::Block(b) => obj("blockstmt", vec![line, ("body", block(b))]),
        StmtKind::Match { scrutinee, arms } => obj("match", vec![line, ("scrutinee", expr(scrutinee)), ("arms", arr(arms, arm))]),
        StmtKind::Capture { field, value } => obj("capture", vec![line, ("field", s(field)), ("value", expr(value))]),
        StmtKind::Record { schema, fields } => obj("record", vec![line, ("schema", s(schema)), ("fields", named_exprs(fields))]),
        StmtKind::Report { value, bind } => obj("report", vec![line, ("value", expr(value)), ("bind", opt(bind, |b| int(*b)))]),
        StmtKind::Expect { cond, evidence } => obj(
            "expect",
            vec![line, ("cond", expr(cond)), ("evidence", opt(evidence, |(a, op, b)| Json::Arr(vec![expr(a), s(op), expr(b)])))],
        ),
        StmtKind::Defer(i) => obj("defer", vec![line, ("index", int(*i as u32))]),
        StmtKind::Scope { local, body } => obj("scope", vec![line, ("local", int(*local)), ("body", block(body))]),
        StmtKind::Drop(p) => obj("drop", vec![line, ("place", place(p))]),
    }
}

fn opt_local(j: &Json, key: &str) -> R<Option<LocalId>> {
    gopt(j, key, |x| x.as_i64().and_then(|n| u32::try_from(n).ok()).ok_or(format!("`{key}` is not a local")))
}

pub fn de_stmt(j: &Json) -> R<Stmt> {
    let line = u32_of(j, "line")?;
    let kind = match kind(j)? {
        "let" => StmtKind::Let { local: u32_of(j, "local")?, init: de_expr(get(j, "init")?)? },
        "letpattern" => StmtKind::LetPattern { pattern: de_pattern(get(j, "pattern")?)?, init: de_expr(get(j, "init")?)? },
        "assign" => StmtKind::Assign { place: de_place(get(j, "place")?)?, value: de_expr(get(j, "value")?)?, drop_old: gb(j, "drop_old")? },
        "compound" => StmtKind::Compound {
            place: de_place(get(j, "place")?)?,
            op: BinOp::from_symbol(&gs(j, "op")?).ok_or("unknown operator")?,
            value: de_expr(get(j, "value")?)?,
        },
        "expr" => StmtKind::Expr(de_expr(get(j, "value")?)?),
        "discard" => StmtKind::Discard(de_expr(get(j, "value")?)?),
        "if" => StmtKind::If { cond: de_expr(get(j, "cond")?)?, then: de_block(get(j, "then")?)?, els: gopt(j, "else", de_block)? },
        "loop" => StmtKind::Loop { label: de_label(j)?, body: de_block(get(j, "body")?)? },
        "for" => {
            let src = get(j, "source")?;
            let source = match kind(src)? {
                "range" => ForSource::Range { lo: de_expr(get(src, "lo")?)?, hi: de_expr(get(src, "hi")?)?, inclusive: gb(src, "inclusive")? },
                "items" => ForSource::Items(de_expr(get(src, "value")?)?),
                k => return Err(format!("unknown for source `{k}`")),
            };
            StmtKind::For { label: de_label(j)?, binding: opt_local(j, "binding")?, source, body: de_block(get(j, "body")?)? }
        }
        "break" => StmtKind::Break(de_label(j)?),
        "continue" => StmtKind::Continue(de_label(j)?),
        "return" => StmtKind::Return(gopt(j, "value", de_expr)?),
        "give" => StmtKind::Give(de_expr(get(j, "value")?)?),
        "recover" => StmtKind::Recover(de_expr(get(j, "value")?)?),
        "fail" => StmtKind::Fail(de_expr(get(j, "value")?)?),
        "blockstmt" => StmtKind::Block(de_block(get(j, "body")?)?),
        "match" => StmtKind::Match { scrutinee: de_expr(get(j, "scrutinee")?)?, arms: gvec(j, "arms", de_arm)? },
        "capture" => StmtKind::Capture { field: gs(j, "field")?, value: de_expr(get(j, "value")?)? },
        "record" => StmtKind::Record { schema: gs(j, "schema")?, fields: de_named_exprs(get(j, "fields")?)? },
        "report" => StmtKind::Report { value: de_expr(get(j, "value")?)?, bind: opt_local(j, "bind")? },
        "expect" => StmtKind::Expect {
            cond: de_expr(get(j, "cond")?)?,
            evidence: gopt(j, "evidence", |e| {
                let a = e.as_arr().ok_or("evidence")?;
                Ok((de_expr(a.first().ok_or("evidence[0]")?)?, a.get(1).and_then(|x| x.as_str()).ok_or("evidence[1]")?.to_string(), de_expr(a.get(2).ok_or("evidence[2]")?)?))
            })?,
        },
        "defer" => StmtKind::Defer(u32_of(j, "index")? as usize),
        "scope" => StmtKind::Scope { local: u32_of(j, "local")?, body: de_block(get(j, "body")?)? },
        "drop" => StmtKind::Drop(de_place(get(j, "place")?)?),
        k => return Err(format!("unknown statement kind `{k}`")),
    };
    // a statement with no temporaries omits the key, including IR stored before statement temporaries existed (7.9)
    let temps = match j.get("temps") {
        None | Some(Json::Null) => Vec::new(),
        Some(_) => gvec(j, "temps", |x| x.as_i64().and_then(|n| u32::try_from(n).ok()).ok_or("temp local".to_string()))?,
    };
    Ok(Stmt { line, kind, temps })
}

fn arm(a: &Arm) -> Json {
    let body = match &a.body {
        ArmBody::Expr(e) => obj("exprbody", vec![("value", expr(e))]),
        ArmBody::Block(b) => obj("blockbody", vec![("body", block(b))]),
    };
    obj("arm", vec![("pattern", pattern(&a.pattern)), ("guard", opt(&a.guard, expr)), ("locals", Json::Arr(a.locals.iter().map(|l| int(*l)).collect())), ("body", body)])
}
fn de_arm(j: &Json) -> R<Arm> {
    let b = get(j, "body")?;
    Ok(Arm {
        pattern: de_pattern(get(j, "pattern")?)?,
        guard: gopt(j, "guard", de_expr)?,
        locals: ga(j, "locals")?.iter().map(|x| x.as_i64().and_then(|n| u32::try_from(n).ok()).ok_or("arm local".to_string())).collect::<R<_>>()?,
        body: match kind(b)? {
            "exprbody" => ArmBody::Expr(de_expr(get(b, "value")?)?),
            "blockbody" => ArmBody::Block(de_block(get(b, "body")?)?),
            k => return Err(format!("unknown arm body `{k}`")),
        },
    })
}

fn named_pats(fs: &[(String, Pattern)]) -> Json {
    Json::Arr(fs.iter().map(|(n, p)| Json::Arr(vec![s(n), pattern(p)])).collect())
}
fn de_named_pats(j: &Json) -> R<Vec<(String, Pattern)>> {
    j.as_arr()
        .ok_or("pattern fields")?
        .iter()
        .map(|p| {
            let a = p.as_arr().ok_or("pattern field")?;
            Ok((a.first().and_then(|x| x.as_str()).ok_or("field name")?.to_string(), de_pattern(a.get(1).ok_or("field pattern")?)?))
        })
        .collect()
}

pub fn pattern(p: &Pattern) -> Json {
    match p {
        Pattern::Wildcard => obj("wildcard", vec![]),
        Pattern::Bind(l) => obj("bind", vec![("local", int(*l))]),
        Pattern::Literal(c) => obj("literal", vec![("value", konst(c))]),
        Pattern::Null => obj("null", vec![]),
        Pattern::Present(x) => obj("present", vec![("inner", pattern(x))]),
        Pattern::Success(x) => obj("success", vec![("inner", pattern(x))]),
        Pattern::Failure(x) => obj("failure", vec![("inner", pattern(x))]),
        Pattern::Variant { ty, variant, fields } => obj("variant", vec![("ty", s(ty)), ("variant", s(variant)), ("fields", named_pats(fields))]),
        Pattern::Class { ty, fields } => obj("class", vec![("ty", s(ty)), ("fields", named_pats(fields))]),
        Pattern::Is { ty, fields, bind } => obj("is", vec![("ty", s(ty)), ("fields", named_pats(fields)), ("bind", opt(bind, |b| int(*b)))]),
        Pattern::Tuple(ps) => obj("tuple", vec![("items", arr(ps, pattern))]),
        Pattern::Record(fs) => obj("record", vec![("fields", named_pats(fs))]),
        Pattern::Otherwise(b) => obj("otherwise", vec![("bind", opt(b, |b| int(*b)))]),
    }
}

pub fn de_pattern(j: &Json) -> R<Pattern> {
    Ok(match kind(j)? {
        "wildcard" => Pattern::Wildcard,
        "bind" => Pattern::Bind(u32_of(j, "local")?),
        "literal" => Pattern::Literal(de_konst(get(j, "value")?)?),
        "null" => Pattern::Null,
        "present" => Pattern::Present(Box::new(de_pattern(get(j, "inner")?)?)),
        "success" => Pattern::Success(Box::new(de_pattern(get(j, "inner")?)?)),
        "failure" => Pattern::Failure(Box::new(de_pattern(get(j, "inner")?)?)),
        "variant" => Pattern::Variant { ty: gs(j, "ty")?, variant: gs(j, "variant")?, fields: de_named_pats(get(j, "fields")?)? },
        "class" => Pattern::Class { ty: gs(j, "ty")?, fields: de_named_pats(get(j, "fields")?)? },
        "is" => Pattern::Is { ty: gs(j, "ty")?, fields: de_named_pats(get(j, "fields")?)?, bind: opt_local(j, "bind")? },
        "tuple" => Pattern::Tuple(gvec(j, "items", de_pattern)?),
        "record" => Pattern::Record(de_named_pats(get(j, "fields")?)?),
        "otherwise" => Pattern::Otherwise(opt_local(j, "bind")?),
        k => return Err(format!("unknown pattern kind `{k}`")),
    })
}

// ---- places, constants, expressions --------------------------------------------------------------------------

pub fn place(p: &Place) -> Json {
    obj(
        "place",
        vec![
            ("local", int(p.local)),
            (
                "proj",
                Json::Arr(
                    p.proj
                        .iter()
                        .map(|x| match x {
                            Proj::Field(n) => obj("field", vec![("name", s(n))]),
                            Proj::Tuple(i) => obj("tuple", vec![("index", int(*i))]),
                            Proj::Index(i) => obj("index", vec![("index", int(*i))]),
                        })
                        .collect(),
                ),
            ),
        ],
    )
}
pub fn de_place(j: &Json) -> R<Place> {
    Ok(Place {
        local: u32_of(j, "local")?,
        proj: gvec(j, "proj", |x| {
            Ok(match kind(x)? {
                "field" => Proj::Field(gs(x, "name")?),
                "tuple" => Proj::Tuple(u32_of(x, "index")?),
                "index" => Proj::Index(u32_of(x, "index")?),
                k => return Err(format!("unknown projection `{k}`")),
            })
        })?,
    })
}

pub fn konst(c: &Const) -> Json {
    match c {
        Const::Void => obj("void", vec![]),
        Const::Bool(b) => obj("bool", vec![("value", Json::Bool(*b))]),
        Const::Int(n) => obj("int", vec![("value", s(&n.to_string()))]),
        Const::Float(b) => obj("float", vec![("bits", s(&b.to_string()))]),
        Const::Float32(b) => obj("float32", vec![("bits", int(*b))]),
        Const::Decimal(d) => obj("decimal", vec![("value", s(d))]),
        Const::Duration(ns) => obj("duration", vec![("ns", int(*ns))]),
        Const::Str(x) => obj("str", vec![("value", s(x))]),
        Const::Bytes(b) => obj("bytes", vec![("value", Json::Arr(b.iter().map(|x| int(*x)).collect()))]),
        Const::Null => obj("null", vec![]),
    }
}
pub fn de_konst(j: &Json) -> R<Const> {
    Ok(match kind(j)? {
        "void" => Const::Void,
        "bool" => Const::Bool(gb(j, "value")?),
        "int" => Const::Int(gs(j, "value")?.parse().map_err(|e| format!("int constant: {e}"))?),
        "float" => Const::Float(gs(j, "bits")?.parse().map_err(|e| format!("float bits: {e}"))?),
        "float32" => Const::Float32(u32_of(j, "bits")?),
        "decimal" => Const::Decimal(gs(j, "value")?),
        "duration" => Const::Duration(gi(j, "ns")?),
        "str" => Const::Str(gs(j, "value")?),
        "bytes" => Const::Bytes(ga(j, "value")?.iter().map(|x| x.as_i64().and_then(|n| u8::try_from(n).ok()).ok_or("byte".to_string())).collect::<R<_>>()?),
        "null" => Const::Null,
        k => return Err(format!("unknown constant kind `{k}`")),
    })
}

fn named_exprs(fs: &[(String, Expr)]) -> Json {
    Json::Arr(fs.iter().map(|(n, e)| Json::Arr(vec![s(n), expr(e)])).collect())
}
fn de_named_exprs(j: &Json) -> R<Vec<(String, Expr)>> {
    j.as_arr()
        .ok_or("named expressions")?
        .iter()
        .map(|p| {
            let a = p.as_arr().ok_or("named expression")?;
            Ok((a.first().and_then(|x| x.as_str()).ok_or("name")?.to_string(), de_expr(a.get(1).ok_or("expression")?)?))
        })
        .collect()
}

fn use_(u: Use) -> Json {
    s(u.word())
}
fn de_use(x: &str) -> R<Use> {
    Ok(match x {
        "read" => Use::Read,
        "move" => Use::Move,
        "copy" => Use::Copy,
        "alias" => Use::Alias,
        "editable" => Use::Editable,
        k => return Err(format!("unknown use `{k}`")),
    })
}

fn bx(e: &Expr) -> Json {
    expr(e)
}

pub fn expr(e: &Expr) -> Json {
    let mut f: Vec<(&str, Json)> = vec![("ty", ty(&e.ty)), ("line", int(e.line))];
    let k = match &e.kind {
        ExprKind::Const(c) => {
            f.push(("value", konst(c)));
            "const"
        }
        ExprKind::Place(p, u) => {
            f.push(("place", place(p)));
            f.push(("use", use_(*u)));
            "place"
        }
        ExprKind::Global(g) => {
            f.push(("name", s(g)));
            "global"
        }
        ExprKind::FuncRef(g) => {
            f.push(("name", s(g)));
            "funcref"
        }
        ExprKind::Codec { op, target, arg } => {
            f.push(("op", s(op.name())));
            f.push(("target", ty(target)));
            f.push(("arg", bx(arg)));
            "codec"
        }
        ExprKind::Binary(op, a, b) => {
            f.push(("op", s(op.symbol())));
            f.push(("left", bx(a)));
            f.push(("right", bx(b)));
            "binary"
        }
        ExprKind::Unary(op, a) => {
            f.push(("op", s(if *op == UnOp::Neg { "-" } else { "!" })));
            f.push(("value", bx(a)));
            "unary"
        }
        ExprKind::And(a, b) => {
            f.push(("left", bx(a)));
            f.push(("right", bx(b)));
            "and"
        }
        ExprKind::Or(a, b) => {
            f.push(("left", bx(a)));
            f.push(("right", bx(b)));
            "or"
        }
        ExprKind::Call { callee, args } => {
            f.push((
                "callee",
                match callee {
                    Callee::Func(n) => obj("func", vec![("name", s(n))]),
                    Callee::Intrinsic(n) => obj("intrinsic", vec![("name", s(n))]),
                    Callee::Virtual { method } => obj("virtual", vec![("method", s(method))]),
                    Callee::Value(v) => obj("value", vec![("value", bx(v))]),
                },
            ));
            f.push(("args", arr(args, |a| obj("arg", vec![("param", s(&a.param)), ("mode", mode(a.mode)), ("value", expr(&a.value))]))));
            "call"
        }
        ExprKind::Construct { ty: t, fields } => {
            f.push(("type", s(t)));
            f.push(("fields", named_exprs(fields)));
            "construct"
        }
        ExprKind::Variant { ty: t, variant, fields } => {
            f.push(("type", s(t)));
            f.push(("variant", s(variant)));
            f.push(("fields", named_exprs(fields)));
            "variant"
        }
        ExprKind::MakeFailure { ty: t, fields } => {
            f.push(("type", s(t)));
            f.push(("fields", named_exprs(fields)));
            "makefailure"
        }
        ExprKind::Tuple(es) => {
            f.push(("items", arr(es, expr)));
            "tuple"
        }
        ExprKind::Record(fs) => {
            f.push(("fields", named_exprs(fs)));
            "record"
        }
        ExprKind::Closure { func, captures } => {
            f.push(("func", s(func)));
            f.push(("captures", arr(captures, expr)));
            "closure"
        }
        ExprKind::Interpolate(parts) => {
            f.push((
                "parts",
                Json::Arr(
                    parts
                        .iter()
                        .map(|p| match p {
                            Part::Lit(x) => obj("lit", vec![("text", s(x))]),
                            Part::Show(e) => obj("show", vec![("value", expr(e))]),
                        })
                        .collect(),
                ),
            ));
            "interpolate"
        }
        ExprKind::Field(a, n) => {
            f.push(("value", bx(a)));
            f.push(("name", s(n)));
            "field"
        }
        ExprKind::TupleField(a, i) => {
            f.push(("value", bx(a)));
            f.push(("index", int(*i)));
            "tuplefield"
        }
        ExprKind::Index(a, i) => {
            f.push(("value", bx(a)));
            f.push(("index", bx(i)));
            "index"
        }
        ExprKind::OptField(a, n) => {
            f.push(("value", bx(a)));
            f.push(("name", s(n)));
            "optfield"
        }
        ExprKind::Present(a) => {
            f.push(("value", bx(a)));
            "present"
        }
        ExprKind::Success(a) => {
            f.push(("value", bx(a)));
            "success"
        }
        ExprKind::Coalesce(a, b) => {
            f.push(("left", bx(a)));
            f.push(("right", bx(b)));
            "coalesce"
        }
        ExprKind::OrFail(a) => {
            f.push(("value", bx(a)));
            "orfail"
        }
        ExprKind::OrRecover(a, b) => {
            f.push(("value", bx(a)));
            f.push(("fallback", bx(b)));
            "orrecover"
        }
        ExprKind::Catch { value, bind, handler } => {
            f.push(("value", bx(value)));
            f.push(("bind", int(*bind)));
            f.push(("handler", block(handler)));
            "catch"
        }
        ExprKind::If { cond, then, els } => {
            f.push(("cond", bx(cond)));
            f.push(("then", block(then)));
            f.push(("else", block(els)));
            "ifexpr"
        }
        ExprKind::Match { scrutinee, arms } => {
            f.push(("scrutinee", bx(scrutinee)));
            f.push(("arms", arr(arms, arm)));
            "matchexpr"
        }
        ExprKind::Block(b) => {
            f.push(("body", block(b)));
            "blockexpr"
        }
        ExprKind::Fail(a) => {
            f.push(("value", bx(a)));
            "failexpr"
        }
        ExprKind::Validate { value, lo, hi, length } => {
            f.push(("value", bx(value)));
            f.push(("lo", opt(lo, |v| s(&v.to_string()))));
            f.push(("hi", opt(hi, |v| s(&v.to_string()))));
            f.push(("length", Json::Bool(*length)));
            "validate"
        }
        ExprKind::Wrap { ty: t, value } => {
            f.push(("type", s(t)));
            f.push(("value", bx(value)));
            "wrap"
        }
        ExprKind::Await(a) => {
            f.push(("value", bx(a)));
            "await"
        }
        ExprKind::ToDynamic { iface, value, borrowed } => {
            f.push(("iface", s(iface)));
            f.push(("value", bx(value)));
            f.push(("borrowed", Json::Bool(*borrowed)));
            "todynamic"
        }
        ExprKind::Temp { local, value } => {
            f.push(("local", int(*local)));
            f.push(("value", bx(value)));
            "temp"
        }
    };
    obj(k, f)
}

fn be(j: &Json, key: &str) -> R<Box<Expr>> {
    Ok(Box::new(de_expr(get(j, key)?)?))
}

pub fn de_expr(j: &Json) -> R<Expr> {
    let t = de_ty(get(j, "ty")?)?;
    let line = u32_of(j, "line")?;
    let big = |key: &str| -> R<Option<i128>> { gopt(j, key, |v| v.as_str().ok_or("bound".to_string())?.parse::<i128>().map_err(|e| e.to_string())) };
    let kind = match kind(j)? {
        "const" => ExprKind::Const(de_konst(get(j, "value")?)?),
        "place" => ExprKind::Place(de_place(get(j, "place")?)?, de_use(&gs(j, "use")?)?),
        "global" => ExprKind::Global(gs(j, "name")?),
        "funcref" => ExprKind::FuncRef(gs(j, "name")?),
        "codec" => ExprKind::Codec { op: CodecOp::of(&gs(j, "op")?).ok_or("unknown codec operation")?, target: de_ty(get(j, "target")?)?, arg: be(j, "arg")? },
        "binary" => ExprKind::Binary(BinOp::from_symbol(&gs(j, "op")?).ok_or("unknown operator")?, be(j, "left")?, be(j, "right")?),
        "unary" => ExprKind::Unary(if gs(j, "op")? == "-" { UnOp::Neg } else { UnOp::Not }, be(j, "value")?),
        "and" => ExprKind::And(be(j, "left")?, be(j, "right")?),
        "or" => ExprKind::Or(be(j, "left")?, be(j, "right")?),
        "call" => {
            let c = get(j, "callee")?;
            let callee = match kind(c)? {
                "func" => Callee::Func(gs(c, "name")?),
                "intrinsic" => Callee::Intrinsic(gs(c, "name")?),
                "virtual" => Callee::Virtual { method: gs(c, "method")? },
                "value" => Callee::Value(be(c, "value")?),
                k => return Err(format!("unknown callee `{k}`")),
            };
            ExprKind::Call { callee, args: gvec(j, "args", |a| Ok(Arg { param: gs(a, "param")?, mode: de_mode(get(a, "mode")?)?, value: de_expr(get(a, "value")?)? }))? }
        }
        "construct" => ExprKind::Construct { ty: gs(j, "type")?, fields: de_named_exprs(get(j, "fields")?)? },
        "variant" => ExprKind::Variant { ty: gs(j, "type")?, variant: gs(j, "variant")?, fields: de_named_exprs(get(j, "fields")?)? },
        "makefailure" => ExprKind::MakeFailure { ty: gs(j, "type")?, fields: de_named_exprs(get(j, "fields")?)? },
        "tuple" => ExprKind::Tuple(gvec(j, "items", de_expr)?),
        "record" => ExprKind::Record(de_named_exprs(get(j, "fields")?)?),
        "closure" => ExprKind::Closure { func: gs(j, "func")?, captures: gvec(j, "captures", de_expr)? },
        "interpolate" => ExprKind::Interpolate(gvec(j, "parts", |p| {
            Ok(match kind(p)? {
                "lit" => Part::Lit(gs(p, "text")?),
                "show" => Part::Show(de_expr(get(p, "value")?)?),
                k => return Err(format!("unknown interpolation part `{k}`")),
            })
        })?),
        "field" => ExprKind::Field(be(j, "value")?, gs(j, "name")?),
        "tuplefield" => ExprKind::TupleField(be(j, "value")?, u32_of(j, "index")?),
        "index" => ExprKind::Index(be(j, "value")?, be(j, "index")?),
        "optfield" => ExprKind::OptField(be(j, "value")?, gs(j, "name")?),
        "present" => ExprKind::Present(be(j, "value")?),
        "success" => ExprKind::Success(be(j, "value")?),
        "coalesce" => ExprKind::Coalesce(be(j, "left")?, be(j, "right")?),
        "orfail" => ExprKind::OrFail(be(j, "value")?),
        "orrecover" => ExprKind::OrRecover(be(j, "value")?, be(j, "fallback")?),
        "catch" => ExprKind::Catch { value: be(j, "value")?, bind: u32_of(j, "bind")?, handler: de_block(get(j, "handler")?)? },
        "ifexpr" => ExprKind::If { cond: be(j, "cond")?, then: de_block(get(j, "then")?)?, els: de_block(get(j, "else")?)? },
        "matchexpr" => ExprKind::Match { scrutinee: be(j, "scrutinee")?, arms: gvec(j, "arms", de_arm)? },
        "blockexpr" => ExprKind::Block(de_block(get(j, "body")?)?),
        "failexpr" => ExprKind::Fail(be(j, "value")?),
        "validate" => ExprKind::Validate { value: be(j, "value")?, lo: big("lo")?, hi: big("hi")?, length: gb(j, "length")? },
        "wrap" => ExprKind::Wrap { ty: gs(j, "type")?, value: be(j, "value")? },
        "await" => ExprKind::Await(be(j, "value")?),
        "todynamic" => ExprKind::ToDynamic { iface: gs(j, "iface")?, value: be(j, "value")?, borrowed: gb(j, "borrowed")? },
        "temp" => ExprKind::Temp { local: u32_of(j, "local")?, value: be(j, "value")? },
        k => return Err(format!("unknown expression kind `{k}`")),
    };
    Ok(Expr { kind, ty: t, line })
}

/// Parse a program from its JSON text.
pub fn parse_program(text: &str) -> R<Program> {
    de_program(&Json::parse(text)?)
}

#[allow(dead_code)]
fn _unused(_: BTreeMap<String, Json>) {}
