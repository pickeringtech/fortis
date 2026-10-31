use fortis_mir::{build, build_source, Callee, ExprKind, Mode, StmtKind};

#[test]
fn borrowed_text_is_copied_before_entering_an_owned_member() {
    std::thread::Builder::new().stack_size(32 * 1024 * 1024).spawn(check_copy).unwrap().join().unwrap();
}

fn check_copy() {
    let source = r#"class Text {
  value: String
  convert from String(owned source) { return Text(value: move source) }
}
func f(source: StringView) Text { return source as Text }
"#;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let program = build_source(source, "mopup.ft", &build::LibIndex::load(&root)).unwrap();
    assert_eq!(program.functions.iter().filter(|f| f.name.contains("convert:from:String")).count(), 1);
    let f = program.functions.iter().find(|f| f.name == "main::function f").unwrap();
    let StmtKind::Return(Some(value)) = &f.body.as_ref().unwrap().stmts[0].kind else { panic!("{:?}", f.body) };
    let ExprKind::Call { args, .. } = &value.kind else { panic!("{value:?}") };
    assert_eq!(args[0].mode, Mode::Owned);
    let ExprKind::Call { callee: Callee::Intrinsic(name), args: copy_args } = &args[0].value.kind else { panic!("{args:?}") };
    assert_eq!(name, "as:view_to_string:StringView:String");
    assert_eq!(copy_args[0].mode, Mode::Read);
}
