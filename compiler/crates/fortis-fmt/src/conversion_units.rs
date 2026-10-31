//! LR 5.6, 6.13 and D-505: the exact minimal parentheses, with an independent expected table.
use super::*;
use fortis_testkit::{forall, Json};
fn check(d: &Json, want: &Json) {
    let context = d.get("context").unwrap().as_str().unwrap();
    let operand = d.get("operand").unwrap().as_str().unwrap();
    let parsed = fortis_syntax::parse_from(context, "expr").unwrap();
    let at = parsed.toks.iter().position(|t| t.text == "old").unwrap();
    let got = minimal_cast(
        context,
        N::new(&parsed.root, &parsed.toks),
        &parsed.toks[at],
        &parsed.toks[at + 2],
        operand,
        "Target",
    );
    assert_eq!(got, want.as_str().unwrap());
    let reference = context.replace("old()", &format!("(({operand}) as Target)"));
    assert_eq!(
        expression_shape(&context.replace("old()", &got)),
        expression_shape(&reference)
    );
}
#[test]
fn lr6_13_generated_identifiers_keep_the_same_tree() {
    forall(
        |r| r.between(0, 100000),
        200,
        None,
        |n| {
            let context = format!("old() + x{n}");
            let parsed = fortis_syntax::parse_from(&context, "expr").unwrap();
            let got = minimal_cast(
                &context,
                N::new(&parsed.root, &parsed.toks),
                &parsed.toks[0],
                &parsed.toks[2],
                &format!("a{n} + b{n}"),
                "Int",
            );
            assert_eq!(got, format!("(a{n} + b{n}) as Int"));
        },
    );
}

fortis_testkit::case_tests!("formatting/grouping.json",check;
    lr6_13_bare_place,
    lr6_13_bare_call,
    lr6_13_bare_negative,
    lr6_13_bare_move,
    lr6_13_bare_cast,
    lr6_13_bare_sum,
    lr6_13_bare_optional,
    lr6_13_bare_lambda,
    lr6_13_field_place,
    lr6_13_field_call,
    lr6_13_field_negative,
    lr6_13_field_move,
    lr6_13_field_cast,
    lr6_13_field_sum,
    lr6_13_field_optional,
    lr6_13_field_lambda,
    lr6_13_call_place,
    lr6_13_call_call,
    lr6_13_call_negative,
    lr6_13_call_move,
    lr6_13_call_cast,
    lr6_13_call_sum,
    lr6_13_call_optional,
    lr6_13_call_lambda,
    lr6_13_index_place,
    lr6_13_index_call,
    lr6_13_index_negative,
    lr6_13_index_move,
    lr6_13_index_cast,
    lr6_13_index_sum,
    lr6_13_index_optional,
    lr6_13_index_lambda,
    lr6_13_negative_place,
    lr6_13_negative_call,
    lr6_13_negative_negative,
    lr6_13_negative_move,
    lr6_13_negative_cast,
    lr6_13_negative_sum,
    lr6_13_negative_optional,
    lr6_13_negative_lambda,
    lr6_13_not_place,
    lr6_13_not_call,
    lr6_13_not_negative,
    lr6_13_not_move,
    lr6_13_not_cast,
    lr6_13_not_sum,
    lr6_13_not_optional,
    lr6_13_not_lambda,
    lr6_13_await_place,
    lr6_13_await_call,
    lr6_13_await_negative,
    lr6_13_await_move,
    lr6_13_await_cast,
    lr6_13_await_sum,
    lr6_13_await_optional,
    lr6_13_await_lambda,
    lr6_13_less_place,
    lr6_13_less_call,
    lr6_13_less_negative,
    lr6_13_less_move,
    lr6_13_less_cast,
    lr6_13_less_sum,
    lr6_13_less_optional,
    lr6_13_less_lambda,
    lr6_13_le_place,
    lr6_13_le_call,
    lr6_13_le_negative,
    lr6_13_le_move,
    lr6_13_le_cast,
    lr6_13_le_sum,
    lr6_13_le_optional,
    lr6_13_le_lambda,
    lr6_13_gt_place,
    lr6_13_gt_call,
    lr6_13_gt_negative,
    lr6_13_gt_move,
    lr6_13_gt_cast,
    lr6_13_gt_sum,
    lr6_13_gt_optional,
    lr6_13_gt_lambda,
    lr6_13_ge_place,
    lr6_13_ge_call,
    lr6_13_ge_negative,
    lr6_13_ge_move,
    lr6_13_ge_cast,
    lr6_13_ge_sum,
    lr6_13_ge_optional,
    lr6_13_ge_lambda,
    lr6_13_add_place,
    lr6_13_add_call,
    lr6_13_add_negative,
    lr6_13_add_move,
    lr6_13_add_cast,
    lr6_13_add_sum,
    lr6_13_add_optional,
    lr6_13_add_lambda,
    lr6_13_mul_place,
    lr6_13_mul_call,
    lr6_13_mul_negative,
    lr6_13_mul_move,
    lr6_13_mul_cast,
    lr6_13_mul_sum,
    lr6_13_mul_optional,
    lr6_13_mul_lambda,
    lr6_13_cast_place,
    lr6_13_cast_call,
    lr6_13_cast_negative,
    lr6_13_cast_move,
    lr6_13_cast_cast,
    lr6_13_cast_sum,
    lr6_13_cast_optional,
    lr6_13_cast_lambda,
    lr6_13_grouped_place,
    lr6_13_grouped_call,
    lr6_13_grouped_negative,
    lr6_13_grouped_move,
    lr6_13_grouped_cast,
    lr6_13_grouped_sum,
    lr6_13_grouped_optional,
    lr6_13_grouped_lambda
);

#[test]
fn d505_unknown_validation_retains_a_diagnostic_note() {
    let cases = fortis_testkit::read_cases("formatting/rewrites.json");
    let row = cases
        .iter()
        .find(|r| r.name == "d505_unknown_table_field")
        .unwrap();
    let source = row.input.as_str().unwrap();
    let (out, count, notes) = rewrite_in(source, Unit::Alone("unit.ft"));
    assert_eq!(out, source);
    assert_eq!(count, 0);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("operand type is not established"));
}
