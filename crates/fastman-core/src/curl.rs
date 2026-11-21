//! Best-effort cURL command parser and generator.

use crate::http::interpolate;
use crate::model::{ApiRequest, Auth, HttpBody, KeyValue};
use anyhow::{Result, bail};
use std::collections::BTreeMap;

/// Parse a POSIX-style `curl ...` command string into an [`ApiRequest`].
pub fn parse_curl(input: &str) -> Result<ApiRequest> {
    let tokens = tokenize_shell(input)?;
    if tokens.is_empty() {
        bail!("Empty cURL command");
    }

    let mut idx = 0;
    if tokens[0]
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|cmd| cmd.eq_ignore_ascii_case("curl") || cmd.eq_ignore_ascii_case("curl.exe"))
    {
        idx = 1;
    }

    let mut explicit_method: Option<String> = None;
    let mut force_get = false;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut body_parts: Vec<String> = Vec::new();
    let mut urlencoded_parts: Vec<KeyValue> = Vec::new();
    let mut auth = Auth::None;

    while idx < tokens.len() {
        let tok = &tokens[idx];
        if tok == "--" {
            idx += 1;
            if idx < tokens.len() && url.is_none() {
                url = Some(tokens[idx].clone());
            }
            break;
        }

        if let Some(rest) = match_flag(tok, &tokens, &mut idx, "-X", "--request") {
            explicit_method = Some(rest.to_uppercase());
        } else if let Some(rest) = match_flag(tok, &tokens, &mut idx, "-H", "--header") {
            if let Some((k, v)) = rest.split_once(':') {
                let key = k.trim();
                let value = v.trim();
                if key.eq_ignore_ascii_case("authorization")
                    && let Some(token) = value
                        .strip_prefix("Bearer ")
                        .or_else(|| value.strip_prefix("bearer "))
                {
                    auth = Auth::Bearer {
                        token: token.trim().to_owned(),
                    };
                } else if !key.is_empty() {
                    headers.push(KeyValue::new(key, value));
                }
            }
        } else if let Some(rest) = match_flag(tok, &tokens, &mut idx, "-A", "--user-agent") {
            headers.push(KeyValue::new("User-Agent", rest));
        } else if let Some(rest) = match_flag(tok, &tokens, &mut idx, "-u", "--user") {
            let (user, pass) = rest.split_once(':').unwrap_or((&rest, ""));
            auth = Auth::Basic {
                username: user.to_owned(),
                password: pass.to_owned(),
            };
        } else if let Some(rest) = match_exact_flag(tok, &tokens, &mut idx, "--data-urlencode") {
            if let Some((k, v)) = rest.split_once('=') {
                urlencoded_parts.push(KeyValue::new(k, v));
            } else {
                urlencoded_parts.push(KeyValue::new(rest, ""));
            }
        } else if let Some(rest) = match_data_flag(tok, &tokens, &mut idx) {
            body_parts.push(rest);
        } else if let Some(rest) = match_exact_flag(tok, &tokens, &mut idx, "--url") {
            url = Some(rest);
        } else if tok == "-G" || tok == "--get" {
            force_get = true;
        } else if tok == "-I" || tok == "--head" {
            explicit_method = Some("HEAD".to_owned());
        } else if tok.starts_with('-') {
            // Skip known value-taking flags that aren't mapped yet so their args don't become the URL.
            if takes_separate_value(tok) && idx + 1 < tokens.len() {
                idx += 1;
            }
        } else if url.is_none() {
            url = Some(tok.clone());
        }
        idx += 1;
    }

    let raw_url = url
        .map(|u| normalize_url(&u))
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow::anyhow!("No URL found in cURL command"))?;

    let (base_url, mut params) = split_query_params(&raw_url);
    let has_body = !body_parts.is_empty() || !urlencoded_parts.is_empty();
    let method = if force_get {
        "GET".to_owned()
    } else {
        explicit_method.unwrap_or_else(|| {
            if has_body {
                "POST".to_owned()
            } else {
                "GET".to_owned()
            }
        })
    };

    let mut body = HttpBody::None;
    if force_get && !body_parts.is_empty() {
        for part in &body_parts {
            params.extend(parse_urlencoded_pairs(part));
        }
    } else if !urlencoded_parts.is_empty() {
        body = HttpBody::FormUrlEncoded(urlencoded_parts);
    } else if !body_parts.is_empty() {
        let joined = body_parts.join("&");
        let is_form = headers.iter().any(|h| {
            h.key.eq_ignore_ascii_case("content-type")
                && h.value
                    .to_ascii_lowercase()
                    .contains("application/x-www-form-urlencoded")
        });
        let is_json_header = headers.iter().any(|h| {
            h.key.eq_ignore_ascii_case("content-type")
                && h.value.to_ascii_lowercase().contains("json")
        });
        if is_form {
            body = HttpBody::FormUrlEncoded(parse_urlencoded_pairs(&joined));
        } else if is_json_header
            || joined.trim_start().starts_with('{')
            || joined.trim_start().starts_with('[')
        {
            body = HttpBody::Json(joined);
        } else {
            body = HttpBody::Text(joined);
        }
    }

    let name = derive_request_name(&base_url);
    let mut request = ApiRequest::new(method, name, base_url);
    request.params = params;
    request.headers = headers;
    request.body = body;
    request.auth = auth;
    Ok(request)
}

