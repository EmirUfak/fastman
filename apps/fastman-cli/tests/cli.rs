use std::{fs, process::Command};

#[test]
fn import_list_and_error_paths() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("data");
    let input = dir.path().join("collection.json");
    fs::write(&input, r#"{"info":{"name":"CLI Collection","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"event":[],"item":[{"name":"Request","request":{"method":"GET","url":"http://localhost"}}]}"#).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
            .env("FASTMAN_HOME", &home)
            .args(args)
            .output()
            .unwrap()
    };
    let empty = run(&["list"]);
    assert!(empty.status.success());
    assert!(empty.stdout.is_empty());
    let imported = run(&["import-postman", input.to_str().unwrap()]);
    assert!(imported.status.success());
    assert!(String::from_utf8_lossy(&imported.stdout).contains("Imported 1 request(s)"));
    assert!(String::from_utf8_lossy(&imported.stderr).contains("Collection scripts not imported"));
    let listed = run(&["list"]);
    assert!(listed.status.success());
    assert!(String::from_utf8_lossy(&listed.stdout).contains("CLI Collection"));
    let version = run(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("fastman-cli {}", env!("CARGO_PKG_VERSION"))
    );
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stderr).contains(env!("CARGO_PKG_VERSION")));
    let bad = run(&["send", "not-a-uuid", "missing"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("Invalid project ID"));
    assert!(!run(&["unknown"]).status.success());
}
