//! The same typed runtime shapes used by Python and the VM must lower to safe Rust.
#[path = "../../../../tests/rust/backend_fixture.rs"]
mod fixture;
use fortis_testkit::Json;

fn check(input: &Json, _expect: &Json) {
    let source = input.get("source").unwrap().as_str().unwrap();
    let program = fixture::build(source);
    let options = fortis_lower::Options {
        runtime: "../fortis-native".into(),
        crate_name: "rc1".into(),
        file: "rc1.ft".into(),
        decl_files: vec![],
        tests: false,
    };
    let lowered = fortis_lower::lower(&program, &options)
        .unwrap_or_else(|_| panic!("failed to lower {source}"));
    assert!(lowered.main_rs.contains("#![forbid(unsafe_code)]"));
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
