//! Opt-in localhost measurements, not timing-sensitive CI assertions.

use fastman_core::{ApiRequest, HttpEngine};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    samples[(samples.len() * percentile).div_ceil(100) - 1]
}

#[tokio::test]
#[ignore = "run explicitly in release mode to measure localhost HTTP performance"]
async fn localhost_request_latency() {
    const WARMUP: usize = 20;
    const SAMPLES: usize = 100;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let server_connections = Arc::clone(&connections);
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            stream.set_nodelay(true).unwrap();
            server_connections.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    let size: usize = line
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .trim_start_matches('/')
                        .split('?')
                        .next()
                        .unwrap()
                        .parse()
                        .unwrap();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).await.unwrap() == 0 {
                            return;
                        }
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\n\r\n");
                    if writer.write_all(header.as_bytes()).await.is_err()
                        || writer.write_all(&vec![b'x'; size]).await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });

    let engine = HttpEngine::new().unwrap();
    let variables = BTreeMap::new();
    println!("localhost HTTP: {WARMUP} warmup, {SAMPLES} samples per payload");
    for size in [0, 1024, 100 * 1024, 2 * 1024 * 1024 - 1] {
        let request = ApiRequest::new("GET", "performance", format!("http://{address}/{size}"));
        let before_connections = connections.load(Ordering::Relaxed);
        let mut samples = Vec::with_capacity(SAMPLES);
        for iteration in 0..WARMUP + SAMPLES {
            let started = Instant::now();
            let response = engine.send(&request, &variables).await.unwrap();
            let elapsed = started.elapsed();
            assert_eq!(response.status, 200);
            assert_eq!(response.bytes_received, size);
            assert!(!response.truncated);
            if iteration >= WARMUP {
                samples.push(elapsed);
            }
        }
        samples.sort_unstable();
        println!(
            "{size:>8} bytes: p50 {:?}, p95 {:?}, p99 {:?}; new connections: {}",
            percentile(&samples, 50),
            percentile(&samples, 95),
            percentile(&samples, 99),
            connections.load(Ordering::Relaxed) - before_connections,
        );
    }
    server.abort();
}
