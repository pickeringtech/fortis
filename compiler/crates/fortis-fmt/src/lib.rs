//! The Fortis formatter (Programme 2.1, DEL-41).
//!
//! `format` gives a module its canonical layout and changes nothing else. Every token keeps its text and order, and
//! every statement keeps its line, because a newline can end a statement (Language Reference 3.2). Only these change:
//!
//! - **Indentation**: two spaces per level. A line inside brackets opened on an earlier line is one level deeper than
//!   the line that opened them; a closing bracket that starts a line returns to the opener's level; a statement
//!   continued by a trailing or leading continuation token (3.2) is one level deeper than its first line.
//! - **Spacing within a line**: one space between tokens, except none inside brackets, before `,` `:` `?` `)` `]`,
//!   around `.` and ranges (`..`, `..<`), inside a refinement (`Int<0..100>`), after a unary operator, after `@`, and
//!   before the `(` or `[` of a call, a capture list, type arguments or an index.
//! - **Blank lines**: a run of blank lines becomes one; none at the start or the end; the file ends in one newline.
//! - **Comments** keep their text, and a trailing comment keeps its distance from the code.
//!
//! The result is checked before it is returned: relexed, it must give the same tokens, and parsed, the same tree.
//! A layout the rules would get wrong is an error, never a changed program. `format(format(x)) == format(x)`.
//!
//! `format_reprinting` first rewrites the old forms v0.0.2 replaced (`old_forms`, X-345), then prints back the inferred facts of a module the checker accepts (Programme 9.3, X-130):
//! the single-word modifiers of each named contract (a lambda's inferred `pure` and `noalloc` are not printed, LR 8.2,
//! X-345). Capture lists and kinds inferred for directly loaned callbacks are also absent from the written
//! token stream (LR 8.2, X-580); explicit capture lists and kinds are kept. The module with the facts written must check exactly as before. `fortis fmt` uses it.

mod old_forms;
mod conversions;
pub mod migrate;

use fortis_syntax::lexer::{lex, Kind, Token};
use fortis_types::facts::{self, Insertion};
pub use fortis_types::check::Unit;
use fortis_syntax::tree::{Child, Node};

#[derive(Debug)]
pub enum FormatError {
    /// the input does not lex or parse: it is not formatted
    Syntax(String),
    /// the formatted text would not be the same program (a formatter defect; nothing is written)
    Changed(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Syntax(m) => write!(f, "not formatted: {m}"),
            FormatError::Changed(m) => write!(f, "formatter defect, nothing written: {m}"),
        }
    }
}

fn significant(t: &Token) -> bool {
    !matches!(t.kind, Kind::Newline | Kind::Nl | Kind::Eof)
}

/// The line a token ends on (a raw string or template may span lines).
fn end_line(t: &Token) -> usize {
    t.line + t.text.matches('\n').count()
}

/// The column just past a token (1-based, in scalars), on its end line.
fn end_col(t: &Token) -> usize {
    match t.text.rfind('\n') {
        Some(i) => t.text[i + 1..].chars().count() + 1,
        None => t.col + t.text.chars().count(),
    }
}


/// Whether `t` (at index i of a line) is a unary operator: an operator in prefix position.
fn is_unary(line: &[&Token], i: usize) -> bool {
    let t = line[i];
    if t.text == "!" {
        return true;
    }
    if t.text != "-" {
        return false;
    }
    if i == 0 {
        return true;
    }
    let p = line[i - 1];
    if p.kind == Kind::RefOpen {
        return true;
    }
    if p.kind == Kind::Punct {
        return !matches!(p.text.as_str(), ")" | "]" | "}" | "?");
    }
    p.kind == Kind::Kw && !matches!(p.text.as_str(), "self" | "true" | "false" | "null")
}

