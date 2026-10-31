//! LR 6.4: call an intrinsic directly, with no program, checker or executor.
use fortis_mir::model::Program;
use fortis_testkit::Json;
use fortis_vm::{intrinsics, machine::Vm, value::Value};
fn normal(v: Value) -> Json {
    match v {
        Value::Int(n) => Json::Int(n as i64),
        Value::Str(s) => Json::str(&*s),
        Value::Failure(f) => {
            let f = f.borrow();
            Json::obj([
                ("code", Json::str(&*f.code)),
                (
                    "payload",
                    Json::obj(f.payload.iter().map(|(k, v)| (&**k, normal(v.clone())))),
                ),
            ])
        }
        _ => panic!("unexpected value"),
    }
}
fn check(d: &Json, expect: &Json) {
    let prog = Program::default();
    let mut vm = Vm::new(&prog, "unit");
    let args = ["a", "b"]
        .iter()
        .map(|k| Value::Int(d.get(k).unwrap().as_i64().unwrap() as i128))
        .collect();
    let name = format!("Int.{}", d.get("op").unwrap().as_str().unwrap());
    let actual = intrinsics::call(&mut vm, &name, args).unwrap();
    assert_eq!(&normal(actual), expect);
}
#[test]
fn lr6_4_wrapping_add_sub_inverse() {
    fortis_testkit::forall(
        |r| (r.between(i64::MIN, i64::MAX), r.between(-100, 100)),
        200,
        None,
        |&(a, b)| {
            let prog = Program::default();
            let mut vm = Vm::new(&prog, "unit");
            let sum = intrinsics::call(
                &mut vm,
                "Int.wrappingAdd",
                vec![Value::Int(a as i128), Value::Int(b as i128)],
            )
            .unwrap();
            let back =
                intrinsics::call(&mut vm, "Int.wrappingSub", vec![sum, Value::Int(b as i128)])
                    .unwrap();
            assert_eq!(back.as_int(), Some(a as i128));
        },
    );
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
