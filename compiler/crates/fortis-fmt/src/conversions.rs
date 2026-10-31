//! The retired conversion spellings (LR 6.13, X-552). The checker supplies the identity of each call; syntax only
//! supplies its boundaries. The grammar decides grouping (D-505, X-542); every candidate is checked in the
//! original unit before it is kept. An explicit dead `or fail` can go only when the conversion is total and the
//! module keeps its verdict. Other outcome consumers retain the legacy outcome shape.

use std::collections::HashMap;

use fortis_syntax::lexer::Token;
use fortis_syntax::view::N;
use fortis_types::check::{check_unit_with, Unit};
use fortis_types::migrate::{apply, Edit, LegacyConversion};
use fortis_types::types::Type;

#[derive(Clone)]
struct Candidate {
    edit: Edit,
    /// `validate` has always returned Outcome, including when `as` proves a newtype's range.
    outcome: Vec<String>,
    without_handler: Vec<(Edit, String)>,
}

fn end(t: &Token) -> (usize, usize) {
    match t.text.rfind('\n') {
        Some(i) => (t.line + t.text.matches('\n').count(), t.text[i + 1..].chars().count() + 1),
        None => (t.line, t.col + t.text.chars().count()),
    }
}

fn source(src: &str, from: (usize, usize), to: (usize, usize)) -> String {
    let mut out = String::new();
    for (i, line) in src.split_inclusive('\n').enumerate() {
        let row = i + 1;
        if row < from.0 || row > to.0 { continue; }
        for (j, c) in line.chars().enumerate() {
            let pos = (row, j + 1);
            if pos >= from && pos < to { out.push(c); }
        }
    }
    out
}

fn text(src: &str, n: N) -> String {
    let ts = n.tokens();
    source(src, (ts[0].line, ts[0].col), end(ts.last().unwrap()))
}

/// Comments between call punctuation and the operand are outside the operand's own slice. Keep them inside the
/// replacement's grouping, before its expression, so neither a removed member name nor a closing parenthesis
/// discards them. Comments already in a retained slice stay exactly there.
fn keep_comments(src: &str, n: N, retained: &[N], c: &mut Candidate) {
    let spans: Vec<_> = retained.iter().map(|n| {
        let ts = n.tokens();
        ((ts[0].line, ts[0].col), end(ts.last().unwrap()))
    }).collect();
    let mut comments = String::new();
    for pair in n.tokens().windows(2) {
        let (a, b) = (end(pair[0]), (pair[1].line, pair[1].col));
        if spans.iter().any(|(from, to)| a >= *from && b <= *to) { continue; }
        let gap = source(src, a, b);
        if gap.contains("//") { comments.push_str(&gap); }
    }
    if !comments.is_empty() {
        c.edit.text = format!("({comments}\n{}\n)", c.edit.text);
        for alternative in &mut c.outcome { *alternative = format!("({comments}\n{alternative}\n)"); }
    }
}

/// Compare expression trees modulo grouping and the grammar's transparent single-child productions. Folding
/// `asExpr` makes the grammar's left-associative chain explicit: `(a as B) as C` and `a as B as C` agree.
/// No operator is reassociated. In particular arithmetic, unary operations and outcome handling keep their trees.
fn shape(n: N) -> String {
    let kids = n.kids();
    if n.name() == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") && kids[2].text_is(")") {
        return shape(kids[1].node().unwrap());
    }
    if kids.len() == 1 {
        if let Some(child) = kids[0].node() { return shape(child); }
    }
    if n.name() == "asExpr" && kids.len() > 1 {
        let mut left = shape(kids[0].node().unwrap());
        for pair in kids[1..].chunks_exact(2) {
            left = format!("(as {left} {})", shape(pair[1].node().unwrap()));
        }
        return left;
    }
    let parts: Vec<_> = kids.into_iter().filter_map(|k| match k {
        fortis_syntax::view::C::N(n) => Some(shape(n)),
        fortis_syntax::view::C::T(t) if t.kind == fortis_syntax::lexer::Kind::Nl => None,
        fortis_syntax::view::C::T(t) => Some(format!("{:?}:{:?}", t.kind, t.text)),
    }).collect();
    format!("({} {})", n.name(), parts.join(" "))
}

