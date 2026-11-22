//! Shared security validation, credential classification, sanitization, and hashing helpers.

use crate::model::{ApiRequest, Auth, KeyValue};
use anyhow::{Result, bail};
use std::collections::HashSet;

const MAX_HTTP_METHOD_LEN: usize = 32;

/// Returns `true` if `method` is a valid, length-bounded RFC 9110 HTTP token.
pub fn is_valid_http_method(method: &str) -> bool {
    let bytes = method.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_HTTP_METHOD_LEN
        && bytes.iter().all(|b| is_http_tchar(*b))
}

/// Returns `true` if `name` is a valid HTTP header field name (RFC 9110 `token`).
pub fn is_valid_header_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty() && bytes.iter().all(|b| is_http_tchar(*b))
}

#[inline]
fn is_http_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Validate that a stored/template or resolved URL uses `http`/`https` (or starts
/// with a `{{variable}}` placeholder) and contains no control characters.
pub fn validate_template_or_http_url(raw: &str) -> Result<()> {
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
        bail!("URL contains disallowed control characters");
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Request URL is empty");
    }
    if trimmed.starts_with("{{") {
        return Ok(());
    }
    if let Some((scheme, _)) = trimmed.split_once("://") {
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            bail!("Only HTTP and HTTPS URLs are supported");
        }
        return Ok(());
    }
    // Reject scheme-without-slashes forms such as `file:/etc/passwd`, `javascript:...`, `data:...`.
    if has_non_port_scheme(trimmed) {
        bail!("Only HTTP and HTTPS URLs are supported");
    }
    Ok(())
}

pub(crate) fn has_non_port_scheme(trimmed: &str) -> bool {
    let before_delim = trimmed.split(['/', '?', '#']).next().unwrap_or(trimmed);
    let Some((prefix, rest)) = before_delim.split_once(':') else {
        return false;
    };
    !prefix.is_empty()
        && !prefix.starts_with('[')
        && prefix.bytes().enumerate().all(|(idx, b)| {
            if idx == 0 {
                b.is_ascii_alphabetic()
            } else {
                b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
            }
        })
        && (rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()))
}

pub fn is_sensitive_header_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    matches!(
        lower.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "private-token"
            | "x-api-key"
            | "api-key"
            | "x-auth-token"
            | "x-access-token"
            | "x-csrf-token"
            | "x-xsrf-token"
            | "x-session-token"
            | "x-functions-key"
            | "x-amz-security-token"
    ) || lower.contains("api-key")
        || lower.contains("apikey")
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("credential")
        || lower.contains("auth")
}

pub fn is_sensitive_param_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    lower == "key"
        || lower.ends_with("_key")
        || lower.ends_with("-key")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("api-key")
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("pwd")
        || lower.contains("auth")
        || lower.contains("credential")
        || lower.contains("signature")
        || lower.contains("sig")
        || lower.contains("session")
        || lower.contains("private_key")
}

pub fn is_sensitive_var_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    is_sensitive_param_name(&lower)
        || matches!(
            lower.as_str(),
            "jwt" | "pat" | "bearer" | "cookie" | "cookies"
        )
}

fn is_api_key_header_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "x-api-key"
            | "api-key"
            | "x-auth-token"
            | "x-access-token"
            | "private-token"
            | "x-functions-key"
            | "x-gitlab-token"
            | "x-github-token"
    ) || lower.contains("api-key")
        || lower.contains("apikey")
}

fn is_api_key_param_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    matches!(lower.as_str(), "api_key" | "apikey" | "api-key" | "key")
        || lower.ends_with("api_key")
        || lower.ends_with("apikey")
        || lower.ends_with("api-key")
}

