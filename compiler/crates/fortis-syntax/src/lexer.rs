//! The Fortis lexer: a port of `conformance/tools/lexer.py` (Language Reference 3.1-3.7 and the lexer notes of
//! fortis.ebnf). Positions count Unicode scalar values, as the reference does.

use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Newline,
    Nl,
    Lower,
    Upper,
    Kw,
    Int,
    Float,
    Dec,
    Dur,
    Str,
    Raw,
    Bytes,
    Template,
    Punct,
    RefOpen,
    RefClose,
    Eof,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Newline => "NEWLINE",
            Kind::Nl => "NL",
            Kind::Lower => "LOWER",
            Kind::Upper => "UPPER",
            Kind::Kw => "KW",
            Kind::Int => "INT",
            Kind::Float => "FLOAT",
            Kind::Dec => "DEC",
            Kind::Dur => "DUR",
            Kind::Str => "STR",
            Kind::Raw => "RAW",
            Kind::Bytes => "BYTES",
            Kind::Template => "TEMPLATE",
            Kind::Punct => "PUNCT",
            Kind::RefOpen => "REF_OPEN",
            Kind::RefClose => "REF_CLOSE",
            Kind::Eof => "EOF",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: Kind,
    pub text: String,
    pub line: usize,
    pub col: usize,
    /// STR: each `${...}` source with its line and column
    pub interps: Vec<(String, usize, usize)>,
    pub closes_type_args: bool,
    /// STR: each hole's parse, made on first use (fortis-types `hole_tree_at`) and owned by the token, so it lives
    /// exactly as long as the module that holds the token
    pub holes: Holes,
}

/// One slot per `${...}` hole of a string token. A clone starts with empty slots: a hole's parse is positioned at its
/// token, and a cloned token is re-addressed (fortis-di `shift`).
#[derive(Default)]
pub struct Holes(pub Vec<std::sync::OnceLock<Option<Box<crate::Parsed>>>>);

impl Holes {
    pub fn sized(n: usize) -> Holes {
        Holes((0..n).map(|_| std::sync::OnceLock::new()).collect())
    }
}

impl Clone for Holes {
    fn clone(&self) -> Holes {
        Holes::sized(self.0.len())
    }
}

impl std::fmt::Debug for Holes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Holes({})", self.0.len())
    }
}

impl Token {
    pub fn new(kind: Kind, text: &str, line: usize, col: usize) -> Token {
        Token { kind, text: text.to_string(), line, col, interps: Vec::new(), closes_type_args: false, holes: Holes::default() }
    }
    pub fn is(&self, kind: Kind, text: &str) -> bool {
        self.kind == kind && self.text == text
    }
}

#[derive(Clone, Debug)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub col: usize,
    pub code: &'static str,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} at {}:{}", self.code, self.message, self.line, self.col)
    }
}

fn err(message: &str, line: usize, col: usize) -> LexError {
    LexError { message: message.to_string(), line, col, code: "E-SYN-000" }
}

pub const KEYWORD_LIST: &[&str] = &[
    "func", "fn", "let", "mut", "const", "type", "alias", "class", "trivial", "enum", "error", "interface", "mixin", "implement",
    "implements", "with", "derive", "requires", "forbids", "excludes", "contract", "aspect", "needs", "before", "after", "on",
    "metadata", "lifetime", "from", "where", "export", "import", "as", "pure", "noalloc", "async", "try", "recovers", "editable",
    "owned", "dynamic", "linear", "trusted", "once", "copy", "move", "if", "else", "while", "for", "in", "loop", "break",
    "continue", "return", "fail", "give", "recover", "match", "when", "otherwise", "is", "or", "catch", "discard", "defer",
    "await", "scope", "true", "false", "null", "present", "success", "failure", "self", "Self", "capture", "record", "report",
    "observation", "required", "version", "encoding", "test", "cases", "property", "forall", "fuzz", "bench", "expect",
    "application", "context", "model", "component", "view", "explore", "server", "client", "hybrid", "inject", "provide",
    "state", "action", "resource",
];

