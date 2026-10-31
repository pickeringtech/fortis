//! D-714: a stream's bytes survive every phase of a command.
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn piped(command: &[&str], source: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fortisc"))
        .args(command)
        .arg("/dev/stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn check_rejects_piped_invalid_source() {
    let out = piped(&["check"], "func main() { missing() }\n");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("E-NAM-001"));
}

#[test]
fn test_retains_piped_failing_test() {
    let out = piped(&["test"], "test \"fails\" { expect false }\n");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let summary =
        fortis_diag::Json::parse(std::str::from_utf8(&out.stdout).unwrap().trim()).unwrap();
    assert_eq!(summary.get("failed").and_then(|v| v.as_i64()), Some(1));
}

#[test]
fn vm_retains_piped_program() {
    let out = piped(&["vm", "--stdout"], "import fortis.host { Console }\nfunc main(console: Console) { console.print(\"retained\") }\n");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "retained\n");
}