fn expression_shape(src: &str) -> Option<String> {
    let p = fortis_syntax::parse_from(src, "expr").ok()?;
    Some(shape(N::new(&p.root, &p.toks)))
}

fn expression_context(src: &str, root: N, first: &Token, last: &Token) -> (String, String) {
    let from = (first.line, first.col);
    let to = end(last);
    let context = root.walk().into_iter().filter(|n| n.name() == "expr").filter(|n| {
        let ts = n.tokens();
        !ts.is_empty() && (ts[0].line, ts[0].col) <= from && end(ts.last().unwrap()) >= to
    }).min_by_key(|n| n.ntokens());
    context.map_or_else(|| (String::new(), String::new()), |n| {
        let ts = n.tokens();
        (source(src, (ts[0].line, ts[0].col), from), source(src, to, end(ts.last().unwrap())))
    })
}

/// Use the parser's precedence grammar itself (fortis.ebnf, outcomeExpr through primaryExpr), rather than a
/// second table in the printer. Start with both pairs, then keep the fewest pairs giving the same tree in the
/// enclosing expression. This also catches the lexical `<` after a type (X-550), postfixes, unary operators and
/// greedy expression-bodied lambdas. Existing grouping inside the operand is left alone.
fn minimal_cast(src: &str, root: N, first: &Token, last: &Token, operand: &str, target: &str) -> String {
    let (prefix, suffix) = expression_context(src, root, first, last);
    let grouped = format!("(({operand}) as {target})");
    let reference = expression_shape(&format!("{prefix}{grouped}{suffix}"));
    if reference.is_some() {
        for cast in [format!("{operand} as {target}"), format!("({operand}) as {target}"), format!("({operand} as {target})")] {
            if expression_shape(&format!("{prefix}{cast}{suffix}")) == reference { return cast; }
        }
    }
    grouped
}

fn minimal_value(src: &str, root: N, first: &Token, last: &Token, value: &str) -> String {
    let (prefix, suffix) = expression_context(src, root, first, last);
    let grouped = expression_shape(&format!("{prefix}({value}){suffix}"));
    if grouped.is_some() && expression_shape(&format!("{prefix}{value}{suffix}")) == grouped {
        value.to_string()
    } else { format!("({value})") }
}

fn candidate(src: &str, root: N, first: &Token, last: &Token, operand: &str, target: &str, validate: bool) -> Candidate {
    let cast = minimal_cast(src, root, first, last, operand, target);
    // The success constructor is a fresh argument context, independent of the old call's consumer.
    let value = if expression_shape(&format!("{operand} as {target}")) == expression_shape(&format!("({operand}) as {target}")) {
        format!("{operand} as {target}")
    } else { format!("({operand}) as {target}") };
    Candidate {
        edit: Edit { from: (first.line, first.col), to: end(last), text: cast.clone(), rule: "R20" },
        outcome: if validate { vec![format!("Outcome[{target}].success({value})"), format!("Outcome[{target}].success({operand})")] } else { vec![] },
        without_handler: vec![],
    }
}

/// A target's visible name, preserving nominal identity when the module imports another type with the same name.
fn spelling(t: &Type, ty: &fortis_types::typer::Typer) -> Option<String> {
    match t {
        Type::Named(n) => {
            let d = ty.dc.resolve_type(t, None)?;
            let resolves = |key: &str| ty.dc.table.get(key).map_or(false, |v| v.name == d.name && v.module == d.module && v.package == d.package);
            let name = if resolves(&n.name) { n.name.clone() } else {
                ty.dc.table.decls.keys().find(|k| resolves(k)).cloned()?
            };
            if n.args.is_empty() { Some(name) } else {
                Some(format!("{name}[{}]", n.args.iter().map(|a| spelling(a, ty)).collect::<Option<Vec<_>>>()?.join(", ")))
            }
        }
        Type::Refined { base, .. } => {
            let base_name = spelling(&Type::Named(base.clone()), ty)?;
            Some(t.show().replacen(&base.show(), &base_name, 1))
        }
        Type::Optional(a) => Some(format!("{}?", spelling(a, ty)?)),
        Type::Loan { inner, .. } => spelling(inner, ty),
        // Conversions cannot target a function, tuple, record, dynamic interface or unsolved type argument.
        _ => None,
    }
}

