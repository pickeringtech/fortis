//! What the elaboration needs to know about a module, read from its tokens: the classes with their fields and
//! `inject` slots, the functions a `using factory` may name, the `context` heads with their body statements, and
//! the imports. The scan is shallow on purpose: bodies of functions are never read, and every type and expression
//! is kept as source text, so the elaborated module carries exactly what the author wrote.

use fortis_syntax::{lex, Kind, Token};

/// Source text addressed by the lexer's (line, column), both counted from 1 in Unicode scalar values.
pub struct Src {
    chars: Vec<char>,
    starts: Vec<usize>,
}

impl Src {
    pub fn new(text: &str) -> Src {
        let chars: Vec<char> = text.chars().collect();
        let mut starts = vec![0];
        for (i, c) in chars.iter().enumerate() {
            if *c == '\n' {
                starts.push(i + 1);
            }
        }
        Src { chars, starts }
    }

    fn offset(&self, line: usize, col: usize) -> usize {
        self.starts.get(line - 1).copied().unwrap_or(self.chars.len()) + col - 1
    }

    /// The text from the start of `a` to the end of `b`.
    pub fn span(&self, a: &Token, b: &Token) -> String {
        let from = self.offset(a.line, a.col);
        let to = (self.offset(b.line, b.col) + b.text.chars().count()).min(self.chars.len());
        self.chars[from.min(to)..to].iter().collect()
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub default: bool,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub name: String,
    pub ty: String,
    pub lazy: bool,
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
pub struct Class {
    pub name: String,
    pub file: String,
    pub lifetime: Option<String>,
    pub generic: bool,
    pub fields: Vec<Field>,
    pub slots: Vec<Slot>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub default: bool,
}

#[derive(Clone, Debug)]
pub struct Func {
    /// `make` or `Class.make`
    pub path: String,
    pub params: Vec<Param>,
    pub fallible: bool,
    pub suspends: bool,
    pub ret: String,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub toks: Vec<Token>,
    pub text: String,
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
pub struct Head {
    pub name: String,
    pub params: String,
    pub param_names: Vec<String>,
    pub body: Vec<Stmt>,
    pub line: usize,
    pub end_line: usize,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: Vec<String>,
    pub members: Vec<String>,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub file: String,
    pub classes: Vec<Class>,
    pub funcs: Vec<Func>,
    pub contexts: Vec<Head>,
    pub imports: Vec<Import>,
    /// class head line -> (column after which `[lifetime r]` may be inserted)
    pub class_name_end: Vec<(String, usize, usize)>,
}

/// The token runs of `toks[from..to]` separated by statement-ending newlines at bracket depth 0.
pub fn statements(toks: &[Token], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = from;
    for i in from..to {
        let t = &toks[i];
        if t.kind == Kind::Punct {
            match t.text.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                _ => {}
            }
        }
        if t.kind == Kind::Nl && depth == 0 {
            if i > start {
                out.push((start, i));
            }
            start = i + 1;
        }
    }
    if to > start {
        out.push((start, to));
    }
    out
}

/// The index of the bracket closing the one at `open`.
pub fn closing(toks: &[Token], open: usize) -> usize {
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if t.kind == Kind::Punct {
            match t.text.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
    }
    toks.len() - 1
}

/// Split `toks[from..to]` at commas of bracket depth 0.
pub fn commas(toks: &[Token], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = from;
    for i in from..to {
        let t = &toks[i];
        if t.kind == Kind::Punct {
            match t.text.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                "," if depth == 0 => {
                    if i > start {
                        out.push((start, i));
                    }
                    start = i + 1;
                    continue;
                }
                _ => {}
            }
        }
    }
    if to > start {
        out.push((start, to));
    }
    out
}

fn is(t: &Token, text: &str) -> bool {
    t.text == text && matches!(t.kind, Kind::Kw | Kind::Punct | Kind::Lower | Kind::Upper)
}

fn params(src: &Src, toks: &[Token], open: usize) -> Vec<Param> {
    let close = closing(toks, open);
    commas(toks, open + 1, close)
        .into_iter()
        .filter_map(|(a, b)| {
            let colon = (a..b).find(|&i| is(&toks[i], ":"))?;
            let eq = (colon..b).find(|&i| is(&toks[i], "="));
            let end = eq.unwrap_or(b);
            Some(Param { name: toks[a].text.clone(), ty: src.span(&toks[colon + 1], &toks[end - 1]), default: eq.is_some() })
        })
        .collect()
}

const MODIFIERS: &[&str] = &["export", "pure", "noalloc", "async", "try", "recovers", "trusted", "once"];

/// A function head starting at `a` (modifiers, contract names, `func`), or None.
fn func_head(src: &Src, toks: &[Token], a: usize, b: usize, owner: Option<&str>) -> Option<Func> {
    let mut i = a;
    let mut fallible = false;
    let mut suspends = false;
    while i < b && (MODIFIERS.contains(&toks[i].text.as_str()) && toks[i].kind == Kind::Kw || toks[i].kind == Kind::Upper) {
        fallible |= toks[i].text == "try";
        suspends |= toks[i].text == "async";
        i += 1;
    }
    if i >= b || !is(&toks[i], "func") || i + 1 >= b {
        return None;
    }
    let name = toks[i + 1].text.clone();
    let mut j = i + 2;
    if j < b && is(&toks[j], "[") {
        j = closing(toks, j) + 1;
    }
    if j >= b || !is(&toks[j], "(") {
        return None;
    }
    let ps = params(src, toks, j);
    if owner.is_some() && ps.first().map_or(false, |p| p.name == "self") {
        return None;
    }
    let close = closing(toks, j);
    let ret_end = (close + 1..b).find(|&k| is(&toks[k], "{") || is(&toks[k], "=>") || is(&toks[k], "where")).unwrap_or(b);
    let ret = if ret_end > close + 1 { src.span(&toks[close + 1], &toks[ret_end - 1]) } else { "Void".into() };
    let path = match owner {
        Some(o) => format!("{o}.{name}"),
        None => name,
    };
    Some(Func { path, params: ps, fallible, suspends, ret, line: toks[i].line })
}

fn class_decl(src: &Src, toks: &[Token], a: usize, b: usize, file: &str, m: &mut Module) {
    let at = match (a..b).find(|&i| is(&toks[i], "class")) {
        Some(i) => i,
        None => return,
    };
    let name = toks[at + 1].text.clone();
    let mut j = at + 2;
    let mut generic = false;
    let mut lifetime = None;
    let mut name_end = toks[at + 1].col + name.chars().count();
    while j < b && is(&toks[j], "[") {
        let close = closing(toks, j);
        if j + 1 < close && is(&toks[j + 1], "lifetime") {
            lifetime = Some(toks[j + 2].text.clone());
        } else {
            generic = true;
        }
        name_end = toks[close].col + 1;
        j = close + 1;
    }
    m.class_name_end.push((name.clone(), toks[at].line, name_end));
    let open = match (j..b).find(|&i| is(&toks[i], "{")) {
        Some(i) => i,
        None => return,
    };
    let close = closing(toks, open);
    let mut class = Class { name: name.clone(), file: file.into(), lifetime, generic, fields: vec![], slots: vec![], line: toks[at].line };
    for (s, e) in statements(toks, open + 1, close) {
        let mut s = s;
        while s < e && is(&toks[s], "@") {
            // an aspect use precedes its method on the same statement only when written on one line
            s += 1;
        }
        let si = if is(&toks[s], "export") && s + 1 < e && is(&toks[s + 1], "inject") { s + 1 } else { s };
        if is(&toks[si], "inject") {
            let s = si;
            let mut k = s + 1;
            let lazy = k < e && is(&toks[k], "lazy");
            if lazy {
                k += 1;
            }
            if k + 2 < e + 1 && is(&toks[k + 1], ":") {
                class.slots.push(Slot { name: toks[k].text.clone(), ty: src.span(&toks[k + 2], &toks[e - 1]), lazy, line: toks[s].line, col: toks[s].col });
            }
            continue;
        }
        let f = if is(&toks[s], "export") { s + 1 } else { s };
        if f + 1 < e && toks[f].kind == Kind::Lower && is(&toks[f + 1], ":") {
            let eq = (f + 2..e).find(|&i| is(&toks[i], "="));
            let end = eq.unwrap_or(e);
            class.fields.push(Field { name: toks[f].text.clone(), ty: src.span(&toks[f + 2], &toks[end - 1]), default: eq.is_some(), line: toks[f].line });
            continue;
        }
        if let Some(func) = func_head(src, toks, s, e, Some(&name)) {
            m.funcs.push(func);
        }
    }
    m.classes.push(class);
}

/// Scan one module's source.
pub fn module(file: &str, text: &str) -> Module {
    let mut m = Module { file: file.into(), ..Default::default() };
    let toks = match lex(text) {
        Ok(t) => t,
        Err(_) => return m,
    };
    let src = Src::new(text);
    let end = toks.len().saturating_sub(1);
    for (a, b) in statements(&toks, 0, end) {
        let mut s = a;
        while s < b && is(&toks[s], "export") {
            s += 1;
        }
        if s >= b {
            continue;
        }
        let t = &toks[s];
        if is(t, "import") {
            let mut path = Vec::new();
            let mut k = s + 1;
            while k < b && matches!(toks[k].kind, Kind::Lower | Kind::Upper | Kind::Kw) && !is(&toks[k], "as") {
                path.push(toks[k].text.clone());
                k += 1;
                if k < b && is(&toks[k], ".") {
                    k += 1;
                } else {
                    break;
                }
            }
            let mut members = Vec::new();
            let mut alias = None;
            if k < b && is(&toks[k], "{") {
                let close = closing(&toks, k);
                for (x, _) in commas(&toks, k + 1, close) {
                    members.push(toks[x].text.clone());
                }
            } else if k + 1 < b && is(&toks[k], "as") {
                alias = Some(toks[k + 1].text.clone());
            }
            m.imports.push(Import { path, members, alias });
        } else if is(t, "context") && t.kind == Kind::Kw {
            let name = toks[s + 1].text.clone();
            let mut k = s + 2;
            let mut ptext = String::new();
            let mut names = Vec::new();
            if is(&toks[k], "(") {
                let close = closing(&toks, k);
                if close > k + 1 {
                    ptext = src.span(&toks[k + 1], &toks[close - 1]);
                }
                names = params(&src, &toks, k).into_iter().map(|p| p.name).collect();
                k = close + 1;
            }
            if k < b && is(&toks[k], "{") {
                let close = closing(&toks, k);
                // a line that starts with `to`, `replaces` or `{` continues the `provide` above it
                let mut spans: Vec<(usize, usize)> = Vec::new();
                for (x, y) in statements(&toks, k + 1, close) {
                    let cont = matches!(toks[x].text.as_str(), "to" | "replaces" | "{");
                    match spans.last_mut() {
                        Some(last) if cont && toks[last.0].text == "provide" => last.1 = y,
                        _ => spans.push((x, y)),
                    }
                }
                let body = spans
                    .into_iter()
                    .map(|(x, y)| {
                        let ts: Vec<Token> = toks[x..y].iter().filter(|t| t.kind != Kind::Nl).cloned().collect();
                        Stmt { toks: ts, text: src.span(&toks[x], &toks[y - 1]), line: toks[x].line, col: toks[x].col }
                    })
                    .collect();
                m.contexts.push(Head { name, params: ptext, param_names: names, body, line: t.line, end_line: toks[close].line });
            }
        } else if (s..b).any(|i| is(&toks[i], "class") && toks[i].kind == Kind::Kw) && (is(t, "class") || is(t, "trivial")) {
            class_decl(&src, &toks, s, b, file, &mut m);
        } else if let Some(f) = func_head(&src, &toks, a, b, None) {
            m.funcs.push(f);
        }
    }
    m
}

/// Whether a module uses the kit's syntax at all: a `context` head or an `inject` member, as tokens, so that the
/// words inside a string or a comment do not count (SP-12 closing review 4).
pub fn uses_di(text: &str) -> bool {
    if !text.contains("inject") && !text.contains("context") {
        return false;
    }
    let Ok(toks) = lex(text) else {
        // text that does not lex is read line by line
        return uses_di_lines(text);
    };
    let word = |t: &Token| matches!(t.kind, Kind::Kw | Kind::Lower);
    toks.iter().enumerate().any(|(i, t)| {
        // `inject` is a reserved word, so wherever it stands it can only be a slot
        if word(t) && t.text == "inject" {
            return true;
        }
        // `context` heads a declaration: first on its line, after `export` and a `server`, `client` or `hybrid`
        // prefix if it has them (D-90: a context the scan missed was never elaborated), a name after it
        let mut j = i;
        while j > 0 && toks[j - 1].line == t.line && ["export", "server", "client", "hybrid"].contains(&toks[j - 1].text.as_str()) {
            j -= 1;
        }
        let first = j == 0 || toks[j - 1].line < t.line || matches!(toks[j - 1].kind, Kind::Newline | Kind::Nl);
        word(t) && t.text == "context" && first && toks.get(i + 1).is_some_and(|n| n.line == t.line && matches!(n.kind, Kind::Upper | Kind::Lower))
    })
}

/// The line-by-line reading of `uses_di`, for text that does not lex.
fn uses_di_lines(text: &str) -> bool {
    text.lines().any(|l| {
        let code = l.split("//").next().unwrap_or("");
        let mut t = code.trim_start();
        for w in ["export ", "server ", "client ", "hybrid "] {
            t = t.strip_prefix(w).map_or(t, str::trim_start);
        }
        t.starts_with("context ") || code.split(|c: char| c.is_whitespace() || c == '{').any(|w| w == "inject") && code.contains("inject ")
    })
}

#[cfg(test)]
mod tests {
    use super::uses_di;

    #[test]
    fn a_context_is_seen_with_export_and_a_prefix() {
        // D-90: these contexts were never elaborated, so nothing validated their bodies
        assert!(uses_di("export context App {\n    provide Repo\n}\n"));
        assert!(uses_di("server context App {\n    provide Repo\n}\n"));
        assert!(uses_di("export hybrid context App {\n    provide Repo\n}\n"));
        // the same through the line-by-line reading of text that does not lex
        assert!(uses_di("export client context App {\n    provide \"unterminated\n}\n"));
        assert!(!uses_di("func f() String => \"export context App\"\n"));
    }

    #[test]
    fn the_kit_is_seen_in_code_only() {
        assert!(uses_di("class Shop {\n    inject repo: Repo\n}\n"));
        assert!(uses_di("context App {\n    bind Repo => SqlRepo()\n}\n"));
        assert!(!uses_di("func ok() String => \"this is inject a harmless string\"\n"));
        assert!(!uses_di("// inject repo: Repo\nfunc ok() Int => 1\n"));
        assert!(!uses_di("func ok() String => \"context Shop is a string\"\n"));
        assert!(!uses_di("func context2() Int => 1\n"));
    }
}
