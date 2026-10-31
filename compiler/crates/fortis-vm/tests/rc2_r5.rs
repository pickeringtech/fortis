//! Shared runtime regressions: type facts reach MIR and survive VM execution.
#[path = "../../../../tests/rust/backend_fixture.rs"]
mod fixture;
use fortis_testkit::Json;

fn check(input: &Json, expect: &Json) {
    let program = fixture::build(input.get("source").unwrap().as_str().unwrap());
    let observed = fortis_vm::run_main(&program, "rc1.ft", &[], &Default::default());
    assert_eq!(observed.exit_code, 0, "{}", observed.text);
    assert_eq!(Json::strs(observed.out), *expect);
}

fortis_testkit::case_tests!("runtime/rc2_r5.json", check;
    array_refined,
    array_nested,
    array_generic,
    array_decimal,
    array_hex,
    array_binary,
    array_octal,
    array_underscores,
    array_hex_underscores,
    array_binary_underscores,
    array_octal_underscores,
    record_shapes,
    record_nested,
    record_order,
    editable_record_local,
    editable_nested_reborrow
);
