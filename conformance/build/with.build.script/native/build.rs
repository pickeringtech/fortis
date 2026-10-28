// A build script runs arbitrary code at build time; Fortis native builds never execute one (PROG-CMP-06).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