/// Try mapping an HTTP header to session-only [`Auth`].
pub fn try_extract_auth_from_header(key: &str, value: &str) -> Option<Auth> {
    let key_trimmed = key.trim();
    let val_trimmed = value.trim();
    if key_trimmed.is_empty() || val_trimmed.is_empty() {
        return None;
    }
    if key_trimmed.eq_ignore_ascii_case("authorization") {
        if val_trimmed.len() >= 7 && val_trimmed[..7].eq_ignore_ascii_case("bearer ") {
            let token = val_trimmed[7..].trim();
            if !token.is_empty() {
                return Some(Auth::Bearer {
                    token: token.to_owned(),
                });
            }
        }
        if val_trimmed.len() >= 6
            && val_trimmed[..6].eq_ignore_ascii_case("basic ")
            && let Some(decoded) = decode_base64_utf8(val_trimmed[6..].trim())
            && !decoded.chars().any(char::is_control)
            && let Some((username, password)) = decoded.split_once(':')
        {
            return Some(Auth::Basic {
                username: username.to_owned(),
                password: password.to_owned(),
            });
        }
        return None;
    }
    if is_api_key_header_name(key_trimmed) {
        return Some(Auth::ApiKeyHeader {
            name: key_trimmed.to_owned(),
            value: val_trimmed.to_owned(),
        });
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveFieldFinding {
    pub request_name: String,
    pub location: String,
    pub field_name: String,
}

impl std::fmt::Display for SensitiveFieldFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} '{}')",
            self.request_name, self.location, self.field_name
        )
    }
}

