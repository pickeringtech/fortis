use fortis_testkit::Json;

fn check(input: &Json, expect: &Json) {
    let source = input.get("source").unwrap().as_str().unwrap();
    let parsed = fortis_syntax::parse_module(source).unwrap();
    let records = fortis_types::check::check_parsed(&parsed, "mopup.ft");
    let mut actual: Vec<_> = records.iter().map(|r| r.code.as_str()).collect();
    actual.sort(); actual.dedup();
    let mut wanted: Vec<_> = expect.get("codes").unwrap().as_arr().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    wanted.sort();
    assert_eq!(actual, wanted, "{records:?}");
    if let Some(fields) = expect.get("fields") {
        assert_eq!(Json::Obj(records[0].fields.clone()), *fields);
    }
    if let Some(message) = expect.get("message").and_then(Json::as_str) {
        assert!(records.iter().any(|r| r.message.contains(message)), "{records:?}");
    }
}

fortis_testkit::case_tests!("types/function_values.json", check;
    alias_good,
    alias_bad,
    module_good,
    module_bad,
    braced_good,
    braced_bad,
    array_good,
    class_good,
    array_bad,
    class_bad,
    array_unspecialised,
    class_unspecialised,
    generic_free,
    generic_member,
    free_where,
    newtype_method,
    class_method,
    class_constructor,
    newtype_constructor,
    enum_choice_some_good,
    enum_choice_some_bad,
    enum_choice_int_some_good,
    enum_choice_int_some_bad,
    enum_some_good,
    enum_some_bad,
    enum_untyped,
    free_good,
    free_bad,
    mode_bad,
    mode_good,
    local_shadow,
    module_shadow,
    marked_method,
    marked_newtype_method,
    generic_default,
    owner_self,
    owned_intrinsic_mode,
    local_closure,
    marked_module_function,
    enum_incomplete,
    dynamic_method,
    parameter_method,
    generic_context,
    owner_context,
    generic_storage,
    marked_main,
    marked_named,
    marked_module_good
);
