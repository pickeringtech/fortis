use super::*;
use fortis_types::types::{named, named_args};

fn closure(region: &str, dynamic: bool) -> Type {
    Type::Fn {
        params: vec![], ret: Box::new(named("Int")), kind: "fn".into(), modifiers: vec![],
        origin: region.into(), dynamic, identity: String::new(), param_modes: vec![], capture_origins: vec![],
    }
}

#[test]
fn returned_closure_holders_keep_a_concrete_environment() {
    let prog = Program::default();
    let f = closure("", false);
    let cases = [
        (named_args("List", vec![f.clone()]), "Vec<impl Fn() -> i64>"),
        (named_args("Outcome", vec![f.clone()]), "rt::Outcome<impl Fn() -> i64>"),
        (Type::Optional(Box::new(f.clone())), "Option<impl Fn() -> i64>"),
        (Type::Tuple(vec![f, named("Int")]), "(impl Fn() -> i64, i64)"),
    ];
    for (ty, expected) in cases {
        assert_eq!(result_ty(&ty, &prog), expected);
    }
}

#[test]
fn explicit_dynamic_holder_still_uses_a_box() {
    assert_eq!(result_ty(&named_args("List", vec![closure("", true)]), &Program::default()),
               "Vec<::std::boxed::Box<dyn Fn() -> i64>>");
}

fn forwarding(region: &str) -> Function {
    let f = closure(region, false);
    Function {
        name: "main::function lend".into(), label: "function lend".into(), module: "main".into(),
        body_module: None, line: 1, modifiers: vec![], body: None, effects: Effects::default(),
        observe: None, closure: None, bounds: vec![], ret: f.clone(),
        params: vec![Param { local: 0, name: "s".into(), mode: Mode::Read, default: None },
                     Param { local: 1, name: "f".into(), mode: Mode::Owned, default: None }],
        locals: vec![LocalDecl { name: "s".into(), ty: named("String"), mutable: false, kind: LocalKind::Param, boxed: false },
                     LocalDecl { name: "f".into(), ty: f, mutable: false, kind: LocalKind::Param, boxed: false }],
    }
}

#[test]
fn owned_callback_outlives_the_returned_region() {
    assert_eq!(signature(&Program::default(), &forwarding("s"), "lend", false, false),
               "fn lend<'s>(s: &'s str, f: impl Fn() -> i64 + 's) -> impl Fn() -> i64 + 's");
}

#[test]
fn static_callback_uses_the_rust_static_lifetime() {
    assert!(signature(&Program::default(), &forwarding("static"), "lend", false, false)
        .contains("f: impl Fn() -> i64 + 'static"));
}
