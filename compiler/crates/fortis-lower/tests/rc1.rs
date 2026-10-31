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
    if source.contains("{ n: Int<0..3>? }") {
        assert!(
            lowered.main_rs.contains("n: Some(1i64)"),
            "{}",
            lowered.main_rs
        );
    }
    if source.contains("(Int?, Int?)") {
        assert!(
            lowered.main_rs.contains("(Some(1i64), None)"),
            "{}",
            lowered.main_rs
        );
    }
    if source.contains("Array[Int, 2].filled\n") {
        assert!(
            !lowered.main_rs.contains("__ft_54"),
            "unresolved T: {}",
            lowered.main_rs
        );
    }
}

fortis_testkit::case_tests!("runtime/rc1.json", check;
    array_expected, array_bound, list_write, editable_record, module_import, optional_record,
    array_return_parameter, optional_tuple, optional_class, module_alias, optional_list_write, optional_collection_get
);
