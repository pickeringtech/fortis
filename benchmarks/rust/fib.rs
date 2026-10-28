fn fib(n: i64) -> i64 {
    if n < 2 {
        return n;
    }
    fib(n.wrapping_sub(1)).wrapping_add(fib(n.wrapping_sub(2)))
}
fn main() {
    println!("{}", fib(36));
}
