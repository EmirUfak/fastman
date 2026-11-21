use crate::model::{ApiRequest, Auth, HttpBody, KeyValue};
use crate::security::{is_valid_http_method, redact_url_secrets};
use reqwest::{
    Client, Method, Url,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use thiserror::Error;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub use crate::template::MAX_INTERPOLATED_BYTES;
const MAX_REDIRECT_HOPS: usize = 10;

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Unknown variable: {{{{{0}}}}}")]
    MissingVariable(String),
    #[error("Unclosed variable expression")]
    UnclosedVariable,
    #[error("Interpolated value exceeds maximum size ({0} bytes)")]
    InterpolatedTooLarge(usize),
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url_error::UrlError),
    #[error("Invalid HTTP method: {0}")]
    Method(String),
    #[error("Invalid header name: {0}")]
    HeaderName(String),
    #[error("Invalid header value for {0}")]
    HeaderValue(String),
    #[error("API key query parameter name is empty")]
    EmptyApiKeyParam,
    #[error("HTTP request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

// A private wrapper avoids a separate direct `url` dependency.
mod url_error {
    #[derive(Debug, thiserror::Error)]
    #[error("{0}")]
    pub struct UrlError(pub String);
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub elapsed_ms: u128,
    pub final_url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub bytes_received: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedirectDecision {
    Follow,
    Stop,
    TooMany,
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str().is_some()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Only follow bounded, same-origin redirects without scheme downgrades or embedded userinfo.
/// Cross-origin and HTTPS-to-HTTP redirects stop before opening a new connection so custom
/// headers (e.g. `X-API-Key`), query secrets, and bodies never leak to third parties.
fn evaluate_redirect(initial: &Url, next: &Url, hops_so_far: usize) -> RedirectDecision {
    if hops_so_far > MAX_REDIRECT_HOPS {
        return RedirectDecision::TooMany;
    }
    if next.scheme() != "http" && next.scheme() != "https" {
        return RedirectDecision::Stop;
    }
    if initial.scheme() == "https" && next.scheme() == "http" {
        return RedirectDecision::Stop;
    }
    if !next.username().is_empty() || next.password().is_some() {
        return RedirectDecision::Stop;
    }
    if !same_origin(initial, next) {
        return RedirectDecision::Stop;
    }
    RedirectDecision::Follow
}

fn sanitize_reqwest_error(err: reqwest::Error) -> reqwest::Error {
    let has_sensitive_url = err.url().is_some_and(|url| {
        url.query().is_some() || !url.username().is_empty() || url.password().is_some()
    });
    if has_sensitive_url {
        err.without_url()
    } else {
        err
    }
}

/// Returns a redacted summary of active proxy environment variables, if set.
pub fn configured_env_proxy_summary() -> Option<String> {
    for var in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(format!("{var}={}", redact_url_secrets(trimmed)));
            }
        }
    }
    None
}

#[derive(Clone)]
pub struct HttpEngine {
    client: Client,
}

