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

fortis_testkit::case_tests!("types/rc4_r7.json", check;
    plain_binding,
    plain_argument,
    plain_return,
    plain_record,
    plain_class,
    plain_generic,
    plain_generic_argument,
    plain_generic_callee,
    plain_lambda,
    try_binding,
    try_argument,
    try_return,
    try_record,
    try_class,
    try_generic,
    try_generic_argument,
    try_generic_callee,
    try_lambda,
    alias_import,
    qualified_import,
    try_alias_import,
    try_qualified_import
);
