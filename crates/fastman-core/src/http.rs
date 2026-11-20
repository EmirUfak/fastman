use crate::model::{ApiRequest, Auth, HttpBody, KeyValue};
use reqwest::{
    Client, Method, Url,
    header::{HeaderName, HeaderValue},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use thiserror::Error;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Unknown variable: {{{{{0}}}}}")]
    MissingVariable(String),
    #[error("Unclosed variable expression")]
    UnclosedVariable,
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url_error::UrlError),
    #[error("Invalid HTTP method: {0}")]
    Method(String),
    #[error("Invalid header name: {0}")]
    HeaderName(String),
    #[error("Invalid header value for {0}")]
    HeaderValue(String),
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

#[derive(Clone)]
pub struct HttpEngine {
    client: Client,
}

impl HttpEngine {
    pub fn new() -> Result<Self, HttpError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;
        Ok(Self { client })
    }

    /// The only code path that performs network I/O. Nothing is sent until
    /// this method is explicitly called by the user.
    pub async fn send(
        &self,
        request: &ApiRequest,
        variables: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, HttpError> {
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
                pairs.append_pair(name, &interpolate(value, variables)?);
            }
        }

        let timeout = Duration::from_millis(request.timeout_ms.clamp(100, 300_000));
        let mut builder = self.client.request(method, url).timeout(timeout);
        for header in &request.headers {
            if !header.enabled || header.key.trim().is_empty() {
                continue;
            }
            let name_text = interpolate(&header.key, variables)?;
            let name = HeaderName::from_bytes(name_text.as_bytes())
                .map_err(|_| HttpError::HeaderName(name_text.clone()))?;
            let value_text = interpolate(&header.value, variables)?;
            let value = HeaderValue::from_str(&value_text)
                .map_err(|_| HttpError::HeaderValue(name_text))?;
            builder = builder.header(name, value);
        }

        builder = match &request.auth {
            Auth::None | Auth::ApiKeyQuery { .. } => builder,
            Auth::Basic { username, password } => builder.basic_auth(
                interpolate(username, variables)?,
                Some(interpolate(password, variables)?),
            ),
            Auth::Bearer { token } => builder.bearer_auth(interpolate(token, variables)?),
            Auth::ApiKeyHeader { name, value } => {
                let header_name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| HttpError::HeaderName(name.clone()))?;
                let header_value = HeaderValue::from_str(&interpolate(value, variables)?)
                    .map_err(|_| HttpError::HeaderValue(name.clone()))?;
                builder.header(header_name, header_value)
            }
        };

        builder = match &request.body {
            HttpBody::None => builder,
            HttpBody::Text(text) => builder.body(interpolate(text, variables)?),
            HttpBody::Json(text) => builder
                .header("Content-Type", "application/json")
                .body(interpolate(text, variables)?),
            HttpBody::FormUrlEncoded(rows) => {
                let data = expand_pairs(rows, variables)?;
                builder.form(&data)
            }
        };
        let started = Instant::now();
        let mut response = builder.send().await?;
        let status = response.status();
        let final_url = response.url().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
            .collect();
        let mut received = Vec::with_capacity(8192);
        let mut truncated = false;
        while let Some(chunk) = response.chunk().await? {
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

/// Single-pass substitution; values are not recursively interpreted. This
/// prevents unbounded or cyclic interpolation of variables.
pub fn interpolate(input: &str, variables: &BTreeMap<String, String>) -> Result<String, HttpError> {
    let mut output = String::with_capacity(input.len());
    let mut remainder = input;
    while let Some(start) = remainder.find("{{") {
        output.push_str(&remainder[..start]);
        let after_open = &remainder[start + 2..];
        let end = after_open.find("}}").ok_or(HttpError::UnclosedVariable)?;
        let name = after_open[..end].trim();
        let value = variables
            .get(name)
            .ok_or_else(|| HttpError::MissingVariable(name.to_owned()))?;
        output.push_str(value);
        remainder = &after_open[end + 2..];
    }
    output.push_str(remainder);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_is_single_pass_and_rejects_unknowns() {
        let vars = BTreeMap::from([
            ("host".to_string(), "https://example.com".to_string()),
            ("nested".to_string(), "{{host}}".to_string()),
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
    }

    #[tokio::test]
    async fn localhost_http_roundtrip() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
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
}
