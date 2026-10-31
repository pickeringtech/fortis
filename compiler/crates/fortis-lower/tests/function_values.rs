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

fortis_testkit::case_tests!("runtime/function_values.json", check;
    alias,
    array,
    array_lambda,
    braced,
    class,
    constructors,
    enum_constructor,
    free,
    generic,
    method,
    mode,
    module,
    newtype_method,
    constrained,
    owned_intrinsic,
    default,
    interface_methods,
    context
);
