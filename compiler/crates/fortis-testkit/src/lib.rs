//! Dependency-free test support. Production crates use this only as a dev-dependency.
pub use fortis_diag::Json;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::path::Path;

#[derive(Debug)]
pub struct Case {
    pub name: String,
    pub input: Json,
    pub expect: Json,
}

pub fn parse_cases(text: &str) -> Result<Vec<Case>, String> {
    let json = Json::parse(text)?;
    let rows = json.as_arr().ok_or("expected an array of cases")?;
    if rows.is_empty() {
        return Err("expected a non-empty case table".into());
    }
    let mut names = BTreeSet::new();
    rows.iter()
        .map(|row| {
            let Json::Obj(obj) = row else {
                return Err("expected a case object".into());
            };
            if obj.len() != 3
                || !["name", "input", "expect"]
                    .iter()
                    .all(|k| obj.contains_key(*k))
            {
                return Err("each case needs exactly name, input, expect".into());
            }
            let name = row
                .get("name")
                .and_then(Json::as_str)
                .ok_or("expected a case name")?;
            if !name.starts_with(|c: char| c.is_ascii_lowercase())
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                || !names.insert(name.to_string())
            {
                return Err(format!("invalid or duplicate case name: {name:?}"));
            }
            Ok(Case {
                name: name.into(),
                input: obj["input"].clone(),
                expect: obj["expect"].clone(),
            })
        })
        .collect()
}

/// Paths are relative to tests/cases, independent of the caller's working directory.
pub fn read_cases(path: &str) -> Vec<Case> {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/cases")
        .join(path);
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    parse_cases(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

/// Each name becomes a normal cargo test. The extra test prevents silently unregistered rows.
#[macro_export]
macro_rules! case_tests {
    ($path:literal, $check:path; $($(#[$attr:meta])* $name:ident),+ $(,)?) => {
        $($(#[$attr])* #[test]
        fn $name() {
            let rows = $crate::read_cases($path);
            let row = rows.iter().find(|r| r.name == stringify!($name)).expect("case missing from table");
            $check(&row.input, &row.expect);
        })+
        #[test]
        fn every_shared_case_is_registered() {
            let mut actual: Vec<_> = $crate::read_cases($path).into_iter().map(|r| r.name).collect();
            let mut registered = vec![$(stringify!($name).to_string()),+];
            actual.sort();
            registered.sort();
            assert_eq!(actual, registered, "register each table row in case_tests!");
        }
    };
}

/// SplitMix64: deterministic across Rust versions, including seed zero. Not for cryptography.
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    pub fn between(&mut self, lo: i64, hi: i64) -> i64 {
        assert!(lo <= hi);
        let width = (hi as i128 - lo as i128 + 1) as u128;
        (lo as i128 + (self.next_u64() as u128 % width) as i128) as i64
    }
}

/// Run `cases` checks. FORTIS_TEST_SEED overrides seed (default 0); failures include the generated value.
pub fn forall<T: Debug>(
    mut generator: impl FnMut(&mut Rng) -> T,
    cases: usize,
    seed: Option<u64>,
    check: impl Fn(&T),
) {
    assert!(cases > 0, "cases must be positive");
    let seed = match std::env::var("FORTIS_TEST_SEED") {
        Ok(s) => s
            .parse()
            .expect("FORTIS_TEST_SEED must be an unsigned 64-bit integer"),
        Err(std::env::VarError::NotPresent) => seed.unwrap_or(0),
        Err(e) => panic!("FORTIS_TEST_SEED: {e}"),
    };
    let mut rng = Rng::new(seed);
    for index in 0..cases {
        let value = generator(&mut rng);
        if let Err(payload) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&value)))
        {
            eprintln!("FORTIS_TEST_SEED={seed} case={index} value={value:?}");
            std::panic::resume_unwind(payload);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_supported_values() {
        let rows = parse_cases(r#"[{"name":"values","input":[null,true,-9223372036854775808,"🙂"],"expect":{"ok":3}}]"#).unwrap();
        assert_eq!(rows[0].name, "values");
        assert_eq!(rows[0].expect.get("ok"), Some(&Json::Int(3)));
        assert_eq!(rows[0].input.as_arr().unwrap()[3], Json::str("🙂"));
    }
    #[test]
    fn rejects_invalid_schema() {
        for text in [
            "[]",
            "{}",
            r#"[{"name":"a","input":0}]"#,
            r#"[{"name":"A","input":0,"expect":0}]"#,
            r#"[{"name":"a","input":1.5,"expect":0}]"#,
            r#"[{"name":"a","input":9223372036854775808,"expect":0}]"#,
        ] {
            assert!(parse_cases(text).is_err(), "accepted {text}");
        }
    }
    #[test]
    fn rejects_duplicate_names() {
        let row = r#"{"name":"same","input":0,"expect":0}"#;
        assert!(parse_cases(&format!("[{row},{row}]"))
            .unwrap_err()
            .contains("duplicate case name"));
    }
    #[test]
    fn seed_replays() {
        let mut a = Rng::new(0);
        let mut b = Rng::new(0);
        let first = a.next_u64();
        assert_eq!(first, 0xe220a8397b1dcdaf); // SplitMix64's seed-zero vector
        assert_eq!(first, b.next_u64());
        for _ in 0..200 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
    #[test]
    fn generated_values_stay_in_bounds() {
        let mut rng = Rng::new(42);
        for _ in 0..200 {
            assert!((-20..=20).contains(&rng.between(-20, 20)));
            assert_eq!(rng.between(7, 7), 7);
            let _ = rng.between(i64::MIN, i64::MAX);
        }
    }
    #[test]
    fn runs_requested_cases() {
        let count = std::cell::Cell::new(0);
        forall(|r| r.next_u64(), 7, Some(1), |_| count.set(count.get() + 1));
        assert_eq!(count.get(), 7);
    }
    #[test]
    #[should_panic(expected = "broken check")]
    fn propagates_property_failures() {
        forall(|_| 17, 1, Some(42), |_| panic!("broken check"));
    }
    #[test]
    #[should_panic(expected = "cases must be positive")]
    fn rejects_zero_cases() {
        forall(|_| 0, 0, None, |_| {});
    }
}
