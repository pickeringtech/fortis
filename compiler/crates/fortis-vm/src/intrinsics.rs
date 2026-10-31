//! The intrinsics of the core library (trusted members without a Fortis body), each honouring the contract
//! written above its signature in library/fortis/*.ft and agreeing with interpreter/intrinsics.py. A fallible
//! intrinsic returns a failure value (the dynamic form of `Outcome`).

use crate::sync::{Cell, RefCell};
use std::collections::HashMap;
use crate::sync::Rc;

use fortis_runtime::{Decimal, Rounding};

use crate::machine::*;
use crate::value::*;

const INT_MIN: i128 = i64::MIN as i128;
const INT_MAX: i128 = i64::MAX as i128;

/// Arguments in source order, without their parameter names.
pub fn gather(vals: Vec<(String, Value)>) -> Vec<Value> {
    vals.into_iter().map(|(_, v)| v.deref()).collect()
}

pub fn builtin_type(v: &Value) -> &'static str {
    match v {
        Value::Array(_) => "Array",
        Value::List(_) => "List",
        Value::Future(_) => "Future",
        Value::Task(_) => "Task",
        Value::Scope(_) => "Scope",
        Value::Bool(_) => "Bool",
        Value::Int(_) => "Int",
        Value::Str(_) => "String",
        Value::Builder(_) => "Builder",
        Value::Map(_) => "Map",
        Value::Set(_) => "Set",
        Value::Cursor(_) => "ListCursor",
        Value::Atomic(k, _) => {
            if &**k == "AtomicInt" {
                "AtomicInt"
            } else {
                "AtomicBool"
            }
        }
        Value::Mutex(_) => "Mutex",
        Value::Time(_) => "Timestamp",
        Value::Bytes(_) => "BytesView",
        Value::Dec(_) => "Decimal",
        Value::Dur(_) => "Duration",
        Value::Failure(_) => "Failure",
        Value::Cap(c) => cap_type(&c.kind),
        Value::Path(_) => "Path",
        Value::Secret(_) => "Secret",
        Value::File(_) => "File",
        Value::Scalar(_) => "Scalar",
        Value::SharedRef(_) => "Shared",
        Value::WeakRef(_) => "Weak",
        Value::Float(_) => "Float",
        Value::F32(_) => "Float32",
        _ => "?",
    }
}

fn cap_type(kind: &str) -> &'static str {
    match kind {
        "fileSystem" => "FileSystem",
        "network" => "Network",
        "clock" => "Clock",
        "random" => "Random",
        "environment" => "Environment",
        "process" => "Process",
        "supervisor" => "Supervisor",
        "console" => "Console",
        "secretAccess" => "SecretAccess",
        "rawSql" => "RawSql",
        _ => "?",
    }
}

/// The capability kind a host passes for a parameter of this type (a development session's handlers).
pub(crate) fn cap_kind(type_name: &str) -> Option<&'static str> {
    ["fileSystem", "network", "clock", "random", "environment", "process", "supervisor", "console", "secretAccess", "rawSql"].into_iter().find(|k| cap_type(k) == type_name)
}

pub fn failure(code: &str, payload: Vec<(&str, Value)>) -> Value {
    Value::Failure(shared(FailureVal {
        code: Rc::from(code),
        payload: payload.into_iter().map(|(k, v)| (Rc::from(k), v)).collect(),
        origin: Rc::from(""),
        message: Rc::from(""),
        cause: None,
        annotations: vec![],
    }))
}

fn int(v: &Value) -> R<i128> {
    v.as_int().ok_or(Exit::Halt(Halt::Dynamic(format!("expected an integer, got {}", v.type_name()))))
}
fn text(v: &Value) -> R<Rc<str>> {
    match v.deref() {
        Value::Str(s) | Value::Scalar(s) => Ok(s),
        other => dynamic(format!("expected a string, got {}", other.type_name())),
    }
}
fn checked(r: i128, op: &str) -> Value {
    if !(INT_MIN..=INT_MAX).contains(&r) {
        failure("numeric.overflow", vec![("operation", Value::str(op))])
    } else {
        Value::Int(r)
    }
}
fn wrap(r: i128) -> i128 {
    ((r - INT_MIN).rem_euclid(1i128 << 64)) + INT_MIN
}
fn sat(r: i128) -> i128 {
    r.clamp(INT_MIN, INT_MAX)
}
fn dec(v: &Value) -> R<Decimal> {
    match v.deref() {
        Value::Dec(d) => Ok(d),
        other => dynamic(format!("expected a Decimal, got {}", other.type_name())),
    }
}
fn dec_result(r: Result<Decimal, fortis_runtime::DecError>, op: &str) -> Value {
    match r {
        Ok(d) => Value::Dec(d),
        Err(e) => {
            let payload = if e == fortis_runtime::DecError::Overflow { vec![("operation", Value::str(op))] } else { vec![] };
            failure(e.code(), payload)
        }
    }
}
fn variant_name(v: &Value) -> R<String> {
    match v.deref() {
        Value::Enum(e) => Ok(e.variant.to_string()),
        other => dynamic(format!("expected an enum variant, got {}", other.type_name())),
    }
}
fn items(v: &Value) -> R<Shared<Vec<Value>>> {
    match v.deref() {
        Value::List(x) | Value::Array(x) => Ok(x),
        other => dynamic(format!("expected a list, got {}", other.type_name())),
    }
}
fn dur(ns: i128) -> Value {
    if ns < INT_MIN || ns > INT_MAX {
        failure("numeric.overflow", vec![("operation", Value::str("duration"))])
    } else {
        Value::Dur(ns as i64)
    }
}

fn dev_root() -> String {
    thread_local! {
        static ROOT: RefCell<Option<String>> = const { RefCell::new(None) };
    }
    ROOT.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            let root = std::env::var("FORTIS_DEV_ROOT").unwrap_or_else(|_| {
                let p = std::env::temp_dir().join(format!("fortis-dev-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&p);
                p.display().to_string()
            });
            *r = Some(root);
        }
        r.clone().unwrap()
    })
}

