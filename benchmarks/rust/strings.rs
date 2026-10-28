// Two million interpolated strings and their total length in scalars, built as the Fortis program builds them:
// one sized string per line, literal parts appended, integers written digit by digit (no formatting machinery).
fn push_int(out: &mut String, v: i64) {
    let mut buf = [0u8; 20];
    let mut n = v.unsigned_abs();
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if v < 0 {
        out.push('-');
    }
    for &d in &buf[at..] {
        out.push(d as char);
    }
}

fn main() {
    let mut total: i64 = 0;
    for i in 0..2_000_000i64 {
        let mut line = String::with_capacity(32);
        line.push_str("item ");
        push_int(&mut line, i);
        line.push_str(" of ");
        push_int(&mut line, 2_000_000i64);
        line.push_str(": ok");
        total = total.wrapping_add(line.chars().count() as i64);
    }
    println!("{total}");
}