impl HttpEngine {
    pub fn new() -> Result<Self, HttpError> {
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            let Some(initial) = attempt.previous().first() else {
                return attempt.stop();
            };
            match evaluate_redirect(initial, attempt.url(), attempt.previous().len()) {
                RedirectDecision::Follow => attempt.follow(),
                RedirectDecision::Stop => attempt.stop(),
                RedirectDecision::TooMany => attempt.error("too many redirects"),
            }
        });
        let client = Client::builder().referer(false).redirect(policy).build()?;
        Ok(Self { client })
    }

    /// The only code path that performs network I/O. Nothing is sent until
    /// this method is explicitly called by the user.
    pub async fn send(
        &self,
        request: &ApiRequest,
        variables: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, HttpError> {
        if !is_valid_http_method(&request.method) {
            return Err(HttpError::Method(request.method.clone()));
        }
        let method = Method::from_bytes(request.method.as_bytes())
            .map_err(|_| HttpError::Method(request.method.clone()))?;
        let url_text = interpolate(&request.url, variables)?;
        let mut url = Url::parse(&url_text)
            .map_err(|e| HttpError::InvalidUrl(url_error::UrlError(e.to_string())))?;
        if url.scheme() != "https" && url.scheme() != "http" {
            return Err(HttpError::InvalidUrl(url_error::UrlError(
                "Only HTTP and HTTPS are supported".into(),
            )));
        }

        {
            let mut pairs = url.query_pairs_mut();
            for item in &request.params {
                if item.enabled && !item.key.trim().is_empty() {
                    pairs.append_pair(
                        &interpolate(&item.key, variables)?,
                        &interpolate(&item.value, variables)?,
                    );
                }
            }
            if let Auth::ApiKeyQuery { name, value } = &request.auth {
                let key_text = interpolate(name, variables)?;
                let key = key_text.trim();
                if key.is_empty() {
                    return Err(HttpError::EmptyApiKeyParam);
                }
                pairs.append_pair(key, &interpolate(value, variables)?);
            }
        }

        let resolved_api_key_header = match &request.auth {
            Auth::ApiKeyHeader { name, value } => {
                let name_text = interpolate(name, variables)?;
                let trimmed = name_text.trim();
                if trimmed.is_empty() {
                    return Err(HttpError::HeaderName("API key header name is empty".into()));
                }
                let header_name = HeaderName::from_bytes(trimmed.as_bytes())
                    .map_err(|_| HttpError::HeaderName(trimmed.to_owned()))?;
                let header_value = HeaderValue::from_str(&interpolate(value, variables)?)
                    .map_err(|_| HttpError::HeaderValue(trimmed.to_owned()))?;
                Some((header_name, header_value))
            }
            _ => None,
        };

        let auth_header_name: Option<HeaderName> = match &request.auth {
            Auth::Basic { .. } | Auth::Bearer { .. } => Some(AUTHORIZATION),
            Auth::ApiKeyHeader { .. } => resolved_api_key_header.as_ref().map(|(k, _)| k.clone()),
            Auth::None | Auth::ApiKeyQuery { .. } => None,
        };

        let mut header_map = HeaderMap::new();
        for header in &request.headers {
            if !header.enabled || header.key.trim().is_empty() {
                continue;
            }
            let name_text = interpolate(&header.key, variables)?;
            let name = HeaderName::from_bytes(name_text.trim().as_bytes())
                .map_err(|_| HttpError::HeaderName(name_text.clone()))?;
            if auth_header_name.as_ref() == Some(&name) {
                continue;
            }
            let value_text = interpolate(&header.value, variables)?;
            let value = HeaderValue::from_str(&value_text)
                .map_err(|_| HttpError::HeaderValue(name_text))?;
            if name == CONTENT_TYPE || name == AUTHORIZATION {
                header_map.insert(name, value);
            } else {
                header_map.append(name, value);
            }
        }

        let has_content_type = header_map.contains_key(CONTENT_TYPE);
        let timeout = Duration::from_millis(request.timeout_ms.clamp(100, 300_000));
        let mut builder = self
            .client
            .request(method, url)
            .timeout(timeout)
            .headers(header_map);

        builder = match &request.auth {
            Auth::None | Auth::ApiKeyQuery { .. } => builder,
            Auth::Basic { username, password } => builder.basic_auth(
                interpolate(username, variables)?,
                Some(interpolate(password, variables)?),
            ),
            Auth::Bearer { token } => builder.bearer_auth(interpolate(token, variables)?),
            Auth::ApiKeyHeader { .. } => {
                if let Some((header_name, header_value)) = resolved_api_key_header {
                    builder.header(header_name, header_value)
                } else {
                    builder
                }
            }
        };

        builder = match &request.body {
            HttpBody::None => builder,
            HttpBody::Text(text) => builder.body(interpolate(text, variables)?),
            HttpBody::Json(text) => {
                if !has_content_type {
                    builder = builder.header(CONTENT_TYPE, "application/json");
                }
                builder.body(interpolate(text, variables)?)
            }
            HttpBody::FormUrlEncoded(rows) => {
                let data = expand_pairs(rows, variables)?;
                if has_content_type {
                    let mut dummy = Url::parse("http://localhost").expect("static URL");
                    {
                        let mut qp = dummy.query_pairs_mut();
                        for (k, v) in &data {
                            qp.append_pair(k, v);
                        }
                    }
                    builder.body(dummy.query().unwrap_or("").to_owned())
                } else {
                    builder.form(&data)
                }
            }
        };
        let started = Instant::now();
        let mut response = builder.send().await.map_err(sanitize_reqwest_error)?;
        let status = response.status();
        let final_url = response.url().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
            .collect();
        let mut received = Vec::with_capacity(8192);
        let mut truncated = false;
        while let Some(chunk) = response.chunk().await.map_err(sanitize_reqwest_error)? {
            let remaining = MAX_RESPONSE_BYTES.saturating_sub(received.len());
            if chunk.len() > remaining {
                received.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }
            received.extend_from_slice(&chunk);
            if received.len() == MAX_RESPONSE_BYTES {
                truncated = true;
                break;
            }
        }
        let bytes_received = received.len();
        let body = String::from_utf8(received).unwrap_or_else(|e| {
            format!(
                "<non-UTF8 response: {} bytes>\n{}",
                bytes_received,
                hex_preview(e.as_bytes(), 80)
            )
        });
        Ok(HttpResponse {
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or("").to_owned(),
            elapsed_ms: started.elapsed().as_millis(),
            final_url,
            headers,
            body,
            bytes_received,
            truncated,
        })
    }
}

