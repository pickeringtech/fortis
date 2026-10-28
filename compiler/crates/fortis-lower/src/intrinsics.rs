//! The intrinsics of the core library as Rust: each trusted member maps to a Rust expression over its arguments,
//! already converted to their parameter modes. The runtime (fortis-native) implements the ones Rust lacks.

use fortis_mir::model::*;
use fortis_types::types::Type;

use crate::gen::{FnGen, Want};
use crate::types::{ident, trivial};

/// A temporary argument: a value, not a place, or a `copy` of one (X-251); a present injection is its payload's.
fn temporary(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Present(x) => temporary(x),
        ExprKind::Place(_, u) => *u == Use::Copy,
        _ => true,
    }
}

/// The future of a launched call bound inside an owning `async move` block, or None when the launch needs none (a
/// cooperative launch whose call lends no temporary). The arguments are evaluated in source order, then the omitted
/// parameters' defaults, each into its own slot, and passed in parameter order. A slot the future owns holds the value
/// (lent to the call in the parameter's form: `&v`, or `Option<&T>` for an optional read parameter, D-2); with
/// `own_all` (a pool launch, a detached task) every argument is owned, otherwise only the temporaries (X-251), and
/// the rest are bound in their ordinary argument form. A closure callee is lent, or moved for `move f(…)`.
pub(crate) fn owning_launch(g: &mut FnGen, callee: &Callee, fargs: &[Arg], own_all: bool, line: u32) -> Option<String> {
    let lends_temp = fargs.iter().any(|a| a.mode == Mode::Read && !trivial(&a.value.ty, g.prog) && temporary(&a.value));
    if !own_all && !lends_temp {
        return None;
    }
    // the callee: its parameters (name, mode, type, default) and the code that calls it
    let (params, head, call): (Vec<(String, Mode, Type, Option<Expr>)>, String, String) = match callee {
        Callee::Func(f) => {
            let fd = g.prog.function(f)?.clone();
            let rust = g.fn_names.get(f).cloned().unwrap_or_else(|| ident(f));
            let ps = fd.params.iter().map(|p| (p.name.clone(), p.mode, fd.locals[p.local as usize].ty.clone(), p.default.clone())).collect();
            (ps, String::new(), rust)
        }
        Callee::Value(fv) => {
            let tys = match fortis_types::types::strip(&fv.ty) {
                Type::Fn { params, .. } if params.len() == fargs.len() => params.clone(),
                _ => return None,
            };
            let ps = fargs.iter().zip(tys).map(|(a, t)| (a.param.clone(), a.mode, t, None)).collect();
            let fc = g.expr(fv, Want::Any);
            let head = match fv.kind {
                ExprKind::Place(_, Use::Move) => format!("#[allow(unused_mut)] let mut __f = {fc};"),
                ExprKind::Place(_, Use::Editable) => format!("let __f = &mut {fc};"),
                _ => format!("let __f = &{fc};"),
            };
            (ps, head, "__f".to_string())
        }
        _ => return None,
    };
    let mut binds = vec![head];
    let mut slots: Vec<(String, String)> = Vec::new();
    let bind = |g: &mut FnGen, k: usize, a: &Arg, mode: Mode, pty: &Type, binds: &mut Vec<String>| -> String {
        let owned_here = own_all || (mode == Mode::Read && !trivial(&a.value.ty, g.prog) && temporary(&a.value));
        if mode == Mode::Editable && !matches!(a.value.kind, ExprKind::Place(..)) {
            // an exclusive loan a call returns: its result is the loan, kept in the slot as it is
            let v = g.expr(&a.value, Want::Value);
            binds.push(format!("let __p{k} = {v};"));
            return format!("__p{k}");
        }
        if !owned_here {
            let a2 = Arg { param: a.param.clone(), mode, value: a.value.clone() };
            let v = g.arg(&a2, Some(pty));
            binds.push(format!("let __p{k} = {v};"));
            return format!("__p{k}");
        }
        let v = g.expr(&a.value, Want::Value);
        let opt = if mode == Mode::Read { crate::types::opt_read_inner(pty, g.prog) } else { None };
        // a closure call's arguments are not coerced: a present value for an optional parameter is wrapped here
        let uncoerced = !matches!(fortis_types::types::strip(&a.value.ty), Type::Optional(_)) && !matches!(a.value.kind, ExprKind::Const(Const::Null)) && fortis_types::types::known(&a.value.ty);
        let v = if opt.is_some() && uncoerced { format!("Some({v})") } else { v };
        match opt {
            Some(inner) => {
                // the owned optional the future holds, typed (a `null` argument names no type)
                binds.push(format!("let __p{k}: Option<{}> = {v};", crate::types::rust_ty(&inner, g.prog)));
                if crate::types::is_string(&inner) { format!("__p{k}.as_deref()") } else { format!("__p{k}.as_ref()") }
            }
            None => {
                binds.push(format!("let __p{k} = {v};"));
                if mode == Mode::Read && !trivial(&a.value.ty, g.prog) { format!("&__p{k}") } else { format!("__p{k}") }
            }
        }
    };
    // source order, except that exclusive loans are taken last: taking one has no effect, and a later argument may
    // still read the place it lends (5.7's reservations)
    let delayed = |a: &Arg| a.mode == Mode::Editable && matches!(a.value.kind, ExprKind::Place(..));
    let order: Vec<usize> = (0..fargs.len()).filter(|k| !delayed(&fargs[*k])).chain((0..fargs.len()).filter(|k| delayed(&fargs[*k]))).collect();
    for k in order {
        let a = &fargs[k];
        let (mode, pty) = params.iter().find(|p| p.0 == a.param).map(|p| (p.1, p.2.clone())).unwrap_or((a.mode, a.value.ty.clone()));
        let passed = bind(g, k, a, mode, &pty, &mut binds);
        slots.push((a.param.clone(), passed));
    }
    let mut k = fargs.len();
    for (pname, mode, pty, default) in &params {
        if slots.iter().any(|(n, _)| n == pname) {
            continue;
        }
        let Some(d) = default else {
            g.err_pub(line, format!("missing argument `{pname}`"));
            return None;
        };
        let a = Arg { param: pname.clone(), mode: *mode, value: d.clone() };
        let passed = bind(g, k, &a, *mode, pty, &mut binds);
        slots.push((pname.clone(), passed));
        k += 1;
    }
    let pass: Vec<String> = params.iter().filter_map(|p| slots.iter().find(|(n, _)| *n == p.0).map(|(_, v)| v.clone())).collect();
    Some(format!("{{ {} async move {{ {call}({}).await }} }}", binds.join(" "), pass.join(", ")))
}

