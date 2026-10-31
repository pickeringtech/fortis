//! LR 6.3 and 6.10: infer one expression or narrow one condition, in memory.
mod u1_support;
use std::collections::BTreeMap;
use fortis_testkit::Json;
use fortis_names::{Library, library::Libraries};
use fortis_syntax::{parse_from, parse_module, view::N};
use fortis_types::{decls::{DeclChecker, Session}, typer::{Typer, env_new, env_set}};
fn check(input: &Json, expect: &Json) {
    let libs = Libraries { libs: vec![Library { modules: BTreeMap::new() }] };
    let session = Session::new(&libs);
    let head = parse_module(input.get("head").and_then(Json::as_str).unwrap_or("")).unwrap();
    let op = input.get("op").unwrap().as_str().unwrap();
    let source = input.get("source").unwrap().as_str().unwrap();
    let source = if op == "statement" { format!("{{ {source}\n}}") } else { source.to_string() };
    let parsed = parse_from(&source, if op == "statement" { "block" } else { "expr" }).unwrap();
    let mut dc = DeclChecker::new(&session, 0, "unit.ft", "main", "");
    dc.load_module_tree(N::new(&head.root, &head.toks), "main", "", false, None, None);
    let mut ty = Typer::new(dc);
    let env = env_new(None);
    let Json::Obj(vars) = input.get("env").unwrap() else { panic!("env") };
    for (name, value) in vars {
        let mut t = u1_support::typ(value);
        if let Some(d) = value.as_str().and_then(|n| ty.dc.table.get(n)) {
            t = fortis_types::decls::named_of_decl(&d, vec![]);
        }
        env_set(&env, name, t, false, "let");
    }
    let expr = N::new(&parsed.root, &parsed.toks);
    let expr = if op == "statement" { expr.find("statement").unwrap() } else { expr };
    if input.get("op").unwrap().as_str() == Some("narrow") {
        let (yes,no,_) = ty.narrow(expr, &env);
        let got = Json::Arr([yes,no].iter().map(|f| {
            let (a,b) = f.ints.get("x").copied().unwrap_or((0,9));
            Json::Arr(vec![Json::Int(a as i64),Json::Int(b as i64)])
        }).collect());
        assert_eq!(&got, expect);
    } else if op == "statement" {
        ty.statement(expr, &env);
    } else {
        let expected = input.get("expected").map(u1_support::typ);
        let actual = ty.infer(Some(expr), &env, expected.as_ref());
        if op != "codes" { assert_eq!(actual, u1_support::typ(expect)); }
    }
    if op == "codes" || op == "statement" {
        let mut codes: Vec<_> = ty.dc.records.iter().map(|r| r.code.clone()).collect();
        codes.sort(); codes.dedup();
        assert_eq!(&Json::strs(codes), expect);
    } else { assert!(ty.dc.records.is_empty(), "{:?}", ty.dc.records); }
}
fortis_testkit::case_tests!("types/expressions.json", check;
    lr63_condition_lt_1,
    lr63_condition_lt_4,
    lr63_condition_lt_8,
    lr63_condition_le_1,
    lr63_condition_le_4,
    lr63_condition_le_8,
    lr63_condition_gt_1,
    lr63_condition_gt_4,
    lr63_condition_gt_8,
    lr63_condition_ge_1,
    lr63_condition_ge_4,
    lr63_condition_ge_8,
    lr63_condition_eq_1,
    lr63_condition_eq_4,
    lr63_condition_eq_8,
    lr63_condition_ne_1,
    lr63_condition_ne_4,
    lr63_condition_ne_8,
    lr63_condition_and,
    lr63_condition_or,
    lr63_condition_flipped,
    lr610_pick_equal,
    lr610_pick_nested,
    lr610_pick_nested_reverse,
    lr610_pick_disjoint,
    lr610_pick_disjoint_reverse,
    lr610_pick_base,
    lr610_pick_base_reverse,
    lr610_pick_bool,
    lr610_null_fixes_nothing,
    lr610_passthrough_arguments_before_expected,
    lr63_match_full,
    lr98_range_complete,
    lr98_range_exclusive_complete,
    lr98_range_gap,
    lr98_range_guard,
    lr98_range_overlap,
    lr98_range_repeated,
    lr98_range_reversed,
    lr98_range_empty,
    lr98_range_open_upper,
    lr98_range_open_lower,
    lr98_range_otherwise,
    lr98_range_literal_not_coverage,
    lr98_enum_complete,
    lr98_enum_alternatives,
    lr98_enum_missing,
    lr98_enum_wildcard,
    lr98_enum_otherwise,
    lr98_enum_repeat,
    lr98_enum_guard,
    lr98_optional_exhaustive,
    lr98_optional_missing,
    lr63_while_lt,
    lr63_while_ge
);
fn defect(name: &str) {
    let rows = fortis_testkit::read_cases("types/expression_defects.json");
    let row = rows.iter().find(|r| r.name == name).unwrap();
    check(&row.input, &row.expect);
}

#[test]
fn lr63_condition_parentheses() { defect("lr63_condition_parentheses"); }

#[test]
fn lr63_if() { defect("lr63_if"); }

#[test]
fn lr63_match() { defect("lr63_match"); }

#[test]
fn lr63_range_exclusive() { defect("lr63_range_exclusive"); }

#[test]
fn every_expression_defect_is_registered() {
    let names: Vec<_> = fortis_testkit::read_cases("types/expression_defects.json").into_iter().map(|r| r.name).collect();
    assert_eq!(names, vec!["lr63_condition_parentheses", "lr63_if", "lr63_match", "lr63_range_exclusive"]);
}

mod f1 {
    fortis_testkit::case_tests!("types/checker_fixes.json", super::check;
        f1_parens_0,
        f1_parens_1,
        f1_parens_2,
        f1_parens_3,
        f1_hull_0,
        f1_hull_1,
        f1_hull_2,
        f1_hull_3,
        f1_join_disjoint,
        f1_join_disjoint_reverse,
        f1_join_contained,
        f1_join_contained_reverse,
        f1_join_base,
        f1_join_base_reverse,
        f1_join_wrong_type,
        f1_tuple_binding,
        f1_record_binding,
        f1_tuple_unproved,
        f1_record_unproved,
        f1_optional_unproved,
        f1_tuple_call_bad,
        f1_generic_tuple_bad,
        f1_tuple_pattern_bad,
        f1_tuple_pattern_good
    );
}