/// `to()` only uses an expected type written by its consumer. A containing call argument or field has a declared
/// parameter/field type; the checker's resolved target supplies its instantiated identity. An unannotated let,
/// collection element or inferred lambda return does not supply a written target.
fn written_context(n: N, pos: (usize, usize), inherited: bool) -> bool {
    let ts = n.tokens();
    if ts.is_empty() || pos < (ts[0].line, ts[0].col) || pos >= end(ts.last().unwrap()) { return false; }
    let next = match n.name() {
        "letStmt" => n.child_node("type").is_some(),
        "arg" => true,
        "funcDecl" => n.child_node("funcHead").and_then(|h| h.child_node("funcSignature")).and_then(|s| s.child_node("returnType")).is_some(),
        "classMember" => n.child_node("methodHead").and_then(|h| h.child_node("funcSignature")).and_then(|s| s.child_node("returnType")).is_some(),
        "lambda" => n.child_node("type").is_some(),
        "fieldDecl" => n.child_node("type").is_some(),
        "listLit" | "mapLit" | "setLit" | "tupleLit" | "recordLit" => false,
        _ => inherited,
    };
    for k in n.kids().into_iter().filter_map(|k| k.node()) {
        let kt = k.tokens();
        if !kt.is_empty() && pos >= (kt[0].line, kt[0].col) && pos < end(kt.last().unwrap()) {
            return written_context(k, pos, next);
        }
    }
    next
}

/// Copy a borrowed expression once. A place has the ordinary marker; an arbitrary expression is lent to a
/// fresh lambda parameter, which is the place copied. Its declared type is the representation's unrefined base:
/// validating its bounds still belongs to the following conversion.
fn copy_once(src: &str, operand: &str, representation: &Type, ty: &fortis_types::typer::Typer) -> Option<String> {
    let bare = operand.strip_prefix("alias ").unwrap_or(operand);
    if fortis_syntax::parse_from(bare, "place").is_ok() {
        return Some(format!("copy {bare}"));
    }
    let base = match representation { Type::Refined { base, .. } => Type::Named(base.clone()), _ => representation.clone() };
    let spelling = spelling(&base, ty)?;
    let mut name = "conversionSource".to_string();
    while src.contains(&name) { name.push('x'); }
    Some(format!("(fn ({name}: {spelling}) {spelling} => copy {name})({operand})"))
}

