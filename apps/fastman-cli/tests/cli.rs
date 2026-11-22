use fastman_core::{ApiRequest, Environment, KeyValue, ProjectStore, SecretRef};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
};

#[test]
fn import_list_and_error_paths() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("data");
    let input = dir.path().join("collection.json");
    fs::write(&input, r#"{"info":{"name":"CLI Collection","schema":"https://example.com/schema/collection/v2.1.0/collection.json"},"event":[],"item":[{"name":"Request","request":{"method":"GET","url":"http://localhost"}}]}"#).unwrap();
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
    let imported = run(&["import-collection", input.to_str().unwrap()]);
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

#[test]
fn import_blocks_plaintext_secrets_by_default_and_escapes_terminal_sequences() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("data");
    let secret_col = dir.path().join("secret.json");
    fs::write(
        &secret_col,
        r#"{"info":{"name":"Evil\u001b[31mAPI","schema":"https://example.com/schema/collection/v2.1.0/collection.json"},"item":[{"name":"AuthReq","request":{"method":"GET","url":"https://example.com?api_key=q_canary","header":[{"key":"Authorization","value":"Token secret_canary"},{"key":"Accept","value":"application/json"}]}}]}"#,
    )
    .unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
            .env("FASTMAN_HOME", &home)
            .args(args)
            .output()
            .unwrap()
    };

    // Default import must refuse to persist sensitive fields.
    let refused = run(&["import-collection", secret_col.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--strip-secrets"));
    assert!(run(&["list"]).stdout.is_empty());

    // --strip-secrets saves project without persisting any canary secrets.
    let stripped = run(&[
        "import-collection",
        "--strip-secrets",
        secret_col.to_str().unwrap(),
    ]);
    assert!(
        stripped.status.success(),
        "{:?}",
        String::from_utf8_lossy(&stripped.stderr)
    );
    let stdout = String::from_utf8_lossy(&stripped.stdout);
    assert!(!stdout.contains('\x1b'));
    assert!(stdout.contains("\\u{1b}[31m"));

    let listed = run(&["list"]);
    let list_out = String::from_utf8_lossy(&listed.stdout);
    assert!(!list_out.contains('\x1b'));
    assert!(list_out.contains("\\u{1b}[31m"));

    let store = ProjectStore::at(&home);
    let projects = store.list().unwrap();
    assert_eq!(projects.len(), 1);
    let json = fs::read_to_string(home.join(&projects[0].id).join("project.json")).unwrap();
    assert!(!json.contains("secret_canary"));
    assert!(!json.contains("q_canary"));

    // --allow-plaintext-secrets explicitly permits storage with warning.
    let allowed = run(&[
        "import-collection",
        "--allow-plaintext-secrets",
        secret_col.to_str().unwrap(),
    ]);
    assert!(allowed.status.success());
    assert!(String::from_utf8_lossy(&allowed.stderr).contains("Warning: persisting"));
}

#[test]
fn send_escapes_control_sequences_unless_raw_is_specified() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("data");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhi\x1b[2Jend!")
                .unwrap();
        }
    });

    let store = ProjectStore::at(&home);
    let mut project = store.create("EscapeTest").unwrap();
    let req = ApiRequest::new("GET", "Ping", format!("http://{addr}/ping"));
    let req_id = req.id.clone();
    project.collections[0].requests.push(req);
    store.save(&project).unwrap();

    let safe = Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
        .env("FASTMAN_HOME", &home)
        .args(["send", &project.id, &req_id])
        .output()
        .unwrap();
    assert!(safe.status.success());
    let safe_out = String::from_utf8_lossy(&safe.stdout);
    assert!(!safe_out.contains('\x1b'));
    assert!(safe_out.contains("hi\\u{1b}[2Jend!"));

    let raw = Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
        .env("FASTMAN_HOME", &home)
        .args(["send", "--raw", &project.id, &req_id])
        .output()
        .unwrap();
    assert!(raw.status.success());
    assert!(raw.stdout.windows(4).any(|w| w == b"\x1b[2J"));

    server.join().unwrap();
}

