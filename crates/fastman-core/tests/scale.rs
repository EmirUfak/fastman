//! Opt-in scale baselines and cross-format regression fixtures (F08).
//! Run: `cargo test --locked -p fastman-core --release --test scale -- --ignored --nocapture`.

use fastman_core::{ApiRequest, Project, ProjectStore, curl, security};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

fn sample(label: &str, iterations: usize, mut operation: impl FnMut()) -> Duration {
    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    let elapsed = started.elapsed();
    println!(
        "{label:>28}: {elapsed:?} total, {:?}/op",
        elapsed / iterations as u32
    );
    elapsed
}

fn scale_project(requests: usize) -> Project {
    let mut project = Project::new("Scale baseline");
    project.collections[0].requests = (0..requests)
        .map(|i| {
            ApiRequest::new(
                "GET",
                format!("request {i}"),
                format!("https://example.com/items/{i}?q=value"),
            )
        })
        .collect();
    project
}

#[test]
#[ignore = "scale benchmark: run in release mode with --ignored --nocapture"]
fn storage_load_save_scale_baseline() {
    const ITERATIONS: usize = 3;
    for size in [100, 1_000, 10_000] {
        let root = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(root.path());
        let project = scale_project(size);
        store.save(&project).unwrap();
        let bytes = std::fs::metadata(root.path().join(&project.id).join("project.json"))
            .unwrap()
            .len();
        println!("\n{size} requests ({bytes} bytes on disk)");

        sample("load", ITERATIONS, || {
            store.load(&project.id).unwrap();
        });
        sample("save", ITERATIONS, || {
            store.save(&project).unwrap();
        });
        sample("list_resilient", ITERATIONS, || {
            store.list_resilient().unwrap();
        });
    }
}

#[test]
fn url_query_and_curl_roundtrip_fixtures() {
    let fixtures = [
        "https://example.com/p?a=1&a=2&empty=&bare#frag",
        "https://example.com/p?space=a%20b&plus=a%2Bb&encoded=a%26b%3Dc#frag",
        "https://example.com/p?uni=%C3%A9t%C3%A9&raw=a+b&tpl={{token}}#frag",
    ];
    let variables = BTreeMap::new();
    for raw in fixtures {
        let (cleaned, params) = security::split_url_query(raw).unwrap();
        assert!(cleaned.ends_with("#frag"), "fragment lost: {cleaned}");
        let mut request = ApiRequest::new("GET", "fixture", cleaned);
        request.params = params;

        let command = curl::to_curl_export(&request, &variables, false)
            .unwrap()
            .command;
        let reparsed = curl::parse_curl(&command).unwrap();
        assert_eq!(reparsed.url, request.url, "URL changed: {command}");
        assert_eq!(reparsed.params, request.params, "params changed: {command}");
    }

    let mut post = ApiRequest::new("POST", "payload", "https://example.com/hooks?mode=test");
    let (cleaned, params) = security::split_url_query(&post.url).unwrap();
    post.url = cleaned;
    post.params = params;
    post.headers
        .push(fastman_core::KeyValue::new("X-Custom", "a'b"));
    post.body = fastman_core::HttpBody::Text("@literal-payload".into());
    let exported = curl::to_curl_export(&post, &variables, false).unwrap();
    let reparsed = curl::parse_curl(&exported.command).unwrap();
    assert_eq!(reparsed.method, "POST");
    assert_eq!(reparsed.url, post.url);
    assert_eq!(reparsed.params, post.params);
    assert_eq!(reparsed.body, post.body);
}
