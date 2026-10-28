//! DEL-03 diagnostic records (Delivery Programme 7.1, 7.3, 7.4): a port of `conformance/tools/diagnostics.py`. A record
//! carries the common fields and its class's mandatory `fields`; JSON lines match the reference's
//! `json.dumps(record, sort_keys=True, ensure_ascii=False)`.

use std::collections::BTreeMap;

/// A JSON value; objects keep sorted keys, as the reference writes them.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }
    pub fn strs<I: IntoIterator<Item = S>, S: Into<String>>(items: I) -> Json {
        Json::Arr(items.into_iter().map(|s| Json::Str(s.into())).collect())
    }
    pub fn obj<I: IntoIterator<Item = (K, Json)>, K: Into<String>>(items: I) -> Json {
        Json::Obj(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(n) => out.push_str(&n.to_string()),
            Json::Str(s) => write_str(s, out),
            Json::Arr(v) => {
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Json::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_str(k, out);
                    out.push_str(": ");
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_line(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    /// Parse JSON text (RFC 8259; numbers must be integers within i64, as every writer here emits).
    /// Parse JSON within the quotas every compiler input parser has (PROG-SEC-01): at most `MAX_BYTES` bytes and
    /// `MAX_DEPTH` levels of nesting; past them the error starts with `E-LIM-005`.
    pub fn parse(text: &str) -> Result<Json, String> {
        if text.len() > MAX_BYTES {
            return Err(format!("E-LIM-005: {} bytes of JSON exceed the budget of {MAX_BYTES} bytes", text.len()));
        }
        let mut p = Parser { s: text.as_bytes(), i: 0, depth: 0 };
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("trailing characters at byte {}", p.i));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }
}

/// The JSON quotas (PROG-SEC-01).
pub const MAX_BYTES: usize = 256 << 20;
pub const MAX_DEPTH: usize = 128;

struct Parser<'s> {
    s: &'s [u8],
    i: usize,
    depth: usize,
}

impl<'s> Parser<'s> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn eat(&mut self, lit: &str) -> Result<(), String> {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(format!("expected `{lit}` at byte {}", self.i))
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        if matches!(self.s.get(self.i), Some(b'[') | Some(b'{')) {
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err(format!("E-LIM-005: JSON nested deeper than the budget of {MAX_DEPTH} levels"));
            }
            let v = self.compound();
            self.depth -= 1;
            return v;
        }
        self.scalar_or_compound()
    }

    fn compound(&mut self) -> Result<Json, String> {
        self.scalar_or_compound()
    }

    fn scalar_or_compound(&mut self) -> Result<Json, String> {
        match self.s.get(self.i) {
            None => Err("unexpected end of JSON".into()),
            Some(b'n') => self.eat("null").map(|_| Json::Null),
            Some(b't') => self.eat("true").map(|_| Json::Bool(true)),
            Some(b'f') => self.eat("false").map(|_| Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(v));
                }
                loop {
                    v.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(v));
                        }
                        _ => return Err(format!("expected `,` or `]` at byte {}", self.i)),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.eat(":")?;
                    let v = self.value()?;
                    m.insert(k, v);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(m));
                        }
                        _ => return Err(format!("expected `,` or `}}` at byte {}", self.i)),
                    }
                }
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).unwrap();
                t.parse::<i64>().map(Json::Int).map_err(|e| format!("number `{t}`: {e}"))
            }
            Some(c) => Err(format!("unexpected `{}` at byte {}", *c as char, self.i)),
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.eat("\"")?;
        let mut out = String::new();
        loop {
            let c = *self.s.get(self.i).ok_or("unterminated string")?;
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("unterminated escape")?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code) {
                                self.eat("\\u")?;
                                let lo = self.hex4()?;
                                code = 0x10000 + ((code - 0xD800) << 10) + (lo - 0xDC00);
                            }
                            out.push(char::from_u32(code).ok_or("invalid code point")?);
                        }
                        _ => return Err(format!("bad escape at byte {}", self.i)),
                    }
                }
                _ => {
                    // copy one UTF-8 sequence
                    let len = if c < 0x80 { 1 } else if c >= 0xF0 { 4 } else if c >= 0xE0 { 3 } else { 2 };
                    let start = self.i - 1;
                    self.i = start + len;
                    out.push_str(std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?);
                }
            }
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let t = std::str::from_utf8(self.s.get(self.i..self.i + 4).ok_or("short \\u escape")?).map_err(|e| e.to_string())?;
        self.i += 4;
        u32::from_str_radix(t, 16).map_err(|e| e.to_string())
    }
}

