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

fortis_testkit::case_tests!("types/rc3_r6.json", check;
    try_value,
    try_controls,
    nested_outcome_rejected,
    try_failure,
    try_effect_retained,
    try_generic_result,
    async_value,
    async_annotation,
    async_try_annotation,
    async_effect_retained,
    async_nested_rejected,
    explicit_outcome,
    associated_try
);
