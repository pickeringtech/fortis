//! The canonical codec of the VM (Language Reference 6.10; DEL-38), a port of the definitional interpreter's
//! (interpreter/codec.py). Both formats are published in conformance/codec/FORMAT.md.
//!
//! Encoding follows the value; decoding follows the expected type (a descriptor built from the IR type and the
//! program's type definitions), running refinements and nominal validation, and yields a `codec.invalid`,
//! `codec.unknown_field` or refinement failure value instead of raising.

use fortis_mir::model::{Expr, Program, TypeKind};
use fortis_runtime::Decimal;
use fortis_types::types::Type;

use crate::machine::{dynamic, Frame, Vm, R};
use crate::sync::Rc;
use crate::value::{display, freeze, shared, EnumVal, FailureVal, Obj, OrderedMap, Value};

const MAGIC: &[u8; 4] = b"FTB\x01";

const T_NULL: u8 = 0x00;
const T_FALSE: u8 = 0x01;
const T_TRUE: u8 = 0x02;
const T_INT: u8 = 0x03;
const T_FLOAT: u8 = 0x04;
const T_DECIMAL: u8 = 0x05;
const T_STRING: u8 = 0x06;
const T_BYTES: u8 = 0x07;
const T_LIST: u8 = 0x08;
const T_MAP: u8 = 0x09;
const T_RECORD: u8 = 0x0A;
const T_VARIANT: u8 = 0x0B;
const T_DURATION: u8 = 0x0C;
const T_TIMESTAMP: u8 = 0x0D;
const T_SCALAR: u8 = 0x0E;
const T_FLOAT32: u8 = 0x0F;
const T_SET: u8 = 0x10;
const T_TUPLE: u8 = 0x11;

/// A decoding failure: its code and detail become the failure value.
struct Invalid {
    code: &'static str,
    detail: String,
}

type D<T> = Result<T, Invalid>;

fn bad<T>(detail: impl Into<String>) -> D<T> {
    Err(Invalid { code: "codec.invalid", detail: detail.into() })
}

// ---------------------------------------------------------------------------------------------- shared helpers

fn float_text(x: f64) -> String {
    if x.is_nan() {
        return "\"NaN\"".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "\"Infinity\"".into() } else { "\"-Infinity\"".into() };
    }
    fortis_runtime::float::display(x)
}

pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Strict base64 (Python's `b64decode(validate=True)`): the standard alphabet, padding to a multiple of four.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| B64.iter().position(|&x| x == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (ci, chunk) in bytes.chunks(4).enumerate() {
        let last = ci == bytes.len() / 4 - 1;
        let pads = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pads > 2 || (pads > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            let v = if i >= 4 - pads { 0 } else { val(c)? };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pads < 2 {
            out.push((n >> 8) as u8);
        }
        if pads < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn varint(out: &mut Vec<u8>, mut n: u128) {
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n != 0 {
            out.push(b | 0x80);
        } else {
            out.push(b);
            return;
        }
    }
}

fn zigzag(n: i128) -> u128 {
    if n >= 0 {
        (n as u128) << 1
    } else {
        (((-(n + 1)) as u128) << 1) + 1
    }
}

// ---------------------------------------------------------------------------------------------- encoding

/// The fields of a class value in declaration order.
fn class_fields(prog: &Program, o: &Obj) -> Vec<(String, Value)> {
    let decl = prog.types.iter().find(|t| t.name == *o.ty).and_then(|t| match &t.kind {
        TypeKind::Class { fields } => Some(fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>()),
        _ => None,
    });
    match decl {
        Some(names) => names.into_iter().filter_map(|n| o.get(&n).map(|v| (n, v.clone()))).collect(),
        None => o.fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
    }
}

/// A variant's payload in declaration order.
fn variant_fields(prog: &Program, e: &EnumVal) -> Vec<(String, Value)> {
    let decl = prog.types.iter().find(|t| t.name == *e.ty).and_then(|t| match &t.kind {
        TypeKind::Enum { variants } => variants.iter().find(|v| v.name == *e.variant).map(|v| v.fields.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()),
        _ => None,
    });
    match decl {
        Some(names) => names.into_iter().filter_map(|n| e.fields.iter().find(|(k, _)| **k == *n).map(|(_, v)| (n.clone(), v.clone()))).collect(),
        None => e.fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
    }
}

fn variant_index(prog: &Program, e: &EnumVal) -> usize {
    prog.types
        .iter()
        .find(|t| t.name == *e.ty)
        .and_then(|t| match &t.kind {
            TypeKind::Enum { variants } => variants.iter().position(|v| v.name == *e.variant),
            _ => None,
        })
        .unwrap_or(0)
}

pub fn to_text(prog: &Program, v: &Value) -> R<String> {
    Ok(match v {
        Value::Null => "null".into(),
        Value::Bool(b) => if *b { "true".into() } else { "false".into() },
        Value::Int(n) => n.to_string(),
        Value::Float(x) => float_text(*x),
        Value::F32(x) => float_text(*x as f64),
        Value::Str(s) | Value::Scalar(s) => json_string(s),
        Value::Dec(d) => json_string(&d.to_string()),
        Value::Dur(ns) => ns.to_string(),
        Value::Time(ns) => json_string(&fortis_runtime::time::display_timestamp(*ns)),
        Value::Bytes(b) => json_string(&base64_encode(b)),
        Value::Map(m) => {
            let parts: R<Vec<String>> = m.borrow().entries.iter().map(|(k, x)| Ok(format!("[{},{}]", to_text(prog, k)?, to_text(prog, x)?))).collect();
            format!("[{}]", parts?.join(","))
        }
        Value::Set(m) => {
            let parts: R<Vec<String>> = m.borrow().entries.iter().map(|(k, _)| to_text(prog, k)).collect();
            format!("[{}]", parts?.join(","))
        }
        Value::List(items) | Value::Array(items) => {
            let parts: R<Vec<String>> = items.borrow().iter().map(|x| to_text(prog, x)).collect();
            format!("[{}]", parts?.join(","))
        }
        Value::Tuple(items) => {
            let parts: R<Vec<String>> = items.iter().map(|x| to_text(prog, x)).collect();
            format!("[{}]", parts?.join(","))
        }
        Value::Record(fs) => {
            let parts: R<Vec<String>> = fs.borrow().iter().map(|(k, x)| Ok(format!("{}:{}", json_string(k), to_text(prog, x)?))).collect();
            format!("{{{}}}", parts?.join(","))
        }
        Value::Obj(o) => {
            let fs = class_fields(prog, &o.borrow());
            let parts: R<Vec<String>> = fs.iter().map(|(k, x)| Ok(format!("{}:{}", json_string(k), to_text(prog, x)?))).collect();
            format!("{{{}}}", parts?.join(","))
        }
        Value::Enum(e) => {
            let fs = variant_fields(prog, e);
            if fs.is_empty() {
                json_string(&e.variant)
            } else {
                let parts: R<Vec<String>> = fs.iter().map(|(k, x)| Ok(format!("{}:{}", json_string(k), to_text(prog, x)?))).collect();
                format!("{{{}:{{{}}}}}", json_string(&e.variant), parts?.join(","))
            }
        }
        Value::Cell(c) => to_text(prog, &c.borrow())?,
        other => return dynamic(format!("{} is not Encodable", other.type_name())),
    })
}

pub fn to_bytes(prog: &Program, v: &Value) -> R<Vec<u8>> {
    let mut out = MAGIC.to_vec();
    enc(prog, v, &mut out)?;
    Ok(out)
}

fn enc(prog: &Program, v: &Value, out: &mut Vec<u8>) -> R<()> {
    match v {
        Value::Null => out.push(T_NULL),
        Value::Bool(b) => out.push(if *b { T_TRUE } else { T_FALSE }),
        Value::Int(n) => {
            out.push(T_INT);
            varint(out, zigzag(*n));
        }
        Value::Float(x) => {
            out.push(T_FLOAT);
            out.extend_from_slice(&x.to_le_bytes());
        }
        Value::F32(x) => {
            out.push(T_FLOAT32);
            out.extend_from_slice(&x.to_le_bytes());
        }
        Value::Str(s) => {
            out.push(T_STRING);
            varint(out, s.len() as u128);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Scalar(s) => {
            out.push(T_SCALAR);
            varint(out, s.chars().next().map_or(0, |c| c as u128));
        }
        Value::Dec(d) => {
            out.push(T_DECIMAL);
            out.push(if d.neg { 1 } else { 0 });
            out.push(d.scale as u8);
            varint(out, d.coeff);
        }
        Value::Dur(ns) => {
            out.push(T_DURATION);
            varint(out, zigzag(*ns as i128));
        }
        Value::Time(ns) => {
            out.push(T_TIMESTAMP);
            varint(out, zigzag(*ns as i128));
        }
        Value::Bytes(b) => {
            out.push(T_BYTES);
            varint(out, b.len() as u128);
            out.extend_from_slice(b);
        }
        Value::Map(m) => {
            out.push(T_MAP);
            let m = m.borrow();
            varint(out, m.entries.len() as u128);
            for (k, x) in &m.entries {
                enc(prog, k, out)?;
                enc(prog, x, out)?;
            }
        }
        Value::Set(m) => {
            out.push(T_SET);
            let m = m.borrow();
            varint(out, m.entries.len() as u128);
            for (k, _) in &m.entries {
                enc(prog, k, out)?;
            }
        }
        Value::List(items) | Value::Array(items) => {
            out.push(T_LIST);
            let items = items.borrow();
            varint(out, items.len() as u128);
            for x in items.iter() {
                enc(prog, x, out)?;
            }
        }
        Value::Tuple(items) => {
            out.push(T_TUPLE);
            varint(out, items.len() as u128);
            for x in items.iter() {
                enc(prog, x, out)?;
            }
        }
        Value::Record(fs) => {
            out.push(T_RECORD);
            let fs = fs.borrow();
            varint(out, fs.len() as u128);
            for (_, x) in fs.iter() {
                enc(prog, x, out)?;
            }
        }
        Value::Obj(o) => {
            out.push(T_RECORD);
            let fs = class_fields(prog, &o.borrow());
            varint(out, fs.len() as u128);
            for (_, x) in &fs {
                enc(prog, x, out)?;
            }
        }
        Value::Enum(e) => {
            out.push(T_VARIANT);
            varint(out, variant_index(prog, e) as u128);
            let fs = variant_fields(prog, e);
            varint(out, fs.len() as u128);
            for (_, x) in &fs {
                enc(prog, x, out)?;
            }
        }
        Value::Cell(c) => enc(prog, &c.borrow(), out)?,
        other => return dynamic(format!("{} is not Encodable", other.type_name())),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------- descriptors

#[derive(Clone, Debug)]
enum Desc {
    /// type range, then a refinement's range
    Int(Option<i128>, Option<i128>, Option<i128>, Option<i128>),
    Bool,
    Float,
    Float32,
    Decimal,
    /// a scalar-count refinement
    Str(Option<i128>, Option<i128>),
    Scalar,
    Bytes,
    Duration,
    Timestamp,
    List(Box<Desc>),
    Array(Box<Desc>, usize),
    Set(Box<Desc>),
    Map(Box<Desc>, Box<Desc>),
    Opt(Box<Desc>),
    Tuple(Vec<Desc>),
    Record(Vec<(String, Desc)>),
    Class(String, Vec<(String, Desc, Option<Expr>)>),
    Enum(String, Vec<(String, Vec<(String, Desc)>)>),
    /// a length refinement on a collection
    Sized(Box<Desc>, Option<i128>, Option<i128>),
}

fn int_range(name: &str) -> Option<(i128, i128)> {
    Some(match name {
        "Int" | "Int64" => (i64::MIN as i128, i64::MAX as i128),
        "Int32" => (i32::MIN as i128, i32::MAX as i128),
        "Int16" => (i16::MIN as i128, i16::MAX as i128),
        "Int8" => (-128, 127),
        "UInt" | "UInt64" => (0, u64::MAX as i128),
        "UInt32" => (0, u32::MAX as i128),
        "UInt16" => (0, u16::MAX as i128),
        "UInt8" | "Byte" => (0, 255),
        _ => return None,
    })
}

fn desc(prog: &Program, t: &Type, tenv: &[(String, Desc)], depth: usize) -> Result<Desc, String> {
    if depth > 40 {
        return Err("a recursive type without indirection is outside the codec".into());
    }
    let d = |t: &Type| desc(prog, t, tenv, depth + 1);
    Ok(match t {
        Type::Optional(i) => Desc::Opt(Box::new(d(i)?)),
        Type::Tuple(ts) => Desc::Tuple(ts.iter().map(d).collect::<Result<_, _>>()?),
        Type::Record(fs) => Desc::Record(fs.iter().map(|(n, t)| Ok((n.clone(), d(t)?))).collect::<Result<_, String>>()?),
        Type::Loan { inner, .. } => d(inner)?,
        Type::TypeParam(n) => match tenv.iter().find(|(p, _)| p == n) {
            Some((_, x)) => x.clone(),
            None => return Err(format!("type parameter {n} is not Decodable")),
        },
        Type::Refined { base, lo, hi, .. } => {
            let name = base.name.as_str();
            if let Some((tlo, thi)) = int_range(name) {
                Desc::Int(Some(tlo), Some(thi), *lo, *hi)
            } else if name == "String" || name == "StringView" {
                Desc::Str(*lo, *hi)
            } else if matches!(name, "List" | "ListView" | "Set" | "Map") {
                Desc::Sized(Box::new(d(&Type::Named(base.clone()))?), *lo, *hi)
            } else {
                d(&Type::Named(base.clone()))?
            }
        }
        Type::Named(n) => {
            let name = n.name.as_str();
            let arg = |i: usize| n.args.get(i).map(d).unwrap_or(Ok(Desc::Bool));
            if let Some((p, x)) = tenv.iter().find(|(p, _)| p == name) {
                let _ = p;
                return Ok(x.clone());
            }
            if let Some((tlo, thi)) = int_range(name) {
                return Ok(Desc::Int(Some(tlo), Some(thi), None, None));
            }
            match name {
                "Bool" => Desc::Bool,
                "Float" => Desc::Float,
                "Float32" => Desc::Float32,
                "Decimal" => Desc::Decimal,
                "String" | "StringView" => Desc::Str(None, None),
                "Scalar" => Desc::Scalar,
                "Bytes" | "BytesView" => Desc::Bytes,
                "Duration" => Desc::Duration,
                "Timestamp" => Desc::Timestamp,
                "List" | "ListView" => Desc::List(Box::new(arg(0)?)),
                "Set" => Desc::Set(Box::new(arg(0)?)),
                "Map" => Desc::Map(Box::new(arg(0)?), Box::new(arg(1)?)),
                "Array" => {
                    let size = n.args.get(1).and_then(|s| match s {
                        Type::Named(x) => x.name.parse::<usize>().ok(),
                        _ => None,
                    });
                    Desc::Array(Box::new(arg(0)?), size.unwrap_or(0))
                }
                _ => {
                    let td = match prog.types.iter().find(|t| t.name == name) {
                        Some(t) => t,
                        None => return Err(format!("{name} is not Decodable")),
                    };
                    let args: Vec<Desc> = n.args.iter().map(d).collect::<Result<_, _>>()?;
                    let env: Vec<(String, Desc)> = td.type_params.iter().cloned().zip(args).collect();
                    let d2 = |t: &Type| desc(prog, t, &env, depth + 1);
                    match &td.kind {
                        TypeKind::Class { fields } => Desc::Class(td.name.clone(), fields.iter().map(|f| Ok((f.name.clone(), d2(&f.ty)?, f.default.clone()))).collect::<Result<_, String>>()?),
                        TypeKind::Enum { variants } => Desc::Enum(
                            td.name.clone(),
                            variants.iter().map(|v| Ok((v.name.clone(), v.fields.iter().map(|(f, t)| Ok((f.clone(), d2(t)?))).collect::<Result<_, String>>()?))).collect::<Result<_, String>>()?,
                        ),
                        TypeKind::Newtype { repr } => d2(repr)?,
                        _ => return Err(format!("{name} is not Decodable")),
                    }
                }
            }
        }
        other => return Err(format!("{} is not Decodable", other.show())),
    })
}

fn check_int(d: &Desc, n: Option<i128>, shown: &str) -> D<i128> {
    let (tlo, thi, rlo, rhi) = match d {
        Desc::Int(a, b, c, e) => (*a, *b, *c, *e),
        _ => (None, None, None, None),
    };
    let n = match n {
        Some(n) => n,
        None => return bad(format!("{shown} does not fit {}..{}", tlo.map_or(String::new(), |x| x.to_string()), thi.map_or(String::new(), |x| x.to_string()))),
    };
    if tlo.map_or(false, |l| n < l) || thi.map_or(false, |h| n > h) {
        return bad(format!("{n} does not fit"));
    }
    if rlo.map_or(false, |l| n < l) || rhi.map_or(false, |h| n > h) {
        return Err(Invalid { code: "refinement.out_of_range", detail: format!("{n} outside its refinement") });
    }
    Ok(n)
}

fn sized(lo: Option<i128>, hi: Option<i128>, v: Value) -> D<Value> {
    let n = match &v {
        Value::List(i) | Value::Array(i) => i.borrow().len(),
        Value::Set(m) | Value::Map(m) => m.borrow().entries.len(),
        _ => 0,
    } as i128;
    if lo.map_or(false, |l| n < l) || hi.map_or(false, |h| n > h) {
        return Err(Invalid { code: "refinement.out_of_range", detail: format!("{n} elements outside the refinement") });
    }
    Ok(v)
}

fn str_count(lo: Option<i128>, hi: Option<i128>, s: &str) -> D<()> {
    let n = s.chars().count() as i128;
    if lo.map_or(false, |l| n < l) || hi.map_or(false, |h| n > h) {
        return Err(Invalid { code: "refinement.out_of_range", detail: format!("string of {n} scalars outside the refinement") });
    }
    Ok(())
}

fn new_set() -> OrderedMap {
    OrderedMap { index: Default::default(), entries: vec![] }
}

/// Add to a decoded set or map; false for a duplicate key.
fn insert_unique(m: &mut OrderedMap, key: Value, v: Value) -> D<bool> {
    let k = match freeze(&key) {
        Ok(k) => k,
        Err(e) => return bad(e),
    };
    if m.get(&k).is_some() {
        return Ok(false);
    }
    m.insert(k, key, v);
    Ok(true)
}

// ---------------------------------------------------------------------------------------------- text decoding

#[derive(Debug)]
enum J {
    Obj(Vec<(String, J)>),
    Arr(Vec<J>),
    Str(String),
    Bool(bool),
    Null,
    /// an integer literal (None: outside i128)
    Int(Option<i128>, String),
    Float(f64),
}

struct Reader<'s> {
    s: &'s [char],
    i: usize,
}

impl Reader<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], ' ' | '\t' | '\r' | '\n') {
            self.i += 1;
        }
    }
    fn starts(&self, w: &str) -> bool {
        let w: Vec<char> = w.chars().collect();
        self.s.len() >= self.i + w.len() && self.s[self.i..self.i + w.len()] == w[..]
    }
    fn at(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn value(&mut self) -> D<J> {
        self.ws();
        let c = match self.at() {
            Some(c) => c,
            None => return bad("unexpected end of input"),
        };
        match c {
            '{' => self.obj(),
            '[' => self.arr(),
            '"' => Ok(J::Str(self.string()?)),
            _ if self.starts("true") => {
                self.i += 4;
                Ok(J::Bool(true))
            }
            _ if self.starts("false") => {
                self.i += 5;
                Ok(J::Bool(false))
            }
            _ if self.starts("null") => {
                self.i += 4;
                Ok(J::Null)
            }
            _ => self.number(),
        }
    }
    fn obj(&mut self) -> D<J> {
        self.i += 1;
        let mut out = vec![];
        self.ws();
        if self.at() == Some('}') {
            self.i += 1;
            return Ok(J::Obj(out));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            if self.at() != Some(':') {
                return bad(format!("expected ':' at {}", self.i));
            }
            self.i += 1;
            out.push((k, self.value()?));
            self.ws();
            let c = self.at();
            self.i += 1;
            match c {
                Some('}') => return Ok(J::Obj(out)),
                Some(',') => {}
                _ => return bad(format!("expected ',' or '}}' at {}", self.i - 1)),
            }
        }
    }
    fn arr(&mut self) -> D<J> {
        self.i += 1;
        let mut out = vec![];
        self.ws();
        if self.at() == Some(']') {
            self.i += 1;
            return Ok(J::Arr(out));
        }
        loop {
            out.push(self.value()?);
            self.ws();
            let c = self.at();
            self.i += 1;
            match c {
                Some(']') => return Ok(J::Arr(out)),
                Some(',') => {}
                _ => return bad(format!("expected ',' or ']' at {}", self.i - 1)),
            }
        }
    }
    fn hex4(&self, at: usize) -> Option<u32> {
        if at + 4 > self.s.len() {
            return None;
        }
        let h: String = self.s[at..at + 4].iter().collect();
        u32::from_str_radix(&h, 16).ok()
    }
    fn string(&mut self) -> D<String> {
        if self.at() != Some('"') {
            return bad(format!("expected a string at {}", self.i));
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = match self.at() {
                Some(c) => c,
                None => return bad("unterminated string"),
            };
            if c == '"' {
                self.i += 1;
                return Ok(out);
            }
            if c == '\\' {
                let e = self.s.get(self.i + 1).copied();
                let m = match e {
                    Some('"') => Some('"'),
                    Some('\\') => Some('\\'),
                    Some('/') => Some('/'),
                    Some('b') => Some('\u{8}'),
                    Some('f') => Some('\u{c}'),
                    Some('n') => Some('\n'),
                    Some('r') => Some('\r'),
                    Some('t') => Some('\t'),
                    _ => None,
                };
                if let Some(x) = m {
                    out.push(x);
                    self.i += 2;
                } else if e == Some('u') {
                    let mut cp = match self.hex4(self.i + 2) {
                        Some(cp) => cp,
                        None => return bad("bad \\u escape"),
                    };
                    self.i += 6;
                    if (0xD800..0xDC00).contains(&cp) && self.starts("\\u") {
                        if let Some(lo) = self.hex4(self.i + 2) {
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00));
                            self.i += 6;
                        }
                    }
                    match char::from_u32(cp) {
                        Some(ch) => out.push(ch),
                        None => return bad("bad \\u escape"),
                    }
                } else {
                    return bad("bad escape");
                }
            } else if (c as u32) < 0x20 {
                return bad("control character in string");
            } else {
                out.push(c);
                self.i += 1;
            }
        }
    }
    fn number(&mut self) -> D<J> {
        let mut j = self.i;
        while j < self.s.len() && "+-0123456789.eE".contains(self.s[j]) {
            j += 1;
        }
        let t: String = self.s[self.i..j].iter().collect();
        if t.is_empty() {
            return bad(format!("unexpected {:?} at {}", self.s[self.i], self.i));
        }
        self.i = j;
        if t.contains(['.', 'e', 'E']) {
            return match t.parse::<f64>() {
                Ok(x) => Ok(J::Float(x)),
                Err(_) => bad(format!("bad number {t:?}")),
            };
        }
        // an integer of any size: one past i128 still decodes as a number that does not fit its type
        let digits = t.trim_start_matches(['+', '-']);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) || t[..t.len() - digits.len()].len() > 1 {
            return bad(format!("bad number {t:?}"));
        }
        Ok(J::Int(t.parse::<i128>().ok(), t))
    }
}

