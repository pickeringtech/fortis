//! LR 4.6, 6.7, 6.13, 7.5, 8.2 and 14; D-505 totality and outcome consumers.
use fortis_testkit::Json;
fn check(d: &Json, e: &Json) {
    // The production checker prepares rewrite facts in memory; its shipped library is code loading only.
    let src = d.as_str().unwrap();
    let expected = fortis_fmt::format(e.get("text").unwrap().as_str().unwrap()).unwrap();
    let reading =
        fortis_fmt::migrate::migrate(src, true, &fortis_fmt::migrate::facts_alone("unit.ft"));
    let (out, _, _) = fortis_fmt::format_migrating(&reading, "unit.ft").unwrap();
    assert_eq!(out, expected);
    let again =
        fortis_fmt::migrate::migrate(&out, true, &fortis_fmt::migrate::facts_alone("unit.ft"));
    assert_eq!(
        fortis_fmt::format_migrating(&again, "unit.ft").unwrap().0,
        out
    );
}

fortis_testkit::case_tests!("formatting/rewrites.json",check;
    d505_total_0,
    d505_total_0_grouped,
    d505_total_1,
    d505_total_1_grouped,
    d505_total_75,
    d505_total_75_grouped,
    d505_total_100,
    d505_total_100_grouped,
    d505_unknown_parameter,
    d505_proved_parameter,
    d505_catch_1,
    d505_catch_9,
    d505_structural_1,
    d505_structural_99,
    d505_unknown_table_field,
    lr6_13_string_text,
    lr6_13_string_nested,
    lr6_13_string_literal,
    lr_migration_void,
    lr_migration_return_move,
    lr_migration_editable_receiver,
    lr_migration_export_main,
    lr_migration_unused_args,
    lr_migration_map_copy,
    lr_migration_test_handler,
    lr_migration_grouped_handler,
    lr_migration_lambda_kept,
    lr_migration_comment,
    lr6_13_from,
    lr6_13_to,
    lr6_13_field,
    lr6_4_reading_add,
    lr6_4_reading_sub,
    lr6_4_reading_mul,
    lr6_4_reading_div,
    lr6_4_reading_rem,
    lr5_8_constructor_pun,
    lr5_8_function_pun,
    lr5_8_different_name_kept,
    lr9_8_adjacent_same,
    lr9_8_adjacent_different,
    lr9_7_bare_result,
    lr9_7_bare_binding
);
