//! LR 7.3-7.7. Declaration heads and types only: no parser or library file loading.
use super::*;
use fortis_names::{Libraries, Library};
use fortis_testkit::{forall, Json};

fn libraries() -> Libraries {
    Libraries {
        libs: vec![Library {
            modules: BTreeMap::new(),
        }],
    }
}
fn typer<'a>(session: &'a crate::decls::Session<'a>) -> Typer<'a> {
    let mut dc = DeclChecker::new(session, 0, "unit", "main", "");
    for name in [
        "StringView",
        "BytesView",
        "ListView",
        "ListCursor",
        "Outcome",
        "Future",
        "Task",
    ] {
        dc.table.decls.insert(
            name.into(),
            Rc::new(Decl::new("class", name, true, "fortis.prelude", "")),
        );
    }
    Typer::new(dc)
}
fn s<'a>(j: &'a Json, key: &str) -> &'a str {
    j.get(key).unwrap().as_str().unwrap()
}
fn typ(j: &Json) -> Type {
    if let Some(n) = j.as_str() {
        return named(n);
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
        "parameter" => Type::TypeParam(a[1].as_str().unwrap().into()),
        "captures" => Type::Fn {
            params: vec![], ret: Box::new(void()), kind: "fn".into(), modifiers: vec![],
            origin: String::new(), dynamic: true, identity: String::new(), param_modes: vec![],
            capture_origins: a[1..].iter().map(|x| { let x = x.as_arr().unwrap(); (x[0].as_str().unwrap().into(), x[1].as_str().unwrap().into()) }).collect(),
        },
        "closure" => Type::Fn {
            capture_origins: Vec::new(),
            params: vec![],
            ret: Box::new(void()),
            kind: "fn".into(),
            modifiers: vec![],
            origin: a[1].as_str().unwrap().into(),
            dynamic: false,
            identity: String::new(),
            param_modes: vec![],
        },
        n => named_args(n, a[1..].iter().map(typ).collect()),
    }
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
fn check_state(j: &Json, want: &Json) {
    let (a, b) = (s(j, "a"), s(j, "b"));
    let mut state = Owners::default();
    state.move_(a);
    let moved = state.is_moved(b);
    state.reinit(b);
    assert_eq!(
        &Json::Arr(vec![
            Json::Bool(overlaps(a, b)),
            Json::Bool(moved),
            Json::Bool(state.is_moved(a))
        ]),
        want
    );
}
fn check_use(j: &Json, want: &Json) {
    let libs = libraries();
    let sess = crate::decls::Session::new(&libs);
    let mut t = typer(&sess);
    let env = env_new(None);
    for n in ["x", "y"] {
        env_set(&env, n, string(), true, "mut");
    }
    match s(j, "state") {
        "moved" => t.own.move_("x"),
        "free" => (),
        mode => {
            t.own.add_loan("x", mode, Some("holder"), 1, 1);
        }
    }
    t.use_place(
        Some(s(j, "path")),
        &Token::new(Kind::Lower, "x", 2, 1),
        s(j, "use"),
        &env,
    );
    assert_eq!(&codes(&t), want);
    assert_eq!(t.use_ok, want.as_arr().unwrap().is_empty());
}
fn check_storage(j: &Json, want: &Json) {
    let libs = libraries();
    let sess = crate::decls::Session::new(&libs);
    let mut t = typer(&sess);
    t.check_stored_type(
        &typ(j.get("type").unwrap()),
        &Token::new(Kind::Lower, "value", 1, 1),
        j.get("field") == Some(&Json::Bool(true)),
    );
    assert_eq!(&codes(&t), want);
}
#[test]
fn lang_07_05_held_loans_end_at_their_block() {
    forall(
        |r| (r.between(1, 7), r.between(1, 7), r.between(0, 1)),
        200,
        None,
        |&(depth, exiting, mode)| {
            let mut own = Owners::default();
            own.add_loan(
                "owner",
                if mode == 0 { "alias" } else { "editable" },
                Some("holder"),
                depth,
                1,
            );
            own.add_loan("other", "alias", None, depth, 1);
            own.end_statement();
            assert_eq!(own.active("owner").len(), 1);
            assert!(own.active("other").is_empty());
            own.end_block(exiting);
            assert_eq!(!own.active("owner").is_empty(), depth < exiting);
            assert_eq!(own.holders.contains("holder"), depth < exiting);
        },
    );
}
#[test]
fn lang_07_04_join_keeps_either_path_move() {
    forall(
        |r| (r.between(1, 9), r.between(1, 9)),
        200,
        None,
        |&(a, b)| {
            let (mut left, mut right) = (Owners::default(), Owners::default());
            for i in 0..a {
                left.move_(&format!("left{i}"));
            }
            for i in 0..b {
                right.move_(&format!("right{i}"));
            }
            assert_eq!(
                left.join(&right).moved,
                left.moved.union(&right.moved).cloned().collect()
            );
            assert_eq!(left.join(&right).moved, right.join(&left).moved);
            assert_eq!(left.join(&left).moved, left.moved);
        },
    );
}
mod state {
    use super::*;
    fortis_testkit::case_tests!("ownership/state.json", check_state;
    lr_7_4_path_0_0,
    lr_7_4_path_0_1,
    lr_7_4_path_0_2,
    lr_7_4_path_0_3,
    lr_7_4_path_0_4,
    lr_7_4_path_0_5,
    lr_7_4_path_0_6,
    lr_7_4_path_1_0,
    lr_7_4_path_1_1,
    lr_7_4_path_1_2,
    lr_7_4_path_1_3,
    lr_7_4_path_1_4,
    lr_7_4_path_1_5,
    lr_7_4_path_1_6,
    lr_7_4_path_2_0,
    lr_7_4_path_2_1,
    lr_7_4_path_2_2,
    lr_7_4_path_2_3,
    lr_7_4_path_2_4,
    lr_7_4_path_2_5,
    lr_7_4_path_2_6,
    lr_7_4_path_3_0,
    lr_7_4_path_3_1,
    lr_7_4_path_3_2,
    lr_7_4_path_3_3,
    lr_7_4_path_3_4,
    lr_7_4_path_3_5,
    lr_7_4_path_3_6,
    lr_7_4_path_4_0,
    lr_7_4_path_4_1,
    lr_7_4_path_4_2,
    lr_7_4_path_4_3,
    lr_7_4_path_4_4,
    lr_7_4_path_4_5,
    lr_7_4_path_4_6,
    lr_7_4_path_5_0,
    lr_7_4_path_5_1,
    lr_7_4_path_5_2,
    lr_7_4_path_5_3,
    lr_7_4_path_5_4,
    lr_7_4_path_5_5,
    lr_7_4_path_5_6,
    lr_7_4_path_6_0,
    lr_7_4_path_6_1,
    lr_7_4_path_6_2,
    lr_7_4_path_6_3,
    lr_7_4_path_6_4,
    lr_7_4_path_6_5,
    lr_7_4_path_6_6
    );
}
mod uses {
    use super::*;
    fortis_testkit::case_tests!("ownership/use.json", check_use;
    lr_7_3_free_x_read,
    lr_7_3_free_x_copy,
    lr_7_3_free_x_alias,
    lr_7_3_free_x_editable,
    lr_7_3_free_x_write,
    lr_7_3_free_x_move,
    lr_7_3_free_x_a_read,
    lr_7_3_free_x_a_copy,
    lr_7_3_free_x_a_alias,
    lr_7_3_free_x_a_editable,
    lr_7_3_free_x_a_write,
    lr_7_3_free_x_a_move,
    lr_7_3_free_y_read,
    lr_7_3_free_y_copy,
    lr_7_3_free_y_alias,
    lr_7_3_free_y_editable,
    lr_7_3_free_y_write,
    lr_7_3_free_y_move,
    lr_7_3_alias_x_read,
    lr_7_3_alias_x_copy,
    lr_7_3_alias_x_alias,
    lr_7_3_alias_x_editable,
    lr_7_3_alias_x_write,
    lr_7_3_alias_x_move,
    lr_7_3_alias_x_a_read,
    lr_7_3_alias_x_a_copy,
    lr_7_3_alias_x_a_alias,
    lr_7_3_alias_x_a_editable,
    lr_7_3_alias_x_a_write,
    lr_7_3_alias_x_a_move,
    lr_7_3_alias_y_read,
    lr_7_3_alias_y_copy,
    lr_7_3_alias_y_alias,
    lr_7_3_alias_y_editable,
    lr_7_3_alias_y_write,
    lr_7_3_alias_y_move,
    lr_7_3_editable_x_read,
    lr_7_3_editable_x_copy,
    lr_7_3_editable_x_alias,
    lr_7_3_editable_x_editable,
    lr_7_3_editable_x_write,
    lr_7_3_editable_x_move,
    lr_7_3_editable_x_a_read,
    lr_7_3_editable_x_a_copy,
    lr_7_3_editable_x_a_alias,
    lr_7_3_editable_x_a_editable,
    lr_7_3_editable_x_a_write,
    lr_7_3_editable_x_a_move,
    lr_7_3_editable_y_read,
    lr_7_3_editable_y_copy,
    lr_7_3_editable_y_alias,
    lr_7_3_editable_y_editable,
    lr_7_3_editable_y_write,
    lr_7_3_editable_y_move,
    lr_7_3_moved_x_read,
    lr_7_3_moved_x_copy,
    lr_7_3_moved_x_alias,
    lr_7_3_moved_x_editable,
    lr_7_3_moved_x_write,
    lr_7_3_moved_x_move,
    lr_7_3_moved_x_a_read,
    lr_7_3_moved_x_a_copy,
    lr_7_3_moved_x_a_alias,
    lr_7_3_moved_x_a_editable,
    lr_7_3_moved_x_a_write,
    lr_7_3_moved_x_a_move,
    lr_7_3_moved_y_read,
    lr_7_3_moved_y_copy,
    lr_7_3_moved_y_alias,
    lr_7_3_moved_y_editable,
    lr_7_3_moved_y_write,
    lr_7_3_moved_y_move
    );
}
mod storage {
    use super::*;
    fortis_testkit::case_tests!("ownership/storage.json", check_storage;
    lr_7_7_int_binding,
    lr_7_7_int_optional,
    lr_7_7_int_tuple,
    lr_7_7_int_record,
    lr_7_7_int_outcome,
    lr_7_7_int_collection,
    lr_7_7_int_class_field,
    lr_7_7_int_shared,
    lr_7_7_string_binding,
    lr_7_7_string_optional,
    lr_7_7_string_tuple,
    lr_7_7_string_record,
    lr_7_7_string_outcome,
    lr_7_7_string_collection,
    lr_7_7_string_class_field,
    lr_7_7_string_shared,
    lr_7_7_view_binding,
    lr_7_7_view_optional,
    lr_7_7_view_tuple,
    lr_7_7_view_record,
    lr_7_7_view_outcome,
    lr_7_7_view_collection,
    lr_7_7_view_class_field,
    lr_7_7_view_shared,
    lr_7_7_bytes_view_binding,
    lr_7_7_bytes_view_optional,
    lr_7_7_bytes_view_tuple,
    lr_7_7_bytes_view_record,
    lr_7_7_bytes_view_outcome,
    lr_7_7_bytes_view_collection,
    lr_7_7_bytes_view_class_field,
    lr_7_7_bytes_view_shared,
    lr_7_7_list_view_binding,
    lr_7_7_list_view_optional,
    lr_7_7_list_view_tuple,
    lr_7_7_list_view_record,
    lr_7_7_list_view_outcome,
    lr_7_7_list_view_collection,
    lr_7_7_list_view_class_field,
    lr_7_7_list_view_shared,
    lr_7_7_cursor_binding,
    lr_7_7_cursor_optional,
    lr_7_7_cursor_tuple,
    lr_7_7_cursor_record,
    lr_7_7_cursor_outcome,
    lr_7_7_cursor_collection,
    lr_7_7_cursor_class_field,
    lr_7_7_cursor_shared,
    lr_7_7_read_loan_binding,
    lr_7_7_read_loan_optional,
    lr_7_7_read_loan_tuple,
    lr_7_7_read_loan_record,
    lr_7_7_read_loan_outcome,
    lr_7_7_read_loan_collection,
    lr_7_7_read_loan_class_field,
    lr_7_7_read_loan_shared,
    lr_7_7_edit_loan_binding,
    lr_7_7_edit_loan_optional,
    lr_7_7_edit_loan_tuple,
    lr_7_7_edit_loan_record,
    lr_7_7_edit_loan_outcome,
    lr_7_7_edit_loan_collection,
    lr_7_7_edit_loan_class_field,
    lr_7_7_edit_loan_shared,
    lr_7_7_static_view_binding,
    lr_7_7_static_view_optional,
    lr_7_7_static_view_tuple,
    lr_7_7_static_view_record,
    lr_7_7_static_view_outcome,
    lr_7_7_static_view_collection,
    lr_7_7_static_view_class_field,
    lr_7_7_static_view_shared,
    lr_7_7_static_loan_binding,
    lr_7_7_static_loan_optional,
    lr_7_7_static_loan_tuple,
    lr_7_7_static_loan_record,
    lr_7_7_static_loan_outcome,
    lr_7_7_static_loan_collection,
    lr_7_7_static_loan_class_field,
    lr_7_7_static_loan_shared,
    lr_7_7_closure_binding,
    lr_7_7_closure_optional,
    lr_7_7_closure_tuple,
    lr_7_7_closure_record,
    lr_7_7_closure_outcome,
    lr_7_7_closure_collection,
    lr_7_7_closure_class_field,
    lr_7_7_closure_shared,
    lr_7_7_static_closure_binding,
    lr_7_7_static_closure_optional,
    lr_7_7_static_closure_tuple,
    lr_7_7_static_closure_record,
    lr_7_7_static_closure_outcome,
    lr_7_7_static_closure_collection,
    lr_7_7_static_closure_class_field,
    lr_7_7_static_closure_shared,
    lr_7_7_owned_closure_binding,
    lr_7_7_owned_closure_optional,
    lr_7_7_owned_closure_tuple,
    lr_7_7_owned_closure_record,
    lr_7_7_owned_closure_outcome,
    lr_7_7_owned_closure_collection,
    lr_7_7_owned_closure_class_field,
    lr_7_7_owned_closure_shared,
    lr_7_7_nested_storage_binding,
    lr_7_7_nested_storage_optional,
    lr_7_7_nested_storage_tuple,
    lr_7_7_nested_storage_record,
    lr_7_7_nested_storage_outcome,
    lr_7_7_nested_storage_collection,
    lr_7_7_nested_storage_class_field,
    lr_7_7_nested_storage_shared
    );
}

mod temporaries {
    use super::*;
    fortis_testkit::case_tests!("ownership/temporaries.json", check_temporary;
    lr_7_9_local_plain,
    lr_7_9_local_coalesce,
    lr_7_9_local_if,
    lr_7_9_local_match,
    lr_7_9_local_recover,
    lr_7_9_local_catch,
    lr_7_9_parameter_loan_plain,
    lr_7_9_parameter_loan_coalesce,
    lr_7_9_parameter_loan_if,
    lr_7_9_parameter_loan_match,
    lr_7_9_parameter_loan_recover,
    lr_7_9_parameter_loan_catch,
    lr_7_9_field_plain,
    lr_7_9_field_coalesce,
    lr_7_9_field_if,
    lr_7_9_field_match,
    lr_7_9_field_recover,
    lr_7_9_field_catch,
    lr_7_9_temporary_plain,
    lr_7_9_temporary_coalesce,
    lr_7_9_temporary_if,
    lr_7_9_temporary_match,
    lr_7_9_temporary_recover,
    lr_7_9_temporary_catch,
    lr_7_9_literal_plain,
    lr_7_9_literal_coalesce,
    lr_7_9_literal_if,
    lr_7_9_literal_match,
    lr_7_9_literal_recover,
    lr_7_9_literal_catch,
    lr_7_9_call_result_plain,
    lr_7_9_call_result_coalesce,
    lr_7_9_call_result_if,
    lr_7_9_call_result_match,
    lr_7_9_call_result_recover,
    lr_7_9_call_result_catch,
    lr_7_9_fallback_plain,
    lr_7_9_fallback_coalesce,
    lr_7_9_fallback_if,
    lr_7_9_fallback_match,
    lr_7_9_fallback_recover,
    lr_7_9_fallback_catch
    );
}

fn expression(j: &Json, toks: &mut Vec<Token>) -> fortis_syntax::tree::Child {
    use fortis_syntax::tree::{Child, Node};
    if let Some(pair) = j.get("token") {
        let a = pair.as_arr().unwrap();
        let kind = match a[0].as_str().unwrap() {
            "LOWER" => Kind::Lower,
            "STR" => Kind::Str,
            "KW" => Kind::Kw,
            "PUNCT" => Kind::Punct,
            _ => panic!("unknown token"),
        };
        let idx = toks.len();
        toks.push(Token::new(kind, a[1].as_str().unwrap(), 1, 1));
        return Child::Tok(idx);
    }
    let rule = fortis_syntax::generated::RULE_NAMES
        .iter()
        .position(|r| *r == s(j, "rule"))
        .unwrap() as u16;
    let children = j
        .get("kids")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .map(|c| expression(c, toks))
        .collect();
    Child::Node(Node {
        rule,
        children,
        inferred_head: Default::default(),
    })
}
fn remember_types<'a>(t: &mut Typer<'a>, n: N<'a>, j: &Json) {
    for (child, spec) in n
        .kids()
        .iter()
        .zip(j.get("kids").unwrap().as_arr().unwrap())
    {
        if let Some(child) = child.node() {
            remember_types(t, child, spec);
        }
    }
    // infer() records this fact before the branch's joined type can erase it (D-562).
    if let Some(ty) = j.get("type") {
        if t.temporary_value(Some(n), Some(&typ(ty))) {
            t.temporary_values.insert(t.core_expr(n).id());
        }
    }
}
fn check_temporary(j: &Json, want: &Json) {
    let mut toks = vec![];
    let fortis_syntax::tree::Child::Node(node) = expression(j, &mut toks) else {
        panic!("expression node")
    };
    let libs = libraries();
    let sess = crate::decls::Session::new(&libs);
    let mut t = typer(&sess);
    let n = N::new(&node, &toks);
    remember_types(&mut t, n, j);
    assert_eq!(&Json::Bool(t.temporary_value(Some(n), None)), want);
}

