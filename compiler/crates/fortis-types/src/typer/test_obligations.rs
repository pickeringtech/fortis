//! LR 6.12, 7.7, 10.7: every source/holder/exit cell, with no program loading.
use super::*;
use fortis_names::{Libraries, Library};
use fortis_testkit::{forall, Json};
fn libs() -> Libraries {
    Libraries {
        libs: vec![Library {
            modules: BTreeMap::new(),
        }],
    }
}
fn s<'a>(j: &'a Json, k: &str) -> &'a str {
    j.get(k).unwrap().as_str().unwrap()
}
fn case_named(name: &str, args: Vec<Type>) -> Type {
    // A user type carries its declaration identity after R4/R7 (D-592).
    if matches!(name, "Box" | "Choice" | "Wrapper" | "Generic") {
        crate::decls::named_of_decl(&Decl::new("class", name, false, "main", ""), args)
    } else {
        named_args(name, args)
    }
}
fn typ(j: &Json) -> Type {
    if let Some(n) = j.as_str() {
        return case_named(n, vec![]);
    }
    let a = j.as_arr().unwrap();
    match a[0].as_str().unwrap() {
        "loan" => Type::Loan {
            inner: Box::new(typ(&a[3])),
            mode: a[1].as_str().unwrap().into(),
            origin: a[2].as_str().unwrap().into(),
        },
        "optional" => optional(typ(&a[1])),
        "tuple" => Type::Tuple(a[1..].iter().map(typ).collect()),
        "record" => Type::Record(vec![("value".into(), typ(&a[1]))]),
        "fn" => Type::Fn {
            capture_origins: Vec::new(),
            params: vec![],
            ret: Box::new(void()),
            kind: a[1].as_str().unwrap().into(),
            modifiers: vec![],
            origin: String::new(),
            dynamic: false,
            identity: String::new(),
            param_modes: vec![],
        },
        n => case_named(n, a[1..].iter().map(typ).collect()),
    }
}
fn typer<'a>(sess: &'a crate::decls::Session<'a>) -> Typer<'a> {
    let mut dc = DeclChecker::new(sess, 0, "unit", "main", "");
    for name in ["Outcome", "Future", "Task", "List"] {
        dc.table.decls.insert(
            name.into(),
            Rc::new(Decl::new("class", name, true, "fortis.prelude", "")),
        );
    }
    for (name, kind) in [("Box", "class"), ("Choice", "enum"), ("Wrapper", "type")] {
        let mut d = Decl::new(kind, name, false, "main", "");
        d.type_params = vec!["T".into()];
        d.linear = vec!["T".into()];
        match kind {
            "class" => {
                d.fields
                    .insert("value".into(), (Type::TypeParam("T".into()), false, false));
            }
            "enum" => {
                d.variants.insert(
                    "Some".into(),
                    vec![("value".into(), Type::TypeParam("T".into()))],
                );
            }
            _ => {
                d.alias = Some(Type::TypeParam("T".into()));
            }
        }
        dc.table.decls.insert(name.into(), Rc::new(d));
    }
    Typer::new(dc)
}
fn codes(t: &Typer<'_>) -> Json {
    Json::Arr(
        t.dc.records
            .iter()
            .map(|r| r.code.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(Json::Str)
            .collect(),
    )
}
fn matrix(j: &Json) -> Json {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    let env = env_new(None);
    let ty = typ(j.get("type").unwrap());
    t.depth = 2;
    let path = match s(j, "source") {
        "field" => "owner.value",
        "temporary" => "$temporary",
        _ => "value",
    };
    env_set(&env, path, ty.clone(), false, "let");
    let tok = Token::new(Kind::Lower, path, 1, 1);
    t.check_stored_type(&ty, &tok, s(j, "holder") == "class_field");
    if t.is_linear(&ty) {
        t.obligate(path, &tok);
    }
    let kind = s(j, "exit");
    if s(j, "holder") == "returned_value" && kind == "return" {
        t.own.move_(path);
    }
    if ["fail", "or_fail"].contains(&kind) {
        t.unwind_point(&Token::new(Kind::Kw, kind, 2, 1), kind, &env, None);
    } else if kind == "loop_early_exit"
        && s(j, "holder") == "collection_element"
        && t.is_linear(&ty)
    {
        t.own.move_(path);
        t.linear_loops
            .push((2, Token::new(Kind::Lower, "item", 1, 1)));
        t.linear_loop_exit(2, &Token::new(Kind::Kw, "break", 2, 1), "break", false);
    } else {
        t.report_open(if kind == "return" { 0 } else { 2 }, kind);
    }
    codes(&t)
}
fn check_matrix(j: &Json, want: &Json) {
    assert_eq!(&matrix(j), want);
}
fn check_type(j: &Json, want: &Json) {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    let ty = typ(j.get("type").unwrap());
    let tok = Token::new(Kind::Lower, "value", 1, 1);
    let actual = match s(j, "op") {
        "linear" => Json::Bool(t.is_linear(&ty)),
        "instantiate" => {
            let mut d = Decl::new(s(j, "kind"), "Generic", false, "main", "");
            d.type_params = vec!["T".into()];
            if j.get("bound") == Some(&Json::Bool(true)) {
                d.linear = vec!["T".into()];
            }
            t.dc.table.decls.insert("Generic".into(), Rc::new(d));
            t.dc.check_linear_instantiation(&ty, &BTreeSet::new(), &tok);
            codes(&t)
        }
        _ => {
            t.obligate("value", &tok);
            match s(j, "action") {
                "consume" => t.consume(Some("value".into()), 2, "consume"),
                "move" => t.own.move_("value"),
                _ => {
                    let env = env_new(None);
                    env_set(&env, "value", ty, false, "let");
                    t.unwind_point(&tok, "await", &env, Some("value"));
                    assert!(t.own.obligations.contains("value"));
                    assert_eq!(&codes(&t), want);
                    return;
                }
            }
            t.report_open(0, "end of block");
            codes(&t)
        }
    };
    assert_eq!(&actual, want);
}
#[test]
fn lr_6_12_matrix_is_exhaustive_and_seeded() {
    let rows = fortis_testkit::read_cases("obligations/matrix.json");
    assert_eq!(rows.len(), 7 * 6 * 9);
    let triples: BTreeSet<_> = rows
        .iter()
        .map(|r| {
            (
                s(&r.input, "source"),
                s(&r.input, "holder"),
                s(&r.input, "exit"),
            )
        })
        .collect();
    assert_eq!(triples.len(), rows.len());
    forall(
        |r| r.between(0, rows.len() as i64 - 1),
        200,
        None,
        |&i| {
            let row = &rows[i as usize];
            check_matrix(&row.input, &row.expect);
        },
    );
}
mod matrix {
    use super::*;
    fortis_testkit::case_tests!("obligations/matrix.json", check_matrix;
    lr_6_12_local_binding_end_block,
    lr_6_12_local_binding_return,
    lr_6_12_local_binding_break,
    lr_6_12_local_binding_continue,
    lr_6_12_local_binding_fail,
    lr_6_12_local_binding_or_fail,
    lr_6_12_local_binding_recover,
    lr_6_12_local_binding_catch,
    lr_6_12_local_binding_loop_early_exit,
    lr_6_12_local_collection_element_end_block,
    lr_6_12_local_collection_element_return,
    lr_6_12_local_collection_element_break,
    lr_6_12_local_collection_element_continue,
    lr_6_12_local_collection_element_fail,
    lr_6_12_local_collection_element_or_fail,
    lr_6_12_local_collection_element_recover,
    lr_6_12_local_collection_element_catch,
    lr_6_12_local_collection_element_loop_early_exit,
    lr_6_12_local_class_field_end_block,
    lr_6_12_local_class_field_return,
    lr_6_12_local_class_field_break,
    lr_6_12_local_class_field_continue,
    lr_6_12_local_class_field_fail,
    lr_6_12_local_class_field_or_fail,
    lr_6_12_local_class_field_recover,
    lr_6_12_local_class_field_catch,
    lr_6_12_local_class_field_loop_early_exit,
    lr_6_12_local_closure_capture_end_block,
    lr_6_12_local_closure_capture_return,
    lr_6_12_local_closure_capture_break,
    lr_6_12_local_closure_capture_continue,
    lr_6_12_local_closure_capture_fail,
    lr_6_12_local_closure_capture_or_fail,
    lr_6_12_local_closure_capture_recover,
    lr_6_12_local_closure_capture_catch,
    lr_6_12_local_closure_capture_loop_early_exit,
    lr_6_12_local_returned_value_end_block,
    lr_6_12_local_returned_value_return,
    lr_6_12_local_returned_value_break,
    lr_6_12_local_returned_value_continue,
    lr_6_12_local_returned_value_fail,
    lr_6_12_local_returned_value_or_fail,
    lr_6_12_local_returned_value_recover,
    lr_6_12_local_returned_value_catch,
    lr_6_12_local_returned_value_loop_early_exit,
    lr_6_12_local_outcome_end_block,
    lr_6_12_local_outcome_return,
    lr_6_12_local_outcome_break,
    lr_6_12_local_outcome_continue,
    lr_6_12_local_outcome_fail,
    lr_6_12_local_outcome_or_fail,
    lr_6_12_local_outcome_recover,
    lr_6_12_local_outcome_catch,
    lr_6_12_local_outcome_loop_early_exit,
    lr_6_12_parameter_loan_binding_end_block,
    lr_6_12_parameter_loan_binding_return,
    lr_6_12_parameter_loan_binding_break,
    lr_6_12_parameter_loan_binding_continue,
    lr_6_12_parameter_loan_binding_fail,
    lr_6_12_parameter_loan_binding_or_fail,
    lr_6_12_parameter_loan_binding_recover,
    lr_6_12_parameter_loan_binding_catch,
    lr_6_12_parameter_loan_binding_loop_early_exit,
    lr_6_12_parameter_loan_collection_element_end_block,
    lr_6_12_parameter_loan_collection_element_return,
    lr_6_12_parameter_loan_collection_element_break,
    lr_6_12_parameter_loan_collection_element_continue,
    lr_6_12_parameter_loan_collection_element_fail,
    lr_6_12_parameter_loan_collection_element_or_fail,
    lr_6_12_parameter_loan_collection_element_recover,
    lr_6_12_parameter_loan_collection_element_catch,
    lr_6_12_parameter_loan_collection_element_loop_early_exit,
    lr_6_12_parameter_loan_class_field_end_block,
    lr_6_12_parameter_loan_class_field_return,
    lr_6_12_parameter_loan_class_field_break,
    lr_6_12_parameter_loan_class_field_continue,
    lr_6_12_parameter_loan_class_field_fail,
    lr_6_12_parameter_loan_class_field_or_fail,
    lr_6_12_parameter_loan_class_field_recover,
    lr_6_12_parameter_loan_class_field_catch,
    lr_6_12_parameter_loan_class_field_loop_early_exit,
    lr_6_12_parameter_loan_closure_capture_end_block,
    lr_6_12_parameter_loan_closure_capture_return,
    lr_6_12_parameter_loan_closure_capture_break,
    lr_6_12_parameter_loan_closure_capture_continue,
    lr_6_12_parameter_loan_closure_capture_fail,
    lr_6_12_parameter_loan_closure_capture_or_fail,
    lr_6_12_parameter_loan_closure_capture_recover,
    lr_6_12_parameter_loan_closure_capture_catch,
    lr_6_12_parameter_loan_closure_capture_loop_early_exit,
    lr_6_12_parameter_loan_returned_value_end_block,
    lr_6_12_parameter_loan_returned_value_return,
    lr_6_12_parameter_loan_returned_value_break,
    lr_6_12_parameter_loan_returned_value_continue,
    lr_6_12_parameter_loan_returned_value_fail,
    lr_6_12_parameter_loan_returned_value_or_fail,
    lr_6_12_parameter_loan_returned_value_recover,
    lr_6_12_parameter_loan_returned_value_catch,
    lr_6_12_parameter_loan_returned_value_loop_early_exit,
    lr_6_12_parameter_loan_outcome_end_block,
    lr_6_12_parameter_loan_outcome_return,
    lr_6_12_parameter_loan_outcome_break,
    lr_6_12_parameter_loan_outcome_continue,
    lr_6_12_parameter_loan_outcome_fail,
    lr_6_12_parameter_loan_outcome_or_fail,
    lr_6_12_parameter_loan_outcome_recover,
    lr_6_12_parameter_loan_outcome_catch,
    lr_6_12_parameter_loan_outcome_loop_early_exit,
    lr_6_12_field_binding_end_block,
    lr_6_12_field_binding_return,
    lr_6_12_field_binding_break,
    lr_6_12_field_binding_continue,
    lr_6_12_field_binding_fail,
    lr_6_12_field_binding_or_fail,
    lr_6_12_field_binding_recover,
    lr_6_12_field_binding_catch,
    lr_6_12_field_binding_loop_early_exit,
    lr_6_12_field_collection_element_end_block,
    lr_6_12_field_collection_element_return,
    lr_6_12_field_collection_element_break,
    lr_6_12_field_collection_element_continue,
    lr_6_12_field_collection_element_fail,
    lr_6_12_field_collection_element_or_fail,
    lr_6_12_field_collection_element_recover,
    lr_6_12_field_collection_element_catch,
    lr_6_12_field_collection_element_loop_early_exit,
    lr_6_12_field_class_field_end_block,
    lr_6_12_field_class_field_return,
    lr_6_12_field_class_field_break,
    lr_6_12_field_class_field_continue,
    lr_6_12_field_class_field_fail,
    lr_6_12_field_class_field_or_fail,
    lr_6_12_field_class_field_recover,
    lr_6_12_field_class_field_catch,
    lr_6_12_field_class_field_loop_early_exit,
    lr_6_12_field_closure_capture_end_block,
    lr_6_12_field_closure_capture_return,
    lr_6_12_field_closure_capture_break,
    lr_6_12_field_closure_capture_continue,
    lr_6_12_field_closure_capture_fail,
    lr_6_12_field_closure_capture_or_fail,
    lr_6_12_field_closure_capture_recover,
    lr_6_12_field_closure_capture_catch,
    lr_6_12_field_closure_capture_loop_early_exit,
    lr_6_12_field_returned_value_end_block,
    lr_6_12_field_returned_value_return,
    lr_6_12_field_returned_value_break,
    lr_6_12_field_returned_value_continue,
    lr_6_12_field_returned_value_fail,
    lr_6_12_field_returned_value_or_fail,
    lr_6_12_field_returned_value_recover,
    lr_6_12_field_returned_value_catch,
    lr_6_12_field_returned_value_loop_early_exit,
    lr_6_12_field_outcome_end_block,
    lr_6_12_field_outcome_return,
    lr_6_12_field_outcome_break,
    lr_6_12_field_outcome_continue,
    lr_6_12_field_outcome_fail,
    lr_6_12_field_outcome_or_fail,
    lr_6_12_field_outcome_recover,
    lr_6_12_field_outcome_catch,
    lr_6_12_field_outcome_loop_early_exit,
    lr_6_12_temporary_binding_end_block,
    lr_6_12_temporary_binding_return,
    lr_6_12_temporary_binding_break,
    lr_6_12_temporary_binding_continue,
    lr_6_12_temporary_binding_fail,
    lr_6_12_temporary_binding_or_fail,
    lr_6_12_temporary_binding_recover,
    lr_6_12_temporary_binding_catch,
    lr_6_12_temporary_binding_loop_early_exit,
    lr_6_12_temporary_collection_element_end_block,
    lr_6_12_temporary_collection_element_return,
    lr_6_12_temporary_collection_element_break,
    lr_6_12_temporary_collection_element_continue,
    lr_6_12_temporary_collection_element_fail,
    lr_6_12_temporary_collection_element_or_fail,
    lr_6_12_temporary_collection_element_recover,
    lr_6_12_temporary_collection_element_catch,
    lr_6_12_temporary_collection_element_loop_early_exit,
    lr_6_12_temporary_class_field_end_block,
    lr_6_12_temporary_class_field_return,
    lr_6_12_temporary_class_field_break,
    lr_6_12_temporary_class_field_continue,
    lr_6_12_temporary_class_field_fail,
    lr_6_12_temporary_class_field_or_fail,
    lr_6_12_temporary_class_field_recover,
    lr_6_12_temporary_class_field_catch,
    lr_6_12_temporary_class_field_loop_early_exit,
    lr_6_12_temporary_closure_capture_end_block,
    lr_6_12_temporary_closure_capture_return,
    lr_6_12_temporary_closure_capture_break,
    lr_6_12_temporary_closure_capture_continue,
    lr_6_12_temporary_closure_capture_fail,
    lr_6_12_temporary_closure_capture_or_fail,
    lr_6_12_temporary_closure_capture_recover,
    lr_6_12_temporary_closure_capture_catch,
    lr_6_12_temporary_closure_capture_loop_early_exit,
    lr_6_12_temporary_returned_value_end_block,
    lr_6_12_temporary_returned_value_return,
    lr_6_12_temporary_returned_value_break,
    lr_6_12_temporary_returned_value_continue,
    lr_6_12_temporary_returned_value_fail,
    lr_6_12_temporary_returned_value_or_fail,
    lr_6_12_temporary_returned_value_recover,
    lr_6_12_temporary_returned_value_catch,
    lr_6_12_temporary_returned_value_loop_early_exit,
    lr_6_12_temporary_outcome_end_block,
    lr_6_12_temporary_outcome_return,
    lr_6_12_temporary_outcome_break,
    lr_6_12_temporary_outcome_continue,
    lr_6_12_temporary_outcome_fail,
    lr_6_12_temporary_outcome_or_fail,
    lr_6_12_temporary_outcome_recover,
    lr_6_12_temporary_outcome_catch,
    lr_6_12_temporary_outcome_loop_early_exit,
    lr_6_12_literal_binding_end_block,
    lr_6_12_literal_binding_return,
    lr_6_12_literal_binding_break,
    lr_6_12_literal_binding_continue,
    lr_6_12_literal_binding_fail,
    lr_6_12_literal_binding_or_fail,
    lr_6_12_literal_binding_recover,
    lr_6_12_literal_binding_catch,
    lr_6_12_literal_binding_loop_early_exit,
    lr_6_12_literal_collection_element_end_block,
    lr_6_12_literal_collection_element_return,
    lr_6_12_literal_collection_element_break,
    lr_6_12_literal_collection_element_continue,
    lr_6_12_literal_collection_element_fail,
    lr_6_12_literal_collection_element_or_fail,
    lr_6_12_literal_collection_element_recover,
    lr_6_12_literal_collection_element_catch,
    lr_6_12_literal_collection_element_loop_early_exit,
    lr_6_12_literal_class_field_end_block,
    lr_6_12_literal_class_field_return,
    lr_6_12_literal_class_field_break,
    lr_6_12_literal_class_field_continue,
    lr_6_12_literal_class_field_fail,
    lr_6_12_literal_class_field_or_fail,
    lr_6_12_literal_class_field_recover,
    lr_6_12_literal_class_field_catch,
    lr_6_12_literal_class_field_loop_early_exit,
    lr_6_12_literal_closure_capture_end_block,
    lr_6_12_literal_closure_capture_return,
    lr_6_12_literal_closure_capture_break,
    lr_6_12_literal_closure_capture_continue,
    lr_6_12_literal_closure_capture_fail,
    lr_6_12_literal_closure_capture_or_fail,
    lr_6_12_literal_closure_capture_recover,
    lr_6_12_literal_closure_capture_catch,
    lr_6_12_literal_closure_capture_loop_early_exit,
    lr_6_12_literal_returned_value_end_block,
    lr_6_12_literal_returned_value_return,
    lr_6_12_literal_returned_value_break,
    lr_6_12_literal_returned_value_continue,
    lr_6_12_literal_returned_value_fail,
    lr_6_12_literal_returned_value_or_fail,
    lr_6_12_literal_returned_value_recover,
    lr_6_12_literal_returned_value_catch,
    lr_6_12_literal_returned_value_loop_early_exit,
    lr_6_12_literal_outcome_end_block,
    lr_6_12_literal_outcome_return,
    lr_6_12_literal_outcome_break,
    lr_6_12_literal_outcome_continue,
    lr_6_12_literal_outcome_fail,
    lr_6_12_literal_outcome_or_fail,
    lr_6_12_literal_outcome_recover,
    lr_6_12_literal_outcome_catch,
    lr_6_12_literal_outcome_loop_early_exit,
    lr_6_12_call_result_binding_end_block,
    lr_6_12_call_result_binding_return,
    lr_6_12_call_result_binding_break,
    lr_6_12_call_result_binding_continue,
    lr_6_12_call_result_binding_fail,
    lr_6_12_call_result_binding_or_fail,
    lr_6_12_call_result_binding_recover,
    lr_6_12_call_result_binding_catch,
    lr_6_12_call_result_binding_loop_early_exit,
    lr_6_12_call_result_collection_element_end_block,
    lr_6_12_call_result_collection_element_return,
    lr_6_12_call_result_collection_element_break,
    lr_6_12_call_result_collection_element_continue,
    lr_6_12_call_result_collection_element_fail,
    lr_6_12_call_result_collection_element_or_fail,
    lr_6_12_call_result_collection_element_recover,
    lr_6_12_call_result_collection_element_catch,
    lr_6_12_call_result_collection_element_loop_early_exit,
    lr_6_12_call_result_class_field_end_block,
    lr_6_12_call_result_class_field_return,
    lr_6_12_call_result_class_field_break,
    lr_6_12_call_result_class_field_continue,
    lr_6_12_call_result_class_field_fail,
    lr_6_12_call_result_class_field_or_fail,
    lr_6_12_call_result_class_field_recover,
    lr_6_12_call_result_class_field_catch,
    lr_6_12_call_result_class_field_loop_early_exit,
    lr_6_12_call_result_closure_capture_end_block,
    lr_6_12_call_result_closure_capture_return,
    lr_6_12_call_result_closure_capture_break,
    lr_6_12_call_result_closure_capture_continue,
    lr_6_12_call_result_closure_capture_fail,
    lr_6_12_call_result_closure_capture_or_fail,
    lr_6_12_call_result_closure_capture_recover,
    lr_6_12_call_result_closure_capture_catch,
    lr_6_12_call_result_closure_capture_loop_early_exit,
    lr_6_12_call_result_returned_value_end_block,
    lr_6_12_call_result_returned_value_return,
    lr_6_12_call_result_returned_value_break,
    lr_6_12_call_result_returned_value_continue,
    lr_6_12_call_result_returned_value_fail,
    lr_6_12_call_result_returned_value_or_fail,
    lr_6_12_call_result_returned_value_recover,
    lr_6_12_call_result_returned_value_catch,
    lr_6_12_call_result_returned_value_loop_early_exit,
    lr_6_12_call_result_outcome_end_block,
    lr_6_12_call_result_outcome_return,
    lr_6_12_call_result_outcome_break,
    lr_6_12_call_result_outcome_continue,
    lr_6_12_call_result_outcome_fail,
    lr_6_12_call_result_outcome_or_fail,
    lr_6_12_call_result_outcome_recover,
    lr_6_12_call_result_outcome_catch,
    lr_6_12_call_result_outcome_loop_early_exit,
    lr_6_12_fallback_binding_end_block,
    lr_6_12_fallback_binding_return,
    lr_6_12_fallback_binding_break,
    lr_6_12_fallback_binding_continue,
    lr_6_12_fallback_binding_fail,
    lr_6_12_fallback_binding_or_fail,
    lr_6_12_fallback_binding_recover,
    lr_6_12_fallback_binding_catch,
    lr_6_12_fallback_binding_loop_early_exit,
    lr_6_12_fallback_collection_element_end_block,
    lr_6_12_fallback_collection_element_return,
    lr_6_12_fallback_collection_element_break,
    lr_6_12_fallback_collection_element_continue,
    lr_6_12_fallback_collection_element_fail,
    lr_6_12_fallback_collection_element_or_fail,
    lr_6_12_fallback_collection_element_recover,
    lr_6_12_fallback_collection_element_catch,
    lr_6_12_fallback_collection_element_loop_early_exit,
    lr_6_12_fallback_class_field_end_block,
    lr_6_12_fallback_class_field_return,
    lr_6_12_fallback_class_field_break,
    lr_6_12_fallback_class_field_continue,
    lr_6_12_fallback_class_field_fail,
    lr_6_12_fallback_class_field_or_fail,
    lr_6_12_fallback_class_field_recover,
    lr_6_12_fallback_class_field_catch,
    lr_6_12_fallback_class_field_loop_early_exit,
    lr_6_12_fallback_closure_capture_end_block,
    lr_6_12_fallback_closure_capture_return,
    lr_6_12_fallback_closure_capture_break,
    lr_6_12_fallback_closure_capture_continue,
    lr_6_12_fallback_closure_capture_fail,
    lr_6_12_fallback_closure_capture_or_fail,
    lr_6_12_fallback_closure_capture_recover,
    lr_6_12_fallback_closure_capture_catch,
    lr_6_12_fallback_closure_capture_loop_early_exit,
    lr_6_12_fallback_returned_value_end_block,
    lr_6_12_fallback_returned_value_return,
    lr_6_12_fallback_returned_value_break,
    lr_6_12_fallback_returned_value_continue,
    lr_6_12_fallback_returned_value_fail,
    lr_6_12_fallback_returned_value_or_fail,
    lr_6_12_fallback_returned_value_recover,
    lr_6_12_fallback_returned_value_catch,
    lr_6_12_fallback_returned_value_loop_early_exit,
    lr_6_12_fallback_outcome_end_block,
    lr_6_12_fallback_outcome_return,
    lr_6_12_fallback_outcome_break,
    lr_6_12_fallback_outcome_continue,
    lr_6_12_fallback_outcome_fail,
    lr_6_12_fallback_outcome_or_fail,
    lr_6_12_fallback_outcome_recover,
    lr_6_12_fallback_outcome_catch,
    lr_6_12_fallback_outcome_loop_early_exit
    );
}
mod types {
    use super::*;
    fortis_testkit::case_tests!("obligations/types.json", check_type;
    lr_6_12_linear_int_plain,
    lr_6_12_linear_int_optional,
    lr_6_12_linear_int_list,
    lr_6_12_linear_int_tuple,
    lr_6_12_linear_int_record,
    lr_6_12_linear_int_box,
    lr_6_12_linear_int_enum,
    lr_6_12_linear_int_newtype,
    lr_6_12_linear_string_plain,
    lr_6_12_linear_string_optional,
    lr_6_12_linear_string_list,
    lr_6_12_linear_string_tuple,
    lr_6_12_linear_string_record,
    lr_6_12_linear_string_box,
    lr_6_12_linear_string_enum,
    lr_6_12_linear_string_newtype,
    lr_6_12_linear_must_plain,
    lr_6_12_linear_must_optional,
    lr_6_12_linear_must_list,
    lr_6_12_linear_must_tuple,
    lr_6_12_linear_must_record,
    lr_6_12_linear_must_box,
    lr_6_12_linear_must_enum,
    lr_6_12_linear_must_newtype,
    lr_6_12_linear_outcome_plain,
    lr_6_12_linear_outcome_optional,
    lr_6_12_linear_outcome_list,
    lr_6_12_linear_outcome_tuple,
    lr_6_12_linear_outcome_record,
    lr_6_12_linear_outcome_box,
    lr_6_12_linear_outcome_enum,
    lr_6_12_linear_outcome_newtype,
    lr_6_12_linear_loan_plain,
    lr_6_12_linear_loan_optional,
    lr_6_12_linear_loan_list,
    lr_6_12_linear_loan_tuple,
    lr_6_12_linear_loan_record,
    lr_6_12_linear_loan_box,
    lr_6_12_linear_loan_enum,
    lr_6_12_linear_loan_newtype,
    lr_10_7_class_unbounded_plain,
    lr_10_7_class_unbounded_linear,
    lr_10_7_class_bounded_plain,
    lr_10_7_class_bounded_linear,
    lr_10_7_enum_unbounded_plain,
    lr_10_7_enum_unbounded_linear,
    lr_10_7_enum_bounded_plain,
    lr_10_7_enum_bounded_linear,
    lr_10_2_consume_must,
    lr_10_2_move_must,
    lr_10_2_unwind_keep_must,
    lr_10_2_consume_failure,
    lr_10_2_move_failure,
    lr_10_2_unwind_keep_failure,
    lr_10_2_consume_outcome,
    lr_10_2_move_outcome,
    lr_10_2_unwind_keep_outcome,
    lr_10_2_consume_future,
    lr_10_2_move_future,
    lr_10_2_unwind_keep_future
    );
}

#[test]
fn d646_lent_type_parameter_is_not_held_by_value() {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let t = typer(&sess);
    let ty = Type::Loan {
        inner: Box::new(Type::TypeParam("T".into())),
        mode: "alias".into(),
        origin: "x".into(),
    };
    assert!(!t.holds_by_value(&ty, "T", true));
}

fn check_callback(j: &Json, want: &Json) {
    let make = |mode: &str| Type::Fn {
            capture_origins: Vec::new(),
        params: vec![named(s(j, "type"))],
        ret: Box::new(void()),
        kind: "fn".into(),
        modifiers: vec![],
        origin: String::new(),
        dynamic: false,
        identity: String::new(),
        param_modes: vec![mode.into()],
    };
    assert_eq!(
        crate::literals::assignable(&make(s(j, "have")), &make(s(j, "want"))).0,
        Some(*want == Json::Bool(true))
    );
}
mod callbacks {
    use super::*;
    fortis_testkit::case_tests!("obligations/callbacks.json",check_callback;
    lr_8_4_int_read_to_read,
    lr_8_4_int_read_to_editable,
    lr_8_4_int_read_to_owned,
    lr_8_4_int_editable_to_read,
    lr_8_4_int_editable_to_editable,
    lr_8_4_int_editable_to_owned,
    lr_8_4_int_owned_to_read,
    lr_8_4_int_owned_to_editable,
    lr_8_4_int_owned_to_owned,
    lr_8_4_string_read_to_read,
    lr_8_4_string_read_to_editable,
    lr_8_4_string_read_to_owned,
    lr_8_4_string_editable_to_read,
    lr_8_4_string_editable_to_editable,
    lr_8_4_string_editable_to_owned,
    lr_8_4_string_owned_to_read,
    lr_8_4_string_owned_to_editable,
    lr_8_4_string_owned_to_owned,
    lr_8_4_must_read_to_read,
    lr_8_4_must_read_to_editable,
    lr_8_4_must_read_to_owned,
    lr_8_4_must_editable_to_read,
    lr_8_4_must_editable_to_editable,
    lr_8_4_must_editable_to_owned,
    lr_8_4_must_owned_to_read,
    lr_8_4_must_owned_to_editable,
    lr_8_4_must_owned_to_owned
    );
}

fn check_loop(j: &Json, want: &Json) {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    for d in 1..=3 {
        t.linear_loops.push((
            d,
            Token::new(Kind::Lower, &format!("item{d}"), d as usize, 1),
        ));
    }
    t.linear_loop_exit(
        j.get("depth").unwrap().as_i64().unwrap(),
        &Token::new(Kind::Kw, "exit", 9, 1),
        s(j, "exit"),
        j.get("unwind") == Some(&Json::Bool(true)),
    );
    let actual = Json::Arr(
        t.dc.records
            .iter()
            .map(|r| {
                let path = r
                    .fields
                    .get("unconsumed_path")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .strip_prefix("remainder of ")
                    .unwrap();
                Json::Arr(vec![Json::str(path), Json::str(&r.code)])
            })
            .collect(),
    );
    assert_eq!(&actual, want);
}
mod loops {
    use super::*;
    fortis_testkit::case_tests!("obligations/loops.json",check_loop;
    lr_5_3_remainders_return,
    lr_5_3_remainders_break_outer,
    lr_5_3_remainders_break_middle,
    lr_5_3_remainders_break_inner,
    lr_5_3_remainders_continue_outer,
    lr_5_3_remainders_continue_middle,
    lr_5_3_remainders_continue_inner,
    lr_5_3_remainders_fail,
    lr_5_3_remainders_or_fail,
    lr_5_3_remainders_await,
    lr_5_3_remainders_recover_outer,
    lr_5_3_remainders_recover_inner
    );
}
