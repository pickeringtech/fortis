//! Appendix C validation uses IR data directly, without a parser, checker or compiler driver.
use fortis_mir::{build::Fb, *};
use fortis_testkit::Json;
use fortis_types::types::{named, named_args, optional};
fn num() -> Expr {
    Expr::new(ExprKind::Const(Const::Int(1)), named("Int"), 7)
}
fn block(kind: StmtKind) -> Block {
    Block {
        stmts: vec![Stmt::new(7, kind)],
        ..Block::default()
    }
}
fn check(d: &Json, expect: &Json) {
    let kind = d.get("kind").unwrap().as_str().unwrap();
    let arg = d.get("arg").unwrap().as_str().unwrap();
    let mut fb = Fb::new("function unit", "main", named("Int"), false);
    let mut modifiers = vec![];
    let mut effects = Effects::default();
    // Supply the valid fail summary so these cases isolate outcome shape and control.
    if kind == "or_fail" || (kind == "catch" && arg == "fail") {
        effects.effects.push(("fail".into(), "proved".into()));
    }
    let body = match kind {
        "break" | "continue" => {
            let label =
                matches!(arg, "label" | "wrong_label" | "nested_label").then(|| "target".into());
            let s = if kind == "break" {
                StmtKind::Break(label)
            } else {
                StmtKind::Continue(label)
            };
            let b = block(s);
            match arg {
                "outer" => b,
                "loop" => block(StmtKind::Loop {
                    label: None,
                    body: b,
                }),
                "label" => block(StmtKind::Loop {
                    label: Some("target".into()),
                    body: b,
                }),
                "wrong_label" => block(StmtKind::Loop {
                    label: Some("other".into()),
                    body: b,
                }),
                _ => block(StmtKind::Loop {
                    label: Some("target".into()),
                    body: block(StmtKind::Loop {
                        label: None,
                        body: b,
                    }),
                }),
            }
        }
        "give" | "recover" => {
            let b = block(if kind == "give" {
                StmtKind::Give(num())
            } else {
                StmtKind::Recover(num())
            });
            match arg {
                "outer" => b,
                "nested" => block(StmtKind::Block(b)),
                "value" => block(StmtKind::Return(Some(Expr::new(
                    ExprKind::Block(b),
                    named("Int"),
                    7,
                )))),
                _ => {
                    let bind = fb.add_local("failure", named("Failure"), false, LocalKind::Binding);
                    block(StmtKind::Return(Some(Expr::new(
                        ExprKind::Catch {
                            value: Box::new(num()),
                            bind,
                            handler: b,
                        },
                        named("Int"),
                        7,
                    ))))
                }
            }
        }
        "call" | "construct" | "variant" | "failure" => {
            let k = match kind {
                "call" => ExprKind::Call {
                    callee: Callee::Func("missing".into()),
                    args: vec![],
                },
                "construct" => ExprKind::Construct {
                    ty: "missing".into(),
                    fields: vec![],
                },
                "variant" => ExprKind::Variant {
                    ty: "missing".into(),
                    variant: "X".into(),
                    fields: vec![],
                },
                _ => ExprKind::MakeFailure {
                    ty: "missing".into(),
                    fields: vec![],
                },
            };
            block(StmtKind::Return(Some(Expr::new(k, named("Int"), 7))))
        }
        "or_fail" | "or_recover" | "coalesce" | "opt_field" | "present" => {
            let mut v = num();
            v.ty = if arg == "optional" {
                optional(named("Int"))
            } else {
                named_args("Outcome", vec![named("Int")])
            };
            let a = Box::new(v);
            let k = match kind {
                "or_fail" => ExprKind::OrFail(a),
                "or_recover" => ExprKind::OrRecover(a, Box::new(num())),
                "coalesce" => ExprKind::Coalesce(a, Box::new(num())),
                "opt_field" => ExprKind::OptField(a, "x".into()),
                _ => ExprKind::Present(a),
            };
            block(StmtKind::Return(Some(Expr::new(k, named("Int"), 7))))
        }
        "catch" => {
            let bind = fb.add_local("failure", named("Failure"), false, LocalKind::Binding);
            let handler = match arg {
                "recover" => block(StmtKind::Recover(num())),
                "return" => block(StmtKind::Return(Some(num()))),
                "fail" => block(StmtKind::Fail(num())),
                "if_both" | "if_one" => block(StmtKind::If {
                    cond: Expr::new(ExprKind::Const(Const::Bool(true)), named("Bool"), 7),
                    then: block(StmtKind::Recover(num())),
                    els: (arg == "if_both").then(|| block(StmtKind::Recover(num()))),
                }),
                _ => Block::default(),
            };
            block(StmtKind::Return(Some(Expr::new(
                ExprKind::Catch {
                    value: Box::new(num()),
                    bind,
                    handler,
                },
                named("Int"),
                7,
            ))))
        }
        "growth" => {
            modifiers.push("noalloc".into());
            block(StmtKind::Expr(Expr::new(
                ExprKind::Call {
                    callee: Callee::Intrinsic(arg.into()),
                    args: vec![],
                },
                named("Int"),
                7,
            )))
        }
        "effect" => {
            modifiers.push("pure".into());
            effects.effects.push((arg.into(), "proved".into()));
            Block::default()
        }
        _ => panic!("unhandled unit fixture"),
    };
    let f = fb.finish(7, Some(body), modifiers, effects, None, None);
    let p = Program {
        functions: vec![f],
        ..Program::default()
    };
    let findings = fortis_validate::validate(&p);
    let mut actual: Vec<_> = findings.iter().map(|v| v.check).collect();
    actual.sort();
    actual.dedup();
    let expected: Vec<_> = expect
        .as_arr()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(actual, expected, "{kind} {arg}: {findings:?}");
    assert!(findings
        .iter()
        .all(|v| v.function == "main::function unit" && v.line == 7));
    assert_eq!(
        findings,
        fortis_validate::validate(&p),
        "deterministic findings"
    );
}

fortis_testkit::case_tests!("validation/invariants.json",check;
    lr5_break_outer,
    lr5_break_loop,
    lr5_break_label,
    lr5_break_wrong_label,
    lr5_break_nested_label,
    lr5_continue_outer,
    lr5_continue_loop,
    lr5_continue_label,
    lr5_continue_wrong_label,
    lr5_continue_nested_label,
    lr5_give_outer,
    lr5_give_nested,
    lr5_give_value,
    lr5_recover_outer,
    lr5_recover_nested,
    lr5_recover_handler,
    c1_undefined_call,
    c1_undefined_construct,
    c1_undefined_variant,
    c1_undefined_failure,
    lr10_optional_or_fail_optional,
    lr10_optional_or_fail_outcome,
    lr10_optional_or_recover_optional,
    lr10_optional_or_recover_outcome,
    lr10_optional_coalesce_optional,
    lr10_optional_coalesce_outcome,
    lr10_optional_opt_field_optional,
    lr10_optional_opt_field_outcome,
    lr10_optional_present_optional,
    lr10_optional_present_outcome,
    lr10_catch_empty,
    lr10_catch_recover,
    lr10_catch_return,
    lr10_catch_fail,
    lr10_catch_if_both,
    lr10_catch_if_one,
    c3_growth_list_push,
    c3_growth_map_set,
    c3_growth_set_add,
    c3_growth_builder_append,
    c3_growth_list_default,
    c3_growth_array_filled,
    lr11_pure_alloc,
    lr11_pure_io,
    lr11_pure_mutate,
    lr11_pure_nondet,
    lr11_pure_block
);