/// The space between two adjacent tokens of one line (`before` is the last token of the previous row).
fn gap(line: &[&Token], i: usize, before: Option<&Token>, written: usize) -> &'static str {
    let (p, c) = (line[i - 1], line[i]);
    let (pt, ct) = (p.text.as_str(), c.text.as_str());
    // a template's text runs from its opening brace to its closing one, whitespace included
    if p.kind == Kind::Template || c.kind == Kind::Template {
        return "";
    }
    if matches!(ct, "," | ")" | "]" | ":" | "?" | "." | "?.") || matches!(pt, "(" | "[" | "." | "?." | "@") {
        return "";
    }
    if matches!(pt, ".." | "..<" | "...") || matches!(ct, ".." | "..<") {
        return "";
    }
    if c.kind == Kind::RefOpen || p.kind == Kind::RefOpen || c.kind == Kind::RefClose {
        return "";
    }
    if (pt == "-" || pt == "!") && is_unary(line, i - 1) {
        return "";
    }
    // `[` after a plain name is an index or type arguments, or a list in a kit's syntax (`to [a, b]`): the
    // tokens cannot tell, so the written spacing stays
    if ct == "[" && p.kind == Kind::Lower {
        return if written == 0 { "" } else { " " };
    }
    if matches!(ct, "(" | "[") {
        // a keyword that names something (a method after `.` or `func`, LR 3.4), a lambda head or an aspect hook
        let named_kw = p.kind == Kind::Kw
            && (matches!(pt, "fn" | "mut" | "once" | "after" | "before" | "failure" | "success" | "present" | "otherwise" | "where")
                || (i >= 2 && matches!(line[i - 2].text.as_str(), "." | "?." | "func"))
                || (i == 1 && before.map_or(false, |b| matches!(b.text.as_str(), "." | "?."))));
        // `)` before `(` is a signature's tuple return type far more often than a call of a call's result
        let callee = matches!(p.kind, Kind::Lower | Kind::Upper | Kind::RefClose) || matches!(pt, "]" | "?") || named_kw;
        return if callee { "" } else { " " };
    }
    if pt == "{" && ct == "}" {
        return "";
    }
    " "
}

/// Tokens on one line with the formatter's spacing (`fortis view` renders declaration heads with it).
pub fn render(tokens: &[&Token]) -> String {
    let mut s = String::new();
    for i in 0..tokens.len() {
        if i > 0 {
            let (p, c) = (tokens[i - 1], tokens[i]);
            let written = if end_line(p) == c.line { c.col.saturating_sub(end_col(p)) } else { 1 };
            s.push_str(gap(tokens, i, None, written));
        }
        s.push_str(&tokens[i].text);
    }
    s
}

/// The bracket a token opens or closes.
fn opens(t: &Token) -> bool {
    t.kind == Kind::Punct && matches!(t.text.as_str(), "(" | "[" | "{")
}
fn closes(t: &Token) -> bool {
    t.kind == Kind::Punct && matches!(t.text.as_str(), ")" | "]" | "}")
}

/// One row of output: the tokens that start on a source line (a token spanning lines carries the row to its end).
struct Row<'a> {
    first: usize,
    last: usize,
    toks: Vec<&'a Token>,
    /// a statement ended (an `NL` token) between the previous row and this one
    after_nl: bool,
}

/// `format` after printing back the inferred facts of an accepted module (a module with records, or a fragment, is
/// laid out only); returns the text and the facts printed.
pub fn format_reprinting(src: &str, file: &str) -> Result<(String, Vec<Insertion>), FormatError> {
    format_migrating(src, file).map(|(t, ins, _)| (t, ins))
}

/// `format_reprinting` after rewriting an accepted module into the v0.0.2 reading forms where they mean the same
/// (`fortis_types::migrate`, X-345); returns the text, the facts printed and the number of rewrites. The rewritten
/// module must be accepted again, or nothing is written.
pub fn format_migrating(src: &str, file: &str) -> Result<(String, Vec<Insertion>, usize), FormatError> {
    format_migrating_in(src, Unit::Alone(file))
}

/// `format_migrating` for a module checked as `unit` says. A module of a package whose imports do not resolve on
/// their own is rewritten, and each rewrite verified, in its package (the module's file replaced in memory, nothing
/// else read differently); its inferred facts are not printed back, as for any module that does not check alone.
pub fn format_migrating_in(src: &str, unit: Unit) -> Result<(String, Vec<Insertion>, usize), FormatError> {
    format_migrating_options_in(src, unit, true)
}

