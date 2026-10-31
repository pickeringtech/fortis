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

fortis_testkit::case_tests!("runtime/rc3_r6.json", check;
    try_value,
    try_controls,
    async_value,
    array_chain,
    generic_workaround,
    generic_try,
    generic_async
);