fn collect(src: &str, root: N, calls: &HashMap<(usize, usize), LegacyConversion>, ty: &fortis_types::typer::Typer) -> (Vec<Candidate>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for n in root.walk() {
        if n.name() == "primaryExpr" {
            let (Some(tr), Some(m), Some(al)) = (n.child_node("typeRef"), n.child_node("memberName"), n.child_node("argList")) else { continue };
            let member = m.first_token().unwrap();
            let Some(resolved) = calls.get(&(member.line, member.col)) else { continue };
            if resolved.kind == "to" { continue; }
            if resolved.kind.starts_with("validate") && !ty.legacy_conversion_sources.get(&(member.line, member.col)).map_or(false, fortis_types::types::known) {
                notes.push(format!("{}:{}: `validate` kept: its operand type is not established; bind the operand with a type before rewriting it as `as`", member.line, member.col));
                continue;
            }
            let args = al.child_nodes("arg");
            if args.len() != 1 { continue; }
            let Some(e) = args[0].child_node("expr") else { continue };
            let ts = n.tokens();
            let operand = text(src, e);
            let operand = if resolved.kind == "validate-copy" {
                // A string literal receives its old owned String allocation from the newtype conversion itself.
                let literal = e.tokens().len() == 1 && e.tokens()[0].kind == fortis_syntax::lexer::Kind::Str;
                let fresh_text = ty.legacy_conversion_sources.get(&(member.line, member.col)).map_or(false, |t| {
                    matches!(t, Type::Named(n) if n.package.is_empty() && n.name == "String")
                        && fortis_syntax::parse_from(&operand, "place").is_err()
                });
                if literal || fresh_text { operand } else {
                    let repr = resolved.target.as_ref().and_then(|t| ty.dc.resolve_type(t, None)).and_then(|d| d.alias.clone());
                    let Some(copied) = repr.and_then(|r| copy_once(src, &operand, &r, ty)) else { continue };
                    copied
                }
            } else { operand };
            let target = text(src, tr);
            let validate = resolved.kind.starts_with("validate");
            let mut c = candidate(src, root, ts[0], ts.last().unwrap(), &operand, &target, validate);
            if resolved.kind == "string-copy" {
                let bare = operand.strip_prefix("move ").or_else(|| operand.strip_prefix("alias ")).unwrap_or(&operand);
                c.edit.text = if bare.starts_with("copy ") { format!("({bare})") } else { format!("(copy {bare})") };
            } else if resolved.kind == "string-owned" {
                c.edit.text = format!("({operand})");
            } else if resolved.kind == "string-borrowed" {
                let Some(copied) = resolved.target.as_ref().and_then(|t| copy_once(src, &operand, t, ty)) else { continue };
                c.edit.text = format!("({copied})");
            }
            keep_comments(src, n, &[e, tr], &mut c);
            if validate {
                // Only the whole handled value may lose its handler, never a call nested in that value.
                // Parenthesised calls are transparent. A proved success never enters a failure handler.
                for handled in root.walk().into_iter().filter(|n| n.name() == "outcomeExpr" && !n.words().is_empty()) {
                    let Some(value) = handled.child_node("coalesceExpr") else { continue };
                    if shape(value) != shape(n) { continue; }
                    let ht = handled.tokens();
                    // The same text can occur twice; this handler must contain this resolved call.
                    if (ht[0].line, ht[0].col) > c.edit.from || end(ht.last().unwrap()) < c.edit.to { continue; }
                    let mut dropped = candidate(src, root, ht[0], ht.last().unwrap(), &operand, &target, false);
                    keep_comments(src, handled, &[e, tr], &mut dropped);
                    c.without_handler.push((dropped.edit, c.outcome[0].clone()));
                    // A proved structural refinement needs no `as` at all (X-553). It is total too.
                    let mut identity = candidate(src, root, ht[0], ht.last().unwrap(), &operand, &target, false);
                    identity.edit.text = minimal_value(src, root, ht[0], ht.last().unwrap(), &operand);
                    keep_comments(src, handled, &[e, tr], &mut identity);
                    c.without_handler.push((identity.edit, c.outcome[1].clone()));
                    break;
                }
            }
            out.push(c);
        } else if n.name() == "postfixExpr" || n.name() == "markerExpr" {
            let ts = n.tokens();
            for k in n.kids().into_iter().filter_map(|k| k.node()) {
                let mc = if k.name() == "methodCall" { Some(k) } else { k.child_node("methodCall") };
                let Some(mc) = mc else { continue };
                let Some(member) = mc.child_node("memberName").and_then(|m| m.first_token()) else { continue };
                let pos = (member.line, member.col);
                let Some(resolved) = calls.get(&pos).filter(|r| r.kind == "to") else { continue };
                let target = resolved.target.as_ref().and_then(|t| spelling(t, ty));
                if !written_context(root, pos, false) || target.is_none() {
                    notes.push(format!("{}:{}: `to()` kept: its target type is not written; add an expected type to rewrite it as `as`", member.line, member.col));
                    continue;
                }
                let mt = mc.tokens();
                let operand = source(src, (ts[0].line, ts[0].col), (mt[0].line, mt[0].col));
                let mut c = candidate(src, root, ts[0], mt.last().unwrap(), &operand, &target.unwrap(), false);
                keep_comments(src, mc, &[], &mut c);
                out.push(c);
            }
        } else if n.name() == "upperIdentList" && ty.dc.table.get("From").map_or(false, |d| fortis_types::decls::is_standard(&d.module)) {
            // This syntax denotes the built-in derivation only when `From` resolves to that interface.
            let kids = n.kids();
            for pair in kids.windows(2) {
                let (Some(name), Some(args)) = (pair[0].tok(), pair[1].node()) else { continue };
                if name.text != "From" || args.name() != "typeArgs" { continue; }
                let ats = args.tokens();
                if ats.len() < 3 { continue; }
                let inside = source(src, end(ats[0]), (ats.last().unwrap().line, ats.last().unwrap().col));
                out.push(Candidate { edit: Edit { from: (name.line, name.col), to: end(ats.last().unwrap()), text: format!("convert from {}", inside.trim()), rule: "R20" }, outcome: vec![], without_handler: vec![] });
            }
        }
    }
    // Interpolation expressions are parsed separately by the checker, with their original positions. Walk those
    // same trees so a call inside `${...}` is resolved and grouped exactly like a call outside a string.
    for token in root.toks {
        for (index, (hole, _, _)) in token.interps.iter().enumerate() {
            if let Some(parsed) = fortis_types::typer::expr::hole_tree_at(token, index, hole) {
                let (edits, found_notes) = collect(src, N::new(&parsed.root, &parsed.toks), calls, ty);
                out.extend(edits);
                notes.extend(found_notes);
            }
        }
    }
    (out, notes)
}

