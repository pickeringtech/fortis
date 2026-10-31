//! LR 8.4, 12.3: async status is invariant (D-749).
use fortis_testkit::Json;
use fortis_types::types::{Type, named, named_args};

fn callable(modifiers: Vec<String>, ret: Type, params: Vec<Type>) -> Type {
    Type::Fn { params, ret: Box::new(ret), kind: "fn".into(), modifiers,
        origin: String::new(), dynamic: false, identity: String::new(),
        param_modes: vec![], capture_origins: vec![] }
}
fn typ(mods: &Json, wrap: &str) -> Type {
    let modifiers = mods.as_arr().unwrap().iter().map(|m| m.as_str().unwrap().to_string()).collect();
    let f = callable(modifiers, named("Int"), vec![named("Int")]);
    match wrap {
        "fn" => f,
        "optional" => Type::Optional(Box::new(f)),
        "tuple" => Type::Tuple(vec![f]),
        "record" => Type::Record(vec![("work".into(), f)]),
        "generic" => named_args("Holder", vec![f]),
        "result" => callable(vec![], f, vec![]),
        _ => panic!("unknown wrapper"),
    }
}
fn check(input: &Json, expect: &Json) {
    let have = input.get("have").unwrap();
    let want = input.get("want").unwrap();
    let wrap = input.get("wrap").unwrap().as_str().unwrap();
    let (got, why) = fortis_types::literals::assignable(&typ(have, wrap), &typ(want, wrap));
    assert_eq!(got.map(Json::Bool).unwrap_or(Json::Null), *expect);
    let is_async = |m: &Json| m.as_arr().unwrap().iter().any(|v| v.as_str() == Some("async"));
    if is_async(want) && !is_async(have) {
        assert!(why.contains("declare the function `async`, or wrap the call in an async lambda"), "{why}");
    }
}
fortis_testkit::case_tests!("types/async_assignment.json", check;
    plain_toplain,
    plain_totry,
    plain_toasync,
    plain_toasync_try,
    try_toplain,
    try_totry,
    try_toasync,
    try_toasync_try,
    async_toplain,
    async_totry,
    async_toasync,
    async_toasync_try,
    async_try_toplain,
    async_try_totry,
    async_try_toasync,
    async_try_toasync_try,
    optional_plain_to_async,
    optional_try_to_async_try,
    optional_async_to_async,
    optional_async_try_to_async_try,
    tuple_plain_to_async,
    tuple_try_to_async_try,
    tuple_async_to_async,
    tuple_async_try_to_async_try,
    record_plain_to_async,
    record_try_to_async_try,
    record_async_to_async,
    record_async_try_to_async_try,
    generic_plain_to_async,
    generic_try_to_async_try,
    generic_async_to_async,
    generic_async_try_to_async_try,
    result_plain_to_async,
    result_try_to_async_try,
    result_async_to_async,
    result_async_try_to_async_try
);
