use fastman_core::{
    HttpEngine, ProjectStore,
    import::collection_v21,
    model::{ApiRequest, Auth, Folder, HttpBody, KeyValue, Project},
};
use std::collections::BTreeMap;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn projects_persist_requests_and_reject_invalid_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProjectStore::at(dir.path());
    assert!(store.list().unwrap().is_empty());
    let mut project = Project::new("Zulu");
    let mut folder = Folder::new("nested");
    let mut request = ApiRequest::new("PATCH", "edit", "http://localhost");
    request.params.push(KeyValue::new("a", "b"));
    request.headers.push(KeyValue::new("X-Test", "value"));
    request.body = HttpBody::FormUrlEncoded(vec![KeyValue::new("field", "value")]);
    request.timeout_ms = 1234;
    request.auth = Auth::Bearer {
        token: "secret-canary".into(),
    };
    let id = request.id.clone();
    folder.requests.push(request);
    project.collections[0].folders.push(folder);
    store.save(&project).unwrap();
    store.create("Alpha").unwrap();
    assert_eq!(
        store
            .list()
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Zulu"]
    );
    let loaded = store.load(&project.id).unwrap();
    let saved = loaded.find_request(&id).unwrap();
    assert_eq!(saved.method, "PATCH");
    assert_eq!(saved.params[0].value, "b");
    assert_eq!(saved.headers[0].value, "value");
    assert_eq!(
        saved.body,
        HttpBody::FormUrlEncoded(vec![KeyValue::new("field", "value")])
    );
    assert_eq!(saved.timeout_ms, 1234);
    assert_eq!(saved.auth, Auth::None);
    let path = dir.path().join(&project.id).join("project.json");
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("secret-canary")
    );
    assert!(loaded.find_request("missing").is_none());
    assert!(store.load("../invalid").is_err());
    let mut invalid = project.clone();
    invalid.schema_version += 1;
    assert!(store.save(&invalid).is_err());
    std::fs::write(path, b"not json").unwrap();
    assert!(store.load(&project.id).is_err());
}

#[test]
fn import_reports_unsupported_features_and_preserves_body() {
    let input = r#"{"info":{"name":"Example","schema":"https://example.com/schema/collection/v2.1.0/collection.json"},
      "auth":{},"event":[],"item":[{"name":"Folder","item":[{"name":"Create","event":[],"request":{
      "method":"POST","url":{"raw":"https://example.com"},"auth":{},
      "header":[{"key":"X-Test","value":"yes","disabled":true}],
      "body":{"mode":"raw","raw":"{}","options":{"raw":{"language":"json"}}}}}]}]}"#;
    let report = collection_v21(input).unwrap();
    assert_eq!(report.requests_imported, 1);
    let request = &report.project.collections[0].folders[0].requests[0];
    assert_eq!(request.body, HttpBody::Json("{}".into()));
    assert!(!request.headers[0].enabled);
    assert_eq!(report.warnings.len(), 4);
    assert!(collection_v21(r#"{"info":{"schema":"v2.0"}}"#).is_err());
    assert!(collection_v21("not json").is_err());
}

async fn exchange(
    mut request: ApiRequest,
    variables: BTreeMap<String, String>,
) -> (String, fastman_core::http::HttpResponse) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    request.url = format!("http://{}/path", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut data = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            assert_ne!(n, 0);
            data.extend_from_slice(&buf[..n]);
            if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&data[..end]);
                let len = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if data.len() >= end + 4 + len {
                    break;
                }
            }
        }
        stream.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\nX-Reply: yes\r\nConnection: close\r\n\r\nok").await.unwrap();
        String::from_utf8(data).unwrap()
    });
    let response = HttpEngine::new()
        .unwrap()
        .send(&request, &variables)
        .await
        .unwrap();
    (server.await.unwrap(), response)
}

