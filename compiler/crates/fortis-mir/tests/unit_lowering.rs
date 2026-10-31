//! LR construct lowering from one typed body. No compiler driver, linking or program execution.
#[path = "../../../../tests/rust/backend_fixture.rs"]
mod fixture;
use fortis_mir::*;
use fortis_testkit::Json;
fn expr_tag(e: &Expr) -> &'static str {
    match &e.kind {
        ExprKind::Temp { value, .. } => expr_tag(value),
        ExprKind::Const(_) => "const",
        ExprKind::Binary(..) => "binary",
        ExprKind::Unary(..) => "unary",
        ExprKind::And(..) => "and",
        ExprKind::Or(..) => "or",
        ExprKind::Tuple(..) => "tuple",
        ExprKind::TupleField(..) => "tuple_field",
        ExprKind::Record(..) => "record",
        ExprKind::Field(..) => "field",
        ExprKind::Present(..) => "present",
        ExprKind::Coalesce(..) => "coalesce",
        ExprKind::Call { .. } => "call",
        ExprKind::Index(..) => "index",
        ExprKind::Interpolate(..) => "interpolate",
        ExprKind::If { .. } => "if",
        ExprKind::Match { .. } => "match",
        ExprKind::OrRecover(..) => "or_recover",
        ExprKind::Catch { .. } => "catch",
        _ => "other",
    }
}
fn stmt_tag(s: &Stmt) -> &'static str {
    match &s.kind {
        StmtKind::Let { .. } => "let",
        StmtKind::Assign { .. } => "assign",
        StmtKind::Compound { .. } => "compound",
        StmtKind::If { .. } => "if",
        StmtKind::Loop { .. } => "loop",
        StmtKind::For { .. } => "for",
        StmtKind::LetPattern { .. } => "let_pattern",
        StmtKind::Discard(..) => "discard",
        StmtKind::Defer(..) => "defer",
        StmtKind::Block(..) => "block",
        _ => "other",
    }
}
fn check(d: &Json, expect: &Json) {
    let p = fixture::build(&fixture::source(d));
    let f = p
        .functions
        .iter()
        .find(|f| f.label == "function value")
        .unwrap();
    let b = f.body.as_ref().unwrap();
    let want = expect.get("ir").unwrap().as_str().unwrap();
    if d.get("kind").unwrap().as_str() == Some("expr") {
        let StmtKind::Return(Some(e)) = &b.stmts[0].kind else {
            panic!("expression body must return")
        };
        assert_eq!(expr_tag(e), want, "{}", fixture::source(d));
    } else {
        assert!(
            b.stmts.iter().any(|s| stmt_tag(s) == want),
            "missing {want}: {:?}",
            b.stmts
        );
    }
}

fortis_testkit::case_tests!("evaluation/fragments.json",check;
    lr3_5_zero,
    lr3_5_max,
    lr3_5_hex,
    lr3_5_binary,
    lr3_5_separated,
    lr3_5_true,
    lr3_5_false,
    lr3_5_text,
    lr3_5_escaped,
    lr3_5_empty_text,
    lr3_5_decimal,
    lr3_5_duration,
    lr3_5_null,
    lr3_5_float,
    lr5_6_neg,
    lr5_6_not_true,
    lr5_6_not_false,
    lr5_6_double_neg,
    lr5_6_add,
    lr5_6_sub,
    lr5_6_mul,
    lr5_6_div,
    lr5_6_rem,
    lr5_6_shl,
    lr5_6_shr,
    lr5_6_and,
    lr5_6_or,
    lr5_6_xor,
    lr5_6_eq,
    lr5_6_ne,
    lr5_6_lt,
    lr5_6_le,
    lr5_6_gt,
    lr5_6_ge,
    lr5_6_precedence,
    lr5_7_and_0,
    lr5_7_or_0,
    lr5_7_and_1,
    lr5_7_or_1,
    lr5_7_and_2,
    lr5_7_or_2,
    lr5_7_and_3,
    lr5_7_or_3,
    lr6_tuple,
    lr6_tuple_field,
    lr6_record,
    lr6_record_field,
    lr6_coalesce_null,
    lr6_coalesce_value,
    lr6_present,
    lr6_list,
    lr6_index,
    lr6_interpolation,
    lr5_2_true,
    lr5_2_false,
    lr5_2_nested,
    lr9_8_match_literal,
    lr9_8_match_range,
    lr10_2_success,
    lr10_2_overflow,
    lr10_2_division_zero,
    lr10_2_checked_region,
    lr6_13_total_decimal,
    lr6_13_parse_ok,
    lr6_13_parse_failure,
    lr10_4_catch_recover,
    lr5_binding,
    lr5_mutable,
    lr5_compound,
    lr5_if_then,
    lr5_if_else,
    lr5_while_skip,
    lr5_while_once,
    lr5_loop_break,
    lr5_range_inclusive,
    lr5_range_exclusive,
    lr5_range_empty,
    lr5_for_list,
    lr5_for_continue,
    lr5_for_break,
    lr5_destructure,
    lr5_discard,
    lr5_defer,
    lr5_nested_block
);

#[test]
fn lr6_3_refined_list_index_d656() {
    let rows = fortis_testkit::read_cases("evaluation/known_limits.json");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "lr6_3_refined_list_index_d656");
    check(&rows[0].input, &rows[0].expect);
}
