//! The same typed runtime shapes used by Python and the VM must lower to safe Rust.
#[path = "../../../../tests/rust/backend_fixture.rs"]
mod fixture;
use fortis_testkit::Json;

fn check(input: &Json, _expect: &Json) {
    let source = input.get("source").unwrap().as_str().unwrap();
    let program = fixture::build(source);
    if let Some(Json::Obj(wrappers)) = input.get("wrapper_returns") {
        for (name, expected) in wrappers {
            let wrapper = program.functions.iter().find(|f| f.name.contains(&format!("intrinsic {name}@"))).unwrap();
            let expected = expected.as_arr().unwrap();
            assert_eq!(wrapper.ret.show(), expected[0].as_str().unwrap());
            assert_eq!(wrapper.has("async"), expected[1].as_bool().unwrap());
            assert!(!wrapper.has("try"), "the adapter forwards the existing outcome");
        }
    }
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

fortis_testkit::case_tests!("runtime/rc3_r6.json", check;
    try_value,
    try_controls,
    async_value,
    array_chain,
    generic_workaround,
    generic_try,
    generic_async
);