/// As `format_migrating_in`, with the conversion retirements disabled by the CLI's `--no-migrate` switch.
pub fn format_migrating_options_in(src: &str, unit: Unit, retire_conversions: bool) -> Result<(String, Vec<Insertion>, usize), FormatError> {
    let file = match unit {
        Unit::Alone(file) => file.to_string(),
        Unit::Package { file, .. } => file.display().to_string(),
    };
    let file = file.as_str();
    // the old forms first (`old_forms`), then the reading forms (`fortis_types::migrate`)
    let src = old_forms::rewrite_in(&src.replace("\r\n", "\n"), unit)?.0;
    let mut ins = Vec::new();
    let (mut text, mut rewritten, notes) = if retire_conversions { conversions::rewrite_in(&src, unit) } else { (src.clone(), 0, vec![]) };
    for note in notes { eprintln!("{file}:{note}"); }
    // a rewrite that overlaps another waits for the next round, so the result is a fixed point
    for _ in 0..4 {
        let Ok(parsed) = fortis_syntax::parse_module(&text) else { break };
        let (records, edits) = fortis_types::migrate::rewrites_in(&parsed, &text, unit);
        if !records.is_empty() {
            if rewritten > 0 {
                return Err(FormatError::Changed(format!("the rewritten module is rejected: {}", records[0].code)));
            }
            break;
        }
        if edits.is_empty() {
            break;
        }
        let (next, n) = fortis_types::migrate::apply(&text, &edits);
        if fortis_syntax::parse_module(&next).is_err() {
            return Err(FormatError::Changed("a rewrite does not parse".into()));
        }
        text = next;
        rewritten += n;
    }
    let src = text.clone();
    if let Ok(parsed) = fortis_syntax::parse_module(&src) {
        // the facts are printed back only for a module that checks on its own
        let (records, found) = facts::inferred(&parsed, file);
        if records.is_empty() && !found.is_empty() {
            text = facts::apply(&src, &found);
            // the facts are the checker's own: written, they must leave the module accepted with the same facts
            let again = fortis_syntax::parse_module(&text).map_err(|e| FormatError::Changed(format!("the reprinted facts do not parse: {e:?}")))?;
            let (records, left) = facts::inferred(&again, file);
            if !records.is_empty() || !left.is_empty() {
                return Err(FormatError::Changed(format!("the reprinted facts change the check: {} records, {} facts still unwritten", records.len(), left.len())));
            }
            ins = found;
        } else if rewritten > 0 && !records.is_empty() {
            // a package module is judged in its package, where each round's rewrite was found and checked
            let records = match unit {
                Unit::Alone(_) => records,
                Unit::Package { .. } => fortis_types::check::check_unit_with(&parsed, &src, unit, |_| ()).0,
            };
            if let Some(r) = records.first() {
                return Err(FormatError::Changed(format!("the rewritten module is rejected: {}", r.code)));
            }
        }
    }
    format(&text).map(|t| (t, ins, rewritten))
}

