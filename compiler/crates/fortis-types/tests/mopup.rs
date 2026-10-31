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

fortis_testkit::case_tests!("mopup/checker.json", check;
    void_constructor,
    void_callback,
    void_block_callback,
    program_void_constructor,
    borrowed_string_literal,
    borrowed_string_view,
    borrowed_string_string,
    borrowed_stringview_literal,
    borrowed_stringview_view,
    borrowed_stringview_string,
    owned_string_literal,
    owned_string_view,
    owned_string_alias_view,
    owned_string_requires_move,
    owned_string_move,
    owned_string_copy_allocates,
    text_pair_overlap,
    derived_text_pair_overlap,
    program_string_distinct,
    owned_view_from_string,
    owned_view_alias_string,
    owned_view_literal,
    qualified_diagnostic_record,
    qualified_condition,
    qualified_boolean_operand,
    borrowed_text_noalloc,
    qualified_void_constructor,
    qualified_conversion,
    void_value_callback,
    qualified_key,
    unambiguous_derive,
    qualified_bound,
    qualified_index,
    qualified_range,
    qualified_bare_return,
    qualified_report,
    qualified_equatable,
    qualified_comparable,
    qualified_method_parameter,
    qualified_method_result,
    qualified_dynamic_bound,
    qualified_interface_bound
);