fn hex_preview(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn expand_pairs(
    items: &[KeyValue],
    variables: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, HttpError> {
    items
        .iter()
        .filter(|item| item.enabled && !item.key.trim().is_empty())
        .map(|item| {
            Ok((
                interpolate(&item.key, variables)?,
                interpolate(&item.value, variables)?,
            ))
        })
        .collect()
}

/// Single-pass substitution; values are not recursively interpreted and total
/// output length is bounded to prevent memory exhaustion bombs.
pub fn interpolate(input: &str, variables: &BTreeMap<String, String>) -> Result<String, HttpError> {
    crate::template::expand(
        input,
        |name, _| {
            let name = name.ok_or(HttpError::UnclosedVariable)?;
            variables
                .get(name)
                .map(String::as_str)
                .ok_or_else(|| HttpError::MissingVariable(name.to_owned()))
        },
        || HttpError::InterpolatedTooLarge(MAX_INTERPOLATED_BYTES),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn interpolation_is_single_pass_and_rejects_unknowns_and_bombs() {
        let vars = BTreeMap::from([
            ("host".to_string(), "https://example.com".to_string()),
            ("nested".to_string(), "{{host}}".to_string()),
            ("big".to_string(), "x".repeat(1024 * 1024)),
        ]);
        assert_eq!(
            interpolate("{{host}}/v1", &vars).unwrap(),
            "https://example.com/v1"
        );
        assert_eq!(interpolate("{{nested}}", &vars).unwrap(), "{{host}}");
        assert!(matches!(
            interpolate("{{unknown}}", &vars),
            Err(HttpError::MissingVariable(_))
        ));
        assert!(matches!(
            interpolate("{{host", &vars),
            Err(HttpError::UnclosedVariable)
        ));
        let bomb = "{{big}}".repeat(5);
        assert!(matches!(
            interpolate(&bomb, &vars),
            Err(HttpError::InterpolatedTooLarge(_))
        ));
    }

    #[test]
    fn redirect_policy_enforces_same_origin_scheme_port_and_hop_limit() {
        let https_origin = Url::parse("https://api.example.com/v1/login").unwrap();
        let same = Url::parse("https://api.example.com:443/v2/next").unwrap();
        let diff_port = Url::parse("https://api.example.com:8443/v2").unwrap();
        let diff_host = Url::parse("https://evil.example/steal").unwrap();
        let downgrade = Url::parse("http://api.example.com/v2").unwrap();
        let userinfo = Url::parse("https://user:pass@api.example.com/v2").unwrap();
        let file_url = Url::parse("file:///etc/passwd").unwrap();

        assert_eq!(
            evaluate_redirect(&https_origin, &same, 1),
            RedirectDecision::Follow
        );
        assert_eq!(
            evaluate_redirect(&https_origin, &same, 10),
            RedirectDecision::Follow
        );
        assert_eq!(
            evaluate_redirect(&https_origin, &same, 11),
            RedirectDecision::TooMany
        );
        assert_eq!(
            evaluate_redirect(&https_origin, &diff_port, 1),
            RedirectDecision::Stop
        );
        assert_eq!(
            evaluate_redirect(&https_origin, &diff_host, 1),
            RedirectDecision::Stop
        );
        assert_eq!(
            evaluate_redirect(&https_origin, &downgrade, 1),
            RedirectDecision::Stop
        );
        assert_eq!(
            evaluate_redirect(&https_origin, &userinfo, 1),
            RedirectDecision::Stop
        );
        assert_eq!(
            evaluate_redirect(&https_origin, &file_url, 1),
            RedirectDecision::Stop
        );
    }

    #[tokio::test]
    async fn localhost_http_roundtrip() {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut conn, _) = server.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let _ = conn.read(&mut buffer).await.unwrap();
            conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let mut request = ApiRequest::new("GET", "Local test", format!("http://{addr}/ping"));
        request.params.push(KeyValue::new("check", "yes"));
        let response = HttpEngine::new()
            .unwrap()
            .send(&request, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
        assert!(response.final_url.contains("check=yes"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn cross_origin_redirects_do_not_leak_custom_api_keys() {
        for status in [301u16, 302, 303, 307, 308] {
            let target_hits = Arc::new(AtomicUsize::new(0));
            let target_hits_bg = Arc::clone(&target_hits);
            let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target_addr = target.local_addr().unwrap();
            let target_task = tokio::spawn(async move {
                while let Ok((mut conn, _)) = target.accept().await {
                    target_hits_bg.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 1024];
                    let _ = conn.read(&mut buf).await;
                    let _ = conn
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await;
                }
            });

            let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin_addr = origin.local_addr().unwrap();
            let origin_task = tokio::spawn(async move {
                let (mut conn, _) = origin.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = conn.read(&mut buf).await.unwrap();
                let reply = format!(
                    "HTTP/1.1 {status} Redirect\r\nLocation: http://{target_addr}/collect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                conn.write_all(reply.as_bytes()).await.unwrap();
            });

            let mut request = ApiRequest::new(
                "POST",
                "Redirect test",
                format!("http://{origin_addr}/start"),
            );
            request
                .headers
                .push(KeyValue::new("X-API-Key", "secret-hdr"));
            request.auth = Auth::ApiKeyQuery {
                name: "api_key".into(),
                value: "secret-qry".into(),
            };
            request.body = HttpBody::Text("secret-body".into());

            let response = HttpEngine::new()
                .unwrap()
                .send(&request, &BTreeMap::new())
                .await
                .unwrap();

            assert_eq!(response.status, status);
            assert!(
                response
                    .final_url
                    .starts_with(&format!("http://{origin_addr}"))
            );
            assert_eq!(target_hits.load(Ordering::SeqCst), 0);

            origin_task.await.unwrap();
            target_task.abort();
        }
    }

    #[tokio::test]
    async fn same_origin_redirect_follows_without_referer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut c1, _) = listener.accept().await.unwrap();
            let mut b1 = [0u8; 4096];
            let _ = c1.read(&mut b1).await.unwrap();
            c1.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /dest\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();

            let (mut c2, _) = listener.accept().await.unwrap();
            let mut b2 = [0u8; 4096];
            let n = c2.read(&mut b2).await.unwrap();
            let second_req = String::from_utf8_lossy(&b2[..n]).to_ascii_lowercase();
            assert!(
                !second_req.contains("referer:"),
                "unexpected referer: {second_req}"
            );
            c2.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndone")
                .await
                .unwrap();
        });

        let mut request = ApiRequest::new("GET", "Same origin", format!("http://{addr}/start"));
        request.auth = Auth::ApiKeyQuery {
            name: "api_key".into(),
            value: "secret".into(),
        };
        let response = HttpEngine::new()
            .unwrap()
            .send(&request, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "done");
        task.await.unwrap();
    }
}
