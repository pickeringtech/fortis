// Two hundred million small values, two method calls each, with the Fortis program's operations.
#[derive(Clone, Copy)]
struct Point {
    x: i64,
    y: i64,
}

impl Point {
    fn manhattan(self) -> i64 {
        self.x.wrapping_add(self.y)
    }
    fn shifted(self, dx: i64, dy: i64) -> Point {
        Point { x: self.x.wrapping_add(dx), y: self.y.wrapping_mul(dy) }
    }
}

fn main() {
    let mut total: i64 = 0;
    let mut p = Point { x: 1, y: 2 };
    for i in 0..200_000_000i64 {
        p = p.shifted(i % 3, (i % 5).wrapping_add(1));
        total = total.wrapping_mul(31).wrapping_add(p.manhattan());
    }
    println!("{total}");
}
