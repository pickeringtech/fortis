use fortis_syntax::lexer::{lex, Kind};
use fortis_testkit::Json;

fn check(input: &Json, expect: &Json) {
    let actual = Json::Arr(
        lex(input.as_str().unwrap())
            .unwrap()
            .iter()
            .filter(|t| t.kind != Kind::Eof)
            .map(|t| Json::Arr(vec![Json::str(t.kind.as_str()), Json::str(&t.text)]))
            .collect(),
    );
    assert_eq!(&actual, expect);
}

fortis_testkit::case_tests!("lexer/tokens.json", check;
    integer_zero,
    integer_decimal,
    integer_groups,
    integer_hex,
    integer_binary,
    integer_octal,
    float_fraction,
    float_exponent,
    float_signed_exponent,
    float32_suffix,
    decimal_integer,
    decimal_fraction,
    boolean_and_null,
    string_empty,
    string_unicode,
    string_escapes,
    string_interpolation,
    raw_string,
    byte_string,
    duration_units,
    declaration_keywords,
    control_keywords,
    integer_member,
    keyword_member
);