fn write_str(s: &str, out: &mut String) {
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
}

pub struct Class {
    pub name: &'static str,
    pub prefixes: &'static [&'static str],
    pub fields: &'static [&'static str],
    pub correction: bool,
}

/// Programme 7.1, by identity.
pub static CLASSES: &[Class] = &[
    Class { name: "Type", prefixes: &["E-TYP", "E-MIX-001"], fields: &["expected_type", "actual_type", "primary_span"], correction: true },
    Class { name: "Ownership", prefixes: &["E-OWN"], fields: &["place", "conflict", "spans", "loan_region"], correction: true },
    Class { name: "Obligation", prefixes: &["E-OBL", "E-ERR-001"], fields: &["obligation_origin", "unconsumed_path"], correction: true },
    Class { name: "Effect", prefixes: &["E-EFF"], fields: &["required_contract", "violating_effect", "witness_path"], correction: true },
    Class { name: "Refinement unproved", prefixes: &["E-REF"], fields: &["required_range", "proved_interval", "widening_point"], correction: true },
    Class { name: "Unsupported shape", prefixes: &["E-OWN-090"], fields: &["lowering_row"], correction: false },
    Class { name: "Resource limit", prefixes: &["E-LIM"], fields: &["budget", "operation"], correction: false },
    Class { name: "Internal defect", prefixes: &["E-INT"], fields: &["reproduction"], correction: false },
    Class { name: "Syntax", prefixes: &["E-SYN"], fields: &["expected_tokens", "found_token"], correction: true },
    Class { name: "Names and modules", prefixes: &["E-NAM"], fields: &["name", "candidates"], correction: true },
    Class { name: "Patterns", prefixes: &["E-MAT"], fields: &["missing_variants", "arm_set"], correction: true },
    Class { name: "Closures", prefixes: &["E-FUN"], fields: &["violation", "capture_span", "use_span"], correction: true },
    Class { name: "Defer and async", prefixes: &["E-DEF", "E-ASY"], fields: &["offending_statement", "rule_reference"], correction: true },
    Class { name: "Observation", prefixes: &["E-OBS"], fields: &["frame_field", "exit_path"], correction: true },
    Class { name: "Dependency injection", prefixes: &["E-DI"], fields: &["consumer", "slot", "candidates"], correction: true },
    Class { name: "Telemetry", prefixes: &["E-TEL"], fields: &["plan_line", "projection", "field"], correction: true },
];

pub fn class_of(code: &str) -> Option<&'static Class> {
    if let Some(c) = CLASSES.iter().find(|c| c.prefixes.contains(&code)) {
        return Some(c);
    }
    CLASSES.iter().find(|c| c.prefixes.iter().any(|p| p.matches('-').count() == 1 && code.starts_with(&format!("{p}-"))))
}

#[derive(Clone, Debug)]
pub struct Record {
    pub code: String,
    pub message: String,
    pub rule: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub severity: &'static str,
    pub status: &'static str,
    pub fields: BTreeMap<String, Json>,
    pub correction: Option<Json>,
}

impl Record {
    pub fn new(code: &str, message: impl Into<String>, rule: &str, file: &str, line: usize, column: usize, fields: Json) -> Record {
        let fields = match fields {
            Json::Obj(m) => m,
            _ => BTreeMap::new(),
        };
        Record { code: code.into(), message: message.into(), rule: rule.into(), file: file.into(), line, column, severity: "error", status: "violated", fields, correction: None }
    }

