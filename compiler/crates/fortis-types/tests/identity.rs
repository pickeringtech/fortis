use fortis_testkit::Json;
use fortis_types::{
    literals::{assignable, origin_clash},
    types::{core_name, NamedT, Type},
};

fn text(data: &Json, key: &str) -> String {
    data.get(key).and_then(Json::as_str).unwrap_or("").into()
}

fn typ(data: &Json) -> Type {
    if data.get("name").is_some() {
        return Type::Named(NamedT {
            name: text(data, "name"),
            args: data
                .get("args")
                .and_then(Json::as_arr)
                .map(|xs| xs.iter().map(typ).collect())
                .unwrap_or_default(),
            package: text(data, "package"),
            origin: text(data, "origin"),
        });
    }
    if let Some(x) = data.get("optional") {
        return Type::Optional(Box::new(typ(x)));
    }
    if let Some(x) = data.get("tuple") {
        return Type::Tuple(x.as_arr().unwrap().iter().map(typ).collect());
    }
    if let Some(x) = data.get("record") {
        return Type::Record(
            x.as_arr()
                .unwrap()
                .iter()
                .map(|row| {
                    let pair = row.as_arr().unwrap();
                    (pair[0].as_str().unwrap().into(), typ(&pair[1]))
                })
                .collect(),
        );
    }
    if let Some(x) = data.get("fn") {
        let pair = x.as_arr().unwrap();
        return Type::Fn {
            capture_origins: Vec::new(),
            params: pair[0].as_arr().unwrap().iter().map(typ).collect(),
            ret: Box::new(typ(&pair[1])),
            kind: "fn".into(),
            modifiers: vec![],
            origin: String::new(),
            dynamic: false,
            identity: String::new(),
            param_modes: vec![],
        };
    }
    if let Some(x) = data.get("loan") {
        return Type::Loan {
            inner: Box::new(typ(x)),
            mode: "alias".into(),
            origin: "owner".into(),
        };
    }
    if let Some(x) = data.get("dynamic") {
        return Type::Dynamic(typ(x).named_ref().unwrap().clone());
    }
    if let Some(x) = data.get("refined") {
        return Type::Refined {
            base: typ(x).named_ref().unwrap().clone(),
            lo: Some(0),
            hi: Some(10),
            kind: "range".into(),
        };
    }
    panic!("invalid type case: {data:?}");
}

fn check(input: &Json, expect: &Json) {
    let a = typ(input.get("a").unwrap());
    match input.get("op").unwrap().as_str().unwrap() {
        "core" => assert_eq!(
            &core_name(&a)
                .map(|s| Json::Str(s.into()))
                .unwrap_or(Json::Null),
            expect
        ),
        "interface" => {
            let b = typ(input.get("b").unwrap());
            assert_eq!(
                &Json::Bool(
                    a.named_ref().unwrap().interface_key()
                        == b.named_ref().unwrap().interface_key()
                ),
                expect
            );
        }
        "assignable" => {
            let b = typ(input.get("b").unwrap());
            for (have, want) in [(&a, &b), (&b, &a)] {
                assert_eq!(
                    &Json::Bool(!origin_clash(have, want).is_empty()),
                    expect.get("clash").unwrap()
                );
                assert_eq!(
                    &assignable(have, want)
                        .0
                        .map(Json::Bool)
                        .unwrap_or(Json::Null),
                    expect.get("assignable").unwrap()
                );
            }
        }
        op => panic!("unknown operation: {op}"),
    }
}

fortis_testkit::case_tests!("identity/nominal.json", check;
    same_local_declaration,
    same_core_declaration,
    local_and_prelude,
    different_modules,
    different_versions,
    core_module_origin,
    list_key_identity,
    list_same_identity,
    map_key_identity,
    map_same_identity,
    set_key_identity,
    set_same_identity,
    deque_key_identity,
    deque_same_identity,
    sortedset_key_identity,
    sortedset_same_identity,
    outcome_key_identity,
    outcome_same_identity,
    box_key_identity,
    box_same_identity,
    optional_identity,
    optional_same_identity,
    tuple_identity,
    tuple_same_identity,
    record_identity,
    record_same_identity,
    function_parameter_identity,
    function_parameter_same_identity,
    function_result_identity,
    function_result_same_identity,
    loan_identity,
    loan_same_identity,
    dynamic_identity,
    dynamic_same_identity,
    refined_identity,
    refined_same_identity,
    deep_nesting_identity,
    deep_nesting_same_identity,
    hashable_bound_identity,
    hashable_same_bound,
    equatable_bound_identity,
    equatable_same_bound,
    interface_module_identity,
    interface_version_identity,
    core_bool,
    shadow_bool,
    core_int,
    shadow_int,
    core_string,
    shadow_string,
    core_outcome,
    shadow_outcome,
    core_list,
    shadow_list
);
