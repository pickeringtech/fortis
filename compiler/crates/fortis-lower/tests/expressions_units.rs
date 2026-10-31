//! LR 5.6, 6.4, 6.7 and 10: explicit IR to native Rust text. No Rust compilation or program execution.
use fortis_lower::{
    gen::{FnGen, Want},
    Audit,
};
use fortis_mir::{build::Fb, *};
use fortis_testkit::Json;
use fortis_types::types::{named, named_args, optional};
use std::collections::HashMap;
fn check(d: &Json, expected: &Json) {
    let kind = d.get("kind").unwrap().as_str().unwrap();
    let op = d.get("op").unwrap().as_str().unwrap();
    let tyname = d.get("type").unwrap().as_str().unwrap();
    let ty = match tyname {
        "Optional" => optional(named("Int")),
        "Record" => fortis_types::types::Type::Record(vec![("value".into(), named("Int"))]),
        "Outcome" | "List" => named_args(tyname, vec![named("Int")]),
        _ => named(tyname),
    };
    let args: Vec<_> = d
        .get("args")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .map(|x| {
            Expr::new(
                ExprKind::Const(Const::Int(x.as_i64().unwrap() as i128)),
                if kind == "literal" {
                    ty.clone()
                } else {
                    named("Int")
                },
                1,
            )
        })
        .collect();
    let integer = |n| Expr::new(ExprKind::Const(Const::Int(n)), named("Int"), 1);
    let boolean = |i: usize| {
        Expr::new(
            ExprKind::Const(Const::Bool(
                d.get("args").unwrap().as_arr().unwrap()[i]
                    .as_i64()
                    .unwrap()
                    != 0,
            )),
            named("Bool"),
            1,
        )
    };
    let item = || {
        Expr::new(
            ExprKind::Construct {
                ty: "Item".into(),
                fields: vec![("value".into(), args[0].clone())],
            },
            named("Item"),
            1,
        )
    };
    let outcome = || {
        Expr::new(
            ExprKind::Success(Box::new(args[0].clone())),
            named_args("Outcome", vec![named("Int")]),
            1,
        )
    };
    let ty = if kind == "tuple" {
        fortis_types::types::Type::Tuple(vec![named("Int"); args.len()])
    } else {
        ty
    };
    let k = match kind {
        "neg" => ExprKind::Unary(UnOp::Neg, Box::new(args[0].clone())),
        "not" => ExprKind::Unary(UnOp::Not, Box::new(boolean(0))),
        "and" => ExprKind::And(Box::new(boolean(0)), Box::new(boolean(1))),
        "or" => ExprKind::Or(Box::new(boolean(0)), Box::new(boolean(1))),
        "construct" => item().kind,
        "variant" => ExprKind::Variant {
            ty: "Item".into(),
            variant: "Value".into(),
            fields: vec![("value".into(), args[0].clone())],
        },
        "record" => ExprKind::Record(vec![("value".into(), args[0].clone())]),
        "field" => ExprKind::Field(Box::new(item()), "value".into()),
        "tuple_field" => ExprKind::TupleField(
            Box::new(Expr::new(
                ExprKind::Tuple(args.clone()),
                fortis_types::types::Type::Tuple(vec![named("Int")]),
                1,
            )),
            0,
        ),
        "index" => ExprKind::Index(
            Box::new(Expr::new(
                ExprKind::Call {
                    callee: Callee::Intrinsic("List.of".into()),
                    args: vec![Arg {
                        param: "item".into(),
                        mode: Mode::Owned,
                        value: args[0].clone(),
                    }],
                },
                named_args("List", vec![named("Int")]),
                1,
            )),
            Box::new(integer(0)),
        ),
        "wrap" => ExprKind::Wrap {
            ty: "Int".into(),
            value: Box::new(args[0].clone()),
        },
        "interpolate" => ExprKind::Interpolate(vec![
            Part::Lit("value=".into()),
            Part::Show(args[0].clone()),
        ]),
        "or_fail" => ExprKind::OrFail(Box::new(outcome())),
        "or_recover" => ExprKind::OrRecover(Box::new(outcome()), Box::new(integer(9))),
        "literal" => args[0].kind.clone(),
        "call" => ExprKind::Call {
            callee: Callee::Intrinsic(op.into()),
            args: args
                .into_iter()
                .enumerate()
                .map(|(i, value)| Arg {
                    param: format!("a{i}"),
                    mode: Mode::Read,
                    value,
                })
                .collect(),
        },
        "binary" => {
            let bop = match op {
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                "%" => BinOp::Rem,
                "<<" => BinOp::Shl,
                ">>" => BinOp::Shr,
                "&" => BinOp::BitAnd,
                "|" => BinOp::BitOr,
                "^" => BinOp::BitXor,
                "==" => BinOp::Eq,
                "!=" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                ">=" => BinOp::Ge,
                _ => panic!("operator"),
            };
            ExprKind::Binary(bop, Box::new(args[0].clone()), Box::new(args[1].clone()))
        }
        "tuple" => ExprKind::Tuple(args),
        "present" => ExprKind::Present(Box::new(args[0].clone())),
        "success" => ExprKind::Success(Box::new(args[0].clone())),
        _ => panic!("kind"),
    };
    let p = Program::default();
    let f = Fb::new("function unit", "main", ty.clone(), false).finish(
        1,
        None,
        vec![],
        Effects::default(),
        None,
        None,
    );
    let mut audit = Audit::default();
    let names = HashMap::new();
    let mut g = FnGen::new(&p, &f, &mut audit, &names);
    let text = g.expr(&Expr::new(k, ty, 1), Want::Value);
    assert_eq!(text, expected.as_str().unwrap());
    assert!(g.errors.is_empty(), "{:?}", g.errors);
}

