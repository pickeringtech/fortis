//! LR 3.1-3.7: exact tokens, including every LR 3.2 continuation family.
use fortis_syntax::{lex, Kind};
use fortis_testkit::Json;
fn check(input: &Json, expect: &Json) {
    let actual = Json::Arr(lex(input.as_str().unwrap()).unwrap().iter()
        .filter(|t| t.kind != Kind::Eof)
        .map(|t| Json::Arr(vec![Json::str(t.kind.as_str()), Json::str(&t.text)])).collect());
    assert_eq!(&actual, expect);
}
#[test]
fn lr32_trailing_with_comments() {
    fortis_testkit::forall(|r| (r.between(0, 5), r.between(1, 8)), 200, None, |&(op, n)| {
        let op = ["+", "-", "*", "/", "==", "&&"][op as usize];
        let source = format!("a {op}\n{}b", "// comment\n".repeat(n as usize));
        let tokens = lex(&source).unwrap();
        assert_eq!(tokens.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), vec!["a", op, "b", "\n", ""]);
    });
}
fortis_testkit::case_tests!("lexer/continuation.json", check;
    lr32_trailing_00,
    lr32_trailing_01,
    lr32_trailing_02,
    lr32_trailing_03,
    lr32_trailing_04,
    lr32_trailing_05,
    lr32_trailing_06,
    lr32_trailing_07,
    lr32_trailing_08,
    lr32_trailing_09,
    lr32_trailing_10,
    lr32_trailing_11,
    lr32_trailing_12,
    lr32_trailing_13,
    lr32_trailing_14,
    lr32_trailing_15,
    lr32_trailing_16,
    lr32_trailing_17,
    lr32_trailing_18,
    lr32_trailing_19,
    lr32_trailing_20,
    lr32_trailing_21,
    lr32_trailing_22,
    lr32_trailing_23,
    lr32_trailing_24,
    lr32_leading_dot,
    lr32_leading_or,
    lr32_leading_catch,
    lr32_leading_coalesce,
    lr32_leading_else,
    lr32_leading_with,
    lr32_leading_where,
    lr32_leading_implements,
    lr32_leading_derive,
    lr32_leading_from,
    lr3_blank,
    lr3_comments,
    lr3_collapsed,
    lr3_crlf,
    lr3_paren,
    lr3_square,
    lr3_body,
    lr3_record,
    lr3_return,
    lr3_give,
    lr3_semicolon_string,
    lr3_raw_multiline,
    lr3_raw_hole,
    lr3_escaped_hole,
    lr3_identifiers,
    lr3_maximal_munch,
    lr3_refinement_line,
    lr3_as_refinement,
    lr3_variant_comparison,
    lr3_index_comparison,
    lr3_refinement_assignment
);
