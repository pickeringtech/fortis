//! LR 3, 5, 6, 9, 10 and 12: execute a single body directly, never a program entry point.
#[path = "../../../../tests/rust/backend_fixture.rs"]
mod fixture;
use fortis_testkit::Json;
use fortis_vm::{
    machine::{Exit, Flow, Frame, Vm},
    value::Value,
};
fn normal(v: &Value) -> Json {
    match v {
        Value::Void | Value::Null => Json::Null,
        Value::Int(n) => Json::Int(*n as i64),
        Value::Bool(b) => Json::Bool(*b),
        Value::Str(s) => Json::str(&**s),
        Value::List(xs) | Value::Array(xs) => Json::Arr(xs.borrow().iter().map(normal).collect()),
        Value::Tuple(xs) => Json::Arr(xs.iter().map(normal).collect()),
        Value::Record(r) => Json::obj(r.borrow().iter().map(|(k, v)| (&**k, normal(v)))),
        Value::Dec(d) => Json::obj([("decimal", Json::str(d.to_string()))]),
        Value::Dur(n) => Json::obj([("duration", Json::Int(*n))]),
        Value::Float(f) => Json::obj([("float", Json::str(f.to_string()))]),
        Value::Bytes(b) => Json::obj([(
            "bytes",
            Json::Arr(b.iter().map(|n| Json::Int(*n as i64)).collect()),
        )]),
        _ => panic!("unsupported result {v:?}"),
    }
}
fn check(d: &Json, expect: &Json) {
    let p = fixture::build(&fixture::source(d));
    let i = p
        .functions
        .iter()
        .position(|f| f.label == "function value")
        .unwrap();
    let f = &p.functions[i];
    let mut frame = Frame {
        locals: vec![Value::Void; f.locals.len()],
        func: i,
        consumed: None,
        lent: Default::default(),
        owned: Default::default(),
        ret_lent: true,
    };
    fn value(j: &Json) -> Value {
        match j {
            Json::Null => Value::Null,
            Json::Int(n) => Value::Int(*n as i128),
            Json::Arr(xs) => Value::list(xs.iter().map(value).collect()),
            _ => panic!("parameter value"),
        }
    }
    for (param, input) in f
        .params
        .iter()
        .zip(d.get("params").unwrap().as_arr().unwrap())
    {
        frame.locals[param.local as usize] = value(input.get("value").unwrap());
    }
    let mut vm = Vm::new(&p, "unit");
    match vm.block(&mut frame, f.body.as_ref().unwrap()) {
        Err(Exit::Flow(Flow::Return(value))) => {
            assert_eq!(&normal(&value), expect.get("value").unwrap())
        }
        other => panic!("expected return, got {other:?}"),
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