fortis_testkit::case_tests!("native_lower/expressions.json",check;
    lr6_4_checked_add_positive,
    lr6_4_checked_sub_positive,
    lr6_4_checked_mul_positive,
    lr6_4_checked_div_positive,
    lr6_4_checked_rem_positive,
    lr6_4_checked_shiftleft_positive,
    lr6_4_wrapping_add_positive,
    lr6_4_wrapping_sub_positive,
    lr6_4_wrapping_mul_positive,
    lr6_4_saturating_add_positive,
    lr6_4_saturating_sub_positive,
    lr6_4_saturating_mul_positive,
    lr5_6_binary_add_positive,
    lr5_6_binary_sub_positive,
    lr5_6_binary_mul_positive,
    lr5_6_binary_div_positive,
    lr5_6_binary_rem_positive,
    lr5_6_binary_shl_positive,
    lr5_6_binary_shr_positive,
    lr5_6_binary_and_positive,
    lr5_6_binary_or_positive,
    lr5_6_binary_xor_positive,
    lr5_6_binary_eq_positive,
    lr5_6_binary_ne_positive,
    lr5_6_binary_lt_positive,
    lr5_6_binary_le_positive,
    lr5_6_binary_gt_positive,
    lr5_6_binary_ge_positive,
    lr6_4_checked_add_negative,
    lr6_4_checked_sub_negative,
    lr6_4_checked_mul_negative,
    lr6_4_checked_div_negative,
    lr6_4_checked_rem_negative,
    lr6_4_checked_shiftleft_negative,
    lr6_4_wrapping_add_negative,
    lr6_4_wrapping_sub_negative,
    lr6_4_wrapping_mul_negative,
    lr6_4_saturating_add_negative,
    lr6_4_saturating_sub_negative,
    lr6_4_saturating_mul_negative,
    lr5_6_binary_add_negative,
    lr5_6_binary_sub_negative,
    lr5_6_binary_mul_negative,
    lr5_6_binary_div_negative,
    lr5_6_binary_rem_negative,
    lr5_6_binary_shl_negative,
    lr5_6_binary_shr_negative,
    lr5_6_binary_and_negative,
    lr5_6_binary_or_negative,
    lr5_6_binary_xor_negative,
    lr5_6_binary_eq_negative,
    lr5_6_binary_ne_negative,
    lr5_6_binary_lt_negative,
    lr5_6_binary_le_negative,
    lr5_6_binary_gt_negative,
    lr5_6_binary_ge_negative,
    lr6_4_checked_add_zero,
    lr6_4_checked_sub_zero,
    lr6_4_checked_mul_zero,
    lr6_4_checked_div_zero,
    lr6_4_checked_rem_zero,
    lr6_4_checked_shiftleft_zero,
    lr6_4_wrapping_add_zero,
    lr6_4_wrapping_sub_zero,
    lr6_4_wrapping_mul_zero,
    lr6_4_saturating_add_zero,
    lr6_4_saturating_sub_zero,
    lr6_4_saturating_mul_zero,
    lr5_6_binary_add_zero,
    lr5_6_binary_sub_zero,
    lr5_6_binary_mul_zero,
    lr5_6_binary_div_zero,
    lr5_6_binary_rem_zero,
    lr5_6_binary_shl_zero,
    lr5_6_binary_shr_zero,
    lr5_6_binary_and_zero,
    lr5_6_binary_or_zero,
    lr5_6_binary_xor_zero,
    lr5_6_binary_eq_zero,
    lr5_6_binary_ne_zero,
    lr5_6_binary_lt_zero,
    lr5_6_binary_le_zero,
    lr5_6_binary_gt_zero,
    lr5_6_binary_ge_zero,
    lr6_4_literal_int8_0,
    lr6_4_literal_int8_1,
    lr6_4_literal_int8_127,
    lr6_4_literal_int16_0,
    lr6_4_literal_int16_1,
    lr6_4_literal_int16_127,
    lr6_4_literal_int32_0,
    lr6_4_literal_int32_1,
    lr6_4_literal_int32_127,
    lr6_4_literal_int_0,
    lr6_4_literal_int_1,
    lr6_4_literal_int_127,
    lr6_4_literal_uint8_0,
    lr6_4_literal_uint8_1,
    lr6_4_literal_uint8_127,
    lr6_4_literal_uint16_0,
    lr6_4_literal_uint16_1,
    lr6_4_literal_uint16_127,
    lr6_4_literal_uint32_0,
    lr6_4_literal_uint32_1,
    lr6_4_literal_uint32_127,
    lr6_4_literal_uint64_0,
    lr6_4_literal_uint64_1,
    lr6_4_literal_uint64_127,
    lr6_7_list_of_0,
    lr6_7_list_of_1,
    lr6_7_list_of_3,
    lr6_tuple_0,
    lr6_tuple_1,
    lr6_tuple_3,
    lr10_present_neg1,
    lr10_present_0,
    lr10_present_3,
    lr10_success_neg1,
    lr10_success_0,
    lr10_success_3,
    lr_native_neg_0,
    lr_native_construct_0,
    lr_native_variant_0,
    lr_native_record_0,
    lr_native_field_0,
    lr_native_tuple_field_0,
    lr_native_index_0,
    lr_native_wrap_0,
    lr_native_interpolate_0,
    lr_native_or_fail_0,
    lr_native_or_recover_0,
    lr_native_neg_3,
    lr_native_construct_3,
    lr_native_variant_3,
    lr_native_record_3,
    lr_native_field_3,
    lr_native_tuple_field_3,
    lr_native_index_3,
    lr_native_wrap_3,
    lr_native_interpolate_3,
    lr_native_or_fail_3,
    lr_native_or_recover_3,
    lr_native_not_false,
    lr_native_and_false_false,
    lr_native_or_false_false,
    lr_native_and_false_true,
    lr_native_or_false_true,
    lr_native_not_true,
    lr_native_and_true_false,
    lr_native_or_true_false,
    lr_native_and_true_true,
    lr_native_or_true_true
);