pub fn collect_sensitive_request_findings(
    request: &ApiRequest,
    out: &mut Vec<SensitiveFieldFinding>,
) {
    for header in &request.headers {
        if is_sensitive_header_name(&header.key) && !header.value.trim().is_empty() {
            out.push(SensitiveFieldFinding {
                request_name: request.name.clone(),
                location: "header".to_owned(),
                field_name: header.key.trim().to_owned(),
            });
        }
    }
    for param in &request.params {
        if is_sensitive_param_name(&param.key) && !param.value.trim().is_empty() {
            out.push(SensitiveFieldFinding {
                request_name: request.name.clone(),
                location: "query param".to_owned(),
                field_name: param.key.trim().to_owned(),
            });
        }
    }
    if let Some((_, username, _)) = strip_url_userinfo(&request.url)
        && !username.is_empty()
    {
        out.push(SensitiveFieldFinding {
            request_name: request.name.clone(),
            location: "URL userinfo".to_owned(),
            field_name: username,
        });
    }
    if let Some((_, query, _)) = split_url_parts(&request.url) {
        for part in query.split('&').filter(|s| !s.is_empty()) {
            let (raw_k, raw_v) = part.split_once('=').unwrap_or((part, ""));
            let key = percent_decode(raw_k);
            let value = percent_decode(raw_v);
            if is_sensitive_param_name(&key) && !value.trim().is_empty() {
                out.push(SensitiveFieldFinding {
                    request_name: request.name.clone(),
                    location: "URL query".to_owned(),
                    field_name: key.trim().to_owned(),
                });
            }
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SanitizeSummary {
    pub moved_to_session_auth: usize,
    pub stripped_fields: usize,
}

/// Move recognized credentials (Authorization Bearer/Basic, API keys, URL userinfo)
/// into session-only `request.auth` (when `request.auth == Auth::None`) and strip
/// all remaining sensitive headers/query params so they are not persisted in plaintext.
pub fn sanitize_request_credentials(request: &mut ApiRequest) -> SanitizeSummary {
    let mut summary = SanitizeSummary::default();

    // 1. Strip URL userinfo, optionally promoting to Auth::Basic.
    if let Some((cleaned_url, user, pass)) = strip_url_userinfo(&request.url) {
        request.url = cleaned_url;
        if matches!(request.auth, Auth::None) && (!user.is_empty() || !pass.is_empty()) {
            request.auth = Auth::Basic {
                username: user,
                password: pass,
            };
            summary.moved_to_session_auth += 1;
        } else {
            summary.stripped_fields += 1;
        }
    }

    // 2. Headers: promote first recognized auth header when Auth::None, strip all sensitive headers.
    let mut kept_headers = Vec::with_capacity(request.headers.len());
    for header in request.headers.drain(..) {
        if is_sensitive_header_name(&header.key) && !header.value.trim().is_empty() {
            if matches!(request.auth, Auth::None)
                && let Some(extracted) = try_extract_auth_from_header(&header.key, &header.value)
            {
                request.auth = extracted;
                summary.moved_to_session_auth += 1;
            } else {
                summary.stripped_fields += 1;
            }
        } else {
            kept_headers.push(header);
        }
    }
    request.headers = kept_headers;

    // 3. Params: promote recognized API key param when Auth::None, strip all sensitive params.
    let mut kept_params = Vec::with_capacity(request.params.len());
    for param in request.params.drain(..) {
        if is_sensitive_param_name(&param.key) && !param.value.trim().is_empty() {
            if matches!(request.auth, Auth::None) && is_api_key_param_name(&param.key) {
                request.auth = Auth::ApiKeyQuery {
                    name: param.key.trim().to_owned(),
                    value: param.value,
                };
                summary.moved_to_session_auth += 1;
            } else {
                summary.stripped_fields += 1;
            }
        } else {
            kept_params.push(param);
        }
    }
    request.params = kept_params;

    // 4. Embedded URL query params.
    if let Some((base, query, frag)) = split_url_parts(&request.url)
        && !query.is_empty()
    {
        let mut kept_pairs = Vec::new();
        let mut changed = false;
        for part in query.split('&').filter(|s| !s.is_empty()) {
            let (raw_k, raw_v) = part.split_once('=').unwrap_or((part, ""));
            let key = percent_decode(raw_k);
            let value = percent_decode(raw_v);
            if is_sensitive_param_name(&key) && !value.trim().is_empty() {
                changed = true;
                if matches!(request.auth, Auth::None) && is_api_key_param_name(&key) {
                    request.auth = Auth::ApiKeyQuery {
                        name: key.trim().to_owned(),
                        value,
                    };
                    summary.moved_to_session_auth += 1;
                } else {
                    summary.stripped_fields += 1;
                }
            } else {
                kept_pairs.push(part);
            }
        }
        if changed {
            let mut rebuilt = base.to_owned();
            if !kept_pairs.is_empty() {
                rebuilt.push('?');
                rebuilt.push_str(&kept_pairs.join("&"));
            }
            if let Some(fragment) = frag {
                rebuilt.push('#');
                rebuilt.push_str(fragment);
            }
            request.url = rebuilt;
        }
    }

    summary
}

/// Replace sensitive query values and userinfo in a URL for safe display/error reporting.
pub fn redact_url_secrets(raw_url: &str) -> String {
    let mut url = strip_url_userinfo(raw_url)
        .map(|(cleaned, _, _)| {
            // Re-insert redacted marker so user knows auth existed without leaking it.
            if let Some((scheme, rest)) = cleaned.split_once("://") {
                format!("{scheme}://<REDACTED>@{rest}")
            } else {
                cleaned
            }
        })
        .unwrap_or_else(|| raw_url.to_owned());

    if let Some((base, query, frag)) = split_url_parts(&url)
        && !query.is_empty()
    {
        let mut modified = false;
        let parts: Vec<String> = query
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|part| {
                let (raw_k, raw_v) = part.split_once('=').unwrap_or((part, ""));
                let key = percent_decode(raw_k);
                if is_sensitive_param_name(&key) && !percent_decode(raw_v).trim().is_empty() {
                    modified = true;
                    format!("{raw_k}=<REDACTED>")
                } else {
                    part.to_owned()
                }
            })
            .collect();
        if modified {
            let mut rebuilt = format!("{base}?{}", parts.join("&"));
            if let Some(fragment) = frag {
                rebuilt.push('#');
                rebuilt.push_str(fragment);
            }
            url = rebuilt;
        }
    }
    url
}

/// Escape all control characters in single-line terminal metadata (project names, warnings).
pub fn sanitize_terminal_line(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_control() {
            escape_control_char(ch, &mut out);
        } else {
            out.push(ch);
        }
    }
    out
}

/// Escape terminal control sequences in response bodies while preserving `\n` and `\t`
/// (`\r\n` is normalized to `\n`; standalone `\r`, ESC, BS, C1, etc. are escaped).
pub fn sanitize_terminal_body(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\n' | '\t' => out.push(ch),
            '\r' => {
                if matches!(chars.peek(), Some('\n')) {
                    chars.next();
                    out.push('\n');
                } else {
                    out.push_str("\\r");
                }
            }
            c if c.is_control() => escape_control_char(c, &mut out),
            c => out.push(c),
        }
    }
    out
}

fn escape_control_char(ch: char, out: &mut String) {
    match ch {
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        c => {
            use std::fmt::Write as _;
            let _ = write!(out, "\\u{{{:x}}}", c as u32);
        }
    }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn percent_decode(input: &str) -> String {
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
                if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
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

/// Returns `true` when `raw` has a URL query component (`?` before any `#fragment`).
pub fn has_url_query(raw: &str) -> bool {
    split_url_parts(raw).is_some()
}

/// Parse `application/x-www-form-urlencoded` pairs with percent-decoding (`+` and `%XX`).
pub fn parse_urlencoded_pairs(query: &str) -> Vec<KeyValue> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            KeyValue::new(percent_decode(k), percent_decode(v))
        })
        .collect()
}