/// Values the decoder builds need the VM (class defaults are constant expressions).
struct Builder<'v, 'p> {
    vm: &'v mut Vm<'p>,
}

impl Builder<'_, '_> {
    fn default_of(&mut self, e: &Expr) -> D<Value> {
        let mut fr = Frame { locals: vec![], func: 0, consumed: None };
        self.vm.eval(&mut fr, e).map_err(|_| Invalid { code: "codec.invalid", detail: "a field default could not be evaluated".into() })
    }

    fn fields(&mut self, fields: &[(String, Desc, Option<Expr>)], t: &J) -> D<Vec<(String, Value)>> {
        let items = match t {
            J::Obj(items) => items,
            _ => return bad("expected an object"),
        };
        let mut got: Vec<(String, Value)> = vec![];
        for (key, val) in items {
            let fd = match fields.iter().find(|(n, _, _)| n == key) {
                Some(f) => f,
                None => return Err(Invalid { code: "codec.unknown_field", detail: key.clone() }),
            };
            if got.iter().any(|(n, _)| n == key) {
                return bad(format!("duplicate field {key:?}"));
            }
            let v = self.text(&fd.1, val)?;
            got.push((key.clone(), v));
        }
        let mut out = vec![];
        for (name, fd, default) in fields {
            if let Some(i) = got.iter().position(|(n, _)| n == name) {
                out.push(got.remove(i));
            } else if let Some(e) = default {
                let v = self.default_of(e)?;
                out.push((name.clone(), v));
            } else if matches!(fd, Desc::Opt(_)) {
                out.push((name.clone(), Value::Null));
            } else {
                return bad(format!("missing field {name:?}"));
            }
        }
        Ok(out)
    }