pub fn format(src: &str) -> Result<String, FormatError> {
    let src = src.replace("\r\n", "\n");
    let toks = lex(&src).map_err(|e| FormatError::Syntax(e.to_string()))?;
    let lines: Vec<&str> = src.split('\n').collect();
    // rows of tokens by starting line; the lexer's `NL` tokens mark where statements end (3.2)
    let mut rows: Vec<Row> = Vec::new();
    let mut nl = true;
    for t in &toks {
        if t.kind == Kind::Nl {
            nl = true;
            continue;
        }
        if !significant(t) {
            continue;
        }
        match rows.last_mut() {
            Some(r) if t.line <= r.last => {
                r.toks.push(t);
                r.last = r.last.max(end_line(t));
            }
            _ => rows.push(Row { first: t.line, last: end_line(t), toks: vec![t], after_nl: nl }),
        }
        nl = false;
    }
    let mut out: Vec<String> = Vec::new();
    // open brackets: (the line they opened on, that line's level)
    let mut stack: Vec<(usize, usize, char)> = Vec::new();
    let mut stmt_level: Option<usize> = None;
    let mut prev_last: Option<&Token> = None;
    let mut prev_opened = false;
    let mut line_no = 1usize;
    let mut blank_run = 0usize;
    let push_comments = |from: usize, to: usize, level: usize, out: &mut Vec<String>, blank_run: &mut usize| {
        for l in from..to {
            let text = lines.get(l - 1).copied().unwrap_or("").trim();
            if text.is_empty() {
                *blank_run += 1;
                continue;
            }
            if *blank_run > 0 && !out.is_empty() {
                out.push(String::new());
            }
            *blank_run = 0;
            out.push(format!("{}{}", "  ".repeat(level), text));
        }
    };
    for row in &rows {
        // comment-only and blank lines before this row
        let lead_closers = row.toks.iter().take_while(|t| closes(t)).count();
        let comment_level = stack.last().map_or(0, |&(_, lv, _)| lv + 1);
        push_comments(line_no, row.first, comment_level, &mut out, &mut blank_run);
        // this row's level
        let mut level = match stack.last() {
            Some(&(_, lv, _)) => lv + 1,
            None => 0,
        };
        if lead_closers > 0 {
            let keep = stack.len().saturating_sub(lead_closers);
            level = if keep < stack.len() { stack[keep].1 } else { level };
        }
        // a row that continues its statement (no `NL` before it) is one level deeper, unless the previous row opened
        // brackets (whose nesting already sets the level) or this one starts by closing some
        let in_list = stack.last().map_or(false, |&(_, _, b)| b != '{') || prev_last.map_or(false, |p| p.text == ",");
        let continued = !row.after_nl && lead_closers == 0 && !prev_opened && prev_last.is_some() && !in_list;
        if continued {
            level += 1;
        }
        // the level of the row that began the current statement
        if !continued {
            stmt_level = Some(level);
        }
        // the row's text
        let mut text = "  ".repeat(level);
        for i in 0..row.toks.len() {
            if i > 0 {
                let (p, c) = (row.toks[i - 1], row.toks[i]);
                let written = if end_line(p) == c.line { c.col.saturating_sub(end_col(p)) } else { 0 };
                let g = gap(&row.toks, i, prev_last, written);
                // two or more spaces between tokens of one line are alignment (a table of fields), kept as written
                if g == " " && written >= 2 {
                    text.push_str(&" ".repeat(written));
                } else {
                    text.push_str(g);
                }
            }
            text.push_str(&row.toks[i].text);
        }
        let last = *row.toks.last().unwrap();
        let rest = lines.get(row.last - 1).map(|l| l.chars().skip(end_col(last) - 1).collect::<String>()).unwrap_or_default();
        let comment = rest.trim_start();
        if !comment.is_empty() {
            let spaces = rest.len() - comment.len();
            text.push_str(&" ".repeat(spaces.max(1)));
            text.push_str(comment.trim_end());
        }
        if blank_run > 0 && !out.is_empty() {
            out.push(String::new());
        }
        blank_run = 0;
        out.push(text.trim_end().to_string());
        // brackets opened and closed on this row; a body brace opened on a continued row nests from the statement's
        // own level (`where`, `catch` and similar clauses), other brackets from the row
        // a `{` after brackets this row closes but an earlier row opened nests from the line that opened them
        // (`U] {` after a broken header, `} else {`); on a continued row (`where`, `catch`) from the statement's
        // first row; otherwise from this row
        let depth_before = stack.len();
        let mut closed_base: Option<usize> = None;
        for t in &row.toks {
            if opens(t) {
                let b = t.text.chars().next().unwrap_or('(');
                let lv = match (b, closed_base) {
                    ('{', Some(c)) => c,
                    ('{', None) if continued => stmt_level.unwrap_or(level),
                    _ => level,
                };
                stack.push((t.line, lv, b));
            } else if closes(t) {
                if let Some((line, lv, _)) = stack.pop() {
                    if line < row.first {
                        closed_base = Some(lv);
                    }
                }
            }
        }
        prev_opened = stack.len() > depth_before;
        prev_last = Some(last);
        line_no = row.last + 1;
    }
    push_comments(line_no, lines.len() + 1, 0, &mut out, &mut blank_run);
    while out.last().map_or(false, |l| l.is_empty()) {
        out.pop();
    }
    let mut result = out.join("\n");
    result.push('\n');
    verify(&src, &result)?;
    Ok(result)
}

/// The formatted text is the same program: the same tokens, and the same tree.
fn verify(before: &str, after: &str) -> Result<(), FormatError> {
    let a = lex(before).map_err(|e| FormatError::Syntax(e.to_string()))?;
    let b = lex(after).map_err(|e| FormatError::Changed(format!("the result does not lex: {e}")))?;
    let ka: Vec<(Kind, &str)> = a.iter().filter(|t| significant(t)).map(|t| (t.kind, t.text.as_str())).collect();
    let kb: Vec<(Kind, &str)> = b.iter().filter(|t| significant(t)).map(|t| (t.kind, t.text.as_str())).collect();
    if ka != kb {
        let at = ka.iter().zip(&kb).position(|(x, y)| x != y).unwrap_or(ka.len().min(kb.len()));
        return Err(FormatError::Changed(format!("token {at} differs: {:?} became {:?}", ka.get(at), kb.get(at))));
    }
    // a module, or a fragment of statements (parsed as a block, as the conformance harness parses one)
    let (pa, pb) = match fortis_syntax::parse_module(before) {
        Ok(pa) => (pa, fortis_syntax::parse_module(after).map_err(|e| FormatError::Changed(format!("the result does not parse: {e:?}")))?),
        Err(e) => {
            let wrap = |s: &str| format!("{{ {s}\n}}\n");
            let pa = fortis_syntax::parse_from(&wrap(before), "block").map_err(|_| FormatError::Syntax(format!("{e:?}")))?;
            let pb = fortis_syntax::parse_from(&wrap(after), "block").map_err(|e| FormatError::Changed(format!("the result does not parse: {e:?}")))?;
            (pa, pb)
        }
    };
    if shape(&pa.root, &pa.toks) != shape(&pb.root, &pb.toks) {
        return Err(FormatError::Changed("the parse tree differs".into()));
    }
    Ok(())
}