#[tokio::test]
async fn sends_request_fields_and_authentication() {
    let variants = [
        (
            Auth::Basic {
                username: "user".into(),
                password: "pass".into(),
            },
            "authorization: Basic dXNlcjpwYXNz",
        ),
        (
            Auth::Bearer {
                token: "{{token}}".into(),
            },
            "authorization: Bearer secret",
        ),
        (
            Auth::ApiKeyHeader {
                name: "x-api-key".into(),
                value: "{{token}}".into(),
            },
            "x-api-key: secret",
        ),
        (
            Auth::ApiKeyQuery {
                name: "key".into(),
                value: "{{token}}".into(),
            },
            "key=secret",
        ),
    ];
    for (auth, expected) in variants {
        let mut request = ApiRequest::new("POST", "test", "");
        request.auth = auth;
        request.params.push(KeyValue::new("q", "{{token}}"));
        request.headers.push(KeyValue::new("X-Custom", "present"));
        request.body = HttpBody::Json(r#"{"ok":true}"#.into());
        let (wire, response) =
            exchange(request, BTreeMap::from([("token".into(), "secret".into())])).await;
        assert!(wire.contains(expected), "{wire}");
        assert!(wire.starts_with("POST /path?"));
        assert!(wire.contains("q=secret"));
        assert!(wire.contains("x-custom: present"));
        assert!(wire.contains("content-type: application/json"));
        assert!(wire.ends_with(r#"{"ok":true}"#));
        assert_eq!(response.status, 201);
        assert_eq!(response.body, "ok");
        assert!(
            response
                .headers
                .iter()
                .any(|(k, v)| k == "x-reply" && v == "yes")
        );
    }
}

#[tokio::test]
async fn form_body_and_invalid_requests() {
    let mut request = ApiRequest::new("POST", "form", "");
    let mut disabled = KeyValue::new("skip", "value");
    disabled.enabled = false;
    request.body = HttpBody::FormUrlEncoded(vec![KeyValue::new("field", "hello world"), disabled]);
    let (wire, _) = exchange(request, BTreeMap::new()).await;
    assert!(wire.contains("content-type: application/x-www-form-urlencoded"));
    assert!(wire.ends_with("field=hello+world"));
    let engine = HttpEngine::new().unwrap();
    let invalid = ApiRequest::new("BAD METHOD", "invalid", "http://127.0.0.1");
    assert!(engine.send(&invalid, &BTreeMap::new()).await.is_err());
    let invalid = ApiRequest::new("GET", "invalid", "file:///etc/passwd");
    assert!(engine.send(&invalid, &BTreeMap::new()).await.is_err());
}

#[tokio::test]
async fn response_limit_and_non_utf8_are_reported() {
    for (body, expected_bytes, truncated) in [
        (vec![b'x'; 2 * 1024 * 1024 + 1], 2 * 1024 * 1024, true),
        (vec![0xff, 0xfe], 2, false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            // A client may close the connection after it reaches the limit.
            let _ = stream.write_all(&body).await;
        });
        let request = ApiRequest::new("GET", "response", format!("http://{addr}"));
        let response = HttpEngine::new()
            .unwrap()
            .send(&request, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(response.bytes_received, expected_bytes);
        assert_eq!(response.truncated, truncated);
        if !truncated {
            assert!(response.body.contains("non-UTF8 response"));
            assert!(response.body.contains("ff fe"));
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn slow_request_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        drop(stream);
    });
    let mut request = ApiRequest::new("GET", "slow", format!("http://{addr}"));
    request.timeout_ms = 100;
    let error = HttpEngine::new()
        .unwrap()
        .send(&request, &BTreeMap::new())
        .await
        .unwrap_err();
    assert!(matches!(error, fastman_core::http::HttpError::Transport(ref e) if e.is_timeout()));
    server.await.unwrap();
}

#[tokio::test]
async fn avoids_duplicate_content_type_and_auth_headers_and_redacts_error_urls() {
    let mut request = ApiRequest::new("POST", "dedup", "");
    request.auth = Auth::Bearer {
        token: "new_secret".into(),
    };
    request
        .headers
        .push(KeyValue::new("Authorization", "Bearer stale_secret"));
    request.headers.push(KeyValue::new(
        "Content-Type",
        "application/merge-patch+json",
    ));
    request.body = HttpBody::Json(r#"{"x":1}"#.into());
    let (wire, _) = exchange(request, BTreeMap::new()).await;
    let lower = wire.to_ascii_lowercase();
    assert_eq!(lower.matches("content-type:").count(), 1, "{wire}");
    assert!(lower.contains("content-type: application/merge-patch+json"));
    assert_eq!(lower.matches("authorization:").count(), 1, "{wire}");
    assert!(wire.contains("Bearer new_secret"));
    assert!(!wire.contains("stale_secret"));

    // Empty ApiKeyQuery name rejected cleanly.
    let mut bad_key = ApiRequest::new("GET", "badkey", "http://127.0.0.1:9");
    bad_key.auth = Auth::ApiKeyQuery {
        name: "  ".into(),
        value: "secret_canary".into(),
    };
    let engine = HttpEngine::new().unwrap();
    assert!(engine.send(&bad_key, &BTreeMap::new()).await.is_err());

    // Transport errors must not leak ApiKeyQuery secrets in the formatted URL.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let mut closed = ApiRequest::new("GET", "closed", format!("http://{addr}/fail"));
    closed.auth = Auth::ApiKeyQuery {
        name: "api_key".into(),
        value: "secret_canary_99".into(),
    };
    let err = engine.send(&closed, &BTreeMap::new()).await.unwrap_err();
    assert!(!err.to_string().contains("secret_canary_99"), "{err}");
}