    fn text(&mut self, d: &Desc, t: &J) -> D<Value> {
        match d {
            Desc::Sized(inner, lo, hi) => {
                let v = self.text(inner, t)?;
                return sized(*lo, *hi, v);
            }
            Desc::Opt(inner) => {
                return if matches!(t, J::Null) { Ok(Value::Null) } else { self.text(inner, t) };
            }
            _ => {}
        }
        if matches!(t, J::Null) {
            return bad("null where a value is expected");
        }
        Ok(match d {
            Desc::Int(..) => match t {
                J::Int(n, shown) => Value::Int(check_int(d, *n, shown)?),
                _ => return bad("expected an integer"),
            },
            Desc::Bool => match t {
                J::Bool(b) => Value::Bool(*b),
                _ => return bad("expected true or false"),
            },
            Desc::Float | Desc::Float32 => {
                let x = match t {
                    J::Float(x) => *x,
                    J::Int(Some(n), _) => *n as f64,
                    J::Int(None, s) => s.parse::<f64>().unwrap_or(f64::INFINITY),
                    J::Str(s) if s == "NaN" => f64::NAN,
                    J::Str(s) if s == "Infinity" => f64::INFINITY,
                    J::Str(s) if s == "-Infinity" => f64::NEG_INFINITY,
                    _ => return bad("expected a number"),
                };
                if matches!(d, Desc::Float) {
                    Value::Float(fortis_runtime::float::canon(x))
                } else {
                    Value::F32(fortis_runtime::float::canon32(x as f32))
                }
            }
            Desc::Decimal => match t {
                J::Str(s) => match Decimal::parse(&format!("{s}d")) {
                    Ok(dv) if valid_decimal_text(s) => Value::Dec(dv),
                    _ => return bad(format!("bad decimal {s:?}")),
                },
                _ => return bad("expected a decimal string"),
            },
            Desc::Str(lo, hi) => match t {
                J::Str(s) => {
                    str_count(*lo, *hi, s)?;
                    Value::str(s)
                }
                _ => return bad("expected a string"),
            },
            Desc::Scalar => match t {
                J::Str(s) if s.chars().count() == 1 => Value::Scalar(Rc::from(s.as_str())),
                _ => return bad("expected a one-scalar string"),
            },
            Desc::Bytes => match t {
                J::Str(s) => match base64_decode(s) {
                    Some(b) => Value::Bytes(Rc::new(b)),
                    None => return bad("bad base64"),
                },
                _ => return bad("expected base64 text"),
            },
            Desc::Duration => match t {
                J::Int(Some(n), _) => match i64::try_from(*n) {
                    Ok(ns) => Value::Dur(ns),
                    Err(_) => return Err(Invalid { code: "numeric.overflow", detail: "duration out of range".into() }),
                },
                J::Int(None, _) => return Err(Invalid { code: "numeric.overflow", detail: "duration out of range".into() }),
                _ => return bad("expected nanoseconds"),
            },
            Desc::Timestamp => match t {
                J::Str(s) => Value::Time(parse_rfc3339(s)?),
                _ => return bad("expected an RFC 3339 string"),
            },
            Desc::List(inner) | Desc::Array(inner, _) | Desc::Set(inner) => {
                let xs = match t {
                    J::Arr(xs) => xs,
                    _ => return bad("expected an array"),
                };
                let items: Vec<Value> = xs.iter().map(|x| self.text(inner, x)).collect::<D<_>>()?;
                match d {
                    Desc::Array(_, n) => {
                        if items.len() != *n {
                            return bad(format!("expected {n} elements"));
                        }
                        Value::Array(shared(items))
                    }
                    Desc::Set(_) => {
                        let mut s = new_set();
                        for x in items {
                            if !insert_unique(&mut s, x, Value::Void)? {
                                return bad("duplicate set element");
                            }
                        }
                        Value::Set(shared(s))
                    }
                    _ => Value::list(items),
                }
            }
            Desc::Map(kd, vd) => {
                let pairs = match t {
                    J::Arr(xs) => xs,
                    _ => return bad("expected an array of pairs"),
                };
                let mut m = new_set();
                for p in pairs {
                    let (k, v) = match p {
                        J::Arr(kv) if kv.len() == 2 => (&kv[0], &kv[1]),
                        _ => return bad("expected a [key, value] pair"),
                    };
                    let key = self.text(kd, k)?;
                    let val = self.text(vd, v)?;
                    if !insert_unique(&mut m, key, val)? {
                        return bad("duplicate map key");
                    }
                }
                Value::Map(shared(m))
            }
            Desc::Tuple(ds) => match t {
                J::Arr(xs) if xs.len() == ds.len() => Value::Tuple(Rc::new(ds.iter().zip(xs).map(|(d, x)| self.text(d, x)).collect::<D<_>>()?)),
                _ => return bad(format!("expected {} elements", ds.len())),
            },
            Desc::Record(fs) => {
                let fields: Vec<(String, Desc, Option<Expr>)> = fs.iter().map(|(n, d)| (n.clone(), d.clone(), None)).collect();
                let vals = self.fields(&fields, t)?;
                Value::Record(shared(vals.into_iter().map(|(k, v)| (Rc::from(k.as_str()), v)).collect()))
            }
            Desc::Class(name, fs) => {
                let vals = self.fields(fs, t)?;
                Value::Obj(shared(Obj { ty: Rc::from(name.as_str()), fields: vals.into_iter().map(|(k, v)| (Rc::from(k.as_str()), v)).collect() }))
            }
            Desc::Enum(name, variants) => match t {
                J::Str(v) => match variants.iter().find(|(n, _)| n == v) {
                    Some((_, fs)) if fs.is_empty() => Value::Enum(Rc::new(EnumVal { ty: Rc::from(name.as_str()), variant: Rc::from(v.as_str()), fields: vec![] })),
                    _ => return bad(format!("no payload-free variant {v:?}")),
                },
                J::Obj(items) if items.len() == 1 => {
                    let (vname, body) = &items[0];
                    let fs = match variants.iter().find(|(n, _)| n == vname) {
                        Some((_, fs)) => fs,
                        None => return bad(format!("no variant {vname:?}")),
                    };
                    let fields: Vec<(String, Desc, Option<Expr>)> = fs.iter().map(|(n, d)| (n.clone(), d.clone(), None)).collect();
                    let vals = self.fields(&fields, body)?;
                    Value::Enum(Rc::new(EnumVal { ty: Rc::from(name.as_str()), variant: Rc::from(vname.as_str()), fields: vals.into_iter().map(|(k, v)| (Rc::from(k.as_str()), v)).collect() }))
                }
                _ => return bad("expected a variant"),
            },
            Desc::Sized(..) | Desc::Opt(_) => unreachable!(),
        })
    }

