//! PROG-DIA-05: canonical wire escaping and Unicode round trips.
use fortis_diag::Json;
fn check(input: &Json, expect: &Json) {
    assert_eq!(input.to_line(), expect.as_str().unwrap());
    assert_eq!(Json::parse(expect.as_str().unwrap()).unwrap(), *input);
}
#[test]
fn prog_dia_05_unicode_round_trip() {
    fortis_testkit::forall(|r| {
        let chars = ['\0', '\t', '\n', '"', '\\', '\u{7f}', 'é', 'λ', '🦀'];
        (0..r.between(0,25)).map(|_| chars[r.between(0,8) as usize]).collect::<String>()
    }, 200, None, |s| {
        let j = Json::str(s); assert_eq!(Json::parse(&j.to_line()).unwrap(), j);
    });
}
fortis_testkit::case_tests!("diagnostics/wire.json", check;
    prog_dia_05_null,
    prog_dia_05_true,
    prog_dia_05_false,
    prog_dia_05_zero,
    prog_dia_05_negative,
    prog_dia_05_i64_min,
    prog_dia_05_i64_max,
    prog_dia_05_empty,
    prog_dia_05_quotes,
    prog_dia_05_backslash,
    prog_dia_05_newline,
    prog_dia_05_tab,
    prog_dia_05_nul,
    prog_dia_05_unicode,
    prog_dia_05_array,
    prog_dia_05_object
);