/// Format an [`ApiRequest`] as a POSIX shell `curl` command.
pub fn to_curl(request: &ApiRequest, variables: &BTreeMap<String, String>) -> String {
    let resolve = |text: &str| interpolate(text, variables).unwrap_or_else(|_| text.to_owned());

    let mut parts = Vec::new();
    parts.push("curl".to_owned());
    let method = request.method.trim().to_uppercase();
    if method != "GET" {
        parts.push(format!("-X {method}"));
    }

    let mut full_url = resolve(&request.url);
    let mut query_pairs: Vec<(String, String)> = request
        .params
        .iter()
        .filter(|p| p.enabled && !p.key.trim().is_empty())
        .map(|p| (resolve(&p.key), resolve(&p.value)))
        .collect();
    if let Auth::ApiKeyQuery { name, value } = &request.auth
        && !name.trim().is_empty()
    {
        query_pairs.push((name.clone(), resolve(value)));
    }
    if !query_pairs.is_empty() {
        let sep = if full_url.contains('?') { '&' } else { '?' };
        full_url.push(sep);
        let encoded: Vec<String> = query_pairs
            .iter()
            .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
            .collect();
        full_url.push_str(&encoded.join("&"));
    }
    parts.push(shell_quote(&full_url));

    let mut has_content_type = false;
    for header in &request.headers {
        if !header.enabled || header.key.trim().is_empty() {
            continue;
        }
        if header.key.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        parts.push(format!(
            "-H {}",
            shell_quote(&format!(
                "{}: {}",
                resolve(&header.key),
                resolve(&header.value)
            ))
        ));
    }

    match &request.auth {
        Auth::None | Auth::ApiKeyQuery { .. } => {}
        Auth::Basic { username, password } => {
            parts.push(format!(
                "-u {}",
                shell_quote(&format!("{}:{}", resolve(username), resolve(password)))
            ));
        }
        Auth::Bearer { token } => {
            parts.push(format!(
                "-H {}",
                shell_quote(&format!("Authorization: Bearer {}", resolve(token)))
            ));
        }
        Auth::ApiKeyHeader { name, value } => {
            if !name.trim().is_empty() {
                parts.push(format!(
                    "-H {}",
                    shell_quote(&format!("{}: {}", name, resolve(value)))
                ));
            }
        }
    }

    match &request.body {
        HttpBody::None => {}
        HttpBody::Json(text) => {
            if !has_content_type {
                parts.push("-H 'Content-Type: application/json'".to_owned());
            }
            if !text.is_empty() {
                parts.push(format!("-d {}", shell_quote(&resolve(text))));
            }
        }
        HttpBody::Text(text) => {
            if !text.is_empty() {
                parts.push(format!("-d {}", shell_quote(&resolve(text))));
            }
        }
        HttpBody::FormUrlEncoded(rows) => {
            for row in rows {
                if row.enabled && !row.key.trim().is_empty() {
                    parts.push(format!(
                        "--data-urlencode {}",
                        shell_quote(&format!("{}={}", resolve(&row.key), resolve(&row.value)))
                    ));
                }
            }
        }
    }

    parts.join(" \\\n  ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains("://") || trimmed.starts_with("{{") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
}

fn derive_request_name(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let clean = without_scheme.trim_end_matches('/');
    if let Some((_, path)) = clean.split_once('/')
        && !path.is_empty()
    {
        let last = path.rsplit('/').next().unwrap_or(path);
        if !last.is_empty() {
            return format!("/{last}");
        }
    }
    clean.to_owned()
}

fn split_query_params(raw_url: &str) -> (String, Vec<KeyValue>) {
    let Some((base, query)) = raw_url.split_once('?') else {
        return (raw_url.to_owned(), Vec::new());
    };
    let query_no_frag = query.split('#').next().unwrap_or(query);
    (base.to_owned(), parse_urlencoded_pairs(query_no_frag))
}

fn parse_urlencoded_pairs(query: &str) -> Vec<KeyValue> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            KeyValue::new(percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
                    && let Ok(byte) = u8::from_str_radix(hex, 16)
                {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn match_flag(
    tok: &str,
    tokens: &[String],
    idx: &mut usize,
    short: &str,
    long: &str,
) -> Option<String> {
    if tok == short || tok == long {
        if *idx + 1 < tokens.len() {
            *idx += 1;
            return Some(tokens[*idx].clone());
        }
        return None;
    }
    if let Some(rest) = tok.strip_prefix(&format!("{long}=")) {
        return Some(rest.to_owned());
    }
    if tok.starts_with(short) && tok.len() > short.len() {
        return Some(tok[short.len()..].to_owned());
    }
    None
}

fn match_exact_flag(tok: &str, tokens: &[String], idx: &mut usize, long: &str) -> Option<String> {
    if tok == long {
        if *idx + 1 < tokens.len() {
            *idx += 1;
            return Some(tokens[*idx].clone());
        }
        return None;
    }
    tok.strip_prefix(&format!("{long}=")).map(str::to_owned)
}

fn match_data_flag(tok: &str, tokens: &[String], idx: &mut usize) -> Option<String> {
    for flag in [
        "-d",
        "--data",
        "--data-raw",
        "--data-binary",
        "--data-ascii",
    ] {
        if tok == flag {
            if *idx + 1 < tokens.len() {
                *idx += 1;
                return Some(tokens[*idx].clone());
            }
            return None;
        }
        if let Some(rest) = tok.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_owned());
        }
    }
    if tok.starts_with("-d") && tok.len() > 2 {
        return Some(tok[2..].to_owned());
    }
    None
}

fn takes_separate_value(tok: &str) -> bool {
    matches!(
        tok,
        "-b" | "--cookie"
            | "-c"
            | "--cookie-jar"
            | "-e"
            | "--referer"
            | "-o"
            | "--output"
            | "-x"
            | "--proxy"
            | "-m"
            | "--max-time"
            | "--connect-timeout"
            | "--resolve"
    )
}

fn tokenize_shell(input: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\\' => {
                if matches!(chars.peek(), Some('\r')) {
                    chars.next();
                }
                if matches!(chars.peek(), Some('\n')) {
                    chars.next();
                    continue;
                }
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                    in_token = true;
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => bail!("Unterminated single quote in cURL command"),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('\r') => {
                                if matches!(chars.peek(), Some('\n')) {
                                    chars.next();
                                }
                            }
                            Some('\n') => {}
                            Some(esc) if matches!(esc, '"' | '\\' | '$' | '`') => {
                                current.push(esc);
                            }
                            Some(esc) => {
                                current.push('\\');
                                current.push(esc);
                            }
                            None => bail!("Unterminated double quote in cURL command"),
                        },
                        Some(c) => current.push(c),
                        None => bail!("Unterminated double quote in cURL command"),
                    }
                }
            }
            '$' if matches!(chars.peek(), Some('\'')) => {
                chars.next();
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => current.push('\n'),
                            Some('r') => current.push('\r'),
                            Some('t') => current.push('\t'),
                            Some(esc) => current.push(esc),
                            None => bail!("Unterminated ANSI-C quote in cURL command"),
                        },
                        Some(c) => current.push(c),
                        None => bail!("Unterminated ANSI-C quote in cURL command"),
                    }
                }
            }
            c => {
                current.push(c);
                in_token = true;
            }
        }
    }

    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_get_with_query_headers_and_bearer() {
        let cmd = r#"curl -X GET "https://api.example.com/v1/users?role=admin&page=2" \
          -H "Accept: application/json" \
          -H "Authorization: Bearer tok_123""#;
        let req = parse_curl(cmd).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.url, "https://api.example.com/v1/users");
        assert_eq!(req.params.len(), 2);
        assert_eq!(req.params[0].key, "role");
        assert_eq!(req.params[0].value, "admin");
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.headers[0].key, "Accept");
        assert_eq!(
            req.auth,
            Auth::Bearer {
                token: "tok_123".into()
            }
        );
    }

    #[test]
    fn parse_post_json_and_basic_auth() {
        let cmd = "curl -u alice:secret -H 'Content-Type: application/json' -d '{\"ok\":true}' https://example.com/items";
        let req = parse_curl(cmd).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(
            req.auth,
            Auth::Basic {
                username: "alice".into(),
                password: "secret".into()
            }
        );
        assert_eq!(req.body, HttpBody::Json("{\"ok\":true}".into()));
    }

    #[test]
    fn to_curl_roundtrip_with_variables() {
        let mut req = ApiRequest::new("POST", "Create", "{{base}}/users");
        req.params.push(KeyValue::new("dry", "true"));
        req.headers.push(KeyValue::new("X-Env", "{{env}}"));
        req.body = HttpBody::Json(r#"{"name":"Ada"}"#.into());
        req.auth = Auth::Bearer {
            token: "{{tok}}".into(),
        };
        let vars = BTreeMap::from([
            ("base".into(), "https://api.example.com".into()),
            ("env".into(), "staging".into()),
            ("tok".into(), "secret".into()),
        ]);
        let generated = to_curl(&req, &vars);
        assert!(generated.contains("-X POST"));
        assert!(generated.contains("https://api.example.com/users?dry=true"));
        assert!(generated.contains("X-Env: staging"));
        assert!(generated.contains("Authorization: Bearer secret"));

        let reparsed = parse_curl(&generated).unwrap();
        assert_eq!(reparsed.method, "POST");
        assert_eq!(reparsed.url, "https://api.example.com/users");
        assert_eq!(reparsed.params, vec![KeyValue::new("dry", "true")]);
        assert_eq!(
            reparsed.auth,
            Auth::Bearer {
                token: "secret".into()
            }
        );
    }
}
