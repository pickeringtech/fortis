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

fortis_testkit::case_tests!("runtime/rc1.json", check;
    array_expected, array_bound, list_write, editable_record, module_import, optional_record, array_return_parameter, optional_tuple, optional_class, module_alias, optional_list_write, optional_collection_get
);

#[test]
fn conversion_failure_keeps_executing_function() {
    use fortis_vm::{intrinsics, machine::Vm, value::Value};
    let program = fortis_mir::Program::default();
    let mut vm = Vm::new(&program, "rc1.ft");
    vm.stack.push("sample".into());
    let result = intrinsics::call(
        &mut vm,
        "as:parse_int:StringView:Int",
        vec![Value::str("1x")],
    )
    .unwrap();
    let Value::Failure(f) = result else {
        panic!("expected conversion failure")
    };
    let f = f.borrow();
    assert_eq!(&*f.code, "conversion.parse");
    assert_eq!(&*f.origin, "sample");
    assert_eq!(f.payload.len(), 1);
    assert_eq!(&*f.payload[0].0, "text");
    assert!(matches!(&f.payload[0].1, Value::Str(text) if &**text == "1x"));
}

#[test]
fn source_labels_hide_mixin_application_suffixes() {
    let mut program = fixture::build("func main() {}\n");
    let function = program.functions.first_mut().unwrap();
    function.label = "lambda@8:12#Box".into();
    assert_eq!(fortis_vm::machine::Vm::source_name_of(function), "lambda@8:12");
}