fn accepted(src: &str, unit: Unit) -> bool {
    let Ok(p) = fortis_syntax::parse_module(src) else { return false };
    check_unit_with(&p, src, unit, |_| ()).0.is_empty()
}

/// Rewrites and notes for an accepted module. A failing candidate leaves the old accepted spelling in place.
pub fn rewrite_in(src: &str, unit: Unit) -> (String, usize, Vec<String>) {
    let mut text = src.to_string();
    let mut count = 0;
    let mut notes = Vec::new();
    // Every successful round removes at least one retired call/derivation. The token count is a finite upper bound.
    let limit = fortis_syntax::lexer::lex(src).map_or(0, |ts| ts.len());
    for _ in 0..limit {
        let Ok(p) = fortis_syntax::parse_module(&text) else { break };
        let (records, found) = check_unit_with(&p, &text, unit, |ty| {
            let root = N::new(&p.root, &p.toks);
            collect(&text, root, &ty.legacy_conversions, ty)
        });
        if !records.is_empty() { break; }
        let Some((candidates, found_notes)) = found else { break };
        notes = found_notes;
        if candidates.is_empty() { break; }
        let mut dropped = false;
        for c in &candidates {
            for (edit, wrapped) in &c.without_handler {
                // An explicitly typed success argument proves that the cast has T, not Outcome[T]. Checking only
                // the handlerless candidate would also accept an implicitly handled fallible conversion in a test.
                let mut proof = c.edit.clone();
                proof.text = wrapped.clone();
                if !accepted(&apply(&text, &[proof]).0, unit) { continue; }
                let (next, n) = apply(&text, &[edit.clone()]);
                if next != text && accepted(&next, unit) {
                    text = next;
                    count += n;
                    dropped = true;
                    break;
                }
            }
            if dropped { break; }
        }
        if dropped { continue; }
        let edits: Vec<Edit> = candidates.iter().map(|c| c.edit.clone()).collect();
        let (next, n) = apply(&text, &edits);
        if next != text && accepted(&next, unit) {
            text = next;
            count += n;
            continue;
        }
        let mut changed = false;
        for c in candidates {
            for replacement in std::iter::once(c.edit.text.clone()).chain(c.outcome) {
                let mut edit = c.edit.clone();
                edit.text = replacement;
                let (next, n) = apply(&text, &[edit]);
                if next != text && accepted(&next, unit) {
                    text = next;
                    count += n;
                    changed = true;
                    break;
                }
            }
            if changed { break; }
        }
        if !changed { break; }
    }
    notes.sort();
    notes.dedup();
    (text, count, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_string_copy(data: &fortis_testkit::Json, expect: &fortis_testkit::Json) {
        let src = data.as_str().unwrap();
        let wanted = expect.as_str().unwrap();
        assert!(accepted(src, Unit::Alone("test.ft")));
        let (out, count, _) = rewrite_in(src, Unit::Alone("test.ft"));
        assert_eq!(out, wanted);
        assert_eq!(count, 1);
        assert_eq!(rewrite_in(&out, Unit::Alone("test.ft")).0, out);
        let ty = if src.starts_with("class Holder") { "Holder" } else { "String" };
        let caller = format!("noalloc func caller(value: owned {ty}) String {{ return fromMove(move value) }}\n");
        for body in [src, &out] {
            let source = format!("{body}{caller}");
            let parsed = fortis_syntax::parse_module(&source).unwrap();
            let (records, _) = check_unit_with(&parsed, &source, Unit::Alone("test.ft"), |_| ());
            let codes: std::collections::BTreeSet<_> = records.iter().map(|r| r.code.as_str()).collect();
            assert_eq!(codes.into_iter().collect::<Vec<_>>(), vec!["E-EFF-001"], "{source}");
        }
    }

    fortis_testkit::case_tests!("formatter/string_copy.json", check_string_copy;
        place, moved, alias, copy, field, aliased_field
    );

    #[test]
    fn validation_rewrite_drops_only_proved_handlers() {
        for handler in ["or recover Age(0)", "catch e { recover Age(0) }"] {
            let src = format!("type Age = Int<0..120>\nrecovers func f() Age {{ return Age.validate(42) {handler} }}");
            let (out, count, _) = rewrite_in(&src, Unit::Alone("test.ft"));
            assert!(count > 0);
            assert!(out.contains("return 42 as Age"), "{out}");
            assert!(!out.contains("Outcome[Age]"), "{out}");
            assert!(!out.contains(handler), "{out}");
            let fallible = format!("type Age = Int<0..120>\nrecovers func f(n: Int) Age {{ return Age.validate(n) {handler} }}");
            let (out, _, _) = rewrite_in(&fallible, Unit::Alone("test.ft"));
            assert!(out.contains(handler), "{out}");
        }
    }

    #[test]
    fn validation_keeps_a_stored_outcome() {
        let src = "type Age = Int<0..120>\nfunc f() Outcome[Age] { return Age.validate(42) }";
        let (out, _, _) = rewrite_in(src, Unit::Alone("test.ft"));
        assert!(out.contains("Outcome[Age].success(42 as Age)"), "{out}");
    }

    #[test]
    fn validation_reuses_fresh_text_and_copies_a_borrowed_result() {
        let fresh = "type Code = String<scalars: 1..10>\ntry func f(s: StringView) Code { return Code.validate(s as String) or fail }";
        let (out, _, _) = rewrite_in(fresh, Unit::Alone("test.ft"));
        assert!(out.contains("return s as String as Code or fail"), "{out}");
        assert!(!out.contains("conversionSource"), "{out}");
        let lent = "type Code = String\nfunc read(s: String) alias String from s { return alias s }\nrecovers func f(s: String) { let c = Code.validate(read(alias s)) or recover Code(\"fallback\")\n discard c }";
        let parsed = fortis_syntax::parse_module(lent).unwrap();
        let (records, sources) = check_unit_with(&parsed, lent, Unit::Alone("test.ft"), |ty| ty.legacy_conversion_sources.clone());
        let (out, _, notes) = rewrite_in(lent, Unit::Alone("test.ft"));
        assert!(out.contains("copy conversionSource"), "{out} {records:?} {sources:?} {notes:?}");
    }

    #[test]
    fn grouping_follows_the_expression_grammar() {
        for (context, operand, want) in [
            ("old()", "x", "x as Target"),
            ("old()", "a - b", "(a - b) as Target"),
            ("old()", "f() or fail", "(f() or fail) as Target"),
            ("old()", "a ?? b", "(a ?? b) as Target"),
            ("old()", "-x", "-x as Target"),
            ("old()", "move x", "move x as Target"),
            ("old()", "x as Source", "x as Source as Target"),
            ("old().field", "x", "(x as Target)"),
            ("old()()", "x", "(x as Target)"),
            ("old()[0]", "x", "(x as Target)"),
            ("-old()", "x", "(x as Target)"),
            ("!old()", "x", "(x as Target)"),
            ("await old()", "x", "(x as Target)"),
            ("old() < 5", "x", "(x as Target)"),
            ("old() <= 5", "x", "x as Target"),
            ("old() > 5", "x", "x as Target"),
            ("old() >= 5", "x", "x as Target"),
            ("old() + 5", "x", "x as Target"),
            ("old() * 5", "x", "x as Target"),
            ("old() as Next", "x", "x as Target"),
            ("(old()).field", "x", "x as Target"),
            ("old()", "fn() => x", "(fn() => x) as Target"),
        ] {
            let parsed = fortis_syntax::parse_from(context, "expr").unwrap();
            let first = parsed.toks.iter().position(|t| t.text == "old").unwrap();
            let got = minimal_cast(context, N::new(&parsed.root, &parsed.toks), &parsed.toks[first], &parsed.toks[first + 2], operand, "Target");
            assert_eq!(got, want, "{context}, operand {operand}");
        }
    }
}

#[cfg(test)]
#[path = "conversion_units.rs"]
mod unit_cases;
