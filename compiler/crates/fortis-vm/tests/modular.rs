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

fortis_testkit::case_tests!("runtime/modular.json", check;
    lr6_4_wrappingadd_0,
    lr6_4_saturatingadd_0,
    lr6_4_wrappingadd_1,
    lr6_4_saturatingadd_1,
    lr6_4_wrappingadd_2,
    lr6_4_saturatingadd_2,
    lr6_4_wrappingadd_3,
    lr6_4_saturatingadd_3,
    lr6_4_wrappingadd_4,
    lr6_4_saturatingadd_4,
    lr6_4_wrappingadd_5,
    lr6_4_saturatingadd_5,
    lr6_4_wrappingadd_6,
    lr6_4_saturatingadd_6,
    lr6_4_wrappingadd_7,
    lr6_4_saturatingadd_7,
    lr6_4_wrappingsub_0,
    lr6_4_saturatingsub_0,
    lr6_4_wrappingsub_1,
    lr6_4_saturatingsub_1,
    lr6_4_wrappingsub_2,
    lr6_4_saturatingsub_2,
    lr6_4_wrappingsub_3,
    lr6_4_saturatingsub_3,
    lr6_4_wrappingsub_4,
    lr6_4_saturatingsub_4,
    lr6_4_wrappingsub_5,
    lr6_4_saturatingsub_5,
    lr6_4_wrappingsub_6,
    lr6_4_saturatingsub_6,
    lr6_4_wrappingsub_7,
    lr6_4_saturatingsub_7,
    lr6_4_wrappingmul_0,
    lr6_4_saturatingmul_0,
    lr6_4_wrappingmul_1,
    lr6_4_saturatingmul_1,
    lr6_4_wrappingmul_2,
    lr6_4_saturatingmul_2,
    lr6_4_wrappingmul_3,
    lr6_4_saturatingmul_3,
    lr6_4_wrappingmul_4,
    lr6_4_saturatingmul_4,
    lr6_4_wrappingmul_5,
    lr6_4_saturatingmul_5,
    lr6_4_wrappingmul_6,
    lr6_4_saturatingmul_6,
    lr6_4_wrappingmul_7,
    lr6_4_saturatingmul_7
);