    // ---- binary ---------------------------------------------------------------------------------------------

    fn bin(&mut self, d: &Desc, r: &mut Bin) -> D<Value> {
        Ok(match d {
            Desc::Sized(inner, lo, hi) => {
                let v = self.bin(inner, r)?;
                sized(*lo, *hi, v)?
            }
            Desc::Opt(inner) => {
                if r.peek() == Some(T_NULL) {
                    r.i += 1;
                    Value::Null
                } else {
                    self.bin(inner, r)?
                }
            }
            Desc::Int(..) => {
                r.expect(T_INT, "an integer")?;
                let n = r.zigzag()?;
                Value::Int(check_int(d, n, "the integer")?)
            }
            Desc::Bool => match r.byte()? {
                T_FALSE => Value::Bool(false),
                T_TRUE => Value::Bool(true),
                _ => return bad("expected a boolean"),
            },
            Desc::Float => {
                r.expect(T_FLOAT, "a float")?;
                let b = r.take(8)?;
                Value::Float(fortis_runtime::float::canon(f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])))
            }
            Desc::Float32 => {
                r.expect(T_FLOAT32, "a float32")?;
                let b = r.take(4)?;
                Value::F32(fortis_runtime::float::canon32(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            }
            Desc::Decimal => {
                r.expect(T_DECIMAL, "a decimal")?;
                let neg = r.byte()?;
                let scale = r.byte()?;
                let coeff = r.varint()?;
                match (neg <= 1, coeff.and_then(|c| Decimal::new(neg == 1, c, scale as u32).ok())) {
                    (true, Some(dv)) => Value::Dec(dv),
                    _ => return bad("bad decimal"),
                }
            }
            Desc::Str(lo, hi) => {
                r.expect(T_STRING, "a string")?;
                let n = r.len()?;
                let raw = r.take(n)?;
                let s = match std::str::from_utf8(raw) {
                    Ok(s) => s.to_string(),
                    Err(_) => return bad("invalid UTF-8"),
                };
                str_count(*lo, *hi, &s)?;
                Value::str(&s)
            }
            Desc::Scalar => {
                r.expect(T_SCALAR, "a scalar")?;
                match r.varint()?.and_then(|cp| u32::try_from(cp).ok()).and_then(char::from_u32) {
                    Some(c) => Value::Scalar(Rc::from(c.to_string().as_str())),
                    None => return bad("not a Unicode scalar"),
                }
            }
            Desc::Bytes => {
                r.expect(T_BYTES, "bytes")?;
                let n = r.len()?;
                Value::Bytes(Rc::new(r.take(n)?.to_vec()))
            }
            Desc::Duration => {
                r.expect(T_DURATION, "a duration")?;
                match r.zigzag()?.and_then(|n| i64::try_from(n).ok()) {
                    Some(ns) => Value::Dur(ns),
                    None => return Err(Invalid { code: "numeric.overflow", detail: "duration out of range".into() }),
                }
            }
            Desc::Timestamp => {
                r.expect(T_TIMESTAMP, "a timestamp")?;
                match r.zigzag()?.and_then(|n| i64::try_from(n).ok()) {
                    Some(ns) => Value::Time(ns),
                    None => return bad("timestamp out of range"),
                }
            }
            Desc::List(inner) | Desc::Array(inner, _) => {
                r.expect(T_LIST, "a list")?;
                let n = r.len()?;
                let mut items = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    items.push(self.bin(inner, r)?);
                }
                match d {
                    Desc::Array(_, k) => {
                        if items.len() != *k {
                            return bad(format!("expected {k} elements"));
                        }
                        Value::Array(shared(items))
                    }
                    _ => Value::list(items),
                }
            }
            Desc::Set(inner) => {
                r.expect(T_SET, "a set")?;
                let n = r.len()?;
                let mut s = new_set();
                for _ in 0..n {
                    let x = self.bin(inner, r)?;
                    if !insert_unique(&mut s, x, Value::Void)? {
                        return bad("duplicate set element");
                    }
                }
                Value::Set(shared(s))
            }
            Desc::Map(kd, vd) => {
                r.expect(T_MAP, "a map")?;
                let n = r.len()?;
                let mut m = new_set();
                for _ in 0..n {
                    let key = self.bin(kd, r)?;
                    let val = self.bin(vd, r)?;
                    if !insert_unique(&mut m, key, val)? {
                        return bad("duplicate map key");
                    }
                }
                Value::Map(shared(m))
            }
            Desc::Tuple(ds) => {
                r.expect(T_TUPLE, "a tuple")?;
                if r.len()? != ds.len() {
                    return bad("wrong tuple arity");
                }
                Value::Tuple(Rc::new(ds.iter().map(|d| self.bin(d, r)).collect::<D<_>>()?))
            }
            Desc::Record(fs) => {
                r.expect(T_RECORD, "a record")?;
                if r.len()? != fs.len() {
                    return bad("wrong field count");
                }
                let vals: Vec<(Rc<str>, Value)> = fs.iter().map(|(n, d)| Ok((Rc::from(n.as_str()), self.bin(d, r)?))).collect::<D<_>>()?;
                Value::Record(shared(vals))
            }
            Desc::Class(name, fs) => {
                r.expect(T_RECORD, "a record")?;
                if r.len()? != fs.len() {
                    return bad("wrong field count");
                }
                let vals: Vec<(Rc<str>, Value)> = fs.iter().map(|(n, d, _)| Ok((Rc::from(n.as_str()), self.bin(d, r)?))).collect::<D<_>>()?;
                Value::Obj(shared(Obj { ty: Rc::from(name.as_str()), fields: vals }))
            }
            Desc::Enum(name, variants) => {
                r.expect(T_VARIANT, "a variant")?;
                let idx = r.len()?;
                let (vname, fs) = match variants.get(idx) {
                    Some(v) => v,
                    None => return bad("variant index out of range"),
                };
                if r.len()? != fs.len() {
                    return bad("wrong payload count");
                }
                let vals: Vec<(Rc<str>, Value)> = fs.iter().map(|(n, d)| Ok((Rc::from(n.as_str()), self.bin(d, r)?))).collect::<D<_>>()?;
                Value::Enum(Rc::new(EnumVal { ty: Rc::from(name.as_str()), variant: Rc::from(vname.as_str()), fields: vals }))
            }
        })
    }
}

