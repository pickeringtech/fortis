//! LR 6.1, 6.3, 6.8: nominal types, refinements, optionals and invariant type arguments.
mod u1_support;
use fortis_testkit::Json;
fn check(input: &Json, expect: &Json) {
    let have = u1_support::typ(input.get("have").unwrap());
    let want = u1_support::typ(input.get("want").unwrap());
    let got = fortis_types::literals::assignable(&have, &want).0.map(Json::Bool).unwrap_or(Json::Null);
    assert_eq!(&got, expect);
}
fn defect(name: &str) {
    let rows = fortis_testkit::read_cases("types/defects.json");
    let row = rows.iter().find(|r| r.name == name).unwrap();
    check(&row.input, &row.expect);
}
#[test]
fn d635_tuple_wrong() { defect("lr6_tuple_wrong"); }
#[test]
fn d636_optional_widens() { defect("lr6_optional_widens"); }
#[test]
fn every_defect_is_registered() {
    let names: Vec<_> = fortis_testkit::read_cases("types/defects.json").into_iter().map(|r| r.name).collect();
    assert_eq!(names, vec!["lr6_tuple_wrong", "lr6_optional_widens"]);
}
fortis_testkit::case_tests!("types/assignability.json", check;
    lr6_int_to_int,
    lr6_int_to_bool,
    lr6_int_to_decimal,
    lr6_int_to_duration,
    lr6_int_to_string,
    lr6_bool_to_int,
    lr6_bool_to_bool,
    lr6_bool_to_decimal,
    lr6_bool_to_duration,
    lr6_bool_to_string,
    lr6_decimal_to_int,
    lr6_decimal_to_bool,
    lr6_decimal_to_decimal,
    lr6_decimal_to_duration,
    lr6_decimal_to_string,
    lr6_duration_to_int,
    lr6_duration_to_bool,
    lr6_duration_to_decimal,
    lr6_duration_to_duration,
    lr6_duration_to_string,
    lr6_string_to_int,
    lr6_string_to_bool,
    lr6_string_to_decimal,
    lr6_string_to_duration,
    lr6_string_to_string,
    lr6_list_same,
    lr6_list_widen,
    lr6_list_narrow,
    lr6_list_different,
    lr6_listview_same,
    lr6_listview_widen,
    lr6_listview_narrow,
    lr6_listview_different,
    lr6_map_same,
    lr6_map_widen,
    lr6_map_narrow,
    lr6_map_different,
    lr6_box_same,
    lr6_box_widen,
    lr6_box_narrow,
    lr6_box_different,
    lr6_optional_inject,
    lr6_optional_null,
    lr6_optional_remove,
    lr6_never,
    lr6_ref_base,
    lr6_ref_contains,
    lr6_ref_unknown,
    lr6_nominal
);

mod f1 {
    fortis_testkit::case_tests!("types/structural.json", super::check;
        f1_tuple_same,
        f1_nested_tuple_same,
        f1_tuple_wrong,
        f1_nested_tuple_wrong,
        f1_tuple_widen,
        f1_nested_tuple_widen,
        f1_tuple_contains,
        f1_nested_tuple_contains,
        f1_tuple_unproved,
        f1_nested_tuple_unproved,
        f1_tuple_container,
        f1_nested_tuple_container,
        f1_tuple_optional,
        f1_nested_tuple_optional,
        f1_list_tuple_same,
        f1_list_tuple_wrong,
        f1_list_tuple_invariant,
        f1_list_tuple_unproved,
        f1_record_same,
        f1_nested_record_same,
        f1_record_wrong,
        f1_nested_record_wrong,
        f1_record_widen,
        f1_nested_record_widen,
        f1_record_contains,
        f1_nested_record_contains,
        f1_record_unproved,
        f1_nested_record_unproved,
        f1_record_container,
        f1_nested_record_container,
        f1_record_optional,
        f1_nested_record_optional,
        f1_list_record_same,
        f1_list_record_wrong,
        f1_list_record_invariant,
        f1_list_record_unproved,
        f1_tuple_short,
        f1_tuple_long,
        f1_record_reordered,
        f1_record_missing,
        f1_record_extra,
        f1_record_renamed,
        f1_optional_nested_widen,
        f1_optional_of_list_invariant,
        f1_list_of_optional_invariant,
        f1_list_of_list_of_optional_invariant,
        f1_tuple_string_contained,
        f1_tuple_string_unproved,
        f1_tuple_string_widen,
        f1_tuple_bytes_contained,
        f1_tuple_bytes_unproved,
        f1_tuple_bytes_widen,
        f1_record_string_contained,
        f1_record_string_unproved,
        f1_record_string_widen,
        f1_record_bytes_contained,
        f1_record_bytes_unproved,
        f1_record_bytes_widen
    );
}