    pub fn class(&self) -> Option<&'static str> {
        class_of(&self.code).map(|c| c.name)
    }

    pub fn to_json(&self) -> Json {
        let mut m = BTreeMap::new();
        m.insert("class".into(), self.class().map_or(Json::Null, Json::str));
        m.insert("code".into(), Json::str(&self.code));
        m.insert("message".into(), Json::str(&self.message));
        m.insert("rule".into(), Json::str(&self.rule));
        m.insert("file".into(), Json::str(&self.file));
        m.insert("line".into(), Json::Int(self.line as i64));
        m.insert("column".into(), Json::Int(self.column as i64));
        m.insert("severity".into(), Json::str(self.severity));
        m.insert("status".into(), Json::str(self.status));
        m.insert("fields".into(), Json::Obj(self.fields.clone()));
        if let Some(c) = &self.correction {
            m.insert("correction".into(), c.clone());
        }
        Json::Obj(m)
    }

    pub fn to_json_line(&self) -> String {
        self.to_json().to_line()
    }

    /// A record read back from its JSON form (`to_json`), or None when it is not one.
    pub fn from_json(j: &Json) -> Option<Record> {
        let s = |k: &str| match j.get(k) {
            Some(Json::Str(v)) => Some(v.clone()),
            _ => None,
        };
        let n = |k: &str| match j.get(k) {
            Some(Json::Int(v)) if *v >= 0 => Some(*v as usize),
            _ => None,
        };
        let severity = match s("severity")?.as_str() {
            "error" => "error",
            "warning" => "warning",
            "info" => "info",
            _ => return None,
        };
        let status = match s("status")?.as_str() {
            "violated" => "violated",
            "unproved" => "unproved",
            "unknown" => "unknown",
            _ => return None,
        };
        let fields = match j.get("fields")? {
            Json::Obj(m) => m.clone(),
            _ => return None,
        };
        Some(Record { code: s("code")?, message: s("message")?, rule: s("rule")?, file: s("file")?, line: n("line")?, column: n("column")?, severity, status, fields, correction: j.get("correction").cloned() })
    }

    /// Errors against reference/schemas/diagnostic.schema.json (the checks of `diagnostics.validate`).
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let cls = match class_of(&self.code) {
            Some(c) => c,
            None => return vec![format!("unknown class for {}", self.code)],
        };
        for f in cls.fields {
            if !self.fields.contains_key(*f) {
                errs.push(format!("class {} requires field {}", cls.name, f));
            }
        }
        if self.line < 1 || self.column < 1 {
            errs.push("line and column are 1-based integers".into());
        }
        if self.correction.is_some() && !cls.correction {
            errs.push(format!("class {} defines no correction", cls.name));
        }
        errs
    }

    pub fn render_text(&self) -> String {
        let mut s = format!(
            "{}:{}:{}: {} {} [{}] ({}, {}): {}",
            self.file,
            self.line,
            self.column,
            self.severity,
            self.code,
            self.rule,
            self.class().unwrap_or("?"),
            self.status,
            self.message
        );
        for (k, v) in &self.fields {
            s.push_str(&format!("\n    {}: {}", k, v.to_line()));
        }
        if let Some(c) = &self.correction {
            let text = |j: Option<&Json>| match j {
                Some(Json::Str(x)) => x.clone(),
                Some(other) => other.to_line(),
                None => String::new(),
            };
            s.push_str(format!("\n    correction: {}", text(c.get("kind"))).trim_end());
            if let Some(Json::Arr(edits)) = c.get("edits") {
                for e in edits {
                    s.push_str(&format!(
                        "\n      {}:{}:{}-{}:{} -> {}",
                        text(e.get("file")),
                        text(e.get("line")),
                        text(e.get("column")),
                        text(e.get("end_line")),
                        text(e.get("end_column")),
                        e.get("replacement").map_or("\"\"".to_string(), |r| r.to_line())
                    ));
                }
            }
        }
        s
    }
}

#[cfg(test)]
mod json_tests {
    use super::Json;

    #[test]
    fn round_trips_every_value_kind() {
        let v = Json::obj([
            ("a", Json::Arr(vec![Json::Null, Json::Bool(true), Json::Int(-42), Json::str("x\"y\\z\n\u{1F600}")])),
            ("b", Json::obj([("c", Json::Int(i64::MAX))])),
        ]);
        assert_eq!(Json::parse(&v.to_line()).unwrap(), v);
    }

    #[test]
    fn rejects_malformed_text() {
        for bad in ["", "{", "[1,]", "{\"a\" 1}", "tru", "\"abc", "1 2", "1.5"] {
            assert!(Json::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn decodes_surrogate_pairs() {
        assert_eq!(Json::parse("\"\\ud83d\\ude00\"").unwrap(), Json::str("\u{1F600}"));
    }
}
