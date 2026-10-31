//! LR 3.1, 3.5, 3.6: malformed input has a stable syntax code.
use fortis_syntax::lex;
use fortis_testkit::Json;
fn check(input: &Json, expect: &Json) {
    assert_eq!(lex(input.as_str().unwrap()).unwrap_err().code, expect.as_str().unwrap());
    assert_eq!(lex("x").unwrap().iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), vec!["x", "\n", ""]);
}
fortis_testkit::case_tests!("lexer/errors.json", check;
    lr3_semicolon,
    lr3_semicolon_after,
    lr3_bom,
    lr3_unicode_identifier,
    lr3_unterminated,
    lr3_raw_unterminated,
    lr3_string_newline,
    lr3_escaped_newline,
    lr3_interpolation_comment,
    lr3_interpolation_open,
    lr3_byte_unicode,
    lr3_hash,
    lr3_backtick,
    lr35_bad_number_00,
    lr35_bad_number_01,
    lr35_bad_number_02,
    lr35_bad_number_03,
    lr35_bad_number_04,
    lr35_bad_number_05,
    lr35_bad_number_06,
    lr35_bad_number_07,
    lr35_bad_number_08,
    lr35_bad_number_09,
    lr35_bad_number_10,
    lr35_bad_number_11
);
