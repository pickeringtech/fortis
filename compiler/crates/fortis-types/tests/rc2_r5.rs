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
    if let Some(at) = expect.get("at").and_then(Json::as_arr) {
        assert!(records.iter().any(|r| Json::Int(r.line as i64) == at[0] && Json::Int(r.column as i64) == at[1] && r.message.contains("an argument of `?.`")), "{records:?}");
    }
    if let Some(fields) = expect.get("fields") {
        assert_eq!(Json::Obj(records[0].fields.clone()), *fields);
    }
    if let Some(message) = expect.get("message").and_then(Json::as_str) {
        assert!(records.iter().any(|r| r.message.contains(message)), "{records:?}");
    }
}

fortis_testkit::case_tests!("types/rc2_r5.json", check;
    optional_fail,
    optional_recover,
    optional_return,
    optional_nested,
    optional_match,
    optional_discard,
    optional_chain,
    optional_must_outcome,
    optional_pass_on,
    fresh_outcome,
    fresh_local,
    fresh_must,
    read_record_write,
    optional_nested_branch,
    fresh_nested_optional_rejected
);
