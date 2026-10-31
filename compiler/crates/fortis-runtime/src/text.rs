//! Text semantics (Language Reference 6.6; X-479): the everyday operations of `String` and `StringView`, implemented
//! once so that the development VM and native code cannot drift apart. The definitional interpreter implements the
//! same contracts separately (interpreter/intrinsics.py), and the conformance corpus runs all three.
//!
//! Positions are scalar offsets, the unit of `slice` and `scalarAt`, never byte offsets. Whitespace is the Unicode
//! `White_Space` property (Rust's `char::is_whitespace`). Case mapping is ASCII only. An empty separator, needle
//! or pattern occurs nowhere in `split` and `replace`; `find("")` is 0.

/// `view.trim()`: without leading and trailing White_Space scalars.
#[inline]
pub fn trim(s: &str) -> &str {
    s.trim_matches(char::is_whitespace)
}

/// `view.trimStart()`: without leading White_Space scalars.
#[inline]
pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(char::is_whitespace)
}

/// `view.trimEnd()`: without trailing White_Space scalars.
#[inline]
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(char::is_whitespace)
}

/// `view.find(needle)`: the scalar offset of the first occurrence, `None` when the needle does not occur.
pub fn find(s: &str, needle: &str) -> Option<i64> {
    s.find(needle).map(|byte| s[..byte].chars().count() as i64)
}

/// `view.split(separator)`: the pieces between non-overlapping occurrences, left to right. An empty separator occurs
/// nowhere, so the text is its own single piece.
pub fn split(s: &str, separator: &str) -> Vec<String> {
    if separator.is_empty() {
        return vec![s.to_string()];
    }
    s.split(separator).map(str::to_string).collect()
}

/// `view.lines()`: a line ends at a line feed, and one carriage return before that line feed ends with it; the text
/// after the last terminator is a line only when it is not empty. A carriage return not before a line feed stays.
pub fn lines(s: &str) -> Vec<String> {
    let mut pieces: Vec<&str> = s.split('\n').collect();
    let last = pieces.pop().unwrap_or("");
    let mut out: Vec<String> = pieces.into_iter().map(|p| p.strip_suffix('\r').unwrap_or(p).to_string()).collect();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// `view.replace(old, new)`: every non-overlapping occurrence of `old`, left to right. An empty `old` occurs nowhere.
pub fn replace(s: &str, old: &str, new: &str) -> String {
    if old.is_empty() {
        s.to_string()
    } else {
        s.replace(old, new)
    }
}

/// `view.asciiUpper()`: a-z to A-Z, every other scalar unchanged.
pub fn ascii_upper(s: &str) -> String {
    s.to_ascii_uppercase()
}

/// `view.asciiLower()`: A-Z to a-z, every other scalar unchanged.
pub fn ascii_lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_unicode_white_space_only() {
        assert_eq!(trim("\u{a0} \t a b \u{3000}\n"), "a b");
        assert_eq!(trim_start("  a "), "a ");
        assert_eq!(trim_end("  a "), "  a");
        // U+001C to U+001F and U+200B are not White_Space
        assert_eq!(trim("\u{1c}a\u{200b}"), "\u{1c}a\u{200b}");
        assert_eq!(trim(""), "");
        assert_eq!(trim(" \t "), "");
    }

    #[test]
    fn finds_scalar_offsets() {
        assert_eq!(find("naïve café", "café"), Some(6));
        assert_eq!(find("abc", ""), Some(0));
        assert_eq!(find("", ""), Some(0));
        assert_eq!(find("abc", "d"), None);
        assert_eq!(find("", "a"), None);
    }

    #[test]
    fn splits_at_every_occurrence() {
        assert_eq!(split("a,b,,c,", ","), vec!["a", "b", "", "c", ""]);
        assert_eq!(split("", ","), vec![""]);
        assert_eq!(split("abc", ""), vec!["abc"]);
        assert_eq!(split(",", ","), vec!["", ""]);
        assert_eq!(split("aaa", "aa"), vec!["", "a"]);
    }

    #[test]
    fn lines_end_at_line_feeds() {
        assert_eq!(lines(""), Vec::<String>::new());
        assert_eq!(lines("a"), vec!["a"]);
        assert_eq!(lines("a\n"), vec!["a"]);
        assert_eq!(lines("\n"), vec![""]);
        assert_eq!(lines("a\r\nb\r\n"), vec!["a", "b"]);
        assert_eq!(lines("a\r\n\r\n"), vec!["a", ""]);
        assert_eq!(lines("a\r"), vec!["a\r"]);
        assert_eq!(lines("a\rb\n"), vec!["a\rb"]);
        assert_eq!(lines("one\r\ntwo\nthree\r"), vec!["one", "two", "three\r"]);
    }

    #[test]
    fn replaces_without_overlap() {
        assert_eq!(replace("aaa", "aa", "b"), "ba");
        assert_eq!(replace("abc", "", "x"), "abc");
        assert_eq!(replace("", "a", "b"), "");
        assert_eq!(replace("héllo", "é", "e"), "hello");
    }

    #[test]
    fn maps_ascii_case_only() {
        assert_eq!(ascii_upper("straße ünï abc"), "STRAßE üNï ABC");
        assert_eq!(ascii_lower("ÀBC Σ"), "Àbc Σ");
    }
}