/// Extract and percent-decode query parameters from `raw_url`, returning the cleaned URL
/// with any `#fragment` preserved.
pub fn split_url_query(raw_url: &str) -> Option<(String, Vec<KeyValue>)> {
    let (base, query, frag) = split_url_parts(raw_url)?;
    let cleaned = match frag {
        Some(f) => format!("{base}#{f}"),
        None => base.to_owned(),
    };
    Some((cleaned, parse_urlencoded_pairs(query)))
}

pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn decode_base64_utf8(input: &str) -> Option<String> {
    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.is_empty() || !clean.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    for chunk in clean.as_chunks::<4>().0 {
        let mut vals = [0u8; 4];
        let mut pad = 0;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                if i < 2 || (i == 2 && chunk[3] != b'=') {
                    return None;
                }
                pad += 1;
                vals[i] = 0;
            } else if pad > 0 {
                return None;
            } else {
                vals[i] = match b {
                    b'A'..=b'Z' => b - b'A',
                    b'a'..=b'z' => b - b'a' + 26,
                    b'0'..=b'9' => b - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => return None,
                };
            }
        }
        let n = ((vals[0] as u32) << 18)
            | ((vals[1] as u32) << 12)
            | ((vals[2] as u32) << 6)
            | (vals[3] as u32);
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    String::from_utf8(out).ok()
}

fn split_url_parts(raw: &str) -> Option<(&str, &str, Option<&str>)> {
    let (before_frag, frag) = raw
        .split_once('#')
        .map_or((raw, None), |(a, b)| (a, Some(b)));
    let (base, query) = before_frag.split_once('?')?;
    Some((base, query, frag))
}

fn strip_url_userinfo(raw: &str) -> Option<(String, String, String)> {
    let scheme_end = raw.find("://")?;
    let prefix = &raw[..scheme_end + 3];
    let after_scheme = &raw[scheme_end + 3..];
    let auth_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..auth_end];
    let (userinfo, host) = authority.rsplit_once('@')?;
    let (u, p) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let cleaned = format!("{prefix}{host}{}", &after_scheme[auth_end..]);
    Some((cleaned, percent_decode(u), percent_decode(p)))
}

/// Format a concise summary of sensitive findings without repeating duplicates.
pub fn summarize_sensitive_findings(findings: &[SensitiveFieldFinding]) -> String {
    let mut seen = HashSet::new();
    let mut labels = Vec::new();
    for item in findings {
        let label = format!("{} '{}'", item.location, item.field_name);
        if seen.insert(label.clone()) {
            labels.push(label);
            if labels.len() >= 5 {
                break;
            }
        }
    }
    if findings.len() > labels.len() {
        format!(
            "{} (+{} more)",
            labels.join(", "),
            findings.len() - labels.len()
        )
    } else {
        labels.join(", ")
    }
}