pub fn keywords() -> &'static HashSet<&'static str> {
    static K: OnceLock<HashSet<&'static str>> = OnceLock::new();
    K.get_or_init(|| KEYWORD_LIST.iter().copied().collect())
}

const TRAILING_CONTINUATION: &[&str] = &[
    ",", "(", "[", "=", "=>", "+", "-", "*", "/", "%", "<<", ">>", "&", "|", "^", "&&", "||", "??", "==", "!=", "<", "<=", ">",
    ">=", "..", "..<", ".",
];
const LEADING_CONTINUATION: &[&str] = &[".", "or", "catch", "??", "else", ")", "]", "with", "where", "implements", "derive", "from"];
const EXPRESSION_BRACE_AFTER: &[&str] = &["=", "(", ",", ":", "[", "??", "return", "give", "fail", "recover", "+=", "-=", "*=", "/=", "%=", "==", "!="];
const PUNCT: &[&str] = &[
    "...", "..<", "..", "?.", "??", "=>", "==", "!=", "<=", ">=", "<<", ">>", "&&", "||", "+=", "-=", "*=", "/=", "%=", "(", ")", "[",
    "]", "{", "}", ",", ".", ":", "=", "+", "-", "*", "/", "%", "<", ">", "!", "&", "|", "^", "?", "@",
];

fn is_punct_or_kw_in(t: &Token, set: &[&str]) -> bool {
    matches!(t.kind, Kind::Punct | Kind::Kw) && set.contains(&t.text.as_str())
}

fn starts_with(src: &[char], i: usize, s: &str) -> bool {
    let mut k = i;
    for c in s.chars() {
        if k >= src.len() || src[k] != c {
            return false;
        }
        k += 1;
    }
    true
}