/// `numeric.Dec.parse(t + "d")` accepts an optional sign, digits and one decimal point.
fn valid_decimal_text(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    let (a, b) = t.split_once('.').unwrap_or((t, ""));
    !t.is_empty() && !a.is_empty() && a.chars().all(|c| c.is_ascii_digit() || c == '_') && b.chars().all(|c| c.is_ascii_digit() || c == '_') && !(t.contains('.') && b.is_empty())
}

fn parse_rfc3339(t: &str) -> D<i64> {
    let b = t.as_bytes();
    let num = |a: usize, n: usize| -> Option<i64> {
        let s = t.get(a..a + n)?;
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    let shape = b.len() >= 20 && b[4] == b'-' && b[7] == b'-' && b[10] == b'T' && b[13] == b':' && b[16] == b':' && b[b.len() - 1] == b'Z';
    let parts = (num(0, 4), num(5, 2), num(8, 2), num(11, 2), num(14, 2), num(17, 2));
    let (y, mo, da, h, mi, s) = match (shape, parts) {
        (true, (Some(y), Some(mo), Some(da), Some(h), Some(mi), Some(s))) => (y, mo, da, h, mi, s),
        _ => return bad(format!("not an RFC 3339 UTC timestamp: {t:?}")),
    };
    let frac = &t[19..t.len() - 1];
    let frac_ns = if frac.is_empty() {
        0
    } else if frac.len() >= 2 && frac.len() <= 10 && frac.starts_with('.') && frac[1..].bytes().all(|c| c.is_ascii_digit()) {
        format!("{:0<9}", &frac[1..]).parse::<i64>().unwrap_or(0)
    } else {
        return bad(format!("not an RFC 3339 UTC timestamp: {t:?}"));
    };
    let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = [31, if leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=9999).contains(&y) || !(1..=12).contains(&mo) || da < 1 || da > dim[(mo - 1) as usize] || h > 23 || mi > 59 || s > 59 {
        return bad(format!("bad date {t:?}"));
    }
    // days from the civil date (Howard Hinnant's algorithm)
    let (yy, m) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * m + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + s;
    Ok(secs * 1_000_000_000 + frac_ns)
}

struct Bin<'b> {
    d: &'b [u8],
    i: usize,
}