/// Standard SHA-256 digest (FIPS 180-4) with zero external crate dependencies.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (slot, word_bytes) in w[..16].iter_mut().zip(block.as_chunks::<4>().0) {
            *slot = u32::from_be_bytes(*word_bytes);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::KeyValue;

    #[test]
    fn http_method_and_header_validation() {
        assert!(is_valid_http_method("GET"));
        assert!(is_valid_http_method("MKCOL"));
        assert!(!is_valid_http_method(""));
        assert!(!is_valid_http_method("GET;id"));
        assert!(!is_valid_http_method("GET$(id)"));
        assert!(!is_valid_http_method("GET\nPOST"));
        assert!(!is_valid_http_method(&"A".repeat(40)));

        assert!(is_valid_header_name("X-API-Key"));
        assert!(!is_valid_header_name("@/etc/passwd"));
        assert!(!is_valid_header_name("Bad:Header"));
        assert!(!is_valid_header_name("Bad\nHeader"));
    }

    #[test]
    fn url_scheme_validation_blocks_non_http_and_allows_templates() {
        assert!(validate_template_or_http_url("https://api.example.com/v1").is_ok());
        assert!(validate_template_or_http_url("http://localhost:8080/test").is_ok());
        assert!(validate_template_or_http_url("localhost:8080/test").is_ok());
        assert!(validate_template_or_http_url("{{base}}/users").is_ok());
        assert!(validate_template_or_http_url("file:///etc/passwd").is_err());
        assert!(validate_template_or_http_url("javascript:alert(1)").is_err());
        assert!(validate_template_or_http_url("gopher://127.0.0.1:6379").is_err());
        assert!(validate_template_or_http_url("https://evil.com\n").is_err());
    }

    #[test]
    fn sanitize_moves_recognized_and_strips_remaining_secrets() {
        let mut req = ApiRequest::new(
            "GET",
            "Secret",
            "https://alice:pw@api.example.com/v1?page=1&api_key=k1&token=t1#sec",
        );
        req.headers
            .push(KeyValue::new("Accept", "application/json"));
        req.headers
            .push(KeyValue::new("Authorization", "Bearer tok_123"));
        req.headers.push(KeyValue::new("Cookie", "session=xyz"));
        req.params.push(KeyValue::new("secret", "s1"));
        req.params.push(KeyValue::new("limit", "10"));

        let mut findings = Vec::new();
        collect_sensitive_request_findings(&req, &mut findings);
        assert_eq!(findings.len(), 6);

        let summary = sanitize_request_credentials(&mut req);
        assert_eq!(summary.moved_to_session_auth, 1);
        assert_eq!(summary.stripped_fields, 5);
        assert_eq!(
            req.auth,
            Auth::Basic {
                username: "alice".into(),
                password: "pw".into()
            }
        );
        assert_eq!(req.url, "https://api.example.com/v1?page=1#sec");
        assert_eq!(
            req.headers,
            vec![KeyValue::new("Accept", "application/json")]
        );
        assert_eq!(req.params, vec![KeyValue::new("limit", "10")]);
    }

    #[test]
    fn sha256_matches_standard_vector() {
        let digest = sha256(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn terminal_sanitizers_escape_control_sequences() {
        assert_eq!(
            sanitize_terminal_line("Bad\x1b[31mName\n"),
            "Bad\\u{1b}[31mName\\n"
        );
        assert_eq!(
            sanitize_terminal_body("line1\r\nline2\rspoof\x1b[2J\t!"),
            "line1\nline2\\rspoof\\u{1b}[2J\t!"
        );
    }

    #[test]
    fn url_query_parsing_decodes_and_preserves_fragments_and_edge_cases() {
        assert_eq!(percent_decode("hello+world%20%2B%2b"), "hello world ++");
        assert_eq!(percent_decode("%26%3D%2F"), "&=/");
        assert_eq!(percent_decode("caf%C3%A9%20%F0%9F%9A%80"), "café 🚀");
        assert_eq!(percent_decode("{{var}}"), "{{var}}");
        assert_eq!(percent_decode("%ZZ%2%"), "%ZZ%2%");
        assert_eq!(percent_decode("%+1"), "% 1");

        assert!(has_url_query("https://example.com/p?a=1"));
        assert!(has_url_query("https://example.com/p?#frag"));
        assert!(has_url_query("{{base}}/x?a=1#frag"));
        assert!(!has_url_query("https://example.com/p#sec?not=query"));
        assert!(!has_url_query("https://example.com/p"));

        let url = "https://api.example.com/search?q=hello%20world&q=second&expr=a%2Bb&raw=x%26y%3Dz&uni=caf%C3%A9&tpl={{var}}&b64=abc==&empty=&flag#results";
        let (cleaned, pairs) = split_url_query(url).unwrap();
        assert_eq!(cleaned, "https://api.example.com/search#results");
        assert_eq!(
            pairs,
            vec![
                KeyValue::new("q", "hello world"),
                KeyValue::new("q", "second"),
                KeyValue::new("expr", "a+b"),
                KeyValue::new("raw", "x&y=z"),
                KeyValue::new("uni", "café"),
                KeyValue::new("tpl", "{{var}}"),
                KeyValue::new("b64", "abc=="),
                KeyValue::new("empty", ""),
                KeyValue::new("flag", ""),
            ]
        );

        let (empty_q_clean, empty_q_pairs) =
            split_url_query("https://api.example.com/search?#top").unwrap();
        assert_eq!(empty_q_clean, "https://api.example.com/search#top");
        assert!(empty_q_pairs.is_empty());
        assert!(split_url_query("https://api.example.com/page#sec?not=query").is_none());
    }
}