pub fn call(g: &mut FnGen, name: &str, args: &[Arg], ty: &Type, line: u32) -> String {
    // variadic `List.of(items...)`
    if name == "List.of" {
        let items: Vec<String> = args.iter().map(|a| g.expr(&a.value, Want::Value)).collect();
        return format!("vec![{}]", items.join(", "));
    }
    // a member of `T` called on a `Shared[T]` receiver reads through the handle (7.8): `console.print(…)` where
    // `console: Shared[Console]` (X-137)
    let through_shared = !name.starts_with("Shared.") && !name.starts_with("Weak.") && args.first().map_or(false, |x| fortis_types::types::strip(&x.value.ty).show().starts_with("Shared["));
    let mut a: Vec<String> = args.iter().map(|x| g.arg(x, None)).collect();
    if through_shared {
        a[0] = format!("&*{}.read()", g.expr(&args[0].value, Want::Any));
    }
    let arg = |i: usize| a.get(i).cloned().unwrap_or_else(|| "()".into());
    let value = |g: &mut FnGen, i: usize| match args.get(i) {
        Some(x) if i == 0 && through_shared => format!("(*{}.read())", g.expr(&x.value, Want::Any)),
        Some(x) => g.expr(&x.value, Want::Value),
        None => "()".into(),
    };
    let any = |g: &mut FnGen, i: usize| match args.get(i) {
        Some(x) if i == 0 && through_shared => format!("(*{}.read())", g.expr(&x.value, Want::Any)),
        Some(x) => g.expr(&x.value, Want::Any),
        None => "()".into(),
    };
    let ns = |unit: &str| -> &'static str {
        match unit {
            "days" => "86_400_000_000_000",
            "hours" => "3_600_000_000_000",
            "minutes" => "60_000_000_000",
            "seconds" => "1_000_000_000",
            _ => "1_000_000",
        }
    };
    let s = match name {
        // ---- host
        "Host.capabilities" => "rt::host::capabilities()".to_string(),
        "Console.print" => format!("{}.print(&{})", any(g, 0), any(g, 1)),
        "Clock.now" => format!("{}.now()", any(g, 0)),
        "Clock.frozen" => format!("{}.frozen({})", any(g, 0), arg(1)),
        "Clock.elapsedSince" => format!("{}.elapsed_since({})", any(g, 0), arg(1)),
        "Random.below" => format!("{}.below({})", any(g, 0), arg(1)),
        "Random.seeded" => format!("{}.seeded({})", any(g, 0), arg(1)),
        "Network.resolve" => format!("{}.resolve({})", any(g, 0), arg(1)),
        "Process.run" => format!("{}.run({}, &{})", any(g, 0), arg(1), args.get(2).map(|a| g.expr(&a.value, Want::Any)).unwrap_or_else(|| "Vec::<String>::new()".into())),
        "RawSql.execute" => format!("{}.execute({})", any(g, 0), arg(1)),
        "Environment.get" => format!("{}.get({})", any(g, 0), arg(1)),
        "reveal" => format!("rt::host::reveal(&{}, {})", any(g, 0), value(g, 1)),
        "Path.parse" => format!("rt::host::Path::parse(&{})", any(g, 0)),
        "FileSystem.readText" => format!("{}.read_text(&{})", any(g, 0), any(g, 1)),
        "FileSystem.writeAll" => format!("{}.write_all(&{}, &{})", any(g, 0), any(g, 1), any(g, 2)),
        "FileSystem.readAll" => format!("{}.read_all(&{})", any(g, 0), any(g, 1)),
        "FileSystem.open" => format!("{}.open(&{})", any(g, 0), any(g, 1)),
        "FileSystem.create" => format!("{}.create(&{})", any(g, 0), any(g, 1)),
        "FileSystem.subtree" => format!("{}.subtree(&{})", any(g, 0), any(g, 1)),
        "File.readAll" => format!("{}.read_all()", any(g, 0)),
        "File.writeAll" => format!("{}.write_all(&{})", any(g, 0), any(g, 1)),
        "File.close" => format!("{}.close()", any(g, 0)),
        // ---- Int (6.4)
        "Int.add" | "Int.sub" | "Int.mul" | "Int.div" | "Int.rem" => format!("rt::numeric::int_{}({}, {})", &name[4..], arg(0), arg(1)),
        "Int.shiftLeft" => format!("rt::numeric::int_shift_left({}, {})", arg(0), arg(1)),
        "Int.wrappingAdd" => format!("{}.wrapping_add({})", arg(0), arg(1)),
        "Int.wrappingSub" => format!("{}.wrapping_sub({})", arg(0), arg(1)),
        "Int.wrappingMul" => format!("{}.wrapping_mul({})", arg(0), arg(1)),
        "Int.saturatingAdd" => format!("{}.saturating_add({})", arg(0), arg(1)),
        "Int.saturatingSub" => format!("{}.saturating_sub({})", arg(0), arg(1)),
        "Int.saturatingMul" => format!("{}.saturating_mul({})", arg(0), arg(1)),
        "Int.validate" => format!("rt::Outcome::Ok({})", arg(0)),
        // ---- floats (6.5)
        "Float.sqrt" => format!("rt::float::sqrt({})", arg(0)),
        "Float.isNaN" | "Float32.isNaN" => format!("{}.is_nan()", arg(0)),
        "Float.totalOrder" => format!("rt::float::total_order({})", arg(0)),
        "Float.bitsEqual" => format!("({}.to_bits() == {}.to_bits())", arg(0), arg(1)),
        "Float32.sqrt" => format!("rt::float::canon32(({} as f64).sqrt() as f32)", arg(0)),
        // ---- Decimal
        "Decimal.add" | "Decimal.sub" | "Decimal.mul" | "Decimal.div" | "Decimal.rem" => format!("rt::numeric::dec({}.{}(&{}), \"decimal\")", arg(0), &name[8..], arg(1)),
        "Decimal.round" => format!("rt::numeric::dec({}.round({} as i128, rt::numeric::rounding(&rt::Show::show(&{}))), \"round\")", arg(0), arg(1), any(g, 2)),
        "Decimal.fromFloat" => format!("rt::numeric::dec(rt::Decimal::from_float({}, {} as i128, rt::numeric::rounding(&rt::Show::show(&{}))), \"fromFloat\")", arg(0), arg(1), any(g, 2)),
        "Decimal.scale" => format!("({}.scale as i64)", arg(0)),
        // ---- text (6.6)
        "String.from" => format!("rt::Show::show(&{})", any(g, 0)),
        "String.scalarCount" | "StringView.scalarCount" => format!("rt::text::scalar_count({})", arg(0)),
        "String.byteCount" | "StringView.byteCount" => format!("rt::text::byte_count({})", arg(0)),
        // a slice is a view of its text (`StringView from self`): borrowed when the result is a loan
        "String.slice" | "StringView.slice" if matches!(ty, Type::Loan { .. }) => format!("rt::text::slice_view({}, {}, {})", arg(0), arg(1), arg(2)),
        "String.slice" | "StringView.slice" => format!("rt::text::slice({}, {}, {})", arg(0), arg(1), arg(2)),
        "String.scalarAt" | "StringView.scalarAt" => format!("rt::text::scalar_at({}, {})", arg(0), arg(1)),
        "Scalar.digitValue" => format!("rt::text::digit_value({})", arg(0)),
        "Scalar.hexValue" => format!("rt::text::hex_value({})", arg(0)),
        "Builder.create" => "rt::text::Builder::create()".to_string(),
        "Builder.append" | "Builder.appendScalar" => format!("{}.append(&{})", any(g, 0), any(g, 1)),
        "Builder.appendLine" => format!("{}.append_line(&{})", any(g, 0), any(g, 1)),
        "Builder.finish" => format!("{}.finish()", any(g, 0)),
        "Text.words" => format!("rt::text::words({})", arg(0)),
        // ---- time
        "Duration.days" | "Duration.hours" | "Duration.minutes" | "Duration.seconds" | "Duration.milliseconds" => format!("rt::Duration::of({}, {})", arg(0), ns(&name[9..])),
        "Duration.add" | "Duration.sub" | "Duration.multiply" => format!("rt::Duration::{}({}, {})", &name[9..], arg(0), arg(1)),
        "Duration.nanoseconds" => format!("{}.nanoseconds()", any(g, 0)),
        "Timestamp.epoch" => "rt::Timestamp(0)".to_string(),
        "Timestamp.add" => format!("rt::Timestamp::add({}, {})", arg(0), arg(1)),
        "Timestamp.after" => format!("({} > {})", arg(0), arg(1)),
        "Timestamp.before" => format!("({} < {})", arg(0), arg(1)),
        // ---- collections (6.7)
        // ---- structured concurrency (12.5; fortis-native task.rs)
        "Scope.launch" | "Scope.launchParallel" => {
            let fut_expr = args.get(1).map(|a| &a.value);
            // the function whose future is launched: a direct call, or a local future one made (D-27)
            let made_by = match fut_expr.map(|e| &e.kind) {
                Some(ExprKind::Call { callee: Callee::Func(f), .. }) => Some(f.clone()),
                Some(ExprKind::Place(p, _)) if p.proj.is_empty() => g.future_fns.get(&p.local).cloned(),
                _ => None,
            };
            let (fname, is_try) = match made_by.as_ref().and_then(|f| g.prog.function(f)) {
                Some(fd) => (crate::gen::source_name(fd), fd.is_try()),
                None => ("work".to_string(), false),
            };
            let scope = any(g, 0);
            // a future launched on a pool or detached owns what it holds: its arguments are bound inside an owning
            // `async move` block, so a temporary copy (`copy data`) lives as long as the future (C.2)
            let owning = name == "Scope.launchParallel" || g.launch_detachable;
            // a future launched directly owns its call's temporaries (12.3, X-251); one launched on a pool or detached
            // owns everything it holds (C.2): such a call's arguments are bound inside an owning `async move` block
            let fut = match fut_expr.map(|e| &e.kind) {
                Some(ExprKind::Call { callee, args: fargs }) => match owning_launch(g, callee, fargs, owning, line) {
                    Some(code) => code,
                    None => value(g, 1),
                },
                _ => value(g, 1),
            };
            let method = if name == "Scope.launchParallel" {
                "launch_parallel"
            } else if g.launch_detachable {
                "launch_detachable"
            } else if is_try {
                "launch_try"
            } else {
                "launch"
            };
            format!("{scope}.{method}({fname:?}, {fut})")
        }
        "Scope.launchBlocking" => {
            let scope = any(g, 0);
            g.closure_as_future = true;
            let work = value(g, 1);
            g.closure_as_future = false;
            format!("{scope}.launch_blocking_future({work})")
        }
        "Scope.detach" => format!("{}.detach({})", any(g, 0), value(g, 1)),
        "Task.abandon" => format!("{{ let _ = {}.abandon().await; }}", value(g, 0)),
        // a cancellation point for pool work: a cancelled task leaves here, unwinding the open blocks (12.4)
        "checkpoint" => {
            let ab = g.exit_code(0, true);
            format!("(match rt::task::checkpoint().await {{ Ok(()) => (), Err(__c) => {{ {ab} return Err(__c); }} }})")
        }
        "join" => {
            let items: Vec<String> = args.iter().map(|x| g.expr(&x.value, Want::Value)).collect();
            format!("rt::task::join2({})", items.join(", "))
        }
        "injectDefect" => format!("rt::defect(&{})", any(g, 0)),
        "List.default" => "Vec::new()".to_string(),
        "List.count" | "ListView.count" | "Array.count" => format!("rt::collections::count(&{})", any(g, 0)), // D-160: checked
        "List.get" | "ListView.get" => {
            // `(alias T from self)?`: a loan of the item, or the item itself when it is trivial
            if trivial(ty, g.prog) || matches!(ty, Type::Optional(i) if crate::types::trivial(i, g.prog)) {
                format!("rt::collections::list_get_ref({}, {}).copied()", arg(0), arg(1))
            } else {
                format!("rt::collections::list_get_ref({}, {})", arg(0), arg(1))
            }
        }
        "Scalar.of" => format!("rt::text::scalar_of({})", arg(0)),
        "List.at" => format!("rt::collections::list_at({}, {})", arg(0), arg(1)),
        "List.push" => {
            // an item narrowed from an optional (`if e != null { push(move e) }`) is unwrapped
            let elem = args.first().and_then(|a| crate::types::args_of(&a.value.ty).first().cloned());
            let item = match (elem, args.get(1)) {
                (Some(t), Some(x)) => g.narrowed(&x.value, &t),
                _ => None,
            };
            format!("{}.push({})", any(g, 0), item.unwrap_or_else(|| value(g, 1)))
        }
        "List.pop" => format!("{}.pop()", any(g, 0)),
        "List.set" => format!("rt::collections::list_set({}, {}, {})", arg(0), arg(1), value(g, 2)),
        "Array.set" => format!("{{ let __i = {}; {}[__i as usize] = {}; }}", arg(1), any(g, 0), value(g, 2)),
        "List.iterate" => format!("rt::collections::Cursor::new({})", arg(0)),
        "ListCursor.next" => format!("{}.next()", any(g, 0)),
        "List.consumeEach" => format!("{{ let mut __f = {}; for __x in {} {{ (__f)(__x); }} }}", value(g, 1), value(g, 0)),
        "Array.filled" => {
            let v = value(g, 0);
            format!("vec![{v}; {} as usize]", arg(1))
        }
        "Map.default" => "rt::OrderedMap::new()".to_string(),
        "Map.get" => {
            let k = key(g, args, 1);
            format!("{}.get({k}).cloned()", any(g, 0))
        }
        "Map.set" => format!("{}.set({}, {})", any(g, 0), value(g, 1), value(g, 2)),
        "Map.count" | "Set.count" => format!("{}.count()", any(g, 0)),
        "Set.default" => "rt::OrderedSet::new()".to_string(),
        "Set.add" => format!("{}.add({})", any(g, 0), value(g, 1)),
        "Set.contains" => {
            let k = key(g, args, 1);
            format!("{}.contains({k})", any(g, 0))
        }
        // ---- outcomes (10)
        "Must.create" | "Must.discharge" => "()".to_string(),
        "Outcome.success" => format!("Ok({})", value(g, 0)),
        "Outcome.failure" => format!("Err({})", value(g, 0)),
        "annotate" => format!("{}.annotate({})", value(g, 0), value(g, 1)),
        // ---- shared ownership (7.8, 12.7)
        "Shared.create" => format!("rt::Shared::create({})", value(g, 0)),
        "Shared.retain" => {
            g.audit.refcount_at(&g.f.name.clone(), line, "retain");
            format!("{}.retain() /* retain */", any(g, 0))
        }
        "Shared.downgrade" => format!("{}.downgrade()", any(g, 0)),
        "Weak.upgrade" => {
            g.audit.refcount_at(&g.f.name.clone(), line, "upgrade");
            format!("{}.upgrade() /* upgrade */", any(g, 0))
        }
        "Weak.retain" => format!("{}.retain()", any(g, 0)),
        "Mutex.create" => format!("rt::Mutex::create({})", value(g, 0)),
        "Mutex.lockBlocking" => format!("{}.lock_blocking()", any(g, 0)),
        "MutexGuard.get" => format!("{}.get()", any(g, 0)),
        "MutexGuard.set" => format!("{}.set({})", any(g, 0), value(g, 1)),
        "MutexGuard.replace" => format!("{}.replace({})", any(g, 0), value(g, 1)),
        "AtomicInt.create" => format!("rt::AtomicInt::create({})", arg(0)),
        "AtomicBool.create" => format!("rt::AtomicBool::create({})", arg(0)),
        "AtomicInt.load" | "AtomicBool.load" => format!("{}.load()", any(g, 0)),
        "AtomicInt.store" | "AtomicBool.store" => format!("{}.store({})", any(g, 0), arg(1)),
        "AtomicInt.fetchAdd" => format!("{}.fetch_add({})", any(g, 0), arg(1)),
        "Outcome.collect" => format!("rt::collections::collect({})", value(g, 0)),
        "Failures.aggregate" => format!("rt::failure::aggregate({}, {:?})", value(g, 0), crate::gen::source_name(g.f)),
        "graphemes" => format!("rt::text::graphemes({})", arg(0)),
        "BytesView.count" | "Bytes.count" | "BytesView.byteCount" | "Bytes.byteCount" => format!("({}.len() as i64)", any(g, 0)),
        "BytesView.get" | "Bytes.get" => format!("rt::collections::list_get({}, {}).map(|b| b as i64)", arg(0), arg(1)),
        "Money.multiply" | "Money.divide" => {
            // a trusted intrinsic of fortis.money: the amount times (or over) an integer, the currency kept
            let op = if name == "Money.multiply" { "mul" } else { "div" };
            let m = any(g, 0);
            format!("(|| -> rt::Outcome<Money> {{ let __a = rt::numeric::dec({m}.amount.{op}(&rt::Decimal::from_int({} as i128).unwrap()), \"decimal\")?; Ok(Money {{ currency: {m}.currency, amount: __a }}) }})()", arg(1))
        }
        _ if name.starts_with("kit:") => {
            let rest = &name["kit:".len()..];
            let (module, tname) = rest.split_once(':').unwrap_or((rest, ""));
            let t = if crate::types::name_of(ty) == Some("Outcome") { "rt::Outcome<()>" } else { "()" };
            format!("rt::dynamic_t::<{t}>(\"kit not available: `{tname}` comes from {module}, which this kernel does not implement\")")
        }
        _ => {
            let _ = ty;
            return g.err_pub(line, format!("intrinsic `{name}` has no native lowering yet"));
        }
    };
    // a runtime operation that can fail behind an infallible signature: a defect if the proof was wrong
    let fallible = matches!(name, "Duration.days" | "Duration.hours" | "Duration.minutes" | "Duration.seconds" | "Duration.milliseconds" | "Duration.add" | "Duration.sub" | "Duration.multiply" | "Timestamp.add") || name.starts_with("Decimal.") && name != "Decimal.scale" || matches!(name, "Int.add" | "Int.sub" | "Int.mul" | "Int.div" | "Int.rem" | "Int.shiftLeft" | "List.at" | "List.set" | "String.slice" | "StringView.slice" | "String.scalarAt" | "StringView.scalarAt" | "Clock.elapsedSince");
    if fallible && name_of_pub(ty) != Some("Outcome") {
        return format!("rt::value({s})");
    }
    s
}

fn name_of_pub(t: &Type) -> Option<&str> {
    crate::types::name_of(t)
}

/// A lookup key as a reference: `&str` for text, `&value` otherwise.
fn key(g: &mut FnGen, args: &[Arg], i: usize) -> String {
    match args.get(i) {
        Some(a) if crate::types::is_string(&a.value.ty) => g.expr(&a.value, Want::Str),
        Some(a) => {
            let v = g.expr(&a.value, Want::Ref);
            v
        }
        None => "()".into(),
    }
}
