//! Best-effort cURL command parser and generator with shell/argument injection guards
//! and optional credential redaction.

use crate::model::{ApiRequest, Auth, HttpBody, KeyValue};
use crate::security::{
    has_non_port_scheme, is_sensitive_header_name, is_sensitive_param_name, is_sensitive_var_name,
    is_valid_header_name, is_valid_http_method, parse_urlencoded_pairs, percent_encode,
    redact_json_secrets, redact_url_secrets, split_url_query, try_extract_auth_from_header,
    validate_template_or_http_url,
};
use crate::template::{self, MAX_INTERPOLATED_BYTES};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, HashSet};

pub const MAX_CURL_IMPORT_BYTES: usize = 1024 * 1024;

/// Returns `true` when `input` looks like a pasted `curl ...` or `curl.exe ...` command.
pub fn looks_like_curl_command(input: &str) -> bool {
    let trimmed = input.trim_start();
    let Some(first) = trimmed.split_whitespace().next() else {
        return false;
    };
    let rest = &trimmed[first.len()..];
    !rest.trim_start().is_empty()
        && (first.eq_ignore_ascii_case("curl") || first.eq_ignore_ascii_case("curl.exe"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurlExport {
    pub command: String,
    pub had_secrets: bool,
    pub had_variables: bool,
}

#[derive(Debug, Clone)]
pub struct CurlParseReport {
    pub request: ApiRequest,
    pub unsupported_flags: Vec<String>,
}

/// Parse a POSIX-style `curl ...` command string into an [`ApiRequest`].
pub fn parse_curl(input: &str) -> Result<ApiRequest> {
    Ok(parse_curl_report(input)?.request)
}

/// Parse a POSIX-style `curl ...` command string and report any unmapped flags
/// whose behavior cannot be represented in [`ApiRequest`].
pub fn parse_curl_report(input: &str) -> Result<CurlParseReport> {
    if input.len() > MAX_CURL_IMPORT_BYTES {
        bail!("cURL command exceeds maximum size ({MAX_CURL_IMPORT_BYTES} bytes)");
    }
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
    let mut data_entries: Vec<DataEntry> = Vec::new();
    let mut saw_data_flag = false;
    let mut auth = Auth::None;
    let mut unsupported_flags: Vec<String> = Vec::new();

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
            let candidate = rest.trim().to_uppercase();
            if !is_valid_http_method(&candidate) {
                bail!("Invalid HTTP method in cURL command: {rest}");
            }
            explicit_method = Some(candidate);
        } else if let Some(rest) = match_flag(tok, &tokens, &mut idx, "-H", "--header") {
            if rest.bytes().any(|b| matches!(b, b'\r' | b'\n' | b'\0')) {
                bail!("Header contains disallowed control characters");
            }
            if let Some((k, v)) = rest.split_once(':') {
                let key = k.trim();
                let value = v.trim();
                if matches!(auth, Auth::None)
                    && key.eq_ignore_ascii_case("authorization")
                    && let Some(extracted) = try_extract_auth_from_header(key, value)
                {
                    auth = extracted;
                } else if !key.is_empty() {
                    headers.push(KeyValue::new(key, value));
                }
            }
        } else if let Some(rest) = match_flag(tok, &tokens, &mut idx, "-A", "--user-agent") {
            if !rest.bytes().any(|b| matches!(b, b'\r' | b'\n' | b'\0')) {
                headers.push(KeyValue::new("User-Agent", rest));
            }
        } else if let Some(rest) = match_flag(tok, &tokens, &mut idx, "-u", "--user") {
            let (user, pass) = rest.split_once(':').unwrap_or((&rest, ""));
            auth = Auth::Basic {
                username: user.to_owned(),
                password: pass.to_owned(),
            };
        } else if let Some(rest) = match_exact_flag(tok, &tokens, &mut idx, "--oauth2-bearer") {
            auth = Auth::Bearer {
                token: rest.trim().to_owned(),
            };
        } else if let Some((kind, val)) = match_data_flag(tok, &tokens, &mut idx)? {
            saw_data_flag = true;
            match kind {
                DataFlagKind::Data(label) => {
                    if val.starts_with('@') {
                        push_unique_flag(&mut unsupported_flags, format!("{label} @file"));
                    } else {
                        data_entries.push(DataEntry::Raw(val));
                    }
                }
                DataFlagKind::DataBinary => {
                    if val.starts_with('@') {
                        push_unique_flag(&mut unsupported_flags, "--data-binary @file".to_owned());
                    } else {
                        data_entries.push(DataEntry::Raw(val));
                    }
                }
                DataFlagKind::DataRaw => {
                    data_entries.push(DataEntry::Raw(val));
                }
                DataFlagKind::DataUrlEncode => {
                    let eq_pos = val.find('=');
                    let at_pos = val.find('@');
                    if at_pos.is_some_and(|at| eq_pos.is_none_or(|eq| at < eq)) {
                        push_unique_flag(
                            &mut unsupported_flags,
                            "--data-urlencode @file".to_owned(),
                        );
                    } else if let Some(eq) = eq_pos {
                        let name = &val[..eq];
                        let content = &val[eq + 1..];
                        if name.is_empty() {
                            data_entries.push(DataEntry::UrlEncodedRaw(content.to_owned()));
                        } else {
                            data_entries.push(DataEntry::UrlEncoded(KeyValue::new(name, content)));
                        }
                    } else {
                        data_entries.push(DataEntry::UrlEncoded(KeyValue::new(val, "")));
                    }
                }
            }
        } else if let Some(rest) = match_exact_flag(tok, &tokens, &mut idx, "--url") {
            url = Some(rest);
        } else if tok == "-G" || tok == "--get" {
            force_get = true;
        } else if tok == "-I" || tok == "--head" {
            explicit_method = Some("HEAD".to_owned());
        } else if tok == "-g" || tok == "--globoff" {
            // Fastman never interprets cURL URL globbing syntax (`{a,b}`, `[1-9]`).
        } else if tok.starts_with('-') {
            if let Some(flag_name) = sanitize_unsupported_flag(tok) {
                push_unique_flag(&mut unsupported_flags, flag_name.to_owned());
            }
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
    validate_template_or_http_url(&raw_url)?;

    let (base_url, mut params) =
        split_url_query(&raw_url).unwrap_or_else(|| (raw_url.clone(), Vec::new()));
    let method = if force_get {
        "GET".to_owned()
    } else {
        explicit_method.unwrap_or_else(|| {
            if saw_data_flag {
                "POST".to_owned()
            } else {
                "GET".to_owned()
            }
        })
    };
    if !is_valid_http_method(&method) {
        bail!("Invalid HTTP method in cURL command: {method}");
    }

    let mut body = HttpBody::None;
    if !data_entries.is_empty() {
        if force_get {
            for entry in data_entries {
                match entry {
                    DataEntry::Raw(s) => params.extend(parse_urlencoded_pairs(&s)),
                    DataEntry::UrlEncoded(kv) => params.push(kv),
                    DataEntry::UrlEncodedRaw(s) => params.push(KeyValue::new(s, "")),
                }
            }
        } else {
            let explicit_ct = headers
                .iter()
                .rev()
                .find(|h| h.key.eq_ignore_ascii_case("content-type"))
                .map(|h| h.value.to_ascii_lowercase());
            let is_form = explicit_ct
                .as_deref()
                .is_some_and(|ct| ct.contains("application/x-www-form-urlencoded"));
            let is_json_header = explicit_ct.as_deref().is_some_and(|ct| ct.contains("json"));
            let has_urlencode = data_entries
                .iter()
                .any(|e| matches!(e, DataEntry::UrlEncoded(_) | DataEntry::UrlEncodedRaw(_)));
            let has_unnamed_urlencode = data_entries
                .iter()
                .any(|e| matches!(e, DataEntry::UrlEncodedRaw(_)));

            if (has_urlencode || is_form) && !has_unnamed_urlencode {
                let mut pairs = Vec::new();
                for entry in data_entries {
                    match entry {
                        DataEntry::Raw(s) => pairs.extend(parse_urlencoded_pairs(&s)),
                        DataEntry::UrlEncoded(kv) => pairs.push(kv),
                        DataEntry::UrlEncodedRaw(_) => unreachable!(),
                    }
                }
                body = HttpBody::FormUrlEncoded(pairs);
            } else {
                let joined = data_entries
                    .into_iter()
                    .map(|entry| match entry {
                        DataEntry::Raw(s) => s,
                        DataEntry::UrlEncoded(kv) => {
                            if kv.value.is_empty() {
                                percent_encode(&kv.key)
                            } else {
                                format!("{}={}", percent_encode(&kv.key), percent_encode(&kv.value))
                            }
                        }
                        DataEntry::UrlEncodedRaw(s) => percent_encode(&s),
                    })
                    .collect::<Vec<_>>()
                    .join("&");
                if is_json_header
                    || (explicit_ct.is_none()
                        && (joined.trim_start().starts_with('{')
                            || joined.trim_start().starts_with('[')))
                {
                    body = HttpBody::Json(joined);
                } else {
                    body = HttpBody::Text(joined);
                }
            }
        }
    }

    let name = derive_request_name(&base_url);
    let mut request = ApiRequest::new(method, name, base_url);
    request.params = params;
    request.headers = headers;
    request.body = body;
    request.auth = auth;
    Ok(CurlParseReport {
        request,
        unsupported_flags,
    })
}

/// Format an [`ApiRequest`] as a POSIX shell `curl` command with all variables resolved.
pub fn to_curl(request: &ApiRequest, variables: &BTreeMap<String, String>) -> Result<String> {
    Ok(to_curl_export(request, variables, false)?.command)
}

struct CurlResolver<'a> {
    variables: &'a BTreeMap<String, String>,
    secret_names: &'a HashSet<String>,
    redact_secrets: bool,
    had_variables: bool,
    had_secrets: bool,
}

impl<'a> CurlResolver<'a> {
    fn resolve(&mut self, text: &str) -> Result<String> {
        expand_template_for_curl(
            text,
            self.variables,
            self.secret_names,
            self.redact_secrets,
            &mut self.had_variables,
            &mut self.had_secrets,
        )
    }
}

/// Format an [`ApiRequest`] as a POSIX shell `curl` command, optionally redacting credentials
/// and leaving `{{variables}}` unexpanded.
pub fn to_curl_export(
    request: &ApiRequest,
    variables: &BTreeMap<String, String>,
    redact_secrets: bool,
) -> Result<CurlExport> {
    let empty_secrets = HashSet::new();
    to_curl_export_with_secrets(request, variables, &empty_secrets, redact_secrets)
}

/// Format an [`ApiRequest`] as a POSIX shell `curl` command while recognizing both heuristic
/// sensitive names and explicitly typed `secret_names` variables.
pub fn to_curl_export_with_secrets(
    request: &ApiRequest,
    variables: &BTreeMap<String, String>,
    secret_names: &HashSet<String>,
    redact_secrets: bool,
) -> Result<CurlExport> {
    let method = request.method.trim().to_uppercase();
    if !is_valid_http_method(&method) {
        bail!("Invalid HTTP method: {}", request.method);
    }

    let mut resolver = CurlResolver {
        variables,
        secret_names,
        redact_secrets,
        had_variables: false,
        had_secrets: false,
    };

    let mut full_url = resolver.resolve(&request.url)?;
    validate_template_or_http_url(&full_url)?;

    let redacted = redact_url_secrets(&full_url);
    if redacted != full_url {
        resolver.had_secrets = true;
        if redact_secrets {
            full_url = redacted;
        }
    }

    let mut query_pairs: Vec<(String, String)> = Vec::new();
    for p in &request.params {
        if !p.enabled || p.key.trim().is_empty() {
            continue;
        }
        let k = resolver.resolve(&p.key)?;
        let v = resolver.resolve(&p.value)?;
        if is_sensitive_param_name(&k) && !v.trim().is_empty() {
            resolver.had_secrets = true;
            if redact_secrets {
                query_pairs.push((k, "<REDACTED>".to_owned()));
                continue;
            }
        }
        query_pairs.push((k, v));
    }

    if let Auth::ApiKeyQuery { name, value } = &request.auth {
        let resolved_name = resolver.resolve(name)?;
        let key = resolved_name.trim();
        if !key.is_empty() {
            resolver.had_secrets = true;
            let val = if redact_secrets {
                "<REDACTED>".to_owned()
            } else {
                resolver.resolve(value)?
            };
            query_pairs.push((key.to_owned(), val));
        }
    }

    if !query_pairs.is_empty() {
        let (base_and_query, frag) = full_url
            .split_once('#')
            .map_or((full_url.as_str(), None), |(b, f)| (b, Some(f)));
        let sep = if !base_and_query.contains('?') {
            "?"
        } else if base_and_query.ends_with(['?', '&']) {
            ""
        } else {
            "&"
        };
        let encoded: Vec<String> = query_pairs
            .iter()
            .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
            .collect();
        full_url = match frag {
            Some(f) => format!("{base_and_query}{sep}{}#{f}", encoded.join("&")),
            None => format!("{base_and_query}{sep}{}", encoded.join("&")),
        };
    }

    let mut parts = Vec::new();
    parts.push("curl".to_owned());
    if full_url.contains(['{', '}', '[', ']']) {
        parts.push("-g".to_owned());
    }
    if method != "GET" {
        parts.push(format!("-X {}", shell_quote(&method)));
    }
    parts.push(format!("--url {}", shell_quote(&full_url)));

    let mut has_content_type = false;
    for header in &request.headers {
        if !header.enabled || header.key.trim().is_empty() {
            continue;
        }
        let key_raw = resolver.resolve(&header.key)?;
        let key = key_raw.trim();
        if key.is_empty() {
            continue;
        }
        if key.starts_with('@')
            || key.contains(':')
            || key.bytes().any(|b| matches!(b, b'\r' | b'\n' | b'\0'))
            || (!key.contains("{{") && !is_valid_header_name(key))
        {
            bail!("Invalid header name: {key}");
        }
        let value = resolver.resolve(&header.value)?;
        if value.bytes().any(|b| matches!(b, b'\r' | b'\n' | b'\0')) {
            bail!("Header value for {key} contains disallowed control characters");
        }
        if key.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        let emitted_value = if is_sensitive_header_name(key) && !value.trim().is_empty() {
            resolver.had_secrets = true;
            if redact_secrets {
                "<REDACTED>".to_owned()
            } else {
                value
            }
        } else {
            value
        };
        parts.push(format!(
            "-H {}",
            shell_quote(&format!("{key}: {emitted_value}"))
        ));
    }

    match &request.auth {
        Auth::None | Auth::ApiKeyQuery { .. } => {}
        Auth::Basic { username, password } => {
            resolver.had_secrets = true;
            let user = resolver.resolve(username)?;
            let pass = if redact_secrets {
                "<REDACTED>".to_owned()
            } else {
                resolver.resolve(password)?
            };
            parts.push(format!("-u {}", shell_quote(&format!("{user}:{pass}"))));
        }
        Auth::Bearer { token } => {
            resolver.had_secrets = true;
            let tok = if redact_secrets {
                "<REDACTED>".to_owned()
            } else {
                resolver.resolve(token)?
            };
            parts.push(format!(
                "-H {}",
                shell_quote(&format!("Authorization: Bearer {tok}"))
            ));
        }
        Auth::ApiKeyHeader { name, value } => {
            let resolved_name = resolver.resolve(name)?;
            let key = resolved_name.trim();
            if !key.is_empty() {
                if key.starts_with('@')
                    || key.contains(':')
                    || (!key.contains("{{") && !is_valid_header_name(key))
                {
                    bail!("Invalid API key header name: {key}");
                }
                resolver.had_secrets = true;
                let val = if redact_secrets {
                    "<REDACTED>".to_owned()
                } else {
                    resolver.resolve(value)?
                };
                parts.push(format!("-H {}", shell_quote(&format!("{key}: {val}"))));
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
                let mut payload = resolver.resolve(text)?;
                if let Some(redacted_json) = redact_json_secrets(&payload) {
                    resolver.had_secrets = true;
                    if redact_secrets {
                        payload = redacted_json;
                    }
                }
                parts.push(format!("--data-raw {}", shell_quote(&payload)));
            }
        }
        HttpBody::Text(text) => {
            if !text.is_empty() {
                parts.push(format!(
                    "--data-raw {}",
                    shell_quote(&resolver.resolve(text)?)
                ));
            }
        }
        HttpBody::FormUrlEncoded(rows) => {
            for row in rows {
                if !row.enabled || row.key.trim().is_empty() {
                    continue;
                }
                let k = resolver.resolve(&row.key)?;
                let mut v = resolver.resolve(&row.value)?;
                if is_sensitive_param_name(&k) && !v.trim().is_empty() {
                    resolver.had_secrets = true;
                    if redact_secrets {
                        v = "<REDACTED>".to_owned();
                    }
                }
                // Avoid curl's `@file` / `name@file` reader in `--data-urlencode`.
                if k.contains('@') || k.contains('=') || k.starts_with('<') {
                    parts.push(format!(
                        "--data-raw {}",
                        shell_quote(&format!("{}={}", percent_encode(&k), percent_encode(&v)))
                    ));
                } else {
                    parts.push(format!(
                        "--data-urlencode {}",
                        shell_quote(&format!("{k}={v}"))
                    ));
                }
            }
        }
    }

    Ok(CurlExport {
        command: parts.join(" \\\n  "),
        had_secrets: resolver.had_secrets,
        had_variables: resolver.had_variables,
    })
}

fn expand_template_for_curl(
    input: &str,
    variables: &BTreeMap<String, String>,
    secret_names: &HashSet<String>,
    redact_secrets: bool,
    had_variables: &mut bool,
    had_secrets: &mut bool,
) -> Result<String> {
    template::expand(
        input,
        |name, raw_token| {
            let Some((name, value)) = name.and_then(|name| Some((name, variables.get(name)?)))
            else {
                return Ok(raw_token);
            };
            *had_variables = true;
            let is_secret = secret_names.contains(name)
                || (is_sensitive_var_name(name) && !value.trim().is_empty());
            if is_secret {
                *had_secrets = true;
            }
            Ok(if redact_secrets { raw_token } else { value })
        },
        || {
            anyhow::anyhow!(
                "Interpolated value exceeds maximum size ({MAX_INTERPOLATED_BYTES} bytes)"
            )
        },
    )
}

/// Quote a string safely for POSIX shells. Uses ANSI-C `$'...'` quoting when control
/// bytes (`< 0x20` or `0x7f`) are present so terminal escape sequences and newlines
/// cannot break out of bracketed paste or execute extra commands.
fn shell_quote(value: &str) -> String {
    if !value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return format!("'{}'", value.replace('\'', "'\\''"));
    }
    let mut out = String::with_capacity(value.len() + 4);
    out.push_str("$'");
    for b in value.bytes() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x00..=0x1f | 0x7f => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\x{b:02x}");
            }
            _ => out.push(b as char),
        }
    }
    out.push('\'');
    out
}

fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains("://")
        || trimmed.starts_with("{{")
        || has_non_port_scheme(trimmed)
    {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
}

fn derive_request_name(url: &str) -> String {
    let url = url.split('#').next().unwrap_or(url);
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

fn match_flag(
    tok: &str,
    tokens: &[String],
    idx: &mut usize,
    short: &str,
    long: &str,
) -> Option<String> {
    if tok == short {
        if *idx + 1 < tokens.len() {
            *idx += 1;
            return Some(tokens[*idx].clone());
        }
        return None;
    }
    if let Some(val) = match_exact_flag(tok, tokens, idx, long) {
        return Some(val);
    }
    if let Some(rest) = tok.strip_prefix(short)
        && !rest.is_empty()
    {
        return Some(rest.to_owned());
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
    tok.strip_prefix(long)
        .and_then(|rest| rest.strip_prefix('='))
        .map(str::to_owned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataFlagKind {
    Data(&'static str),
    DataRaw,
    DataBinary,
    DataUrlEncode,
}

enum DataEntry {
    Raw(String),
    UrlEncoded(KeyValue),
    UrlEncodedRaw(String),
}

fn push_unique_flag(list: &mut Vec<String>, flag: String) {
    if !list.iter().any(|existing| existing == &flag) {
        list.push(flag);
    }
}

fn match_data_flag(
    tok: &str,
    tokens: &[String],
    idx: &mut usize,
) -> Result<Option<(DataFlagKind, String)>> {
    for (flag, kind) in [
        ("--data", DataFlagKind::Data("--data")),
        ("--data-ascii", DataFlagKind::Data("--data-ascii")),
        ("--data-raw", DataFlagKind::DataRaw),
        ("--data-binary", DataFlagKind::DataBinary),
        ("--data-urlencode", DataFlagKind::DataUrlEncode),
    ] {
        if tok == flag {
            if *idx + 1 < tokens.len() {
                *idx += 1;
                return Ok(Some((kind, tokens[*idx].clone())));
            }
            bail!("Missing value for cURL flag {flag}");
        }
        if let Some(val) = tok
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Ok(Some((kind, val.to_owned())));
        }
    }
    if tok == "-d" {
        if *idx + 1 < tokens.len() {
            *idx += 1;
            return Ok(Some((DataFlagKind::Data("-d"), tokens[*idx].clone())));
        }
        bail!("Missing value for cURL flag -d");
    }
    if let Some(rest) = tok.strip_prefix("-d")
        && !rest.is_empty()
    {
        return Ok(Some((DataFlagKind::Data("-d"), rest.to_owned())));
    }
    Ok(None)
}

fn is_short_value_flag(short: &str) -> bool {
    matches!(
        short,
        "-b" | "-c" | "-e" | "-o" | "-x" | "-m" | "-E" | "-U" | "-F" | "-T" | "-K" | "-D" | "-w"
    )
}

fn sanitize_unsupported_flag(tok: &str) -> Option<&str> {
    if tok == "-" {
        return None;
    }
    if !tok.starts_with("--") && tok.len() > 2 && is_short_value_flag(&tok[..2]) {
        return Some(&tok[..2]);
    }
    if let Some((flag, _)) = tok.split_once('=') {
        return (!flag.is_empty()).then_some(flag);
    }
    Some(tok)
}

fn takes_separate_value(tok: &str) -> bool {
    is_short_value_flag(tok)
        || matches!(
            tok,
            "--cookie"
                | "--cookie-jar"
                | "--referer"
                | "--output"
                | "--proxy"
                | "--max-time"
                | "--connect-timeout"
                | "--resolve"
                | "--oauth2-bearer"
                | "--cert"
                | "--cert-type"
                | "--key"
                | "--key-type"
                | "--pass"
                | "--proxy-user"
                | "--form"
                | "--form-string"
                | "--upload-file"
                | "--config"
                | "--cacert"
                | "--capath"
                | "--dump-header"
                | "--write-out"
                | "--interface"
                | "--max-redirs"
                | "--retry"
                | "--retry-delay"
                | "--retry-max-time"
                | "--proto"
                | "--proto-redir"
                | "--connect-to"
                | "--dns-servers"
                | "--unix-socket"
                | "--abstract-unix-socket"
                | "--aws-sigv4"
                | "--pinnedpubkey"
                | "--crlfile"
                | "--etag-save"
                | "--etag-compare"
                | "--json"
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
    fn parse_curl_handles_basic_header_oauth2_bearer_and_unknown_flag_values() {
        // Basic Authorization header should decode into session-only Auth::Basic.
        let basic_cmd = "curl -H 'Authorization: Basic YWxpY2U6c2VjcmV0' https://example.com/me";
        let basic_req = parse_curl(basic_cmd).unwrap();
        assert_eq!(
            basic_req.auth,
            Auth::Basic {
                username: "alice".into(),
                password: "secret".into(),
            }
        );
        assert!(basic_req.headers.is_empty());

        // --oauth2-bearer and -U must not turn their secret args into request URLs.
        let oauth_cmd =
            "curl --oauth2-bearer top_secret -U proxy_u:proxy_p https://api.example.com/data";
        let oauth_req = parse_curl(oauth_cmd).unwrap();
        assert_eq!(oauth_req.url, "https://api.example.com/data");
        assert_eq!(
            oauth_req.auth,
            Auth::Bearer {
                token: "top_secret".into(),
            }
        );
    }

    #[test]
    fn parse_curl_get_moves_data_urlencode_to_params() {
        let cmd = "curl -G --data-urlencode 'q=hello world' https://api.example.com/search";
        let req = parse_curl(cmd).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.body, HttpBody::None);
        assert_eq!(req.params, vec![KeyValue::new("q", "hello world")]);
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
        let generated = to_curl(&req, &vars).unwrap();
        assert!(generated.contains("-X 'POST'"));
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

    #[test]
    fn to_curl_rejects_shell_injection_methods_and_quotes_token_chars() {
        for bad in [
            "GET;id",
            "GET$(id)",
            "GET`id`",
            "GET\nid",
            "GET 'touch /tmp/x'",
            "GET \"x\"",
        ] {
            if bad == "GET`id`" {
                // Backtick is an RFC 9110 tchar, so verify it IS shell-quoted safely!
                let req = ApiRequest::new(bad, "test", "https://example.com");
                let out = to_curl(&req, &BTreeMap::new()).unwrap();
                assert!(out.contains("-X 'GET`ID`'"), "{out}");
                continue;
            }
            let req = ApiRequest::new(bad, "test", "https://example.com");
            assert!(to_curl(&req, &BTreeMap::new()).is_err(), "accepted {bad}");
            assert!(
                parse_curl(&format!("curl -X \"{bad}\" https://example.com")).is_err(),
                "parsed {bad}"
            );
        }
        // Valid RFC 9110 tokens with shell-active chars (& or ') must be safely quoted.
        let req = ApiRequest::new("POST&'ID", "test", "https://example.com");
        let out = to_curl(&req, &BTreeMap::new()).unwrap();
        assert!(out.contains("-X 'POST&'\\''ID'"), "{out}");
    }

    #[test]
    fn to_curl_escapes_control_chars_avoids_at_file_reads_and_preserves_fragments() {
        let mut req = ApiRequest::new("POST", "Body", "https://example.com/p#sec");
        req.params.push(KeyValue::new("a", "1"));
        req.body = HttpBody::Text("@/etc/passwd\x1b[201~\nid".into());
        let out = to_curl(&req, &BTreeMap::new()).unwrap();
        assert!(out.contains("https://example.com/p?a=1#sec"), "{out}");
        assert!(
            out.contains("--data-raw $'@/etc/passwd\\x1b[201~\\nid'"),
            "{out}"
        );
        assert!(!out.contains("\x1b"), "raw ESC leaked into command");

        // Disallowed URL scheme rejected.
        let file_req = ApiRequest::new("GET", "File", "file:///etc/passwd");
        assert!(to_curl(&file_req, &BTreeMap::new()).is_err());
    }

    #[test]
    fn to_curl_redacts_secrets_and_variables_when_requested() {
        let mut req = ApiRequest::new("GET", "Auth", "{{base}}/items?api_key=raw_q");
        req.headers
            .push(KeyValue::new("Authorization", "Token custom_tok"));
        req.headers
            .push(KeyValue::new("Accept", "application/json"));
        req.auth = Auth::Bearer {
            token: "{{tok}}".into(),
        };
        let vars = BTreeMap::from([
            ("base".into(), "https://api.example.com".into()),
            ("tok".into(), "live_secret_value".into()),
        ]);
        let redacted = to_curl_export(&req, &vars, true).unwrap();
        assert!(redacted.had_secrets);
        assert!(redacted.had_variables);
        assert!(!redacted.command.contains("live_secret_value"));
        assert!(!redacted.command.contains("custom_tok"));
        assert!(!redacted.command.contains("raw_q"));
        assert!(redacted.command.contains("{{base}}"));
        assert!(
            redacted
                .command
                .contains("Authorization: Bearer <REDACTED>")
        );
        assert!(redacted.command.contains("Authorization: <REDACTED>"));
    }

    #[test]
    fn looks_like_curl_command_detects_curl_prefixes_only() {
        assert!(looks_like_curl_command("curl https://example.com"));
        assert!(looks_like_curl_command(
            "  CURL.EXE -X POST https://example.com"
        ));
        assert!(looks_like_curl_command("curl\n  --url https://example.com"));
        assert!(!looks_like_curl_command("curl"));
        assert!(!looks_like_curl_command("https://example.com/curl"));
        assert!(!looks_like_curl_command("curlify https://example.com"));
    }

    #[test]
    fn parse_curl_report_surfaces_unsupported_flags_without_leaking_values() {
        let cmd = "curl -g -k --form file=@secret.txt -Fpart=secret_part --cookie=sid=canary_cookie --json '{\"a\":1}' https://api.example.com/upload";
        let report = parse_curl_report(cmd).unwrap();
        assert_eq!(report.request.url, "https://api.example.com/upload");
        assert_eq!(
            report.unsupported_flags,
            vec!["-k", "--form", "-F", "--cookie", "--json"]
        );
        let joined = report.unsupported_flags.join(" ");
        assert!(!joined.contains("canary_cookie"));
        assert!(!joined.contains("secret_part"));
        assert!(!joined.contains("secret.txt"));

        // Supported `-g` alone emits no unsupported warnings.
        let clean = parse_curl_report("curl -g --url 'https://{{host}}/items'").unwrap();
        assert!(clean.unsupported_flags.is_empty());
    }

    #[test]
    fn parse_curl_data_raw_preserves_leading_at_as_literal() {
        for (cmd, expected_body) in [
            (
                "curl --data-raw '@literal' https://example.com/items",
                "@literal",
            ),
            (
                "curl --data-raw='@/etc/passwd' https://example.com/items",
                "@/etc/passwd",
            ),
            (
                "curl --data-raw $'@line1\\nline2' https://example.com/items",
                "@line1\nline2",
            ),
        ] {
            let report = parse_curl_report(cmd).unwrap();
            assert_eq!(report.request.method, "POST");
            assert_eq!(report.request.body, HttpBody::Text(expected_body.into()));
            assert!(
                report.unsupported_flags.is_empty(),
                "unexpected unsupported flags for {cmd}: {:?}",
                report.unsupported_flags
            );
        }
    }

    #[test]
    fn parse_curl_reports_file_backed_data_flags_without_leaking_paths() {
        for (cmd, expected_flag) in [
            (
                "curl -d @secret.json https://example.com/upload",
                "-d @file",
            ),
            ("curl -d@secret.json https://example.com/upload", "-d @file"),
            (
                "curl --data @secret.json https://example.com/upload",
                "--data @file",
            ),
            (
                "curl --data=@secret.json https://example.com/upload",
                "--data @file",
            ),
            (
                "curl --data-ascii @secret.json https://example.com/upload",
                "--data-ascii @file",
            ),
            (
                "curl --data-binary @secret.bin https://example.com/upload",
                "--data-binary @file",
            ),
            (
                "curl --data-urlencode @secret.txt https://example.com/upload",
                "--data-urlencode @file",
            ),
            (
                "curl --data-urlencode payload@secret.txt https://example.com/upload",
                "--data-urlencode @file",
            ),
            (
                "curl --data-urlencode 'payload@file=1.txt' https://example.com/upload",
                "--data-urlencode @file",
            ),
        ] {
            let report = parse_curl_report(cmd).unwrap();
            assert_eq!(report.request.method, "POST", "{cmd}");
            assert_eq!(report.request.body, HttpBody::None, "{cmd}");
            assert_eq!(report.unsupported_flags, vec![expected_flag], "{cmd}");
            let joined = report.unsupported_flags.join(" ");
            assert!(!joined.contains("secret"), "{cmd}: {joined}");
            assert!(!joined.contains("file=1.txt"), "{cmd}: {joined}");
        }

        let inline = parse_curl_report(
            "curl --data-urlencode 'email=alice@example.com' https://example.com/register",
        )
        .unwrap();
        assert_eq!(inline.request.method, "POST");
        assert_eq!(
            inline.request.body,
            HttpBody::FormUrlEncoded(vec![KeyValue::new("email", "alice@example.com")])
        );
        assert!(inline.unsupported_flags.is_empty());
    }

    #[test]
    fn parse_curl_preserves_order_newlines_and_content_type_across_mixed_data_flags() {
        let mixed_post = parse_curl_report(
            "curl -d 'a=1' --data-urlencode 'b=hello world' --data-raw 'c=@val' https://example.com/form",
        )
        .unwrap();
        assert_eq!(mixed_post.request.method, "POST");
        assert_eq!(
            mixed_post.request.body,
            HttpBody::FormUrlEncoded(vec![
                KeyValue::new("a", "1"),
                KeyValue::new("b", "hello world"),
                KeyValue::new("c", "@val"),
            ])
        );

        let mixed_get = parse_curl_report(
            "curl -G --data-urlencode 'first=a b' -d 'second=c%2Bd' --data-raw 'third=@ok' https://example.com/search",
        )
        .unwrap();
        assert_eq!(mixed_get.request.method, "GET");
        assert_eq!(mixed_get.request.body, HttpBody::None);
        assert_eq!(
            mixed_get.request.params,
            vec![
                KeyValue::new("first", "a b"),
                KeyValue::new("second", "c+d"),
                KeyValue::new("third", "@ok"),
            ]
        );

        let binary_nl =
            parse_curl_report("curl --data-binary $'line1\\r\\nline2' https://example.com/bin")
                .unwrap();
        assert_eq!(
            binary_nl.request.body,
            HttpBody::Text("line1\r\nline2".into())
        );

        let short_eq = parse_curl_report("curl -d=foo https://example.com/x").unwrap();
        assert_eq!(short_eq.request.body, HttpBody::Text("=foo".into()));

        let explicit_text = parse_curl_report(
            "curl -H 'Content-Type: text/plain' -d '{\"not\":\"json\"}' https://example.com/txt",
        )
        .unwrap();
        assert_eq!(
            explicit_text.request.body,
            HttpBody::Text("{\"not\":\"json\"}".into())
        );

        let multi_form = parse_curl_report(
            "curl -H 'Content-Type: application/x-www-form-urlencoded' -d 'a=1' -d 'b=2%2B3' https://example.com/f",
        )
        .unwrap();
        assert_eq!(
            multi_form.request.body,
            HttpBody::FormUrlEncoded(vec![KeyValue::new("a", "1"), KeyValue::new("b", "2+3"),])
        );

        assert!(parse_curl_report("curl https://example.com -d").is_err());

        let frag_req =
            parse_curl("curl 'https://example.com/p?q=hello%20world&expr=a%2Bb#sec'").unwrap();
        assert_eq!(frag_req.url, "https://example.com/p#sec");
        assert_eq!(frag_req.name, "/p");
        assert_eq!(
            frag_req.params,
            vec![
                KeyValue::new("q", "hello world"),
                KeyValue::new("expr", "a+b"),
            ]
        );
        let exported = to_curl(&frag_req, &BTreeMap::new()).unwrap();
        assert!(
            exported.contains("https://example.com/p?q=hello%20world&expr=a%2Bb#sec"),
            "{exported}"
        );
        assert!(!exported.contains("%2520"));
        assert!(!exported.contains("%252B"));
    }

    #[test]
    fn to_curl_export_with_secrets_redacts_json_literals_and_typed_secret_vars() {
        let mut req = ApiRequest::new("POST", "SecretBody", "https://api.example.com/{{custom}}");
        req.body = HttpBody::Json(
            r#"{"password":"inline_pw_canary","nested":{"token":"{{custom}}"}}"#.into(),
        );
        let vars = BTreeMap::from([("custom".into(), "secret_custom_value".into())]);
        let secret_names = HashSet::from(["custom".to_owned()]);

        let redacted = to_curl_export_with_secrets(&req, &vars, &secret_names, true).unwrap();
        assert!(redacted.had_secrets);
        assert!(redacted.had_variables);
        assert!(!redacted.command.contains("secret_custom_value"));
        assert!(!redacted.command.contains("inline_pw_canary"));
        assert!(redacted.command.contains("{{custom}}"));
        assert!(redacted.command.contains("<REDACTED>"));

        let full = to_curl_export_with_secrets(&req, &vars, &secret_names, false).unwrap();
        assert!(full.had_secrets);
        assert!(full.command.contains("secret_custom_value"));
        assert!(full.command.contains("inline_pw_canary"));
    }
}
