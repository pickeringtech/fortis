//! A failure's message (Language Reference 10.3): the `message` of an `error` declaration is a template, stored
//! as written between its quotes, and rendered when the failure is displayed. Each `${name}` that names a payload
//! field becomes that field's text, as the record's `payload` shows it; any other text is kept as it is. The
//! development VM and native code render through this one function; the interpreter's `render_message` is the
//! same rule (D-340).

/// The template with each `${name}` that names a payload field replaced by that field's text.
pub fn render_message<K: AsRef<str>, V: AsRef<str>>(template: &str, payload: &[(K, V)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match payload.iter().find(|(k, _)| k.as_ref() == name) {
                    Some((_, v)) => out.push_str(v.as_ref()),
                    None => {
                        out.push_str("${");
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[at..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::render_message;

    #[test]
    fn renders_payload_fields() {
        let p = [("n", "3"), ("who", "ann")];
        assert_eq!(render_message("late ${n}", &p), "late 3");
        assert_eq!(render_message("${who} is ${n} late, ${n}", &p), "ann is 3 late, 3");
        assert_eq!(render_message("no ${field} here", &p), "no ${field} here");
        assert_eq!(render_message("open ${n", &p), "open ${n");
        assert_eq!(render_message("", &p), "");
    }
}
