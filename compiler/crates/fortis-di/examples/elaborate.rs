//! `cargo run -p fortis-di --example elaborate FILE`: the elaborated module, then each problem and the plans.
fn main() {
    let path = std::env::args().nth(1).expect("FILE");
    let p = std::path::Path::new(&path);
    let text = fortis_di::source(p).expect("readable");
    println!("{text}");
    for r in fortis_di::records(p) {
        eprintln!("{}", r.to_json_line());
    }
    for j in fortis_di::plans(p) {
        eprintln!("{}", j.to_line());
    }
}
