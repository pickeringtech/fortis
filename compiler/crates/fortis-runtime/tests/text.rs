use fortis_runtime::text;
use fortis_testkit::Json;

fn check(input: &Json, expect: &Json) {
    let op = input.get("op").unwrap().as_str().unwrap();
    let s = input.get("text").unwrap().as_str().unwrap();
    let args = input.get("args").unwrap().as_arr().unwrap();
    let arg = |i: usize| args[i].as_str().unwrap();
    let actual = match op {
        "trim" => Json::str(text::trim(s)),
        "split" => Json::strs(text::split(s, arg(0))),
        "find" => text::find(s, arg(0)).map(Json::Int).unwrap_or(Json::Null),
        "replace" => Json::str(text::replace(s, arg(0), arg(1))),
        "asciiUpper" => Json::str(text::ascii_upper(s)),
        _ => panic!("unknown text operation: {op}"),
    };
    assert_eq!(&actual, expect);
}

fortis_testkit::case_tests!("text/intrinsics.json", check;
    trim_ascii,
    trim_unicode,
    trim_not_white_space,
    trim_empty,
    trim_all_white_space,
    split_repeated,
    split_empty_separator,
    split_empty_text,
    split_non_overlapping,
    split_unicode,
    find_scalar_offset,
    find_empty_needle,
    find_missing,
    find_first_occurrence,
    replace_all,
    replace_empty_pattern,
    replace_non_overlapping,
    replace_delete,
    ascii_upper_letters,
    ascii_upper_preserves_unicode
);
