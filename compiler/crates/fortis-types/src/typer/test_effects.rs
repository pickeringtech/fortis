//! LR 11.1-11.6: direct summary and judgement tests, with in-memory declaration heads.
use super::*;
use crate::effects::{contract_contradictions, Summary};
use fortis_names::{Libraries, Library};
use fortis_testkit::{forall, Json};
fn libs() -> Libraries {
    Libraries {
        libs: vec![Library {
            modules: BTreeMap::new(),
        }],
    }
}
fn typer<'a>(sess: &'a crate::decls::Session<'a>) -> Typer<'a> {
    Typer::new(DeclChecker::new(sess, 0, "unit", "main", ""))
}
fn sig(mods: Vec<String>) -> Sig<'static> {
    Sig::simple("unit", vec![], void(), mods)
}
fn s<'a>(j: &'a Json, k: &str) -> &'a str {
    j.get(k).unwrap().as_str().unwrap()
}
fn strings(j: &Json) -> Vec<String> {
    j.as_arr()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect()
}
fn statuses(summary: &Summary) -> Json {
    Json::Obj(
        summary
            .effects
            .iter()
            .map(|(e, _)| (e.clone(), Json::str(summary.status(e))))
            .collect(),
    )
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
fn check_callable(j: &Json, want: &Json) {
    let modifiers = strings(j);
    assert_eq!(
        &statuses(&Typer::summary_from_modifiers(&sig(modifiers.clone()))),
        want
    );
    if modifiers
        .iter()
        .all(|m| ["pure", "noalloc"].contains(&m.as_str()))
    {
        let libraries = libs();
        let sess = crate::decls::Session::new(&libraries);
        let t = typer(&sess);
        let mut signature = sig(vec![]);
        signature.params = vec![(
            "callback".into(),
            Type::Fn {
                params: vec![],
                ret: Box::new(void()),
                kind: "fn".into(),
                modifiers,
                origin: String::new(),
                dynamic: false,
                identity: String::new(),
                capture_origins: vec![],
                param_modes: vec![],
            },
            String::new(),
        )];
        assert_eq!(
            &statuses(&t.callee_summary(None, &signature, "function unit")),
            want
        );
    }
}
fn check_import(j: &Json, want: &Json) {
    let mut source = Summary::default();
    let e = s(j, "effect");
    match s(j, "certainty") {
        "certain" => source.add(e, "trusted", None),
        k => source.add_uncertain(e, "trusted", k, None),
    }
    let mut dest = Summary::default();
    dest.imports(&source, Some(&Token::new(Kind::Lower, "call", 2, 1)), &[]);
    assert_eq!(
        dest.effects,
        vec![(s(want, "effect").into(), "trusted".into())]
    );
    assert_eq!(dest.status(e), s(want, "status"));
    assert_eq!(dest.sites[e].line, 2);
}
fn check_claim(j: &Json, want: &Json) {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    t.eff.add(s(j, "effect"), "proved", None);
    t.check_claims(
        &sig(vec![s(j, "modifier").into()]),
        &Token::new(Kind::Lower, "unit", 1, 1),
        "function unit",
    );
    assert_eq!(&codes(&t), want);
}
fn parts(j: &Json) -> Vec<(String, BTreeMap<String, BTreeSet<String>>)> {
    j.as_arr()
        .unwrap()
        .iter()
        .map(|row| {
            let a = row.as_arr().unwrap();
            let Json::Obj(items) = &a[1] else {
                panic!("contract items")
            };
            (
                a[0].as_str().unwrap().into(),
                items
                    .iter()
                    .map(|(k, v)| (k.clone(), strings(v).into_iter().collect()))
                    .collect(),
            )
        })
        .collect()
}
fn check_contract(j: &Json, want: &Json) {
    let parts = parts(j);
    let actual = Json::Arr(
        contract_contradictions(&parts, false)
            .iter()
            .map(|c| {
                Json::Arr(
                    [c.kind, &c.first, &c.second, &c.claim, &c.effect]
                        .iter()
                        .map(|s| Json::str(*s))
                        .collect(),
                )
            })
            .collect(),
    );
    assert_eq!(&actual, want);
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    let mut signature = sig(vec![]);
    for (origin, items) in parts {
        signature.contracts.push(origin.clone());
        let mut d = Decl::new("contract", &origin, false, "main", "");
        d.items = items;
        t.dc.table.decls.insert(&origin, Rc::new(d));
    }
    t.check_contradictions(
        &signature,
        &Token::new(Kind::Lower, "unit", 1, 1),
        "function unit",
    );
    assert_eq!(
        codes(&t),
        Json::Arr(if want.as_arr().unwrap().is_empty() {
            vec![]
        } else {
            vec![Json::str("E-EFF-050")]
        })
    );
}
fn check_authority(j: &Json, want: &Json) {
    for shared in [false, true] {
        let libraries = libs();
        let sess = crate::decls::Session::new(&libraries);
        let mut t = typer(&sess);
        let name = s(j, "name");
        let shadow = j.get("shadow") == Some(&Json::Bool(true));
        let mut d = Decl::new(
            "class",
            name,
            false,
            if shadow { "main" } else { "fortis.prelude" },
            "",
        );
        if !shadow {
            d.implements = vec![nt("Authority")];
        }
        let own_type = crate::decls::named_of_decl(&d, vec![]);
        t.dc.table.decls.insert(name.into(), Rc::new(d));
        let ty = if shared {
            named_args("Shared", vec![own_type])
        } else {
            own_type
        };
        assert_eq!(&Json::Bool(t.is_authority(&ty)), want);
        t.eff.add("io", "proved", None);
        t.eff.needs_auth = true;
        let mut signature = sig(vec![]);
        signature.params = vec![("authority".into(), ty, String::new())];
        t.check_claims(
            &signature,
            &Token::new(Kind::Lower, "unit", 1, 1),
            "function unit",
        );
        assert_eq!(
            codes(&t),
            Json::Arr(if *want == Json::Bool(true) {
                vec![]
            } else {
                vec![Json::str("E-EFF-010")]
            })
        );
    }
}
fn check_footprint(j: &Json, want: &Json) {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    let env = env_new(None);
    env_set(&env, "value", int(), false, s(j, "kind"));
    let mut place = "value";
    if j.get("alias") == Some(&Json::Bool(true)) {
        t.own.add_loan("value", "editable", Some("child"), 1, 1);
        place = "child";
    }
    let mut callee = Summary::default();
    callee.footprint.insert("argument".into());
    t.import_call(
        &callee,
        &Token::new(Kind::Lower, "call", 2, 1),
        None,
        &[("argument".into(), place.into())],
        &env,
        false,
    );
    let actual = Json::obj([
        (
            "effects",
            Json::Arr(t.eff.effect_set().into_iter().map(Json::Str).collect()),
        ),
        (
            "footprint",
            Json::Arr(t.eff.footprint.into_iter().map(Json::Str).collect()),
        ),
    ]);
    assert_eq!(&actual, want);
}
const EFFECTS: [&str; 8] = [
    "alloc", "io", "mutate", "nondet", "suspend", "block", "fail", "recovers",
];
fn summary(mask: i64) -> Summary {
    let mut s = Summary::default();
    for (i, e) in EFFECTS.iter().enumerate() {
        if mask >> i & 1 == 1 {
            s.add(e, "proved", None);
        }
    }
    s
}
#[test]
fn lr_11_3_union_is_associative_and_idempotent() {
    forall(
        |r| (r.between(0, 255), r.between(0, 255), r.between(0, 255)),
        200,
        None,
        |&(x, y, z)| {
            let (a, b, c) = (summary(x), summary(y), summary(z));
            let mut left = Summary::default();
            left.imports(&a, None, &[]);
            left.imports(&b, None, &[]);
            left.imports(&c, None, &[]);
            let mut bc = b.clone();
            bc.imports(&c, None, &[]);
            let mut right = a.clone();
            right.imports(&bc, None, &[]);
            right.imports(&a, None, &[]);
            assert_eq!(left.effect_set(), right.effect_set());
            assert_eq!(left.effect_set(), summary(x | y | z).effect_set());
        },
    );
}
#[test]
fn lr_11_3_call_imports_all_effect_subsets() {
    forall(
        |r| r.between(0, 255),
        1,
        None,
        |&offset| {
            for i in 0..256 {
                for asynchronous in [false, true] {
                    let libraries = libs();
                    let sess = crate::decls::Session::new(&libraries);
                    let mut t = typer(&sess);
                    let callee = summary((i + offset) % 256);
                    t.import_call(
                        &callee,
                        &Token::new(Kind::Lower, "call", 1, 1),
                        None,
                        &[],
                        &env_new(None),
                        asynchronous,
                    );
                    let mut want = callee.effect_set();
                    want.remove("fail");
                    if asynchronous {
                        want.remove("suspend");
                    }
                    assert_eq!(t.eff.effect_set(), want);
                }
            }
        },
    );
}
#[test]
fn certain_evidence_dominates_callback_and_trust_uncertainty() {
    forall(
        |r| r.between(0, 5),
        200,
        None,
        |&order| {
            let orders = [
                [0, 1, 2],
                [0, 2, 1],
                [1, 0, 2],
                [1, 2, 0],
                [2, 0, 1],
                [2, 1, 0],
            ];
            let mut s = Summary::default();
            for mode in orders[order as usize] {
                match mode {
                    0 => s.add("alloc", "proved", None),
                    1 => s.add_uncertain("alloc", "trusted", "possible", None),
                    _ => s.add_uncertain("alloc", "trusted", "unknown", None),
                }
            }
            assert_eq!(s.status("alloc"), "violated");
            assert_eq!(s.effects, vec![("alloc".into(), "proved".into())]);
        },
    );
}
#[test]
fn lr_11_5_same_contract_is_checked_once() {
    forall(
        |r| r.between(0, 2),
        200,
        None,
        |&i| {
            let (m, e) = [
                ("try", "fail"),
                ("async", "suspend"),
                ("recovers", "recovers"),
            ][i as usize];
            let parts = vec![(
                "C".into(),
                BTreeMap::from([
                    ("modifiers".into(), BTreeSet::from([m.into()])),
                    ("forbids".into(), BTreeSet::from([e.into()])),
                ]),
            )];
            assert_eq!(contract_contradictions(&parts, false).len(), 1);
            assert!(contract_contradictions(&parts, true).is_empty());
        },
    );
}
mod callables {
    use super::*;
    fortis_testkit::case_tests!("effects/callables.json", check_callable;
    lr_11_3_callable_modifiers_00,
    lr_11_3_callable_modifiers_01,
    lr_11_3_callable_modifiers_02,
    lr_11_3_callable_modifiers_03,
    lr_11_3_callable_modifiers_04,
    lr_11_3_callable_modifiers_05,
    lr_11_3_callable_modifiers_06,
    lr_11_3_callable_modifiers_07,
    lr_11_3_callable_modifiers_08,
    lr_11_3_callable_modifiers_09,
    lr_11_3_callable_modifiers_10,
    lr_11_3_callable_modifiers_11,
    lr_11_3_callable_modifiers_12,
    lr_11_3_callable_modifiers_13,
    lr_11_3_callable_modifiers_14,
    lr_11_3_callable_modifiers_15,
    lr_11_3_callable_modifiers_16,
    lr_11_3_callable_modifiers_17,
    lr_11_3_callable_modifiers_18,
    lr_11_3_callable_modifiers_19,
    lr_11_3_callable_modifiers_20,
    lr_11_3_callable_modifiers_21,
    lr_11_3_callable_modifiers_22,
    lr_11_3_callable_modifiers_23,
    lr_11_3_callable_modifiers_24,
    lr_11_3_callable_modifiers_25,
    lr_11_3_callable_modifiers_26,
    lr_11_3_callable_modifiers_27,
    lr_11_3_callable_modifiers_28,
    lr_11_3_callable_modifiers_29,
    lr_11_3_callable_modifiers_30,
    lr_11_3_callable_modifiers_31
    );
}
mod imports {
    use super::*;
    fortis_testkit::case_tests!("effects/imports.json", check_import;
    lr_11_3_import_alloc_certain,
    lr_11_3_import_alloc_possible,
    lr_11_3_import_alloc_unknown,
    lr_11_3_import_io_certain,
    lr_11_3_import_io_possible,
    lr_11_3_import_io_unknown,
    lr_11_3_import_mutate_certain,
    lr_11_3_import_mutate_possible,
    lr_11_3_import_mutate_unknown,
    lr_11_3_import_nondet_certain,
    lr_11_3_import_nondet_possible,
    lr_11_3_import_nondet_unknown,
    lr_11_3_import_suspend_certain,
    lr_11_3_import_suspend_possible,
    lr_11_3_import_suspend_unknown,
    lr_11_3_import_block_certain,
    lr_11_3_import_block_possible,
    lr_11_3_import_block_unknown,
    lr_11_3_import_fail_certain,
    lr_11_3_import_fail_possible,
    lr_11_3_import_fail_unknown,
    lr_11_3_import_recovers_certain,
    lr_11_3_import_recovers_possible,
    lr_11_3_import_recovers_unknown
    );
}
mod claims {
    use super::*;
    fortis_testkit::case_tests!("effects/claims.json", check_claim;
    lr_11_2_pure_alloc,
    lr_11_2_pure_io,
    lr_11_2_pure_mutate,
    lr_11_2_pure_nondet,
    lr_11_2_pure_suspend,
    lr_11_2_pure_block,
    lr_11_2_pure_recovers,
    lr_11_2_noalloc_alloc,
    lr_11_2_noalloc_io,
    lr_11_2_noalloc_mutate,
    lr_11_2_noalloc_nondet,
    lr_11_2_noalloc_suspend,
    lr_11_2_noalloc_block,
    lr_11_2_noalloc_recovers,
    lr_11_2_async_alloc,
    lr_11_2_async_io,
    lr_11_2_async_mutate,
    lr_11_2_async_nondet,
    lr_11_2_async_suspend,
    lr_11_2_async_block,
    lr_11_2_async_recovers,
    lr_11_2_recovers_alloc,
    lr_11_2_recovers_io,
    lr_11_2_recovers_mutate,
    lr_11_2_recovers_nondet,
    lr_11_2_recovers_suspend,
    lr_11_2_recovers_block,
    lr_11_2_recovers_recovers
    );
}
mod contracts {
    use super::*;
    fortis_testkit::case_tests!("effects/contracts.json", check_contract;
    lr_11_5_try_forbids_alloc,
    lr_11_5_try_forbids_io,
    lr_11_5_try_forbids_mutate,
    lr_11_5_try_forbids_nondet,
    lr_11_5_try_forbids_suspend,
    lr_11_5_try_forbids_block,
    lr_11_5_try_forbids_fail,
    lr_11_5_try_forbids_recovers,
    lr_11_5_async_forbids_alloc,
    lr_11_5_async_forbids_io,
    lr_11_5_async_forbids_mutate,
    lr_11_5_async_forbids_nondet,
    lr_11_5_async_forbids_suspend,
    lr_11_5_async_forbids_block,
    lr_11_5_async_forbids_fail,
    lr_11_5_async_forbids_recovers,
    lr_11_5_recovers_forbids_alloc,
    lr_11_5_recovers_forbids_io,
    lr_11_5_recovers_forbids_mutate,
    lr_11_5_recovers_forbids_nondet,
    lr_11_5_recovers_forbids_suspend,
    lr_11_5_recovers_forbids_block,
    lr_11_5_recovers_forbids_fail,
    lr_11_5_recovers_forbids_recovers,
    lr_11_5_pure_requires_filesystem,
    lr_11_5_requires_excludes_filesystem,
    lr_11_5_disjoint_excludes_filesystem,
    lr_11_5_pure_requires_network,
    lr_11_5_requires_excludes_network,
    lr_11_5_disjoint_excludes_network,
    lr_11_5_pure_requires_clock,
    lr_11_5_requires_excludes_clock,
    lr_11_5_disjoint_excludes_clock,
    lr_11_5_pure_requires_random,
    lr_11_5_requires_excludes_random,
    lr_11_5_disjoint_excludes_random,
    lr_11_5_pure_requires_environment,
    lr_11_5_requires_excludes_environment,
    lr_11_5_disjoint_excludes_environment,
    lr_11_5_pure_requires_process,
    lr_11_5_requires_excludes_process,
    lr_11_5_disjoint_excludes_process,
    lr_11_5_pure_requires_supervisor,
    lr_11_5_requires_excludes_supervisor,
    lr_11_5_disjoint_excludes_supervisor,
    lr_11_5_pure_requires_console,
    lr_11_5_requires_excludes_console,
    lr_11_5_disjoint_excludes_console,
    lr_11_5_pure_requires_secretaccess,
    lr_11_5_requires_excludes_secretaccess,
    lr_11_5_disjoint_excludes_secretaccess,
    lr_11_5_pure_requires_rawsql,
    lr_11_5_requires_excludes_rawsql,
    lr_11_5_disjoint_excludes_rawsql
    );
}
mod authorities {
    use super::*;
    fortis_testkit::case_tests!("effects/authorities.json", check_authority;
    lr_11_6_filesystem_authority,
    lr_11_6_filesystem_local_shadow,
    lr_11_6_network_authority,
    lr_11_6_network_local_shadow,
    lr_11_6_clock_authority,
    lr_11_6_clock_local_shadow,
    lr_11_6_random_authority,
    lr_11_6_random_local_shadow,
    lr_11_6_environment_authority,
    lr_11_6_environment_local_shadow,
    lr_11_6_process_authority,
    lr_11_6_process_local_shadow,
    lr_11_6_supervisor_authority,
    lr_11_6_supervisor_local_shadow,
    lr_11_6_console_authority,
    lr_11_6_console_local_shadow,
    lr_11_6_secretaccess_authority,
    lr_11_6_secretaccess_local_shadow,
    lr_11_6_rawsql_authority,
    lr_11_6_rawsql_local_shadow
    );
}
mod footprints {
    use super::*;
    fortis_testkit::case_tests!("effects/footprints.json", check_footprint;
    lr_11_3_write_let_direct,
    lr_11_3_write_let_loan_path,
    lr_11_3_write_owned_direct,
    lr_11_3_write_owned_loan_path,
    lr_11_3_write_editable_direct,
    lr_11_3_write_editable_loan_path,
    lr_11_3_write_capture_editable_direct,
    lr_11_3_write_capture_editable_loan_path,
    lr_11_3_write_capture_move_direct,
    lr_11_3_write_capture_move_loan_path,
    lr_11_3_write_param_direct,
    lr_11_3_write_param_loan_path
    );
}

#[test]
fn lr_11_5_forbidden_effects_are_transitive() {
    forall(
        |r| r.between(0, 7),
        1,
        None,
        |&offset| {
            for i in 0..8 {
                for present in [false, true] {
                    let e = EFFECTS[((i + offset) % 8) as usize];
                    let libraries = libs();
                    let sess = crate::decls::Session::new(&libraries);
                    let mut t = typer(&sess);
                    let mut d = Decl::new("contract", "C", false, "main", "");
                    d.items.insert("forbids".into(), BTreeSet::from([e.into()]));
                    t.dc.table.decls.insert("C".into(), Rc::new(d));
                    let mut callee = Summary::default();
                    if present {
                        callee.add(e, "proved", None);
                    }
                    t.eff.imports(&callee, None, &[]);
                    let mut signature = sig(vec![]);
                    signature.contracts = vec!["C".into()];
                    t.check_contracts(
                        &signature,
                        &Token::new(Kind::Lower, "unit", 1, 1),
                        "function unit",
                    );
                    assert_eq!(
                        codes(&t),
                        Json::Arr(if present {
                            vec![Json::str("E-EFF-001")]
                        } else {
                            vec![]
                        })
                    );
                }
            }
        },
    );
}
#[test]
fn lr_11_5_authority_requirements_distinguish_parameters_and_callees() {
    forall(
        |r| r.between(0, 1),
        1,
        None,
        |&offset| {
            for i in 0..2 {
                for place in ["absent", "parameter", "shared_parameter", "callee"] {
                    let clause = ["requires", "excludes"][((i + offset) % 2) as usize];
                    let libraries = libs();
                    let sess = crate::decls::Session::new(&libraries);
                    let mut t = typer(&sess);
                    let mut d = Decl::new("contract", "C", false, "main", "");
                    d.items
                        .insert(clause.into(), BTreeSet::from(["Console".into()]));
                    t.dc.table.decls.insert("C".into(), Rc::new(d));
                    let mut signature = sig(vec![]);
                    signature.contracts = vec!["C".into()];
                    if place.ends_with("parameter") {
                        let ty = if place == "shared_parameter" {
                            named_args("Shared", vec![named("Console")])
                        } else {
                            named("Console")
                        };
                        signature.params = vec![("console".into(), ty, String::new())];
                    }
                    if place == "callee" {
                        t.eff.authorities.insert("Console".into());
                    }
                    t.check_contracts(
                        &signature,
                        &Token::new(Kind::Lower, "unit", 1, 1),
                        "function unit",
                    );
                    let rejects = if clause == "requires" {
                        !["parameter", "shared_parameter"].contains(&place)
                    } else {
                        place != "absent"
                    };
                    assert_eq!(
                        codes(&t),
                        Json::Arr(if rejects {
                            vec![Json::str("E-EFF-001")]
                        } else {
                            vec![]
                        })
                    );
                }
            }
        },
    );
}

fn check_capability(j: &Json, want: &Json) {
    let libraries = libs();
    let sess = crate::decls::Session::new(&libraries);
    let mut t = typer(&sess);
    t.current_label = if j.get("entry") == Some(&Json::Bool(true)) {
        "function main"
    } else {
        "function worker"
    }
    .into();
    if s(j, "context") != "body" {
        t.ctx.push(s(j, "context").into());
    }
    let mut signature = sig(vec![]);
    if j.get("parameter") == Some(&Json::Bool(true)) {
        signature.params = vec![("console".into(), named("Console"), String::new())];
    }
    let mut d = Decl::new("func", "main", false, "main", "");
    d.sig = Some(signature.clone());
    t.dc.table.decls.insert("main".into(), Rc::new(d));
    for i in 0..j.get("calls").unwrap().as_i64().unwrap() {
        t.capabilities_use(
            &Token::new(Kind::Lower, "capabilities", i as usize + 1, 1),
            j.get("called") == Some(&Json::Bool(true)),
        );
    }
    let label = t.current_label.clone();
    t.check_claims(&signature, &Token::new(Kind::Lower, "main", 1, 1), &label);
    assert_eq!(&codes(&t), want);
}
mod capabilities {
    use super::*;
    fortis_testkit::case_tests!("effects/capabilities.json",check_capability;
    lr_11_6_host_body_main,
    lr_11_6_host_body_worker,
    lr_11_6_host_loop_main,
    lr_11_6_host_loop_worker,
    lr_11_6_host_whilecond_main,
    lr_11_6_host_whilecond_worker,
    lr_11_6_host_lambda_main,
    lr_11_6_host_lambda_worker,
    lr_11_6_host_hook_main,
    lr_11_6_host_hook_worker,
    lr_11_6_host_value_main,
    lr_11_6_host_value_worker,
    lr_11_6_host_twice,
    lr_11_6_host_parameter
    );
}
