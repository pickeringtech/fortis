//! LR 6 and Appendix C.1: representation types are checked as Rust text, without compilation.
use fortis_testkit::Json;
use fortis_types::types::{named, named_args, optional};
fn check(d: &Json, e: &Json) {
    let n = d.get("name").unwrap().as_str().unwrap();
    let args = d
        .get("args")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .map(|x| named(x.as_str().unwrap()))
        .collect();
    let mut t = named_args(n, args);
    if d.get("kind").unwrap().as_str() == Some("optional") {
        t = optional(t)
    };
    assert_eq!(
        fortis_lower::types::rust_ty(&t, &fortis_mir::Program::default()),
        e.as_str().unwrap()
    );
}

fortis_testkit::case_tests!("native_lower/types.json",check;
    c1_type_int,
    c1_type_int_optional,
    c1_type_int64,
    c1_type_int64_optional,
    c1_type_int32,
    c1_type_int32_optional,
    c1_type_int16,
    c1_type_int16_optional,
    c1_type_int8,
    c1_type_int8_optional,
    c1_type_uint,
    c1_type_uint_optional,
    c1_type_uint64,
    c1_type_uint64_optional,
    c1_type_uint32,
    c1_type_uint32_optional,
    c1_type_uint16,
    c1_type_uint16_optional,
    c1_type_uint8,
    c1_type_uint8_optional,
    c1_type_byte,
    c1_type_byte_optional,
    c1_type_bool,
    c1_type_bool_optional,
    c1_type_float,
    c1_type_float_optional,
    c1_type_float32,
    c1_type_float32_optional,
    c1_type_decimal,
    c1_type_decimal_optional,
    c1_type_duration,
    c1_type_duration_optional,
    c1_type_timestamp,
    c1_type_timestamp_optional,
    c1_type_string,
    c1_type_string_optional,
    c1_type_stringview,
    c1_type_stringview_optional,
    c1_type_scalar,
    c1_type_scalar_optional,
    c1_type_bytes,
    c1_type_bytes_optional,
    c1_type_bytesview,
    c1_type_bytesview_optional,
    c1_type_void,
    c1_type_void_optional,
    c1_type_never,
    c1_type_never_optional,
    c1_type_failure,
    c1_type_failure_optional,
    c1_type_list_int,
    c1_type_list_string,
    c1_type_list_bool,
    c1_type_array_int,
    c1_type_array_string,
    c1_type_array_bool,
    c1_type_listview_int,
    c1_type_listview_string,
    c1_type_listview_bool,
    c1_type_set_int,
    c1_type_set_string,
    c1_type_set_bool,
    c1_type_outcome_int,
    c1_type_outcome_string,
    c1_type_outcome_bool,
    c1_type_shared_int,
    c1_type_shared_string,
    c1_type_shared_bool,
    c1_type_weak_int,
    c1_type_weak_string,
    c1_type_weak_bool,
    c1_type_mutex_int,
    c1_type_mutex_string,
    c1_type_mutex_bool,
    c1_type_listcursor_int,
    c1_type_listcursor_string,
    c1_type_listcursor_bool,
    c1_type_mutexguard_int,
    c1_type_mutexguard_string,
    c1_type_mutexguard_bool,
    c1_type_map_int,
    c1_type_map_string,
    c1_type_map_bool,
    c1_type_variantmap_int,
    c1_type_variantmap_string,
    c1_type_variantmap_bool
);
