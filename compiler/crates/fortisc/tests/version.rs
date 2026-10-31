use std::process::Command;

#[test]
fn cli_version_matches_workspace() {
    let result = Command::new(env!("CARGO_BIN_EXE_fortisc")).arg("--version").output().unwrap();
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), format!("fortisc {}\n", env!("CARGO_PKG_VERSION")));
    assert!(result.stderr.is_empty());
}
