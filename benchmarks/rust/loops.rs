fn main() {
    let mut acc: i64 = 0;
    for i in 0..500_000_000i64 {
        acc = acc.wrapping_mul(31).wrapping_add(i);
    }
    println!("{acc}");
}
