// A bottom-up merge sort of four million pseudo-random integers, with the Fortis program's operations: saturating
// index arithmetic, an optional read of each element, fresh output lists per pass and optional final reads.
fn sorted(items: &[i64]) -> Vec<i64> {
    let n = items.len() as i64;
    let mut current = items.to_vec();
    let mut width: i64 = 1;
    while width < n {
        let mut next = Vec::new();
        let mut lo: i64 = 0;
        while lo < n {
            let mid = lo.saturating_add(width);
            let hi = lo.saturating_add(width.saturating_mul(2));
            let mid2 = if mid < n { mid } else { n };
            let hi2 = if hi < n { hi } else { n };
            let (mut i, mut j) = (lo, mid2);
            while i < mid2 || j < hi2 {
                let a = current.get(i as usize).copied().unwrap_or(0);
                let b = current.get(j as usize).copied().unwrap_or(0);
                if j >= hi2 || (i < mid2 && a <= b) {
                    next.push(a);
                    i = i.saturating_add(1);
                } else {
                    next.push(b);
                    j = j.saturating_add(1);
                }
            }
            lo = hi2;
        }
        current = next;
        width = width.saturating_mul(2);
    }
    current
}

fn main() {
    let mut items: Vec<i64> = Vec::new();
    let mut seed: i64 = 12345;
    for _ in 0..4_000_000 {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345) % 2147483648;
        items.push(seed % 1000000);
    }
    let s = sorted(&items);
    let at = |k: usize| s.get(k).copied().unwrap_or(0);
    println!("{} {} {} {}", s.len(), at(0), at(2000000), at(3999999));
}
