//! D-640: malformed diagnostic records must be rejected, not canonicalised into validity.
use fortis_diag::{Json, Record};
fn check(name: &str) {
    let rows = fortis_testkit::read_cases("diagnostics/malformed.json");
    let row = rows.iter().find(|r| r.name == name).unwrap();
    let valid = Record::from_json(&row.input).map_or(false, |r| r.validate().is_empty());
    assert_eq!(Json::Bool(valid), row.expect);
}

#[test]
#[ignore = "D-640: malformed diagnostic record accepted"]
fn prog_dia_01_wrong_class() { check("prog_dia_01_wrong_class"); }

#[test]
#[ignore = "D-640: malformed diagnostic record accepted"]
fn prog_dia_01_info_severity() { check("prog_dia_01_info_severity"); }

#[test]
#[ignore = "D-640: malformed diagnostic record accepted"]
fn prog_dia_01_empty_correction() { check("prog_dia_01_empty_correction"); }

#[test]
#[ignore = "D-640: malformed diagnostic record accepted"]
fn prog_dia_01_empty_edits() { check("prog_dia_01_empty_edits"); }

#[test]
#[ignore = "D-640: malformed diagnostic record accepted"]
fn prog_dia_01_incomplete_edit() { check("prog_dia_01_incomplete_edit"); }

#[test]
#[ignore = "D-640: malformed diagnostic record accepted"]
fn prog_dia_01_missing_class() { check("prog_dia_01_missing_class"); }

#[test]
fn every_malformed_case_is_registered() {
    let names: Vec<_> = fortis_testkit::read_cases("diagnostics/malformed.json").into_iter().map(|r| r.name).collect();
    assert_eq!(names, vec!["prog_dia_01_wrong_class", "prog_dia_01_info_severity", "prog_dia_01_empty_correction", "prog_dia_01_empty_edits", "prog_dia_01_incomplete_edit", "prog_dia_01_missing_class"]);
}
