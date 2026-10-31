//! LR 6.7: direct host collection operations and runtime calls emitted by native lowering.
use fortis_lower::{
    gen::{FnGen, Want},
    Audit,
};
use fortis_mir::{build::Fb, *};
use fortis_testkit::Json;
use fortis_types::types::{named, named_args, optional};
use std::collections::HashMap;
fn check(d: &Json, expected: &Json) {
    let s = |k| d.get(k).unwrap().as_str().unwrap();
    let elem = named(s("element"));
    let ct = named_args(
        s("type"),
        if s("type") == "Map" {
            vec![named("Int"), elem.clone()]
        } else {
            vec![elem.clone()]
        },
    );
    let ret = match s("result") {
        "Optional" => optional(elem.clone()),
        "Outcome" => named_args(
            "Outcome",
            vec![if s("method") == "set" {
                named("Void")
            } else {
                elem.clone()
            }],
        ),
        "Cursor" => named_args("ListCursor", vec![elem]),
        n => named(n),
    };
    let mode = if s("mode") == "editable" {
        Mode::Editable
    } else {
        Mode::Read
    };
    let mut fb = Fb::new("function unit", "main", ret.clone(), false);
    let id = fb.add_local("xs", ct.clone(), true, LocalKind::Let);
    let mut args = vec![Arg {
        param: "self".into(),
        mode,
        value: Expr::new(
            ExprKind::Place(
                Place::local(id),
                if mode == Mode::Editable {
                    Use::Editable
                } else {
                    Use::Read
                },
            ),
            ct,
            1,
        ),
    }];
    args.extend(
        d.get("args")
            .unwrap()
            .as_arr()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, n)| Arg {
                param: format!("arg{i}"),
                mode: Mode::Read,
                value: Expr::new(
                    ExprKind::Const(Const::Int(n.as_i64().unwrap() as i128)),
                    named("Int"),
                    1,
                ),
            }),
    );
    let p = Program::default();
    let f = fb.finish(1, None, vec![], Effects::default(), None, None);
    let mut audit = Audit::default();
    let names = HashMap::new();
    let mut g = FnGen::new(&p, &f, &mut audit, &names);
    let e = Expr::new(
        ExprKind::Call {
            callee: Callee::Intrinsic(format!("{}.{}", s("type"), s("method"))),
            args,
        },
        ret,
        1,
    );
    assert_eq!(g.expr(&e, Want::Value), expected.as_str().unwrap());
    assert!(g.errors.is_empty());
}

fortis_testkit::case_tests!("native_lower/collections.json",check;
    lr6_7_list_count_int,
    lr6_7_list_get_int,
    lr6_7_list_get_string,
    lr6_7_array_count_int,
    lr6_7_array_get_int,
    lr6_7_array_get_string,
    lr6_7_listview_count_int,
    lr6_7_listview_get_int,
    lr6_7_listview_get_string,
    lr6_7_list_push_int,
    lr6_7_list_pop_int,
    lr6_7_list_clear_int,
    lr6_7_list_set_int,
    lr6_7_array_set_int,
    lr6_7_list_at_int,
    lr6_7_list_iterate_int,
    lr6_7_map_count_int,
    lr6_7_map_remove_int,
    lr6_7_set_count_int,
    lr6_7_set_remove_int,
    lr6_7_map_set_int,
    lr6_7_map_containskey_int,
    lr6_7_map_get_int,
    lr6_7_map_get_string,
    lr6_7_set_add_int,
    lr6_7_set_contains_int
);