pub(crate) fn cap(kind: &str) -> Value {
    Value::Cap(Rc::new(CapVal { kind: Rc::from(kind), state: RefCell::new(HashMap::new()) }))
}

fn fs_resolve(c: &CapVal, path: &str) -> Result<String, Value> {
    let root = match c.state.borrow().get("root") {
        Some(Value::Str(r)) => r.to_string(),
        _ => return Err(failure("fs.outside_root", vec![("path", Value::str(path))])),
    };
    let p = std::path::Path::new(path);
    let mut norm: Vec<String> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if norm.pop().is_none() {
                    return Err(failure("fs.outside_root", vec![("path", Value::str(path))]));
                }
            }
            c => norm.push(c.to_string()),
        }
    }
    if p.is_absolute() {
        return Err(failure("fs.outside_root", vec![("path", Value::str(path))]));
    }
    Ok(format!("{root}/{}", norm.join("/")))
}

fn splitmix(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (state, z ^ (z >> 31))
}

pub fn call(vm: &mut Vm, name: &str, a: Vec<Value>) -> R<Value> {
    let arg = |i: usize| -> Value { a.get(i).cloned().unwrap_or(Value::Void) };
    if let Some(target) = name.strip_prefix("enum_parse:") {
        let value = text(&arg(0))?;
        let Some(def) = vm.types.get(target) else { return dynamic("enum parse target missing") };
        let fortis_mir::model::TypeKind::Enum { variants } = &def.kind else { return dynamic("enum parse target is not an enum") };
        for variant in variants {
            if value.eq_ignore_ascii_case(&variant.name) {
                return Ok(Value::Enum(Rc::new(EnumVal { ty: Rc::from(target), variant: Rc::from(variant.name.as_str()), fields: vec![] })));
            }
        }
        let names = variants.iter().map(|v| v.name.as_str()).collect::<Vec<_>>().join(", ");
        let failed = failure("conversion.parse", vec![("text", Value::Str(value)), ("variants", Value::str(&names))]);
        if let Value::Failure(f) = &failed {
            f.borrow_mut().origin = Rc::from(vm.stack.last().map(String::as_str).unwrap_or("main"));
        }
        return Ok(failed);
    }
    // `x as T` (6.13, X-475): a row of the built-in conversion table, named `as:<kind>:<source>:<target>`
    if let Some(row) = name.strip_prefix("as:") {
        let value = crate::convert::convert(row, arg(0))?;
        if let Value::Failure(f) = &value {
            f.borrow_mut().origin = Rc::from(vm.stack.last().map(String::as_str).unwrap_or("main"));
        }
        return Ok(value);
    }
    Ok(match name {
        // ---- Int (6.4)
        "Int.add" => checked(int(&arg(0))? + int(&arg(1))?, "add"),
        "Int.sub" => checked(int(&arg(0))? - int(&arg(1))?, "sub"),
        "Int.mul" => checked(int(&arg(0))?.saturating_mul(int(&arg(1))?), "mul"),
        "Int.div" => {
            let (x, y) = (int(&arg(0))?, int(&arg(1))?);
            if y == 0 {
                failure("numeric.divide_by_zero", vec![])
            } else {
                let q = x.abs() / y.abs();
                checked(if (x >= 0) == (y >= 0) { q } else { -q }, "div")
            }
        }
        "Int.rem" => {
            let (x, y) = (int(&arg(0))?, int(&arg(1))?);
            if y == 0 {
                failure("numeric.divide_by_zero", vec![])
            } else {
                let r = x.abs() % y.abs();
                Value::Int(if x >= 0 { r } else { -r })
            }
        }
        "Int.shiftLeft" => {
            let (x, b) = (int(&arg(0))?, int(&arg(1))?);
            if !(0..64).contains(&b) {
                failure("numeric.shift_range", vec![("bits", Value::Int(b))])
            } else {
                checked(x.checked_shl(b as u32).unwrap_or(i128::MAX), "shiftLeft")
            }
        }
        "Int.wrappingAdd" => Value::Int(wrap(int(&arg(0))? + int(&arg(1))?)),
        "Int.wrappingSub" => Value::Int(wrap(int(&arg(0))? - int(&arg(1))?)),
        "Int.wrappingMul" => Value::Int(wrap(int(&arg(0))?.wrapping_mul(int(&arg(1))?))),
        "Int.saturatingAdd" => Value::Int(sat(int(&arg(0))? + int(&arg(1))?)),
        "Int.saturatingSub" => Value::Int(sat(int(&arg(0))? - int(&arg(1))?)),
        "Int.saturatingMul" => Value::Int(sat(int(&arg(0))?.saturating_mul(int(&arg(1))?))),
        "Int.validate" => arg(0),
        // ---- floats (6.5)
        "Float.sqrt" => match arg(0) {
            Value::Float(x) => Value::Float(fortis_runtime::float::sqrt(x)),
            v => return dynamic(format!("Float.sqrt of {}", v.type_name())),
        },
        "Float.isNaN" => Value::Bool(matches!(arg(0), Value::Float(x) if x.is_nan())),
        "Float.totalOrder" => match arg(0) {
            Value::Float(x) => Value::Int(fortis_runtime::float::total_order(x) as i128),
            _ => return dynamic("Float.totalOrder"),
        },
        "Float.bitsEqual" => match (arg(0), arg(1)) {
            (Value::Float(x), Value::Float(y)) => Value::Bool(x.to_bits() == y.to_bits()),
            _ => return dynamic("Float.bitsEqual"),
        },
        "Float32.sqrt" => match arg(0) {
            Value::F32(x) => Value::F32(fortis_runtime::float::canon32(fortis_runtime::float::sqrt(x as f64) as f32)),
            _ => return dynamic("Float32.sqrt"),
        },
        "Float32.isNaN" => Value::Bool(matches!(arg(0), Value::F32(x) if x.is_nan())),
        // ---- Decimal (6.4)
        "Decimal.add" => dec_result(dec(&arg(0))?.add(&dec(&arg(1))?), "decimal"),
        "Decimal.sub" => dec_result(dec(&arg(0))?.sub(&dec(&arg(1))?), "decimal"),
        "Decimal.mul" => dec_result(dec(&arg(0))?.mul(&dec(&arg(1))?), "decimal"),
        "Decimal.div" => dec_result(dec(&arg(0))?.div(&dec(&arg(1))?), "decimal"),
        "Decimal.rem" => dec_result(dec(&arg(0))?.rem(&dec(&arg(1))?), "decimal"),
        "Decimal.round" => {
            let mode = Rounding::from_name(&variant_name(&arg(2))?).ok_or(Exit::Halt(Halt::Dynamic("rounding mode".into())))?;
            dec_result(dec(&arg(0))?.round(int(&arg(1))?, mode), "round")
        }
        "Decimal.fromFloat" => {
            let mode = Rounding::from_name(&variant_name(&arg(2))?).ok_or(Exit::Halt(Halt::Dynamic("rounding mode".into())))?;
            let x = match arg(0) {
                Value::Float(x) => x,
                _ => return dynamic("Decimal.fromFloat of a non-float"),
            };
            dec_result(Decimal::from_float(x, int(&arg(1))?, mode), "fromFloat")
        }
        "Decimal.scale" => Value::Int(dec(&arg(0))?.scale as i128),
        // ---- text (6.6)
        "String.from" => Value::Str(Rc::from(display(&arg(0)))),
        "String.scalarCount" | "StringView.scalarCount" => Value::Int(text(&arg(0))?.chars().count() as i128),
        "String.byteCount" | "StringView.byteCount" => Value::Int(text(&arg(0))?.len() as i128),
        "String.slice" | "StringView.slice" => {
            let s = text(&arg(0))?;
            let (x, y) = (int(&arg(1))?, int(&arg(2))?);
            let cs: Vec<char> = s.chars().collect();
            if 0 <= x && x <= y && y as usize <= cs.len() {
                Value::Str(Rc::from(cs[x as usize..y as usize].iter().collect::<String>()))
            } else {
                failure("text.slice_bounds", vec![("start", Value::Int(x)), ("end", Value::Int(y))])
            }
        }
        "String.scalarAt" | "StringView.scalarAt" => {
            let s = text(&arg(0))?;
            let i = int(&arg(1))?;
            match (i >= 0).then(|| s.chars().nth(i as usize)).flatten() {
                Some(c) => Value::Scalar(Rc::from(c.to_string())),
                None => failure("text.index", vec![("index", Value::Int(i))]),
            }
        }
        "Bytes.byteCount" | "BytesView.byteCount" | "BytesView.count" | "Bytes.count" => match arg(0) {
            Value::Bytes(b) => Value::Int(b.len() as i128),
            _ => return dynamic("byteCount of non-bytes"),
        },
        "Bytes.get" | "BytesView.get" => match arg(0) {
            Value::Bytes(b) => {
                let i = int(&arg(1))?;
                if i >= 0 && (i as usize) < b.len() {
                    Value::Int(b[i as usize] as i128)
                } else {
                    Value::Null
                }
            }
            _ => return dynamic("get of non-bytes"),
        },
        "Scalar.of" => {
            let s = text(&arg(0))?;
            if s.chars().count() != 1 {
                return dynamic("Scalar.of of a view that is not one scalar");
            }
            Value::Scalar(s)
        }
        "Scalar.digitValue" => {
            let s = text(&arg(0))?;
            match s.chars().collect::<Vec<_>>().as_slice() {
                [c] if c.is_ascii_digit() => Value::Int(c.to_digit(10).unwrap() as i128),
                _ => Value::Null,
            }
        }
        "Scalar.hexValue" => {
            let s = text(&arg(0))?;
            match s.chars().collect::<Vec<_>>().as_slice() {
                [c] if c.is_ascii_hexdigit() => Value::Int(c.to_digit(16).unwrap() as i128),
                _ => Value::Null,
            }
        }
        "Builder.create" => Value::Builder(shared(String::new())),
        "Builder.append" | "Builder.appendLine" | "Builder.appendScalar" => match arg(0) {
            Value::Builder(b) => {
                let s = display(&arg(1));
                b.borrow_mut().push_str(&s);
                if name == "Builder.appendLine" {
                    b.borrow_mut().push('\n');
                }
                Value::Void
            }
            _ => return dynamic("append to a non-builder"),
        },
        "Builder.finish" => match arg(0) {
            Value::Builder(b) => Value::Str(Rc::from(b.borrow().as_str())),
            _ => return dynamic("finish of a non-builder"),
        },
        "Text.words" => Value::list(text(&arg(0))?.split_whitespace().map(Value::str).collect()),
        // the everyday text operations (6.6, X-479), implemented once in fortis-runtime for the VM and native code
        "String.trim" | "StringView.trim" => Value::str(fortis_runtime::text::trim(&text(&arg(0))?)),
        "String.trimStart" | "StringView.trimStart" => Value::str(fortis_runtime::text::trim_start(&text(&arg(0))?)),
        "String.trimEnd" | "StringView.trimEnd" => Value::str(fortis_runtime::text::trim_end(&text(&arg(0))?)),
        "String.find" | "StringView.find" => match fortis_runtime::text::find(&text(&arg(0))?, &text(&arg(1))?) {
            Some(offset) => Value::Int(offset as i128),
            None => Value::Null,
        },
        "String.startsWith" | "StringView.startsWith" => Value::Bool(text(&arg(0))?.starts_with(&*text(&arg(1))?)),
        "String.endsWith" | "StringView.endsWith" => Value::Bool(text(&arg(0))?.ends_with(&*text(&arg(1))?)),
        "String.split" | "StringView.split" => Value::list(fortis_runtime::text::split(&text(&arg(0))?, &text(&arg(1))?).iter().map(|p| Value::str(p)).collect()),
        "String.lines" | "StringView.lines" => Value::list(fortis_runtime::text::lines(&text(&arg(0))?).iter().map(|p| Value::str(p)).collect()),
        "String.replace" | "StringView.replace" => Value::str(&fortis_runtime::text::replace(&text(&arg(0))?, &text(&arg(1))?, &text(&arg(2))?)),
        "String.asciiUpper" | "StringView.asciiUpper" => Value::str(&fortis_runtime::text::ascii_upper(&text(&arg(0))?)),
        "String.asciiLower" | "StringView.asciiLower" => Value::str(&fortis_runtime::text::ascii_lower(&text(&arg(0))?)),
        "graphemes" => Value::Int(graphemes(&text(&arg(0))?)),
        // ---- time
        "Duration.days" => dur(int(&arg(0))? * 86_400_000_000_000),
        "Duration.hours" => dur(int(&arg(0))? * 3_600_000_000_000),
        "Duration.minutes" => dur(int(&arg(0))? * 60_000_000_000),
        "Duration.seconds" => dur(int(&arg(0))? * 1_000_000_000),
        "Duration.milliseconds" => dur(int(&arg(0))? * 1_000_000),
        "Duration.add" | "Duration.sub" | "Duration.multiply" => match (arg(0), arg(1)) {
            (Value::Dur(x), Value::Dur(y)) if name == "Duration.add" => dur(x as i128 + y as i128),
            (Value::Dur(x), Value::Dur(y)) if name == "Duration.sub" => dur(x as i128 - y as i128),
            (Value::Dur(x), Value::Int(k)) => dur(x as i128 * k),
            _ => return dynamic(format!("{name} operands")),
        },
        "Duration.nanoseconds" => match arg(0) {
            Value::Dur(ns) => Value::Int(ns as i128),
            _ => return dynamic("nanoseconds"),
        },
        "Timestamp.epoch" => Value::Time(0),
        "Timestamp.add" => match (arg(0), arg(1)) {
            (Value::Time(t), Value::Dur(d)) => {
                let r = t as i128 + d as i128;
                if !(INT_MIN..=INT_MAX).contains(&r) {
                    failure("numeric.overflow", vec![("operation", Value::str("Timestamp.add"))])
                } else {
                    Value::Time(r as i64)
                }
            }
            _ => return dynamic("Timestamp.add"),
        },
        "Timestamp.after" | "Timestamp.before" => match (arg(0), arg(1)) {
            (Value::Time(x), Value::Time(y)) => Value::Bool(if name.ends_with("after") { x > y } else { x < y }),
            _ => return dynamic(name.to_string()),
        },
        // ---- money
        "Money.multiply" | "Money.divide" => {
            let m = match arg(0) {
                Value::Obj(o) => o,
                _ => return dynamic("Money operand"),
            };
            let (cur, amount) = {
                let o = m.borrow();
                (o.get("currency").cloned().unwrap_or(Value::Null), o.get("amount").cloned().unwrap_or(Value::Null))
            };
            let k = Decimal::from_int(int(&arg(1))?).map_err(|_| Exit::Halt(Halt::Dynamic("Money factor".into())))?;
            let r = if name == "Money.multiply" { dec(&amount)?.mul(&k) } else { dec(&amount)?.div(&k) };
            match r {
                Ok(d) => Value::Obj(shared(Obj::new(Rc::from("Money"), vec![(Rc::from("currency"), cur), (Rc::from("amount"), Value::Dec(d))]))),
                Err(e) => dec_result(Err(e), "decimal"),
            }
        }
        // ---- collections (6.7)
        "List.default" => Value::list(vec![]),
        "List.of" => Value::list(a.clone()),
        "List.count" | "ListView.count" | "Array.count" => Value::Int(items(&arg(0))?.borrow().len() as i128),
        // a view reads the array's own elements (X-578)
        "Array.asView" => arg(0),
        "List.get" | "ListView.get" | "Array.get" => {
            let xs = items(&arg(0))?;
            let i = int(&arg(1))?;
            let b = xs.borrow();
            if i >= 0 && (i as usize) < b.len() {
                b[i as usize].clone()
            } else {
                Value::Null
            }
        }
        "List.push" => {
            items(&arg(0))?.borrow_mut().push(arg(1));
            Value::Void
        }
        "List.pop" => items(&arg(0))?.borrow_mut().pop().unwrap_or(Value::Null),
        "List.at" => {
            let xs = items(&arg(0))?;
            let i = int(&arg(1))?;
            let b = xs.borrow();
            if i >= 0 && (i as usize) < b.len() {
                b[i as usize].clone()
            } else {
                failure("collection.index", vec![("index", Value::Int(i))])
            }
        }
        "List.set" | "Array.set" => {
            let xs = items(&arg(0))?;
            let i = int(&arg(1))?;
            let len = xs.borrow().len();
            if i < 0 || i as usize >= len {
                if name == "Array.set" {
                    return dynamic(format!("Array.set index {i} outside 0..{len} (proof-required)"));
                }
                failure("collection.index", vec![("index", Value::Int(i))])
            } else {
                let old = std::mem::replace(&mut xs.borrow_mut()[i as usize], arg(2));
                vm.destroy(old)?;
                Value::Void
            }
        }
        "List.clear" => {
            let xs = items(&arg(0))?;
            let old = std::mem::take(&mut *xs.borrow_mut());
            for v in old {
                vm.destroy(v)?;
            }
            Value::Void
        }
        "List.iterate" => Value::Cursor(shared((items(&arg(0))?.borrow().clone(), 0))),
        // `Deque.iterate` (X-576): the library class keeps `front`, the first item last, and `back`, the last item last;
        // the cursor walks them from the front to the back
        "Deque.iterate" => {
            let o = match arg(0).deref() {
                Value::Obj(o) => o,
                _ => return dynamic("Deque receiver"),
            };
            let o = o.borrow();
            let mut all: Vec<Value> = Vec::new();
            if let Some(f) = o.get("front") {
                all.extend(items(f)?.borrow().iter().rev().cloned());
            }
            if let Some(b) = o.get("back") {
                all.extend(items(b)?.borrow().iter().cloned());
            }
            Value::Cursor(shared((all, 0)))
        }
        "ListCursor.next" => match arg(0) {
            Value::Cursor(c) => {
                let mut c = c.borrow_mut();
                if c.1 >= c.0.len() {
                    Value::Null
                } else {
                    c.1 += 1;
                    c.0[c.1 - 1].clone()
                }
            }
            _ => return dynamic("next of a non-cursor"),
        },
        "List.consumeEach" => {
            let xs = items(&arg(0))?;
            let all = std::mem::take(&mut *xs.borrow_mut());
            let f = arg(1);
            for x in all {
                vm.call_value(&f, vec![x])?;
            }
            Value::Void
        }
        "Array.filled" => {
            let n = int(&arg(1))?;
            Value::Array(shared((0..n.max(0)).map(|_| deep_copy(&arg(0))).collect()))
        }
        "Map.default" => Value::Map(shared(OrderedMap::default())),
        "Map.get" => match arg(0) {
            Value::Map(m) => {
                let key = arg(1);
                if vm.custom_equality(&key) {
                    // a key whose type writes its own `equals`/`hash`: found by that equality, not a structural key
                    let entries = m.borrow().entries.clone();
                    let mut found = Value::Null;
                    for (k, v) in entries {
                        if vm.equal(&k, &key)? {
                            found = v;
                            break;
                        }
                    }
                    found
                } else {
                    let k = freeze(&key).map_err(|e| Exit::Halt(Halt::Dynamic(e)))?;
                    m.borrow().get(&k).map(|(_, v)| v.clone()).unwrap_or(Value::Null)
                }
            }
            _ => return dynamic("Map.get"),
        },
        "Map.set" => match arg(0) {
            Value::Map(m) => {
                let (key, val) = (arg(1), arg(2));
                if vm.custom_equality(&key) {
                    let entries = m.borrow().entries.clone();
                    let mut hit = None;
                    for (i, (k, _)) in entries.iter().enumerate() {
                        if vm.equal(k, &key)? {
                            hit = Some(i);
                            break;
                        }
                    }
                    match hit {
                        Some(i) => {
                            let old = std::mem::replace(&mut m.borrow_mut().entries[i].1, val);
                            vm.destroy(old)?;               // replacing keeps the first insertion's position
                        }
                        None => m.borrow_mut().entries.push((key, val)),
                    }
                    Value::Void
                } else {
                    let k = freeze(&key).map_err(|e| Exit::Halt(Halt::Dynamic(e)))?;
                    let old = m.borrow_mut().insert(k, key, val);
                    if let Some(o) = old {
                        vm.destroy(o)?;
                    }
                    Value::Void
                }
            }
            _ => return dynamic("Map.set"),
        },
        "Map.containsKey" => match arg(0) {
            Value::Map(m) => {
                let key = arg(1);
                if vm.custom_equality(&key) {
                    let entries = m.borrow().entries.clone();
                    let mut found = false;
                    for (k, _) in entries.iter() {
                        if vm.equal(k, &key)? {
                            found = true;
                            break;
                        }
                    }
                    Value::Bool(found)
                } else {
                    let k = freeze(&key).map_err(|e| Exit::Halt(Halt::Dynamic(e)))?;
                    Value::Bool(m.borrow().get(&k).is_some())
                }
            }
            _ => return dynamic("Map.containsKey"),
        },
        // `Map.remove` gives the removed value to the caller (null when absent); `Set.remove` says whether the item was there
        "Map.remove" | "Set.remove" => match arg(0) {
            Value::Map(m) | Value::Set(m) => {
                let key = arg(1);
                let pos = if vm.custom_equality(&key) {
                    let entries = m.borrow().entries.clone();
                    let mut hit = None;
                    for (i, (k, _)) in entries.iter().enumerate() {
                        if vm.equal(k, &key)? {
                            hit = Some(i);
                            break;
                        }
                    }
                    hit
                } else {
                    let k = freeze(&key).map_err(|e| Exit::Halt(Halt::Dynamic(e)))?;
                    m.borrow().index.get(&k).copied()
                };
                let is_set = name == "Set.remove";
                match pos {
                    None => {
                        if is_set {
                            Value::Bool(false)
                        } else {
                            Value::Null
                        }
                    }
                    Some(i) => {
                        let (k, v) = m.borrow_mut().remove_at(i);
                        vm.destroy(k)?;
                        if is_set {
                            Value::Bool(true)
                        } else {
                            v
                        }
                    }
                }
            }
            _ => return dynamic(name.to_string()),
        },
        "Map.count" | "Set.count" => match arg(0) {
            Value::Map(m) | Value::Set(m) => Value::Int(m.borrow().len() as i128),
            _ => return dynamic("count"),
        },
        "Set.default" => Value::Set(shared(OrderedMap::default())),
        "Set.add" => match arg(0) {
            Value::Set(m) => {
                let key = arg(1);
                if vm.custom_equality(&key) {
                    let entries = m.borrow().entries.clone();
                    for (k, _) in entries.iter() {
                        if vm.equal(k, &key)? {
                            return Ok(Value::Bool(false));
                        }
                    }
                    m.borrow_mut().entries.push((key, Value::Void));
                    Value::Bool(true)
                } else {
                    let k = freeze(&key).map_err(|e| Exit::Halt(Halt::Dynamic(e)))?;
                    if m.borrow().get(&k).is_some() {
                        Value::Bool(false)
                    } else {
                        m.borrow_mut().insert(k, key, Value::Void);
                        Value::Bool(true)
                    }
                }
            }
            _ => return dynamic("Set.add"),
        },
        "Set.contains" => match arg(0) {
            Value::Set(m) => {
                let key = arg(1);
                if vm.custom_equality(&key) {
                    let entries = m.borrow().entries.clone();
                    let mut found = false;
                    for (k, _) in entries.iter() {
                        if vm.equal(k, &key)? {
                            found = true;
                            break;
                        }
                    }
                    Value::Bool(found)
                } else {
                    let k = freeze(&key).map_err(|e| Exit::Halt(Halt::Dynamic(e)))?;
                    Value::Bool(m.borrow().get(&k).is_some())
                }
            }
            _ => return dynamic("Set.contains"),
        },
        // ---- outcomes (10)
        "Must.create" | "Must.discharge" => Value::Void,
        "Outcome.success" | "Outcome.failure" => arg(0),
        "Outcome.collect" => {
            let xs = items(&arg(0))?.borrow().clone();
            match xs.iter().find(|x| x.is_failure()) {
                Some(f) => f.clone(),
                None => Value::list(xs),
            }
        }
        "Failures.aggregate" => {
            let fs: Vec<Shared<FailureVal>> = items(&arg(0))?.borrow().iter().filter_map(|x| if let Value::Failure(f) = x { Some(f.clone()) } else { None }).collect();
            let origin = vm.stack.last().cloned().unwrap_or_else(|| "main".into());
            let agg = shared(FailureVal { code: Rc::from("failure.aggregate"), payload: vec![(Rc::from("count"), Value::Int(fs.len() as i128))], origin: Rc::from(origin), message: Rc::from(""), cause: None, annotations: vec![] });
            let mut prev = agg.clone();
            for f in fs {
                prev.borrow_mut().cause = Some(f.clone());
                prev = f;
            }
            Value::Failure(agg)
        }
        "annotate" => match arg(0) {
            Value::Failure(f) => {
                f.borrow_mut().annotations.push(arg(1));
                Value::Failure(f)
            }
            _ => return dynamic("annotate of a non-failure"),
        },
        // ---- shared ownership and synchronisation (7.8, 12)
        "Shared.create" => Value::SharedRef(Rc::new(SharedBox { value: RefCell::new(arg(0)), strong: Cell::new(1) })),
        "Shared.retain" => match arg(0) {
            Value::SharedRef(b) => {
                b.strong.set(b.strong.get() + 1);
                Value::SharedRef(b)
            }
            _ => return dynamic("retain"),
        },
        "Shared.downgrade" => match arg(0) {
            Value::SharedRef(b) => Value::WeakRef(b),
            _ => return dynamic("downgrade"),
        },
        "Weak.upgrade" => match arg(0) {
            Value::WeakRef(b) => {
                if b.strong.get() == 0 {
                    Value::Null
                } else {
                    b.strong.set(b.strong.get() + 1);
                    Value::SharedRef(b)
                }
            }
            _ => return dynamic("upgrade"),
        },
        "Weak.retain" => match arg(0) {
            Value::WeakRef(b) => Value::WeakRef(b),
            _ => return dynamic("Weak.retain"),
        },
        "Mutex.create" => Value::Mutex(shared(MutexVal { value: arg(0), held: false })),
        // ---- structured concurrency (12.3–12.6; the executor in executor.rs)
        "Scope.launch" | "Scope.launchParallel" | "Scope.launchBlocking" => {
            let sc = match arg(0).deref() {
                Value::Scope(s) => s,
                other => return dynamic(format!("{name} on {}", display(&other))),
            };
            let executor = match name {
                "Scope.launch" => "cooperative",
                "Scope.launchParallel" => "parallel",
                _ => "blocking",
            };
            match arg(1).deref() {
                Value::Future(f) => vm.launch_future(&f, Some(&sc), executor)?,
                c @ (Value::Closure(_) | Value::Func(_)) => vm.launch(crate::executor::Work::Closure(c), "work", Some(&sc), executor)?,
                other => return dynamic(format!("{name} of {}, which is not a future", display(&other))),
            }
        }
        "Scope.detach" => match (arg(0).deref(), arg(1).deref()) {
            (Value::Scope(sc), Value::Task(t)) => {
                vm.detach(&sc, t.id)?;
                Value::Void
            }
            _ => return dynamic("Scope.detach takes a task"),
        },
        "Task.abandon" => match arg(0).deref() {
            Value::Task(t) => {
                vm.cancel(t.id);
                vm.join_task(t.id)?;
                Value::Void
            }
            _ => return dynamic("Task.abandon"),
        },
        "join" => Value::Join(Rc::new(a.iter().map(|v| v.deref()).collect())),
        "checkpoint" => {
            vm.checkpoint()?;
            Value::Void
        }
        "injectDefect" => {
            let kind = match arg(0).deref() {
                Value::Str(s) => s.to_string(),
                other => display(&other),
            };
            return defect(kind);
        }
        "Clock.sleep" => match arg(1).deref() {
            Value::Dur(ns) => Value::Sleep(ns),
            other => return dynamic(format!("Clock.sleep of {}", display(&other))),
        },
        // `lock` completes without suspending on the development executor: one logical thread holds no lock
        // across an await of another task (12.7)
        "Mutex.lock" => Value::Ready(Rc::new(call(vm, "Mutex.lockBlocking", a.clone())?)),
        "Mutex.lockBlocking" => match arg(0) {
            Value::Mutex(m) => {
                if m.borrow().held {
                    return dynamic("lockBlocking on a held mutex would block forever on the single-threaded VM");
                }
                m.borrow_mut().held = true;
                Value::Guard(m)
            }
            _ => return dynamic("lockBlocking"),
        },
        // the protected value, through the exclusive guard (12.7; D-23)
        "MutexGuard.get" => match arg(0) {
            Value::Guard(m) => deep_copy(&m.borrow().value),
            _ => return dynamic("MutexGuard.get"),
        },
        "MutexGuard.set" => match arg(0) {
            Value::Guard(m) => {
                let old = std::mem::replace(&mut m.borrow_mut().value, arg(1));
                vm.destroy(old)?;
                Value::Void
            }
            _ => return dynamic("MutexGuard.set"),
        },
        "MutexGuard.replace" => match arg(0) {
            Value::Guard(m) => std::mem::replace(&mut m.borrow_mut().value, arg(1)),
            _ => return dynamic("MutexGuard.replace"),
        },
        "AtomicInt.create" => Value::Atomic(Rc::from("AtomicInt"), shared(arg(0))),
        "AtomicBool.create" => Value::Atomic(Rc::from("AtomicBool"), shared(arg(0))),
        "AtomicInt.load" | "AtomicBool.load" => match arg(0) {
            Value::Atomic(_, v) => v.borrow().clone(),
            _ => return dynamic("load"),
        },
        "AtomicInt.store" | "AtomicBool.store" => match arg(0) {
            Value::Atomic(_, v) => {
                *v.borrow_mut() = arg(1);
                Value::Void
            }
            _ => return dynamic("store"),
        },
        "AtomicInt.fetchAdd" => match arg(0) {
            Value::Atomic(_, v) => {
                let old = int(&v.borrow())?;
                *v.borrow_mut() = Value::Int(wrap(old + int(&arg(1))?));
                Value::Int(old)
            }
            _ => return dynamic("fetchAdd"),
        },
        // ---- host: development adapters (15, 11.6)
        "Host.capabilities" => {
            // the capabilities are created once per run (11.6): the checker admits one direct call, in the entry
            // `main`; a second creation means that guarantee is broken, the runtime-invariant defect (10.8; D-110)
            if vm.caps_made.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return defect("runtime_invariant");
            }
            let kinds = ["fileSystem", "network", "clock", "random", "environment", "process", "supervisor", "console", "secretAccess", "rawSql"];
            let mut fields = Vec::new();
            for k in kinds {
                let c = cap(k);
                if let Value::Cap(cv) = &c {
                    if k == "fileSystem" {
                        cv.state.borrow_mut().insert("root".into(), Value::str(&dev_root()));
                    }
                    if k == "random" {
                        let seed: i128 = std::env::var("FORTIS_DEV_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
                        cv.state.borrow_mut().insert("seed".into(), Value::Int(seed));
                    }
                }
                fields.push((Rc::from(k), c));
            }
            Value::Obj(shared(Obj::new(Rc::from("Capabilities"), fields)))
        }
        "Path.parse" => {
            let t = text(&arg(0))?;
            if t.is_empty() || t.contains('\0') {
                failure("path.invalid", vec![("text", Value::Str(t))])
            } else {
                Value::Path(t)
            }
        }
        "reveal" => match arg(0) {
            Value::Secret(v) => deep_copy(&v),
            _ => return dynamic("reveal of a non-secret"),
        },
        // a log statement (13, LANG-13-04): one `log` record on the diagnostic plane, never the program's output
        "log.info" | "log.warn" | "log.error" => {
            // the writing function by its source name: a lambda's is its enclosing function's, as native names it
            let function = vm.stack.last().map(|n| n.split('#').next().unwrap_or("").to_string()).unwrap_or_else(|| "main".into());
            let rec = vec![("kind".into(), "log".into()), ("level".into(), name[4..].to_string()), ("message".into(), display(&arg(0))), ("function".into(), function)];
            vm.emit(rec);
            Value::Void
        }
        "Console.print" => {
            let s = display(&arg(1));
            // a session attributes each line to its root invocation and generation (SP-07 design 2)
            if let Some(g) = &vm.gen {
                // the driver's VM runs a synchronous invocation with its root set; a task's VM has its task's root
                let root = if vm.root != 0 { vm.root } else { vm.sched.as_ref().map(|sc| sc.lock().tasks[vm.task].root).unwrap_or(0) };
                vm.reports.push(vec![("kind".into(), "print".into()), ("root".into(), root.to_string()), ("gen".into(), g.id.to_string()), ("text".into(), s.clone())]);
            }
            vm.out.push(s);
            Value::Void
        }
        "Network.resolve" => {
            let h = text(&arg(1))?;
            if &*h == "localhost" {
                Value::str("127.0.0.1")
            } else {
                failure("network.unreachable", vec![("host", Value::Str(h))])
            }
        }
        "Clock.now" => match arg(0) {
            Value::Cap(c) => match c.state.borrow().get("frozen") {
                Some(v) => v.clone(),
                None => Value::Time(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)),
            },
            _ => return dynamic("Clock.now"),
        },
        "Clock.frozen" => {
            let c = cap("clock");
            if let Value::Cap(cv) = &c {
                cv.state.borrow_mut().insert("frozen".into(), arg(1));
            }
            c
        }
        "Clock.elapsedSince" => {
            let now = call(vm, "Clock.now", vec![arg(0)])?;
            match (now, arg(1)) {
                (Value::Time(n), Value::Time(t)) => {
                    if t > n {
                        failure("clock.future", vec![])
                    } else {
                        dur(n as i128 - t as i128)
                    }
                }
                _ => return dynamic("elapsedSince"),
            }
        }
        "Random.below" => match arg(0) {
            Value::Cap(c) => {
                let bound = int(&arg(1))?;
                if bound < 1 {
                    return dynamic("Random.below needs bound >= 1 (proof-required)");
                }
                let b = bound as u128;
                let limit = ((1u128 << 64) / b) * b;
                loop {
                    let seed = c.state.borrow().get("seed").and_then(|v| v.as_int()).unwrap_or(0) as u64;
                    let (s, z) = splitmix(seed);
                    c.state.borrow_mut().insert("seed".into(), Value::Int(s as i128));
                    if (z as u128) < limit {
                        break Value::Int((z as u128 % b) as i128);
                    }
                }
            }
            _ => return dynamic("Random.below"),
        },
        "Random.seeded" => {
            let r = cap("random");
            if let Value::Cap(cv) = &r {
                cv.state.borrow_mut().insert("seed".into(), Value::Int(int(&arg(1))?.rem_euclid(1i128 << 64)));
            }
            r
        }
        "Environment.get" => {
            let n = text(&arg(1))?;
            if n.is_empty() || n.contains('=') {
                Value::Null
            } else {
                match std::env::var(&*n) {
                    Ok(v) => Value::Secret(Rc::new(Value::str(&v))),
                    Err(_) => Value::Null,
                }
            }
        }
        "Process.run" => {
            let p = text(&arg(1))?;
            match &*p {
                "true" => Value::Int(0),
                "false" => Value::Int(1),
                _ => failure("process.denied", vec![("program", Value::Str(p))]),
            }
        }
        "RawSql.execute" => failure("sql.unavailable", vec![]),
        "FileSystem.readText" | "FileSystem.readAll" | "FileSystem.writeAll" | "FileSystem.open" | "FileSystem.create" | "FileSystem.subtree" => {
            let c = match arg(0) {
                Value::Cap(c) => c,
                _ => return dynamic(name.to_string()),
            };
            let p = match arg(1) {
                Value::Path(p) => p,
                _ => return dynamic("expected a Path"),
            };
            if name == "FileSystem.subtree" {
                let sub = cap("fileSystem");
                if let (Ok(full), Value::Cap(sc)) = (fs_resolve(&c, &p), &sub) {
                    let _ = std::fs::create_dir_all(&full);
                    sc.state.borrow_mut().insert("root".into(), Value::str(&full));
                }
                return Ok(sub);
            }
            let full = match fs_resolve(&c, &p) {
                Ok(f) => f,
                Err(e) => return Ok(e),
            };
            let fp = std::path::Path::new(&full);
            let pv = || vec![("path", Value::Path(p.clone()))];
            match name {
                "FileSystem.readText" | "FileSystem.readAll" | "FileSystem.open" => {
                    if !fp.exists() {
                        return Ok(failure("fs.not_found", pv()));
                    }
                    if !fp.is_file() {
                        return Ok(failure(if name == "FileSystem.open" { "fs.not_found" } else { "fs.not_a_file" }, pv()));
                    }
                    match name {
                        "FileSystem.readText" => match std::fs::read_to_string(fp) {
                            Ok(s) => Value::str(&s),
                            Err(_) => failure("fs.invalid_text", pv()),
                        },
                        "FileSystem.readAll" => Value::Bytes(Rc::new(std::fs::read(fp).unwrap_or_default())),
                        _ => Value::File(shared(FileVal { path: full.clone(), mode: 'r', closed: false, pos: 0 })),
                    }
                }
                _ => {
                    let parent_ok = fp.parent().map_or(true, |d| d.is_dir());
                    if !parent_ok {
                        return Ok(failure("fs.not_found", pv()));
                    }
                    if name == "FileSystem.writeAll" {
                        let _ = std::fs::write(fp, text(&arg(2))?.as_bytes());
                        Value::Void
                    } else {
                        let _ = std::fs::write(fp, b"");
                        Value::File(shared(FileVal { path: full.clone(), mode: 'w', closed: false, pos: 0 }))
                    }
                }
            }
        }
        "File.readAll" | "File.writeAll" | "File.close" => {
            let f = match arg(0) {
                Value::File(f) => f,
                _ => return dynamic(name.to_string()),
            };
            if name == "File.close" {
                f.borrow_mut().closed = true;
                return Ok(Value::Void);
            }
            if f.borrow().closed {
                return Ok(failure("fs.closed", vec![]));
            }
            if name == "File.readAll" {
                if f.borrow().mode != 'r' {
                    return Ok(failure("fs.wrong_mode", vec![]));
                }
                let data = std::fs::read(&f.borrow().path).unwrap_or_default();
                let pos = f.borrow().pos.min(data.len());
                let rest = data[pos..].to_vec();
                f.borrow_mut().pos += rest.len();
                Value::Bytes(Rc::new(rest))
            } else {
                if f.borrow().mode != 'w' {
                    return Ok(failure("fs.wrong_mode", vec![]));
                }
                let bytes = match arg(1) {
                    Value::Bytes(b) => b,
                    _ => return dynamic("File.writeAll of non-bytes"),
                };
                use std::io::Write;
                if let Ok(mut h) = std::fs::OpenOptions::new().append(true).open(&f.borrow().path) {
                    let _ = h.write_all(&bytes);
                }
                Value::Void
            }
        }
        other => {
            if let Some(rest) = other.strip_prefix("kit:") {
                let (module, tname) = rest.split_once(':').unwrap_or((rest, ""));
                return dynamic(format!("kit not available: `{tname}` comes from {module}, which this kernel does not implement"));
            }
            // a type's own `equals`/`compare` that its declaration does not provide: a core type's, or a derived
            // Equatable/Comparable's, run by the kernel (X-516; D-84, D-85). The checker proved the receiver's type
            // implements the interface; a program type is an object or enum, a core type names it in the table.
            if let Some((tname, m)) = other.rsplit_once('.') {
                if (m == "equals" || m == "compare") && a.len() == 2 {
                    let recv = arg(0);
                    let core_ok = (m == "equals" && fortis_types::decls::builtin_implements(tname, "Equatable"))
                        || (m == "compare" && fortis_types::decls::builtin_implements(tname, "Comparable"));
                    if matches!(recv, Value::Obj(_) | Value::Enum(_)) || core_ok {
                        return if m == "equals" {
                            Ok(Value::Bool(vm.equal(&recv, &arg(1))?))
                        } else {
                            let variant = match vm.compare(&recv, &arg(1))? {
                                std::cmp::Ordering::Less => "Less",
                                std::cmp::Ordering::Equal => "Equal",
                                std::cmp::Ordering::Greater => "Greater",
                            };
                            Ok(Value::Enum(Rc::new(EnumVal { ty: Rc::from("Ordering"), variant: Rc::from(variant), fields: vec![] })))
                        };
                    }
                }
            }
            return dynamic(format!("`{other}` is not implemented in the VM"));
        }
    })
}

/// The shared Unicode grapheme implementation (D-713).
fn graphemes(s: &str) -> i128 {
    fortis_runtime::graphemes::count(s) as i128
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D-110's backstop: the checker admits one creation of the capabilities per run; the VM refuses a second one
    /// with the runtime-invariant defect (10.8), shared by every VM of the run.
    #[test]
    fn a_second_creation_is_the_runtime_invariant_defect() {
        let prog = fortis_mir::model::Program::default();
        let mut vm = Vm::new(&prog, "t.ft");
        assert!(call(&mut vm, "Host.capabilities", vec![]).is_ok(), "the first creation is admitted");
        let mut task = Vm::new(&prog, "t.ft");
        task.caps_made = vm.caps_made.clone();
        match call(&mut task, "Host.capabilities", vec![]) {
            Err(Exit::Halt(Halt::Defect(k))) => assert_eq!(k, "runtime_invariant"),
            _ => panic!("a second creation in the run must be the runtime-invariant defect"),
        }
        assert!(call(&mut Vm::new(&prog, "t.ft"), "Host.capabilities", vec![]).is_ok(), "another run creates its own");
        let _ = std::fs::remove_dir_all(dev_root());
    }
}