/// A tree without positions or the text of newline tokens.
pub fn shape(n: &Node, toks: &[Token]) -> String {
    let mut s = String::new();
    walk(n, toks, &mut s);
    s
}

fn walk(n: &Node, toks: &[Token], s: &mut String) {
    s.push('(');
    s.push_str(n.name());
    for c in &n.children {
        match c {
            Child::Node(m) => walk(m, toks, s),
            Child::Tok(i) => {
                let t = &toks[*i];
                s.push(' ');
                s.push_str(t.kind.as_str());
                if !matches!(t.kind, Kind::Nl | Kind::Newline) {
                    s.push('=');
                    s.push_str(&t.text);
                }
            }
        }
    }
    s.push(')');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(s: &str) -> String {
        format(s).unwrap()
    }

    #[test]
    fn a_package_module_is_rewritten_in_its_package() {
        // `app` imports `Tally` from `util`, so it checks only in its package: alone it keeps its markers
        let dir = std::env::temp_dir().join(format!("fortis-fmt-pkg-{}", std::process::id()));
        let pkg = dir.join("demo.tally");
        std::fs::create_dir_all(pkg.join("src")).unwrap();
        std::fs::write(pkg.join("project.yaml"), "schema: fortis.project/v1\nname: demo.tally\nversion: 0.1.0\nentry: app\nexports: []\ndependencies:\n  fortis: \"1.0\"\n").unwrap();
        std::fs::write(pkg.join("src/util.ft"), "export class Tally {\n  export n: Int\n  export func bump(editable self) {\n    self.n = Int.saturatingAdd(self.n, 1)\n  }\n}\n").unwrap();
        let src = "import demo.tally.util { Tally }\n\nfunc count() Tally {\n  let mut t = Tally(n: 0)\n  editable t.bump()\n  return move t\n}\n\nfunc main() {\n  let t = count()\n}\n";
        let app = pkg.join("src/app.ft");
        std::fs::write(&app, src).unwrap();
        let alone = format_migrating(src, "app.ft").unwrap().0;
        let packaged = format_migrating_in(src, Unit::Package { dir: &pkg, file: &app }).unwrap().0;
        std::fs::remove_dir_all(&dir).ok();
        assert!(alone.contains("editable t.bump()") && alone.contains("return move t"), "{alone}");
        assert!(packaged.contains("\n  t.bump()\n") && packaged.contains("\n  return t\n"), "{packaged}");
    }

    #[test]
    fn indents_and_spaces() {
        let src = "func f(a:Int,b:Int) Int{\nlet x=a+b\n    return x\n}\n";
        assert_eq!(fmt(src), "func f(a: Int, b: Int) Int {\n  let x = a + b\n  return x\n}\n");
    }

    #[test]
    fn keeps_comments_and_collapses_blank_lines() {
        let src = "// head\n\n\n\nfunc f() Int {\n  // inside\n  return 1      // why\n}\n\n\n";
        assert_eq!(fmt(src), "// head\n\nfunc f() Int {\n  // inside\n  return 1      // why\n}\n");
    }

    #[test]
    fn continuation_and_brackets() {
        let src = "func f(xs: List[Int]) Int {\nlet n = xs\n.count()\nlet y = g(1,\n2)\nreturn -n\n}\n";
        assert_eq!(fmt(src), "func f(xs: List[Int]) Int {\n  let n = xs\n    .count()\n  let y = g(1,\n    2)\n  return -n\n}\n");
    }

    #[test]
    fn refinements_ranges_and_lambdas() {
        let src = "type Score = Int < 0 .. 100 >\nfunc f() Int {\n  let g = fn [ k ] ( x : Int ) Int => x\n  for i in 0 ..< 3 {\n  }\n  return 0\n}\n";
        let out = fmt(src);
        assert!(out.contains("let g = fn[k](x: Int) Int => x"), "{out}");
        assert!(out.contains("for i in 0..<3 {"), "{out}");
    }

    #[test]
    fn idempotent() {
        let src = "func f(a:Int) Int{\nif a>0{\nreturn a\n} else {\nreturn -a\n}\n}\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once);
    }
}
