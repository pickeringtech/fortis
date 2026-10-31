//! LR 6.4: exact checked results and failure payloads.
use fortis_native::{
    failure::{Dyn, Failure},
    numeric::*,
};
use fortis_testkit::Json;
fn failure(f: Failure) -> Json {
    Json::obj([
        ("code", Json::str(f.code)),
        (
            "payload",
            Json::obj(f.payload.into_iter().map(|(k, v)| {
                (
                    k,
                    match v {
                        Dyn::Int(n) => Json::Int(n as i64),
                        Dyn::Str(s) => Json::str(s),
                        _ => panic!("unexpected payload"),
                    },
                )
            })),
        ),
    ])
}
fn check(d: &Json, expect: &Json) {
    let a = d.get("a").unwrap().as_i64().unwrap();
    let b = d.get("b").unwrap().as_i64().unwrap();
    let result = match d.get("op").unwrap().as_str().unwrap() {
        "add" => int_add(a, b),
        "sub" => int_sub(a, b),
        "mul" => int_mul(a, b),
        "div" => int_div(a, b),
        "rem" => int_rem(a, b),
        "shiftLeft" => int_shift_left(a, b),
        op => panic!("unknown {op}"),
    };
    assert_eq!(&result.map(Json::Int).unwrap_or_else(failure), expect);
}

fortis_testkit::case_tests!("runtime/integers.json", check;
    lr6_4_add_0,
    lr6_4_add_1,
    lr6_4_add_2,
    lr6_4_add_3,
    lr6_4_add_4,
    lr6_4_add_5,
    lr6_4_add_6,
    lr6_4_add_7,
    lr6_4_sub_0,
    lr6_4_sub_1,
    lr6_4_sub_2,
    lr6_4_sub_3,
    lr6_4_sub_4,
    lr6_4_sub_5,
    lr6_4_sub_6,
    lr6_4_sub_7,
    lr6_4_mul_0,
    lr6_4_mul_1,
    lr6_4_mul_2,
    lr6_4_mul_3,
    lr6_4_mul_4,
    lr6_4_mul_5,
    lr6_4_mul_6,
    lr6_4_mul_7,
    lr6_4_div_pp,
    lr6_4_div_np,
    lr6_4_div_pn,
    lr6_4_div_nn,
    lr6_4_div_zero,
    lr6_4_div_zero_divisor,
    lr6_4_div_min_neg_one,
    lr6_4_div_large,
    lr6_4_rem_pp,
    lr6_4_rem_np,
    lr6_4_rem_pn,
    lr6_4_rem_nn,
    lr6_4_rem_zero,
    lr6_4_rem_zero_divisor,
    lr6_4_rem_min_neg_one,
    lr6_4_rem_large,
    lr6_4_shift_zero,
    lr6_4_shift_sign,
    lr6_4_shift_overflow,
    lr6_4_shift_negative,
    lr6_4_shift_width,
    lr6_4_shift_zero_large
);
