//! Duration (nanoseconds, Language Reference 6.2) and Timestamp (nanoseconds since the Unix epoch, 15) with their
//! display forms.

pub const UNITS: [(&str, i64); 6] = [("h", 3_600_000_000_000), ("min", 60_000_000_000), ("s", 1_000_000_000), ("ms", 1_000_000), ("us", 1_000), ("ns", 1)];

/// The largest unit that divides the value exactly; zero is `0s`.
pub fn display_duration(ns: i64) -> String {
    if ns == 0 {
        return "0s".into();
    }
    for (u, m) in UNITS {
        if ns % m == 0 {
            return format!("{}{u}", ns / m);
        }
    }
    format!("{ns}ns")
}

/// RFC 3339 in UTC, with a fractional second only when the instant has one.
pub fn display_timestamp(ns: i64) -> String {
    let secs = ns.div_euclid(1_000_000_000);
    let nanos = ns.rem_euclid(1_000_000_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let base = format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60);
    if nanos == 0 {
        format!("{base}Z")
    } else {
        let frac = format!("{nanos:09}");
        format!("{base}.{}Z", frac.trim_end_matches('0'))
    }
}

/// Howard Hinnant's days-to-civil algorithm (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(display_duration(0), "0s");
        assert_eq!(display_duration(90_000_000_000), "90s");
        assert_eq!(display_duration(120_000_000_000), "2min");
        assert_eq!(display_duration(1_500_000), "1500us");
        assert_eq!(display_duration(-3_600_000_000_000), "-1h");
    }

    #[test]
    fn timestamps() {
        assert_eq!(display_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(display_timestamp(1_700_000_000_123_000_000), "2023-11-14T22:13:20.123Z");
        assert_eq!(display_timestamp(-1), "1969-12-31T23:59:59.999999999Z");
    }
}
