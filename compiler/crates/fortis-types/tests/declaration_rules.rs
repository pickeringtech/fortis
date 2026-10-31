//! LR 4.4, 6.13, 9.1, 9.5 and 9.6: declaration rules, without checking bodies.
mod u1_names_support;
use fortis_testkit::Json;
use fortis_syntax::{parse_module, view::N};
use fortis_types::{decls::{DeclChecker, Session, tok_of}, typer::Typer};
fn check(input: &Json, expect: &Json) {
    let libs = u1_names_support::libraries();
    let session = Session::new(&libs);
    let parsed = parse_module(input.as_str().unwrap()).unwrap();
    let mut dc = DeclChecker::new(&session, 0, "unit.ft", "main", "");
    dc.load(N::new(&parsed.root,&parsed.toks), false);
    dc.check();
    let mut ty = Typer::new(dc);
    let decls: Vec<_> = ty.dc.table.decls.values().cloned().collect();
    for d in decls {
        if d.module == "main" && d.exported {
            if let Some(sig) = &d.sig { ty.check_exported_sig(sig, &d.name, &tok_of(&d)); }
        }
    }
    let dc = ty.dc;
    let mut codes: Vec<_> = dc.records.iter().map(|r| r.code.clone()).collect();
    codes.sort(); codes.dedup();
    assert_eq!(&Json::strs(codes), expect, "{:?}", dc.records);
}
fortis_testkit::case_tests!("names/declarations.json", check;
    lr91_duplicate_field_field,
    lr91_duplicate_field_method,
    lr91_duplicate_method_field,
    lr91_duplicate_method_method,
    lr95_complete,
    lr95_missing,
    lr95_parameter,
    lr95_result,
    lr95_arity,
    lr95_generic,
    lr95_marker,
    lr95_required_missing,
    lr95_required_present,
    lr95_cycle,
    lr95_block,
    lr95_block_duplicate,
    lr95_block_overlap,
    lr95_block_builtin,
    lr95_block_newtype,
    lr95_block_optional,
    lr95_block_generic,
    lr95_block_enum,
    lr96_provided,
    lr96_requirement,
    lr96_missing,
    lr96_wrong_requirement,
    lr96_conflict,
    lr96_override,
    lr96_fulfils_interface,
    lr613_to,
    lr613_from,
    lr613_try,
    lr613_duplicate,
    lr613_duplicate_member,
    lr613_identity,
    lr613_generic,
    lr613_refined_target,
    lr613_derived,
    lr613_missing_field,
    lr613_fallible_field,
    lr613_total_field,
    lr613_enum_text,
    lr613_enum_payload,
    lr613_enum_generic,
    lr613_enum_case_collision,
    lr613_enum_wrong_source,
    lr613_enum_view_source,
    lr613_enum_duplicate_string,
    lr613_enum_duplicate_view,
    lr44_private_class_result,
    lr44_private_class_parameter,
    lr44_private_enum_result,
    lr44_private_enum_parameter,
    lr44_private_newtype_result,
    lr44_private_newtype_parameter,
    lr44_private_interface_result,
    lr44_private_interface_parameter,
    lr95_orphan
);
#[test]
fn lr91_duplicate_field_constant() {
    let rows = fortis_testkit::read_cases("names/defects.json");
    let row = rows.iter().find(|r| r.name == "lr91_duplicate_field_constant").unwrap();
    check(&row.input, &row.expect);
}

#[test]
fn lr91_duplicate_method_constant() {
    let rows = fortis_testkit::read_cases("names/defects.json");
    let row = rows.iter().find(|r| r.name == "lr91_duplicate_method_constant").unwrap();
    check(&row.input, &row.expect);
}

#[test]
fn lr91_duplicate_constant_field() {
    let rows = fortis_testkit::read_cases("names/defects.json");
    let row = rows.iter().find(|r| r.name == "lr91_duplicate_constant_field").unwrap();
    check(&row.input, &row.expect);
}

#[test]
fn lr91_duplicate_constant_method() {
    let rows = fortis_testkit::read_cases("names/defects.json");
    let row = rows.iter().find(|r| r.name == "lr91_duplicate_constant_method").unwrap();
    check(&row.input, &row.expect);
}

#[test]
fn lr91_duplicate_constant_constant() {
    let rows = fortis_testkit::read_cases("names/defects.json");
    let row = rows.iter().find(|r| r.name == "lr91_duplicate_constant_constant").unwrap();
    check(&row.input, &row.expect);
}

#[test]
fn every_declaration_defect_is_registered() {
    let names: Vec<_> = fortis_testkit::read_cases("names/defects.json").into_iter().map(|r| r.name).collect();
    assert_eq!(names, vec!["lr91_duplicate_field_constant", "lr91_duplicate_method_constant", "lr91_duplicate_constant_field", "lr91_duplicate_constant_method", "lr91_duplicate_constant_constant"]);
}

mod f1 {
    fortis_testkit::case_tests!("names/member_namespace.json", super::check;
        f1_field_field_duplicate,
        f1_field_field_distinct,
        f1_field_method_duplicate,
        f1_field_method_distinct,
        f1_field_constant_duplicate,
        f1_field_constant_distinct,
        f1_method_field_duplicate,
        f1_method_field_distinct,
        f1_method_method_duplicate,
        f1_method_method_distinct,
        f1_method_constant_duplicate,
        f1_method_constant_distinct,
        f1_constant_field_duplicate,
        f1_constant_field_distinct,
        f1_constant_method_duplicate,
        f1_constant_method_distinct,
        f1_constant_constant_duplicate,
        f1_constant_constant_distinct
    );
}
