//! LR 6.10: unification through type constructors, with null leaving T unfixed.
mod u1_support;
use std::collections::BTreeMap;
use fortis_testkit::Json;
use fortis_names::{Library, library::Libraries};
use fortis_types::{decls::{DeclChecker, Session}, typer::Typer};
fn check(input: &Json, expect: &Json) {
    let libs = Libraries { libs: vec![Library { modules: BTreeMap::new() }] };
    let session = Session::new(&libs);
    let ty = Typer::new(DeclChecker::new(&session, 0, "unit.ft", "main", ""));
    let mut found = BTreeMap::new();
    ty.unify(&u1_support::typ(input.get("pattern").unwrap()), &u1_support::typ(input.get("actual").unwrap()), &mut found);
    let Json::Obj(expected) = expect else { panic!("expected substitution map") };
    assert_eq!(found, expected.iter().map(|(k,v)| (k.clone(), u1_support::typ(v))).collect());
}
fortis_testkit::case_tests!("types/unification.json", check;
    lr610_unify_direct_int,
    lr610_unify_optional_int,
    lr610_unify_list_int,
    lr610_unify_view_int,
    lr610_unify_tuple_int,
    lr610_unify_direct_bool,
    lr610_unify_optional_bool,
    lr610_unify_list_bool,
    lr610_unify_view_bool,
    lr610_unify_tuple_bool,
    lr610_unify_direct_decimal,
    lr610_unify_optional_decimal,
    lr610_unify_list_decimal,
    lr610_unify_view_decimal,
    lr610_unify_tuple_decimal,
    lr610_unify_direct_refined,
    lr610_unify_optional_refined,
    lr610_unify_list_refined,
    lr610_unify_view_refined,
    lr610_unify_tuple_refined,
    lr610_unify_null,
    lr610_unify_never,
    lr610_unify_wrong_container,
    lr610_unify_two_params,
    f1_record_infers_by_name,
    f1_tuple_nested_infers
);
