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

fortis_testkit::case_tests!("types/rc1_checkers.json", check;
    optional_must,
    ordinary_must,
    optional_outcome,
    optional_string,
    optional_nested_move,
    optional_generic,
    list_bad,
    list_optional_bad,
    list_good,
    list_optional_null,
    list_generic,
    editable_record,
    editable_optional_field,
    editable_nested_record,
    editable_tuple,
    editable_collection_record,
    editable_same,
    record_read_widen,
    record_owned_widen,
    guard_invalidates,
    guard_reads,
    guard_copy,
    trim_count,
    trim_lends,
    trim_declared_origin,
    count_preserves_other_loan,
    optional_linear_body,
    optional_capture,
    optional_fresh_closure,
    editable_record_field,
    record_view_keeps_loan,
    trim_comparison,
    retired_validation_owned_result,
    editable_collection_element,
    editable_dynamic_field,
    editable_optional_injection,
    editable_plain_callback,
    editable_bad_callback,
    editable_function_field,
    editable_function_kind,
    editable_function_same
);
