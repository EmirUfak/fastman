//! Best-effort, deliberately limited Postman collection v2.1 importer.
//! Scripts, auth inheritance, file uploads and complex protocol requests are
//! not executed and are reported as unsupported rather than silently emulated.

use crate::model::{ApiRequest, Collection, Folder, HttpBody, KeyValue, Project};
use crate::security::{
    SanitizeSummary, SensitiveFieldFinding, collect_sensitive_request_findings,
    is_valid_http_method, sanitize_request_credentials, validate_template_or_http_url,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

pub const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FOLDER_DEPTH: usize = 32;
pub const MAX_IMPORTED_ITEMS: usize = 10_000;

#[derive(Debug)]
pub struct ImportReport {
    pub project: Project,
    pub warnings: Vec<String>,
    pub requests_imported: usize,
    pub sensitive_fields: Vec<SensitiveFieldFinding>,
}

impl ImportReport {
    /// Moves recognized credentials (`Authorization: Bearer`/`Basic`, API key headers/query)
    /// into session-only [`crate::model::Auth`] and strips any remaining sensitive headers
    /// and query credentials so they are not persisted in plaintext `project.json`.
    pub fn sanitize_credentials(&mut self) -> SanitizeSummary {
        fn visit_folder(folder: &mut Folder, total: &mut SanitizeSummary) {
            for req in &mut folder.requests {
                let s = sanitize_request_credentials(req);
                total.moved_to_session_auth += s.moved_to_session_auth;
                total.stripped_fields += s.stripped_fields;
            }
            for sub in &mut folder.folders {
                visit_folder(sub, total);
            }
        }

        let mut total = SanitizeSummary::default();
        for coll in &mut self.project.collections {
            for req in &mut coll.requests {
                let s = sanitize_request_credentials(req);
                total.moved_to_session_auth += s.moved_to_session_auth;
                total.stripped_fields += s.stripped_fields;
            }
            for sub in &mut coll.folders {
                visit_folder(sub, &mut total);
            }
        }
        self.sensitive_fields.clear();
        total
    }
}

/// Read a Postman collection file with a strict upper size bound before allocation.
pub fn read_postman_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("Open {}", path.display()))?;
    if let Ok(meta) = file.metadata()
        && meta.len() > MAX_IMPORT_BYTES as u64
    {
        bail!(
            "Import file {} exceeds maximum allowed size ({MAX_IMPORT_BYTES} bytes)",
            path.display()
        );
    }
    let mut buf = Vec::new();
    file.take((MAX_IMPORT_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .with_context(|| format!("Read {}", path.display()))?;
    if buf.len() > MAX_IMPORT_BYTES {
        bail!(
            "Import file {} exceeds maximum allowed size ({MAX_IMPORT_BYTES} bytes)",
            path.display()
        );
    }
    String::from_utf8(buf).with_context(|| format!("Decode {} as UTF-8", path.display()))
}

struct ImportCounters {
    requests: usize,
    items: usize,
}

impl ImportCounters {
    fn bump_item(&mut self) -> Result<()> {
        self.items += 1;
        if self.items > MAX_IMPORTED_ITEMS {
            bail!("Postman collection exceeds maximum item limit ({MAX_IMPORTED_ITEMS})");
        }
        Ok(())
    }
}

pub fn postman_v21(input: &str) -> Result<ImportReport> {
    if input.len() > MAX_IMPORT_BYTES {
        bail!("Postman collection exceeds maximum size ({MAX_IMPORT_BYTES} bytes)");
    }
    let root: Value = serde_json::from_str(input).context("Parse Postman collection")?;
    let version = root
        .pointer("/info/schema")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !version.contains("collection/v2.1") {
        bail!("Expected a Postman Collection v2.1 export");
    }
    let name = root
        .pointer("/info/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported API");
    let mut project = Project::new(name);
    let mut collection = Collection::new(name);
    let mut warnings = Vec::new();
    let mut sensitive_fields = Vec::new();
    let mut counters = ImportCounters {
        requests: 0,
        items: 0,
    };
    if let Some(items) = root.get("item").and_then(Value::as_array) {
        for item in items {
            counters.bump_item()?;
            if item.get("item").is_some() {
                collection.folders.push(folder(
                    item,
                    1,
                    &mut warnings,
                    &mut sensitive_fields,
                    &mut counters,
                )?);
            } else if let Some(req) = request(item, &mut warnings)? {
                collect_sensitive_request_findings(&req, &mut sensitive_fields);
                collection.requests.push(req);
                counters.requests += 1;
            }
        }
    }
    project.collections = vec![collection];
    if root.get("event").is_some() {
        warnings.push("Collection scripts not imported".into());
    }
    if root.get("auth").is_some() {
        warnings.push("Collection auth not imported".into());
    }
    Ok(ImportReport {
        project,
        warnings,
        requests_imported: counters.requests,
        sensitive_fields,
    })
}

fn folder(
    item: &Value,
    depth: usize,
    warnings: &mut Vec<String>,
    sensitive_fields: &mut Vec<SensitiveFieldFinding>,
    counters: &mut ImportCounters,
) -> Result<Folder> {
    if depth > MAX_FOLDER_DEPTH {
        bail!("Postman collection exceeds maximum folder nesting depth ({MAX_FOLDER_DEPTH})");
    }
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Folder");
    let mut folder = Folder::new(name);
    if let Some(children) = item.get("item").and_then(Value::as_array) {
        for child in children {
            counters.bump_item()?;
            if child.get("item").is_some() {
                folder.folders.push(self::folder(
                    child,
                    depth + 1,
                    warnings,
                    sensitive_fields,
                    counters,
                )?);
            } else if let Some(req) = self::request(child, warnings)? {
                collect_sensitive_request_findings(&req, sensitive_fields);
                folder.requests.push(req);
                counters.requests += 1;
            }
        }
    }
    Ok(folder)
}

fn request(item: &Value, warnings: &mut Vec<String>) -> Result<Option<ApiRequest>> {
    let Some(data) = item.get("request") else {
        return Ok(None);
    };
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Untitled Request");
    let raw_method = match data.get("method") {
        Some(Value::String(m)) => m.trim().to_ascii_uppercase(),
        Some(_) => bail!("Invalid HTTP method type in request '{name}'"),
        None => "GET".to_owned(),
    };
    if !is_valid_http_method(&raw_method) {
        bail!("Invalid HTTP method '{raw_method}' in request '{name}'");
    }
    let url = match data.get("url") {
        Some(Value::String(value)) => value.trim().to_owned(),
        Some(value) => match value.get("raw").and_then(Value::as_str) {
            Some(raw) => raw.trim().to_owned(),
            None => return Ok(None),
        },
        None => return Ok(None),
    };
    validate_template_or_http_url(&url)
        .with_context(|| format!("Invalid URL in imported request '{name}'"))?;

    let mut result = ApiRequest::new(raw_method, name, url);
    if let Some(headers) = data.get("header").and_then(Value::as_array) {
        for header in headers {
            let key = header
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let value = header.get("value").and_then(Value::as_str).unwrap_or("");
            if !key.is_empty() {
                let mut pair = KeyValue::new(key, value);
                pair.enabled = !header
                    .get("disabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                result.headers.push(pair);
            }
        }
    }
    if let Some(body) = data.get("body") {
        match body.get("mode").and_then(Value::as_str) {
            Some("raw") => {
                let content = body
                    .get("raw")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let language = body
                    .pointer("/options/raw/language")
                    .and_then(Value::as_str);
                result.body = if language == Some("json") {
                    HttpBody::Json(content)
                } else {
                    HttpBody::Text(content)
                };
            }
            Some("urlencoded") => {
                let entries = body.get("urlencoded").and_then(Value::as_array);
                let values = entries
                    .into_iter()
                    .flatten()
                    .filter_map(|value| {
                        let key = value.get("key")?.as_str()?;
                        let mut pair = KeyValue::new(
                            key,
                            value.get("value").and_then(Value::as_str).unwrap_or(""),
                        );
                        pair.enabled = !value
                            .get("disabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        Some(pair)
                    })
                    .collect();
                result.body = HttpBody::FormUrlEncoded(values);
            }
            Some(_) => warnings.push(format!("Unsupported body type in {name}")),
            None => {}
        }
    }
    if data.get("auth").is_some() {
        warnings.push(format!("Auth not imported for {name}"));
    }
    if item.get("event").is_some() {
        warnings.push(format!("Scripts not imported for {name}"));
    }
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Auth;

    #[test]
    fn import_nested_postman_collection() {
        let sample = r#"{
            "info":{"name":"Sample","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item":[{"name":"Users","item":[{"name":"List","request":{
              "method":"GET","url":{"raw":"https://example.com/users"},
              "header":[{"key":"Accept","value":"application/json"}]
            }}]}]
        }"#;
        let result = postman_v21(sample).unwrap();
        assert_eq!(result.requests_imported, 1);
        assert!(result.sensitive_fields.is_empty());
        assert_eq!(
            result.project.collections[0].folders[0].requests[0].name,
            "List"
        );
    }

    #[test]
    fn rejects_invalid_methods_schemes_and_deep_nesting() {
        let bad_method = r#"{
            "info":{"name":"Bad","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item":[{"name":"Evil","request":{"method":"GET; rm -rf /","url":"https://example.com"}}]
        }"#;
        assert!(postman_v21(bad_method).is_err());

        let bad_url = r#"{
            "info":{"name":"Bad","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item":[{"name":"File","request":{"method":"GET","url":"file:///etc/passwd"}}]
        }"#;
        assert!(postman_v21(bad_url).is_err());

        let mut deep =
            r#"{"name":"Leaf","request":{"method":"GET","url":"https://example.com"}}"#.to_string();
        for _ in 0..=MAX_FOLDER_DEPTH {
            deep = format!(r#"{{"name":"F","item":[{deep}]}}"#);
        }
        let deep_json = format!(
            r#"{{"info":{{"name":"Deep","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"}},"item":[{deep}]}}"#
        );
        assert!(postman_v21(&deep_json).is_err());
    }

    #[test]
    fn detects_and_sanitizes_sensitive_headers_including_non_bearer_auth() {
        let sample = r#"{
            "info":{"name":"  ","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item":[
                {"name":"BearerReq","request":{
                    "method":"GET",
                    "url":{"raw":"https://example.com/a?api_key=q_secret"},
                    "header":[{"key":"Authorization","value":"Bearer tok_123"},{"key":"Accept","value":"application/json"}]
                }},
                {"name":"CustomAuthReq","request":{
                    "method":"POST",
                    "url":{"raw":"https://example.com/b"},
                    "header":[{"key":"Authorization","value":"Token custom_999"},{"key":"Cookie","value":"sid=abc"}]
                }}
            ]
        }"#;
        let mut report = postman_v21(sample).unwrap();
        assert_eq!(report.project.name, "Imported API");
        assert_eq!(report.sensitive_fields.len(), 4);

        let summary = report.sanitize_credentials();
        assert_eq!(summary.moved_to_session_auth, 1);
        assert_eq!(summary.stripped_fields, 3);
        assert!(report.sensitive_fields.is_empty());

        let reqs = &report.project.collections[0].requests;
        assert_eq!(
            reqs[0].auth,
            Auth::Bearer {
                token: "tok_123".into()
            }
        );
        assert_eq!(reqs[0].url, "https://example.com/a");
        assert_eq!(
            reqs[0].headers,
            vec![KeyValue::new("Accept", "application/json")]
        );
        assert_eq!(reqs[1].auth, Auth::None);
        assert!(reqs[1].headers.is_empty());
    }
}