fn find(src: &[char], from: usize, s: &str) -> Option<usize> {
    let mut i = from;
    while i < src.len() {
        if starts_with(src, i, s) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn text_of(src: &[char], a: usize, b: usize) -> String {
    src[a..b].iter().collect()
}

fn py_isalnum(c: char) -> bool {
    c.is_alphanumeric()
}

thread_local! {
    /// how deeply strings nest inside interpolations at this point of the scan (PROG-SEC-01)
    static STRING_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Index just past a double-quoted string starting at src[i] == '"' (escapes and `${ }` interpolation); strings
/// nested inside interpolations deeper than the nesting budget are refused (`E-LIM-002`).
fn scan_string(src: &[char], i: usize, line: usize, col: usize, interps: Option<&mut Vec<(String, usize, usize)>>) -> Result<usize, LexError> {
    let depth = STRING_DEPTH.with(|d| {
        d.set(d.get() + 1);
        d.get()
    });
    let r = if depth > crate::limits::MAX_NESTING {
        Err(LexError { message: format!("string interpolation nested {depth} deep exceeds the budget of nesting depth {}", crate::limits::MAX_NESTING), line, col, code: "E-LIM-002" })
    } else {
        scan_string_at(src, i, line, col, interps)
    };
    STRING_DEPTH.with(|d| d.set(d.get() - 1));
    r
}

fn scan_string_at(src: &[char], i: usize, line: usize, col: usize, mut interps: Option<&mut Vec<(String, usize, usize)>>) -> Result<usize, LexError> {
    let mut j = i + 1;
    while j < src.len() {
        let c = src[j];
        if c == '\\' {
            if j + 1 < src.len() && src[j + 1] == '\n' {
                return Err(err("newline inside string literal", line, col));
            }
            j += 2;
            continue;
        }
        if c == '"' {
            return Ok(j + 1);
        }
        if c == '\n' {
            return Err(err("newline inside string literal", line, col));
        }
        if c == '$' && starts_with(src, j, "${") && interps.is_some() {
            let mut depth = 1;
            let start = j + 2;
            j += 2;
            while j < src.len() && depth > 0 {
                let c = src[j];
                if c == '{' {
                    depth += 1;
                    j += 1;
                } else if c == '}' {
                    depth -= 1;
                    j += 1;
                } else if c == 'b' && starts_with(src, j, "b\"") && !(j > 0 && (py_isalnum(src[j - 1]) || src[j - 1] == '_')) {
                    j = scan_string(src, j + 1, line, col, None)?;
                } else if starts_with(src, j, "\"\"\"") {
                    let end = find(src, j + 3, "\"\"\"");
                    match end {
                        Some(e) if !src[j..e].contains(&'\n') => j = e + 3,
                        _ => return Err(err("unterminated raw string inside interpolation", line, col)),
                    }
                } else if c == '"' {
                    let mut scratch = Vec::new();
                    j = scan_string(src, j, line, col, Some(&mut scratch))?;
                } else if starts_with(src, j, "//") {
                    return Err(err("comment inside an interpolation", line, col));
                } else if c == '\n' {
                    return Err(err("newline inside interpolation", line, col));
                } else {
                    j += 1;
                }
            }
            if depth > 0 {
                return Err(err("unterminated interpolation", line, col));
            }
            if let Some(v) = interps.as_deref_mut() {
                v.push((text_of(src, start, j - 1), line, col + (start - i)));
            }
            continue;
        }
        j += 1;
    }
    Err(err("unterminated string literal", line, col))
}

fn is_member_of_type(sig: &[Token]) -> bool {
    let n = sig.len();
    if n >= 3 && sig[n - 2].kind == Kind::Punct && sig[n - 2].text == "." {
        let before = &sig[n - 3];
        return matches!(before.kind, Kind::Upper | Kind::RefClose) || (before.kind == Kind::Punct && before.text == "]" && before.closes_type_args);
    }
    false
}

fn opens_template(out: &[Token], stack: &[&'static str]) -> bool {
    let mut k: isize = out.len() as isize - 1;
    let mut had_params = false;
    let at = |k: isize| -> &Token { &out[k as usize] };
    if k >= 0 && at(k).kind == Kind::Punct && at(k).text == ")" {
        let mut depth = 0;
        let mut closed = false;
        while k >= 0 {
            let t = at(k);
            if t.kind == Kind::Punct && t.text == ")" {
                depth += 1;
            } else if t.kind == Kind::Punct && t.text == "(" {
                depth -= 1;
                if depth == 0 {
                    k -= 1;
                    closed = true;
                    break;
                }
            }
            k -= 1;
        }
        if !closed {
            return false;
        }
        had_params = true;
    }
    if k >= 0 && matches!(at(k).kind, Kind::Upper | Kind::Lower) {
        k -= 1;
    } else if had_params {
        return false;
    }
    if !(k >= 0 && at(k).kind == Kind::Kw && at(k).text == "view") {
        return false;
    }
    k -= 1;
    if k >= 0 && at(k).kind == Kind::Kw && ["server", "client", "hybrid"].contains(&at(k).text.as_str()) {
        k -= 1;
    }
    if k >= 0 && at(k).kind == Kind::Kw && at(k).text == "export" {
        k -= 1;
    }
    if k < 0 {
        return true;
    }
    let before = at(k);
    if before.kind == Kind::Punct && before.text == "{" {
        return stack.last() == Some(&"body{");
    }
    if before.kind != Kind::Newline {
        return false;
    }
    if let Some(top) = stack.last() {
        if *top != "body{" {
            return false;
        }
    }
    let mut j = k - 1;
    while j >= 0 && at(j).kind == Kind::Newline {
        j -= 1;
    }
    j < 0 || !is_punct_or_kw_in(at(j), TRAILING_CONTINUATION)
}

fn ident_end(src: &[char], i: usize) -> Option<usize> {
    let c = src[i];
    if !(c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    let mut j = i + 1;
    while j < src.len() && (src[j].is_ascii_alphanumeric() || src[j] == '_') {
        j += 1;
    }
    Some(j)
}

/// `[0-9](?:_?[0-9])*` from i: the end, if it matches.
fn digits(src: &[char], i: usize, radix: u32) -> Option<usize> {
    let ok = |c: char| c.is_digit(radix);
    if i >= src.len() || !ok(src[i]) {
        return None;
    }
    let mut j = i + 1;
    loop {
        if j < src.len() && ok(src[j]) {
            j += 1;
        } else if j + 1 < src.len() && src[j] == '_' && ok(src[j + 1]) {
            j += 2;
        } else {
            return Some(j);
        }
    }
}

fn lit_suffix(src: &[char], i: usize, s: &str) -> Option<usize> {
    if starts_with(src, i, s) {
        Some(i + s.chars().count())
    } else {
        None
    }
}

/// The numeric literal patterns of lexer.py, tried in order: (kind, end).
fn number(src: &[char], i: usize) -> Vec<(Kind, usize)> {
    let mut out = Vec::new();
    // INT: hex, binary, octal
    if src[i] == '0' && i + 1 < src.len() {
        let radix = match src[i + 1] {
            'x' | 'X' => 16,
            'b' | 'B' => 2,
            'o' | 'O' => 8,
            _ => 0,
        };
        if radix > 0 {
            if let Some(e) = digits(src, i + 2, radix) {
                out.push((Kind::Int, e));
            }
        }
    }
    let d = digits(src, i, 10);
    // DUR
    if let Some(e) = d {
        for suf in ["ns", "us", "ms", "s", "min", "h"] {
            if let Some(e2) = lit_suffix(src, e, suf) {
                out.push((Kind::Dur, e2));
                break;
            }
        }
    }
    // DEC: digits (\.digits)? d
    if let Some(e) = d {
        let mut e1 = e;
        if e1 < src.len() && src[e1] == '.' {
            if let Some(f) = digits(src, e1 + 1, 10) {
                e1 = f;
            }
        }
        if e1 < src.len() && src[e1] == 'd' {
            out.push((Kind::Dec, e1 + 1));
        }
    }
    // FLOAT: digits (\.digits)? [eE][+-]? digits (f32)? | digits \. digits (f32)?
    if let Some(e) = d {
        let mut found = None;
        let mut e1 = e;
        if e1 < src.len() && src[e1] == '.' {
            if let Some(f) = digits(src, e1 + 1, 10) {
                e1 = f;
            }
        }
        if e1 < src.len() && (src[e1] == 'e' || src[e1] == 'E') {
            let mut e2 = e1 + 1;
            if e2 < src.len() && (src[e2] == '+' || src[e2] == '-') {
                e2 += 1;
            }
            if let Some(e3) = digits(src, e2, 10) {
                found = Some(lit_suffix(src, e3, "f32").unwrap_or(e3));
            }
        }
        if found.is_none() && e < src.len() && src[e] == '.' {
            if let Some(f) = digits(src, e + 1, 10) {
                found = Some(lit_suffix(src, f, "f32").unwrap_or(f));
            }
        }
        if let Some(f) = found {
            out.push((Kind::Float, f));
        }
    }
    if let Some(e) = d {
        out.push((Kind::Int, e));
    }
    out
}

/// Tokens with every physical newline as a NEWLINE token, before continuation handling.
fn raw_tokens(text: &str) -> Result<Vec<Token>, LexError> {
    let src: Vec<char> = text.chars().collect();
    if src.first() == Some(&'\u{feff}') {
        return Err(err("byte-order mark", 1, 1));
    }
    let n = src.len();
    let mut out: Vec<Token> = Vec::new();
    let mut sig: Vec<Token> = Vec::new();
    let (mut i, mut line, mut line_start) = (0usize, 1usize, 0usize);
    let mut ref_depth: i32 = 0;
    let mut square_after_upper: Vec<bool> = Vec::new();
    let mut stack: Vec<&'static str> = Vec::new();
    macro_rules! push {
        ($t:expr) => {{
            let t: Token = $t;
            out.push(t.clone());
            sig.push(t);
        }};
    }
    while i < n {
        let c = src[i];
        let col = i - line_start + 1;
        if c == '\n' {
            out.push(Token::new(Kind::Newline, "\n", line, col));
            i += 1;
            line += 1;
            line_start = i;
            continue;
        }
        if c == ' ' || c == '\t' || c == '\r' {
            i += 1;
            continue;
        }
        if starts_with(&src, i, "//") {
            i = find(&src, i, "\n").unwrap_or(n);
            continue;
        }
        if c == ';' {
            return Err(LexError { message: "';' is not a token".into(), line, col, code: "E-SYN-001" });
        }
        if starts_with(&src, i, "\"\"\"") {
            let j = match find(&src, i + 3, "\"\"\"") {
                Some(j) => j,
                None => return Err(err("unterminated raw string", line, col)),
            };
            let t = text_of(&src, i, j + 3);
            push!(Token::new(Kind::Raw, &t, line, col));
            let nls = t.matches('\n').count();
            line += nls;
            if nls > 0 {
                let last = t.chars().collect::<Vec<_>>().iter().rposition(|&c| c == '\n').unwrap();
                line_start = i + last + 1;
            }
            i = j + 3;
            continue;
        }
        if c == '"' {
            let mut interps = Vec::new();
            let j = scan_string(&src, i, line, col, Some(&mut interps))?;
            let mut t = Token::new(Kind::Str, &text_of(&src, i, j), line, col);
            t.holes = Holes::sized(interps.len());
            t.interps = interps;
            push!(t);
            i = j;
            continue;
        }
        if c == 'b' && i + 1 < n && src[i + 1] == '"' {
            let j = scan_string(&src, i + 1, line, col, None)?;
            let t = text_of(&src, i, j);
            if !t.is_ascii() {
                return Err(err("non-ASCII character in byte string", line, col));
            }
            push!(Token::new(Kind::Bytes, &t, line, col));
            i = j;
            continue;
        }
        if let Some(e) = ident_end(&src, i) {
            let t = text_of(&src, i, e);
            let kind = if keywords().contains(t.as_str()) {
                Kind::Kw
            } else if t.chars().next().unwrap().is_uppercase() {
                Kind::Upper
            } else {
                Kind::Lower
            };
            push!(Token::new(kind, &t, line, col));
            i = e;
            continue;
        }
        if c.is_numeric() {
            let mut matched = false;
            for (kind, e) in number(&src, i) {
                if !(e < n && (py_isalnum(src[e]) || src[e] == '_')) {
                    push!(Token::new(kind, &text_of(&src, i, e), line, col));
                    i = e;
                    matched = true;
                    break;
                }
            }
            if !matched {
                return Err(err("malformed numeric literal", line, col));
            }
            continue;
        }
        if c == '{' && opens_template(&out, &stack) {
            let (mut depth, mut j) = (1, i + 1);
            while j < n && depth > 0 {
                if src[j] == '{' {
                    depth += 1;
                } else if src[j] == '}' {
                    depth -= 1;
                }
                j += 1;
            }
            if depth > 0 {
                return Err(err("unterminated view body", line, col));
            }
            push!(Token::new(Kind::Punct, "{", line, col));
            let t = text_of(&src, i + 1, j - 1);
            push!(Token::new(Kind::Template, &t, line, col + 1));
            let nls = t.matches('\n').count();
            line += nls;
            if nls > 0 {
                let last = t.chars().collect::<Vec<_>>().iter().rposition(|&c| c == '\n').unwrap();
                line_start = i + 1 + last + 1;
            }
            push!(Token::new(Kind::Punct, "}", line, j - line_start));
            i = j;
            continue;
        }
        let mut handled = false;
        for p_text in PUNCT {
            if !starts_with(&src, i, p_text) {
                continue;
            }
            handled = true;
            let mut kind = Kind::Punct;
            let p = sig.last().cloned();
            let p_upper_type = p.as_ref().map_or(false, |p| (p.kind == Kind::Upper && !is_member_of_type(&sig)) || (p.text == "]" && p.kind == Kind::Punct && p.closes_type_args));
            if *p_text == "<" && p.is_some() && p_upper_type {
                kind = Kind::RefOpen;
                ref_depth += 1;
            } else if *p_text == ">" && ref_depth > 0 {
                kind = Kind::RefClose;
                ref_depth -= 1;
            } else if *p_text == ">=" && ref_depth > 0 {
                push!(Token::new(Kind::RefClose, ">", line, col));
                ref_depth -= 1;
                push!(Token::new(Kind::Punct, "=", line, col + 1));
                i += 2;
                break;
            } else if *p_text == ">>" && ref_depth > 0 {
                push!(Token::new(Kind::RefClose, ">", line, col));
                ref_depth -= 1;
                i += 1;
                break;
            }
            let mut tok = Token::new(kind, p_text, line, col);
            if *p_text == "[" {
                square_after_upper.push(p.as_ref().map_or(false, |p| p.kind == Kind::Upper));
            } else if *p_text == "]" {
                tok.closes_type_args = square_after_upper.pop().unwrap_or(false);
            }
            if *p_text == "(" || *p_text == "[" {
                stack.push(if *p_text == "(" { "(" } else { "[" });
            } else if *p_text == "{" {
                let is_expr = p.as_ref().map_or(false, |p| is_punct_or_kw_in(p, EXPRESSION_BRACE_AFTER));
                stack.push(if is_expr { "expr{" } else { "body{" });
            } else if (*p_text == ")" || *p_text == "]" || *p_text == "}") && !stack.is_empty() {
                stack.pop();
            }
            push!(tok);
            i += p_text.chars().count();
            break;
        }
        if !handled {
            return Err(err(&format!("unexpected character {:?}", c.to_string()).replace('"', "'"), line, col));
        }
    }
    out.push(Token::new(Kind::Newline, "\n", line, i - line_start + 1));
    Ok(out)
}

/// Tokens with NL emitted only where a newline ends a statement (LR 3.2).
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let raw = raw_tokens(src)?;
    let mut out: Vec<Token> = Vec::new();
    let mut stack: Vec<&'static str> = Vec::new();
    let mut k = 0;
    while k < raw.len() {
        let t = &raw[k];
        if t.kind == Kind::Newline {
            let mut j = k + 1;
            while j < raw.len() && raw[j].kind == Kind::Newline {
                j += 1;
            }
            let nxt = raw.get(j);
            let prv = out.last();
            let mut suppressed = false;
            if matches!(stack.last(), Some(&"(") | Some(&"[") | Some(&"expr{")) {
                suppressed = true;
            } else if prv.map_or(false, |p| is_punct_or_kw_in(p, TRAILING_CONTINUATION)) {
                suppressed = true;
            } else if nxt.map_or(false, |n| is_punct_or_kw_in(n, LEADING_CONTINUATION)) {
                suppressed = true;
            }
            if !suppressed && prv.map_or(false, |p| p.kind != Kind::Nl) {
                out.push(Token::new(Kind::Nl, "\n", t.line, t.col));
            }
            k = j;
            continue;
        }
        if t.kind == Kind::Punct {
            if t.text == "(" || t.text == "[" {
                stack.push(if t.text == "(" { "(" } else { "[" });
            } else if t.text == "{" {
                let is_expr = out.last().map_or(false, |p| is_punct_or_kw_in(p, EXPRESSION_BRACE_AFTER));
                stack.push(if is_expr { "expr{" } else { "body{" });
            } else if (t.text == ")" || t.text == "]" || t.text == "}") && !stack.is_empty() {
                stack.pop();
            }
        }
        out.push(t.clone());
        k += 1;
    }
    let last = raw.last().unwrap();
    if out.last().map_or(false, |t| t.kind != Kind::Nl) {
        out.push(Token::new(Kind::Nl, "\n", last.line, last.col));
    }
    out.push(Token::new(Kind::Eof, "", last.line, last.col));
    Ok(out)
}