fn check_provenance(j: &Json, expected: &Json) {
    let libs = libraries();
    let sess = crate::decls::Session::new(&libs);
    let mut t = typer(&sess);
    let (have, want) = (typ(j.get("have").unwrap()), typ(j.get("want").unwrap()));
    let token = Token::new(Kind::Lower, "value", 1, 1);
    let result = match s(j, "op") {
        "owned_argument" => {
            let mut pairs = Vec::new();
            Typer::closure_values(&have, &want, &mut pairs);
            let got: BTreeSet<_> = pairs.iter().flat_map(|(_, cs, _)| cs.iter().cloned()).collect();
            assert_eq!(&Json::Arr(got.into_iter().map(|(o, m)| Json::Arr(vec![Json::Str(o), Json::Str(m)])).collect()), expected);
            return;
        },
        "substitute" => t.subst(&have, &BTreeMap::from([("T".into(), want)])),
        "infer" => {
            let mut m = BTreeMap::from([("T".into(), want)]);
            t.unify(&Type::TypeParam("T".into()), &have, &mut m);
            m.remove("T").unwrap()
        },
        "annotation" => t.with_closure(&want, &have),
        "coalesce" => t.coalesce_result(&want, &have, None, None, &env_new(None)),
        _ => t.unify_branches(&[(want, token.clone()), (have, token)], "unit", None),
    };
    let mut origins = Vec::new();
    Typer::captured_loans(&result, &mut origins);
    origins.sort(); origins.dedup();
    assert_eq!(&Json::Arr(origins.into_iter().map(|(o, m)| Json::Arr(vec![Json::Str(o), Json::Str(m)])).collect()), expected);
    assert_eq!(codes(&t), Json::Arr(vec![]));
}

fortis_testkit::case_tests!("ownership/provenance.json", check_provenance;
    d645_owned_argument_plain,
    d645_owned_argument_optional,
    d645_owned_argument_tuple,
    d645_owned_argument_record,
    d645_owned_argument_outcome,
    d645_owned_argument_list,
    d645_substitute_plain,
    d645_infer_plain,
    d645_annotation_plain,
    d645_branches_plain,
    d645_coalesce_plain,
    d645_substitute_optional,
    d645_infer_optional,
    d645_annotation_optional,
    d645_branches_optional,
    d645_coalesce_optional,
    d645_substitute_tuple,
    d645_infer_tuple,
    d645_annotation_tuple,
    d645_branches_tuple,
    d645_coalesce_tuple,
    d645_substitute_record,
    d645_infer_record,
    d645_annotation_record,
    d645_branches_record,
    d645_coalesce_record,
    d645_substitute_outcome,
    d645_infer_outcome,
    d645_annotation_outcome,
    d645_branches_outcome,
    d645_coalesce_outcome,
    d645_substitute_list,
    d645_infer_list,
    d645_annotation_list,
    d645_branches_list,
    d645_coalesce_list
);
