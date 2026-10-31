//! LR 4.4: qualified calls are resolved by the expression typer.
mod u1_names_support;
use fortis_testkit::Json;
use fortis_syntax::{parse_from, parse_module, view::N};
use fortis_types::{decls::{DeclChecker, Session}, typer::{Typer, env_new}};
fn check(input: &Json, expect: &Json) {
    let libs = u1_names_support::libraries();
    let session = Session::new(&libs);
    let head = parse_module(input.get("head").unwrap().as_str().unwrap()).unwrap();
    let parsed = parse_from(input.get("source").unwrap().as_str().unwrap(), "expr").unwrap();
    let mut dc = DeclChecker::new(&session, 0, "unit.ft", "main", "");
    dc.load(N::new(&head.root, &head.toks), false);
    let mut ty = Typer::new(dc);
    ty.aliases.insert("items".into(), "acme.items".into());
    ty.infer(Some(N::new(&parsed.root,&parsed.toks)), &env_new(None), None);
    let mut codes: Vec<_> = ty.dc.records.iter().map(|r| r.code.clone()).collect();
    codes.sort(); codes.dedup();
    assert_eq!(&Json::strs(codes), expect, "{:?}", ty.dc.records);
}
fortis_testkit::case_tests!("names/member_calls.json", check;
    lr4_missing_member,
    lr4_private_member,
    lr4_module_public,
    lr4_module_constant,
    lr4_module_constructor
);
