//! A docset output comparison must distinguish literal pipes, embedded newlines and empty print calls.
use std::process::Command;

#[test]
fn vm_stdout_preserves_console_text_and_failure_status() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let dir = std::env::temp_dir().join(format!("fortis-vm-stdout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("console.ft");
    let source = r#"import fortis.host { Console }
error Stop {
  code: "test.stop"
}
try func main(console: Console) {
  console.print("tea | cake")
  console.print("one\ntwo")
  console.print("")
  console.print("observed: fail:literal")
  fail Stop()
}
"#;
    std::fs::write(&path, source).unwrap();
    let run = |raw| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_fortisc"));
        cmd.current_dir(&root).arg("vm");
        if raw {
            cmd.arg("--stdout");
        }
        cmd.arg(&path).output().unwrap()
    };
    let raw = run(true);
    let envelope = run(false);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(raw.status.code(), Some(1));
    assert_eq!(
        raw.stdout,
        b"tea | cake\none\ntwo\n\nobserved: fail:literal\n"
    );
    let stderr = String::from_utf8(raw.stderr).unwrap();
    assert!(stderr.contains("trace: "), "{stderr}");
    assert!(stderr.contains("\"code\": \"test.stop\""), "{stderr}");
    assert_eq!(envelope.status.code(), Some(1));
    assert!(String::from_utf8(envelope.stdout)
        .unwrap()
        .contains("observed: tea | cake|one\ntwo||observed: fail:literal|fail:test.stop"));
}
