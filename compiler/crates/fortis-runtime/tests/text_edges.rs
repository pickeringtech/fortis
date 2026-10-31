//! LR 6.6: whitespace, line terminators and ASCII-only case mapping.
use fortis_runtime::text;
use fortis_testkit::Json;
fn check(d: &Json, expect: &Json) {
    let s = d.get("text").unwrap().as_str().unwrap();
    let actual = match d.get("op").unwrap().as_str().unwrap() {
        "trimStart" => Json::str(text::trim_start(s)),
        "trimEnd" => Json::str(text::trim_end(s)),
        "lines" => Json::strs(text::lines(s)),
        "asciiLower" => Json::str(text::ascii_lower(s)),
        _ => panic!("unknown text operation"),
    };
    assert_eq!(&actual, expect);
}

fortis_testkit::case_tests!("runtime/text_edges.json", check;
    lr6_6_trimstart_0,
    lr6_6_trimstart_1,
    lr6_6_trimstart_2,
    lr6_6_trimstart_3,
    lr6_6_trimstart_4,
    lr6_6_trimend_0,
    lr6_6_trimend_1,
    lr6_6_trimend_2,
    lr6_6_trimend_3,
    lr6_6_trimend_4,
    lr6_6_lines_0,
    lr6_6_lines_1,
    lr6_6_lines_2,
    lr6_6_lines_3,
    lr6_6_lines_4,
    lr6_6_lines_5,
    lr6_6_lines_6,
    lr6_6_lines_7,
    lr6_6_lines_8,
    lr6_6_asciilower_0,
    lr6_6_asciilower_1,
    lr6_6_asciilower_2,
    lr6_6_asciilower_3,
    lr6_6_asciilower_4
);
