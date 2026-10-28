fn main() {
    let mut items: Vec<i64> = Vec::new();
    for i in 0..20_000_000i64 {
        items.push(i);
    }
    let doubled: Vec<i64> = items.iter().map(|x| x.wrapping_mul(3)).collect();
    let kept: Vec<i64> = doubled.iter().copied().filter(|x| x % 4 == 0).collect();
    let mut total: i64 = 0;
    for x in &kept {
        total = total.wrapping_add(*x);
    }
    println!("{} {}", kept.len(), total);
}