#[test]
fn send_resolves_environments_and_env_secret_refs_without_plaintext_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("data");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 512];
            while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
                if raw.len() > 16 * 1024 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&raw);
            let request_line = text.lines().next().unwrap_or("");
            let api_key_line = text
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("x-api-key:"))
                .unwrap_or("");
            let body = format!("{request_line}\n{api_key_line}");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let store = ProjectStore::at(&home);
    let mut project = store.create("EnvSecretCli").unwrap();
    project.variables.insert("host".into(), addr.to_string());
    project.variables.insert("stage".into(), "base".into());
    project.secret_variables.insert(
        "api_key".into(),
        SecretRef::Env {
            var: "FASTMAN_TEST_CLI_SECRET".into(),
        },
    );

    let mut local_env = Environment::new("local");
    local_env.variables.insert("stage".into(), "local".into());
    let local_id = local_env.id.clone();

    let mut staging_env = Environment::new("staging");
    staging_env
        .variables
        .insert("stage".into(), "staging".into());

    project.environments = vec![local_env, staging_env];
    project.active_environment = Some(local_id.clone());

    let mut req = ApiRequest::new("GET", "Check", "http://{{host}}/{{stage}}");
    req.headers.push(KeyValue::new("X-Api-Key", "{{api_key}}"));
    let req_id = req.id.clone();
    project.collections[0].requests.push(req);
    store.save(&project).unwrap();

    // Ensure disk JSON stores only the env var reference, never the runtime secret.
    let disk_json = fs::read_to_string(home.join(&project.id).join("project.json")).unwrap();
    assert!(disk_json.contains("FASTMAN_TEST_CLI_SECRET"));
    assert!(!disk_json.contains("super_secret_cli_token"));

    // 1) Default active environment (`local`)
    let active_out = Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
        .env("FASTMAN_HOME", &home)
        .env("FASTMAN_TEST_CLI_SECRET", "super_secret_cli_token")
        .args(["send", &project.id, &req_id])
        .output()
        .unwrap();
    assert!(
        active_out.status.success(),
        "{}",
        String::from_utf8_lossy(&active_out.stderr)
    );
    let active_stdout = String::from_utf8_lossy(&active_out.stdout);
    assert!(active_stdout.contains("GET /local HTTP/1.1"));
    assert!(!active_stdout.contains("super_secret_cli_token"));
    assert!(active_stdout.contains("<REDACTED>"));

    // 2) Override by name; explicit raw output preserves the server's echo.
    let staging_out = Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
        .env("FASTMAN_HOME", &home)
        .env("FASTMAN_TEST_CLI_SECRET", "super_secret_cli_token")
        .args(["send", "--raw", "--env", "staging", &project.id, &req_id])
        .output()
        .unwrap();
    assert!(
        staging_out.status.success(),
        "{}",
        String::from_utf8_lossy(&staging_out.stderr)
    );
    let staging_stdout = String::from_utf8_lossy(&staging_out.stdout);
    assert!(staging_stdout.contains("GET /staging HTTP/1.1"));
    assert!(staging_stdout.contains("super_secret_cli_token"));

    // 3) Override with `--env=<id>` form
    let id_flag = format!("--env={local_id}");
    let by_id_out = Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
        .env("FASTMAN_HOME", &home)
        .env("FASTMAN_TEST_CLI_SECRET", "super_secret_cli_token")
        .args(["send", &id_flag, &project.id, &req_id])
        .output()
        .unwrap();
    assert!(by_id_out.status.success());
    assert!(String::from_utf8_lossy(&by_id_out.stdout).contains("GET /local HTTP/1.1"));

    // 4) Unknown `--env` fails non-zero before making a network call
    let bad_env = Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
        .env("FASTMAN_HOME", &home)
        .env("FASTMAN_TEST_CLI_SECRET", "super_secret_cli_token")
        .args(["send", "--env", "nonexistent", &project.id, &req_id])
        .output()
        .unwrap();
    assert!(!bad_env.status.success());
    assert!(String::from_utf8_lossy(&bad_env.stderr).contains("Environment not found"));

    // 5) Missing secret env var fails non-zero with unresolved secret message
    let missing_secret = Command::new(env!("CARGO_BIN_EXE_fastman-cli"))
        .env("FASTMAN_HOME", &home)
        .env_remove("FASTMAN_TEST_CLI_SECRET")
        .args(["send", &project.id, &req_id])
        .output()
        .unwrap();
    assert!(!missing_secret.status.success());
    let missing_err = String::from_utf8_lossy(&missing_secret.stderr);
    assert!(missing_err.contains("api_key"));

    server.join().unwrap();
}