impl Bin<'_> {
    fn peek(&self) -> Option<u8> {
        self.d.get(self.i).copied()
    }
    fn byte(&mut self) -> D<u8> {
        match self.d.get(self.i) {
            Some(b) => {
                self.i += 1;
                Ok(*b)
            }
            None => bad("truncated input"),
        }
    }
    fn take(&mut self, n: usize) -> D<&[u8]> {
        if self.i + n > self.d.len() {
            return bad("truncated input");
        }
        let out = &self.d[self.i..self.i + n];
        self.i += n;
        Ok(out)
    }
    /// A varint; None when it exceeds the integers the codec represents.
    fn varint(&mut self) -> D<Option<u128>> {
        let (mut n, mut shift, mut over) = (0u128, 0u32, false);
        loop {
            let b = self.byte()?;
            if shift < 128 {
                let part = ((b & 0x7F) as u128).checked_shl(shift).unwrap_or(0);
                if shift > 121 && (b & 0x7F) as u128 >> (128 - shift) != 0 {
                    over = true;
                }
                n |= part;
            } else if b & 0x7F != 0 {
                over = true;
            }
            if b & 0x80 == 0 {
                return Ok(if over { None } else { Some(n) });
            }
            shift += 7;
            if shift > 70 {
                return bad("varint too long");
            }
        }
    }
    fn len(&mut self) -> D<usize> {
        match self.varint()?.and_then(|n| usize::try_from(n).ok()) {
            Some(n) => Ok(n),
            None => bad("length out of range"),
        }
    }
    fn zigzag(&mut self) -> D<Option<i128>> {
        Ok(self.varint()?.map(|n| if n & 1 == 0 { (n >> 1) as i128 } else { -(((n + 1) >> 1) as i128) }))
    }
    fn expect(&mut self, tag: u8, what: &str) -> D<()> {
        let b = self.byte()?;
        if b != tag {
            return bad(format!("expected {what} (tag {tag:#04x}), found tag {b:#04x}"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------- the entry

impl<'p> Vm<'p> {
    /// `T.fromText(s)` / `T.fromBytes(b)`: the decoded value, or a codec failure value (6.10).
    pub fn decode(&mut self, text: bool, target: &Type, data: &Value) -> R<Value> {
        let d = match desc(self.prog, target, &[], 0) {
            Ok(d) => d,
            Err(m) => return dynamic(m),
        };
        let r = if text {
            let s = match data.deref() {
                Value::Str(s) => s.to_string(),
                other => return dynamic(format!("fromText of {}", display(&other))),
            };
            let chars: Vec<char> = s.chars().collect();
            let mut rd = Reader { s: &chars, i: 0 };
            match rd.value() {
                Ok(tree) => {
                    rd.ws();
                    if rd.i != chars.len() {
                        bad("trailing characters after the value")
                    } else {
                        Builder { vm: self }.text(&d, &tree)
                    }
                }
                Err(e) => Err(e),
            }
        } else {
            let bytes = match data.deref() {
                Value::Bytes(b) => b.to_vec(),
                other => return dynamic(format!("fromBytes of {}", display(&other))),
            };
            if bytes.len() < 4 || &bytes[..4] != MAGIC {
                bad("missing canonical binary header FTB\\x01")
            } else {
                let mut rd = Bin { d: &bytes, i: 4 };
                match (Builder { vm: self }).bin(&d, &mut rd) {
                    Ok(v) if rd.i != bytes.len() => {
                        drop(v);
                        bad("trailing bytes after the value")
                    }
                    other => other,
                }
            }
        };
        Ok(match r {
            Ok(v) => v,
            Err(inv) => {
                let origin = self.stack.last().cloned().unwrap_or_else(|| "main".into());
                Value::Failure(shared(FailureVal {
                    code: Rc::from(inv.code),
                    payload: vec![(Rc::from("detail"), Value::str(&inv.detail))],
                    origin: Rc::from(origin.as_str()),
                    message: Rc::from(""),
                    cause: None,
                    annotations: vec![],
                }))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_and_rejects_bad_padding() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0, 255, 128, 7]] {
            assert_eq!(base64_decode(&base64_encode(data)).as_deref(), Some(data));
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert!(base64_decode("Zm9").is_none());
        assert!(base64_decode("Z=9v").is_none());
        assert!(base64_decode("Zm9v!A==").is_none());
    }

    #[test]
    fn zigzag_and_varint_match_the_format() {
        let mut out = vec![];
        varint(&mut out, zigzag(-1));
        varint(&mut out, zigzag(1));
        varint(&mut out, zigzag(300));
        assert_eq!(out, vec![1, 2, 0xD8, 0x04]);
        let mut r = Bin { d: &out, i: 0 };
        assert_eq!(r.zigzag().ok().flatten(), Some(-1));
        assert_eq!(r.zigzag().ok().flatten(), Some(1));
        assert_eq!(r.zigzag().ok().flatten(), Some(300));
    }

    #[test]
    fn rfc3339_epoch_and_fractions() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z").ok(), Some(0));
        assert_eq!(parse_rfc3339("2000-02-29T12:00:00.5Z").ok(), Some(951_825_600_500_000_000));
        assert!(parse_rfc3339("2001-02-29T00:00:00Z").is_err());
        assert!(parse_rfc3339("1970-01-01 00:00:00Z").is_err());
    }

    #[test]
    fn json_strings_escape_controls() {
        assert_eq!(json_string("a\"b\\\n\u{1}"), "\"a\\\"b\\\\\\n\\u0001\"");
    }
}
