//! UAX #29 GB3-GB9c and GB11-GB13. Hangul GB6-GB8 remain outside the contract.
use crate::grapheme_data as data;

fn contains(ranges: &[(u32, u32)], n: u32) -> bool {
    let i = ranges.partition_point(|&(start, _)| start <= n);
    i > 0 && n <= ranges[i - 1].1
}

pub fn count(text: &str) -> i64 {
    let (mut total, mut previous, mut ri_run) = (0, None, 0);
    let (mut pict_run, mut zwj_after_pict) = (false, false);
    let mut conjunct = 0; // 1: consonant and extenders; 2: at least one linker too
    for ch in text.chars() {
        let n = ch as u32;
        let extend = contains(data::EXTEND, n);
        let pict = contains(data::EXTENDED_PICTOGRAPHIC, n);
        let control = contains(data::CONTROL, n) || ch == '\r' || ch == '\n';
        let ri = (0x1F1E6..=0x1F1FF).contains(&n);
        let consonant = contains(data::INCB_CONSONANT, n);
        let joined = match previous {
            Some((13, _, _)) if n == 10 => true,
            Some((_, false, prepend)) if !control => {
                extend
                    || n == 0x200D
                    || contains(data::SPACINGMARK, n)
                    || prepend
                    || (conjunct == 2 && consonant)
                    || (zwj_after_pict && pict)
                    || (ri && ri_run % 2 == 1)
            }
            _ => false,
        };
        total += i64::from(!joined);
        zwj_after_pict = n == 0x200D && pict_run;
        pict_run = pict || (extend && pict_run);
        if consonant {
            conjunct = 1;
        } else if conjunct != 0 && contains(data::INCB_LINKER, n) {
            conjunct = 2;
        } else if !contains(data::INCB_EXTEND, n) {
            conjunct = 0;
        }
        ri_run = if ri { ri_run + 1 } else { 0 };
        previous = Some((n, control, contains(data::PREPEND, n)));
    }
    total
}
