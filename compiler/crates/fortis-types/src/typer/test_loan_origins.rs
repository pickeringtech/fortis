//! LR 6.11, 7.3, 7.6, 8.3: copies, child loans, return regions and capture probes.
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
fn tok(text: &str) -> Token {
    Token::new(Kind::Lower, text, 1, 1)
}
fn typer<'a>(sess: &'a crate::decls::Session<'a>) -> Typer<'a> {
    let mut dc = DeclChecker::new(sess, 0, "unit", "main", "");
    dc.table.decls.insert(
        "Must",
        Rc::new(Decl::new("class", "Must", true, "fortis.prelude", "")),
    );
    let mut d = Decl::new("class", "Box", false, "main", "");
    d.type_params = vec!["T".into()];
    d.linear = vec!["T".into()];
    d.fields
        .insert("value", (Type::TypeParam("T".into()), true, false));
    dc.table.decls.insert("Box", Rc::new(d));
    let mut ticket = Decl::new("class", "Ticket", false, "main", "");
    ticket.fields.insert("value", (named("Must"), true, false));
    dc.table.decls.insert("Ticket", Rc::new(ticket));
    let mut choice = Decl::new("enum", "Choice", false, "main", "");
    choice.variants.insert("held", vec![("value".into(), named("Must"))]);
    dc.table.decls.insert("Choice", Rc::new(choice));
    let mut phantom = Decl::new("class", "Phantom", false, "main", "");
    phantom.type_params = vec!["T".into()]; phantom.linear = vec!["T".into()];
    dc.table.decls.insert("Phantom", Rc::new(phantom));
    dc.linear_params.insert("L".into());
    dc.tparam_bounds.insert("L".into(), BTreeSet::from(["Copyable".into()]));
    Typer::new(dc)
}
fn case_named(name: &str, args: Vec<Type>) -> Type {
    // A user type carries its declaration identity after R4/R7 (D-592).
    if matches!(name, "Box" | "Ticket" | "Choice" | "Phantom") {
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
        "unknown" => unknown(""),
        "parameter" => Type::TypeParam(a[1].as_str().unwrap().into()),
        "optional" => optional(typ(&a[1])),
        "tuple" => Type::Tuple(a[1..].iter().map(typ).collect()),
        "record" => Type::Record(vec![("value".into(), typ(&a[1]))]),
        "fn" => Type::Fn {
            capture_origins: Vec::new(),
            params: vec![],
            ret: Box::new(void()),
            kind: "fn".into(),
            modifiers: vec![],
            origin: String::new(),
            dynamic: false,
            identity: String::new(),
            param_modes: vec![],
        },
        "loan" => Type::Loan {
            inner: Box::new(typ(&a[1])),
            mode: "alias".into(),
            origin: "p".into(),
        },
        n => case_named(n, a[1..].iter().map(typ).collect()),
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
fn node(rule: &str, children: Vec<fortis_syntax::tree::Child>) -> fortis_syntax::tree::Node {
    fortis_syntax::tree::Node {
        rule: fortis_syntax::generated::RULE_NAMES
            .iter()
            .position(|r| *r == rule)
            .unwrap() as u16,
        children,
        inferred_head: Default::default(),
    }
}
fn check_return(j: &Json, want: &Json) {
    let ts = vec![tok(s(j, "source"))];
    let n = node("primaryExpr", vec![fortis_syntax::tree::Child::Tok(0)]);
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    let env = env_new(None);
    for name in ["p", "q"] {
        env_set(&env, name, string(), false, "param");
    }
    env_set(&env, "local", string(), false, "let");
    let origin = s(j, "origin");
    let source = s(j, "source");
    let ret = if origin == "none" {
        named("StringView")
    } else {
        Type::Loan {
            inner: Box::new(string()),
            mode: "alias".into(),
            origin: origin.into(),
        }
    };
    t.ret_type = Some(if j.get("optional") == Some(&Json::Bool(true)) {
        optional(ret)
    } else {
        ret
    });
    t.pending = Some((source.into(), "alias".into(), ts[0].clone()));
    t.check_returned_loans(
        N::new(&n, &ts),
        &Type::Loan {
            inner: Box::new(string()),
            mode: "alias".into(),
            origin: source.into(),
        },
        &env,
        "return",
    );
    assert_eq!(&codes(&t), want);
}
fn check_child(j: &Json, want: &Json) {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    let env = env_new(None);
    env_set(&env, "owner", string(), true, "mut");
    for name in ["parent", "child"] {
        env_set(&env, name, string(), false, "let");
    }
    t.own
        .add_loan("owner", s(j, "parent"), Some("parent"), 1, 1);
    t.own.add_loan("parent", s(j, "child"), Some("child"), 2, 2);
    t.use_place(
        Some(s(j, "target")),
        &tok(s(j, "target")),
        s(j, "use"),
        &env,
    );
    assert_eq!(&codes(&t), want);
}
fn check_copy(j: &Json, want: &Json) {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let t = typer(&sess);
    assert_eq!(&Json::Bool(t.dc.is_copyable(&typ(j))), want);
}
fn expression(j: &Json, toks: &mut Vec<Token>) -> fortis_syntax::tree::Child {
    use fortis_syntax::tree::Child;
    if let Some(pair) = j.get("token") {
        let a = pair.as_arr().unwrap();
        let kind = match a[0].as_str().unwrap() {
            "KW" => Kind::Kw,
            "PUNCT" => Kind::Punct,
            "STR" => Kind::Str,
            "INT" => Kind::Int,
            "LOWER" => Kind::Lower,
            _ => panic!("token kind"),
        };
        let i = toks.len();
        toks.push(Token::new(kind, a[1].as_str().unwrap(), 1, 1));
        return Child::Tok(i);
    }
    let kids = j
        .get("kids")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .map(|c| expression(c, toks))
        .collect();
    Child::Node(node(s(j, "rule"), kids))
}
fn check_capture(j: &Json, want: &Json) {
    let mut ts = vec![];
    let fortis_syntax::tree::Child::Node(n) = expression(j.get("tree").unwrap(), &mut ts) else {
        panic!("node")
    };
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    let env = env_new(None);
    env_set(&env, "value", named(s(j, "type")), true, "mut");
    let n = N::new(&n, &ts);
    if j.get("direct") == Some(&Json::Bool(true)) {
        t.infer_capture_node = Some(n.id());
    }
    t.infer_lambda_captures(n, &env, None);
    let head = Json::Arr(
        n.node
            .inferred_head
            .get()
            .map(|p| {
                p.toks
                    .iter()
                    .filter(|t| !matches!(t.kind, Kind::Eof | Kind::Nl | Kind::Newline))
                    .map(|t| Json::str(&t.text))
                    .collect()
            })
            .unwrap_or_default(),
    );
    assert_eq!(&Json::obj([("codes", codes(&t)), ("head", head)]), want);
    assert!(t.own.moved.is_empty());
    assert!(t.eff.effects.is_empty());
}
#[test]
fn lr_7_3_child_ends_without_ending_parent() {
    forall(
        |r| (r.between(1, 7), r.between(1, 7)),
        200,
        None,
        |&(a, b)| {
            let (parent, child) = (a.min(b), a.max(b) + 1);
            let mut own = Owners::default();
            own.add_loan("owner", "editable", Some("parent"), parent, 1);
            own.add_loan("parent", "alias", Some("child"), child, 1);
            own.end_block(child);
            assert!(own.holders.contains("parent"));
            assert!(!own.holders.contains("child"));
            assert_eq!(own.owner_path("parent.field"), "owner.field");
        },
    );
}
mod returns {
    use super::*;
    fortis_testkit::case_tests!("loan_origins/returns.json", check_return;
    lr_7_6_from_p_as_p_direct,
    lr_7_6_from_p_as_p_optional,
    lr_7_6_from_p_as_q_direct,
    lr_7_6_from_p_as_q_optional,
    lr_7_6_from_p_as_none_direct,
    lr_7_6_from_p_as_none_optional,
    lr_7_6_from_q_as_p_direct,
    lr_7_6_from_q_as_p_optional,
    lr_7_6_from_q_as_q_direct,
    lr_7_6_from_q_as_q_optional,
    lr_7_6_from_q_as_none_direct,
    lr_7_6_from_q_as_none_optional,
    lr_7_6_from_local_as_p_direct,
    lr_7_6_from_local_as_p_optional,
    lr_7_6_from_local_as_q_direct,
    lr_7_6_from_local_as_q_optional,
    lr_7_6_from_local_as_none_direct,
    lr_7_6_from_local_as_none_optional,
    lr_7_6_from_self_as_p_direct,
    lr_7_6_from_self_as_p_optional,
    lr_7_6_from_self_as_q_direct,
    lr_7_6_from_self_as_q_optional,
    lr_7_6_from_self_as_none_direct,
    lr_7_6_from_self_as_none_optional
    );
}
mod children {
    use super::*;
    fortis_testkit::case_tests!("loan_origins/children.json", check_child;
    lr_7_3_alias_alias_owner_read,
    lr_7_3_alias_alias_owner_copy,
    lr_7_3_alias_alias_owner_alias,
    lr_7_3_alias_alias_owner_editable,
    lr_7_3_alias_alias_owner_write,
    lr_7_3_alias_alias_owner_move,
    lr_7_3_alias_alias_parent_read,
    lr_7_3_alias_alias_parent_copy,
    lr_7_3_alias_alias_parent_alias,
    lr_7_3_alias_alias_parent_editable,
    lr_7_3_alias_alias_parent_write,
    lr_7_3_alias_alias_parent_move,
    lr_7_3_alias_alias_child_read,
    lr_7_3_alias_alias_child_copy,
    lr_7_3_alias_alias_child_alias,
    lr_7_3_alias_alias_child_editable,
    lr_7_3_alias_alias_child_write,
    d647_lr_7_3_alias_alias_child_move,
    lr_7_3_editable_alias_owner_read,
    lr_7_3_editable_alias_owner_copy,
    lr_7_3_editable_alias_owner_alias,
    lr_7_3_editable_alias_owner_editable,
    lr_7_3_editable_alias_owner_write,
    lr_7_3_editable_alias_owner_move,
    lr_7_3_editable_alias_parent_read,
    lr_7_3_editable_alias_parent_copy,
    lr_7_3_editable_alias_parent_alias,
    lr_7_3_editable_alias_parent_editable,
    lr_7_3_editable_alias_parent_write,
    lr_7_3_editable_alias_parent_move,
    d647_lr_7_3_editable_alias_child_read,
    d647_lr_7_3_editable_alias_child_copy,
    d647_lr_7_3_editable_alias_child_alias,
    lr_7_3_editable_alias_child_editable,
    lr_7_3_editable_alias_child_write,
    d647_lr_7_3_editable_alias_child_move,
    lr_7_3_editable_editable_owner_read,
    lr_7_3_editable_editable_owner_copy,
    lr_7_3_editable_editable_owner_alias,
    lr_7_3_editable_editable_owner_editable,
    lr_7_3_editable_editable_owner_write,
    lr_7_3_editable_editable_owner_move,
    lr_7_3_editable_editable_parent_read,
    lr_7_3_editable_editable_parent_copy,
    lr_7_3_editable_editable_parent_alias,
    lr_7_3_editable_editable_parent_editable,
    lr_7_3_editable_editable_parent_write,
    lr_7_3_editable_editable_parent_move,
    d647_lr_7_3_editable_editable_child_read,
    d647_lr_7_3_editable_editable_child_copy,
    d647_lr_7_3_editable_editable_child_alias,
    d647_lr_7_3_editable_editable_child_editable,
    lr_7_3_editable_editable_child_write,
    d647_lr_7_3_editable_editable_child_move
    );
}
mod copies {
    use super::*;
    fortis_testkit::case_tests!("loan_origins/copies.json", check_copy;
    lr_6_11_copy_int_plain,
    lr_6_11_copy_int_optional,
    lr_6_11_copy_int_list,
    lr_6_11_copy_int_tuple,
    lr_6_11_copy_int_record,
    lr_6_11_copy_int_box,
    lr_6_11_copy_string_plain,
    lr_6_11_copy_string_optional,
    lr_6_11_copy_string_list,
    lr_6_11_copy_string_tuple,
    lr_6_11_copy_string_record,
    lr_6_11_copy_string_box,
    lr_6_11_copy_bytes_plain,
    lr_6_11_copy_bytes_optional,
    lr_6_11_copy_bytes_list,
    lr_6_11_copy_bytes_tuple,
    lr_6_11_copy_bytes_record,
    lr_6_11_copy_bytes_box,
    lr_6_11_copy_shared_plain,
    lr_6_11_copy_shared_optional,
    lr_6_11_copy_shared_list,
    lr_6_11_copy_shared_tuple,
    lr_6_11_copy_shared_record,
    lr_6_11_copy_shared_box,
    lr_6_11_copy_weak_plain,
    lr_6_11_copy_weak_optional,
    lr_6_11_copy_weak_list,
    lr_6_11_copy_weak_tuple,
    lr_6_11_copy_weak_record,
    lr_6_11_copy_weak_box,
    lr_6_11_copy_mutex_plain,
    lr_6_11_copy_mutex_optional,
    lr_6_11_copy_mutex_list,
    lr_6_11_copy_mutex_tuple,
    lr_6_11_copy_mutex_record,
    lr_6_11_copy_mutex_box,
    lr_6_11_copy_outcome_plain,
    lr_6_11_copy_outcome_optional,
    lr_6_11_copy_outcome_list,
    lr_6_11_copy_outcome_tuple,
    lr_6_11_copy_outcome_record,
    lr_6_11_copy_outcome_box,
    lr_6_11_copy_future_plain,
    lr_6_11_copy_future_optional,
    lr_6_11_copy_future_list,
    lr_6_11_copy_future_tuple,
    lr_6_11_copy_future_record,
    lr_6_11_copy_future_box,
    d649_class,
    d649_enum,
    d649_phantom,
    d649_empty_phantom,
    d649_parameter,
    d649_lr_6_11_copy_must_plain,
    d649_lr_6_11_copy_must_optional,
    d649_lr_6_11_copy_must_list,
    d649_lr_6_11_copy_must_tuple,
    d649_lr_6_11_copy_must_record,
    lr_6_11_copy_must_box,
    lr_6_11_copy_closure_plain,
    lr_6_11_copy_closure_optional,
    lr_6_11_copy_closure_list,
    lr_6_11_copy_closure_tuple,
    lr_6_11_copy_closure_record,
    lr_6_11_copy_closure_box,
    lr_6_11_copy_loan_plain,
    lr_6_11_copy_loan_optional,
    lr_6_11_copy_loan_list,
    lr_6_11_copy_loan_tuple,
    lr_6_11_copy_loan_record,
    lr_6_11_copy_loan_box
    );
}
mod captures {
    use super::*;
    fortis_testkit::case_tests!("loan_origins/captures.json", check_capture;
    lr_8_3_inferred_int_read_callback,
    lr_8_3_inferred_int_read_stored,
    lr_8_3_inferred_int_copy_callback,
    lr_8_3_inferred_int_copy_stored,
    lr_8_3_inferred_int_move_callback,
    lr_8_3_inferred_int_move_stored,
    lr_8_3_inferred_int_editable_callback,
    lr_8_3_inferred_int_editable_stored,
    lr_8_3_inferred_int_write_callback,
    lr_8_3_inferred_int_write_stored,
    lr_8_3_inferred_string_read_callback,
    lr_8_3_inferred_string_read_stored,
    lr_8_3_inferred_string_copy_callback,
    lr_8_3_inferred_string_copy_stored,
    lr_8_3_inferred_string_move_callback,
    lr_8_3_inferred_string_move_stored,
    lr_8_3_inferred_string_editable_callback,
    lr_8_3_inferred_string_editable_stored,
    lr_8_3_inferred_string_write_callback,
    lr_8_3_inferred_string_write_stored
    );
}


mod result_loans {
    use super::*;
    fn check(input: &Json, expect: &Json) {
        let libs = libs();
        let sess = crate::decls::Session::new(&libs);
        assert_eq!(Json::Bool(typer(&sess).carries_onward(&typ(input))), *expect);
    }
    fortis_testkit::case_tests!("loan_origins/results.json", check;
        lr_7_6_scalar,
        lr_7_6_owned_text,
        lr_7_6_view,
        lr_7_6_record_view,
        lr_7_6_record_scalar,
        lr_7_6_tuple_view,
        lr_7_6_optional_view,
        lr_7_6_outcome_view,
        lr_7_6_future_scalar,
        lr_7_6_unknown,
        lr_7_6_record_unknown,
        lr_7_6_generic_result,
        lr_7_6_generic_unknown
    );
}
