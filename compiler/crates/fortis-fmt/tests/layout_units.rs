//! LR 3.2: layout preserves the parse tree, and formatting is idempotent.
use fortis_testkit::{forall, Json};
fn shape(s: &str) -> String {
    let p = fortis_syntax::parse_module(s).unwrap();
    fortis_fmt::shape(&p.root, &p.toks)
}
fn check(d: &Json, e: &Json) {
    let src = d.as_str().unwrap();
    let out = fortis_fmt::format(src).unwrap();
    assert_eq!(out, e.as_str().unwrap());
    assert_eq!(shape(src), shape(&out));
    assert_eq!(fortis_fmt::format(&out).unwrap(), out);
}
#[test]
fn lr3_2_indentation_property() {
    forall(
        |r| (r.between(0, 24), r.between(-99, 99)),
        200,
        None,
        |(indent, n)| {
            let src = format!(
                "{}func value() Int {{\n{}return {n}\n}}\n",
                " ".repeat(*indent as usize),
                " ".repeat((24 - *indent) as usize)
            );
            let once = fortis_fmt::format(&src).unwrap();
            assert_eq!(fortis_fmt::format(&once).unwrap(), once);
            assert_eq!(shape(&src), shape(&once));
        },
    );
}

fortis_testkit::case_tests!("formatting/layout.json",check;
    lr3_2_layout_0,
    lr3_2_layout_1,
    lr3_2_layout_2,
    lr3_2_layout_3,
    lr3_2_layout_4,
    lr3_2_layout_5,
    lr3_2_layout_6,
    lr3_2_layout_7,
    lr3_2_layout_8,
    lr3_2_layout_9,
    lr3_2_layout_10,
    lr3_2_layout_11,
    lr3_2_layout_12,
    lr3_2_layout_13,
    lr3_2_layout_14,
    lr3_2_layout_15,
    lr3_2_layout_16,
    lr3_2_layout_17,
    lr3_2_layout_18,
    lr3_2_layout_19,
    lr3_2_layout_20,
    lr3_2_layout_21,
    lr3_2_layout_22,
    lr3_2_layout_23,
    lr3_2_layout_24,
    lr3_2_layout_25,
    lr3_2_layout_26,
    lr3_2_layout_27,
    lr3_2_layout_28,
    lr3_2_layout_29,
    lr3_2_layout_30,
    lr3_2_layout_31,
    lr3_2_layout_32,
    lr3_2_layout_33,
    lr3_2_layout_34,
    lr3_2_layout_35,
    lr3_2_layout_36,
    lr3_2_layout_37,
    lr3_2_layout_38,
    lr3_2_layout_39,
    lr3_2_gap_add,
    lr3_2_gap_sub,
    lr3_2_gap_mul,
    lr3_2_gap_div,
    lr3_2_gap_rem,
    lr3_2_gap_shl,
    lr3_2_gap_shr,
    lr3_2_gap_bitand,
    lr3_2_gap_bitor,
    lr3_2_gap_xor,
    lr3_2_gap_eq,
    lr3_2_gap_ne,
    lr3_2_gap_lt,
    lr3_2_gap_le,
    lr3_2_gap_gt,
    lr3_2_gap_ge,
    lr3_2_gap_neg,
    lr3_2_gap_not,
    lr3_2_gap_and,
    lr3_2_gap_or
);
