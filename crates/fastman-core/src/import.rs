//! Best-effort, deliberately limited Postman collection v2.1 importer.
//! Scripts, auth inheritance, file uploads and complex protocol requests are
//! not executed and are reported as unsupported rather than silently emulated.
use crate::model::{ApiRequest, Collection, Folder, HttpBody, KeyValue, Project};
use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug)]
pub struct ImportReport {
    pub project: Project,
    pub warnings: Vec<String>,
    pub requests_imported: usize,
}

pub fn postman_v21(input: &str) -> Result<ImportReport> {
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
        .unwrap_or("Imported API");
    let mut project = Project::new(name);
    let mut collection = Collection::new(name);
    let mut warnings = Vec::new();
    let mut count = 0;
    if let Some(items) = root.get("item").and_then(Value::as_array) {
        for item in items {
            if item.get("item").is_some() {
                collection
                    .folders
                    .push(folder(item, &mut warnings, &mut count));
            } else if let Some(request) = request(item, &mut warnings) {
                collection.requests.push(request);
                count += 1;
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
        requests_imported: count,
    })
}

fn folder(item: &Value, warnings: &mut Vec<String>, count: &mut usize) -> Folder {
    let name = item.get("name").and_then(Value::as_str).unwrap_or("Folder");
    let mut folder = Folder::new(name);
    if let Some(children) = item.get("item").and_then(Value::as_array) {
        for child in children {
            if child.get("item").is_some() {
                folder.folders.push(self::folder(child, warnings, count));
            } else if let Some(request) = self::request(child, warnings) {
                folder.requests.push(request);
                *count += 1;
            }
        }
    }
    folder
}

fn request(item: &Value, warnings: &mut Vec<String>) -> Option<ApiRequest> {
    let data = item.get("request")?;
    let method = data.get("method").and_then(Value::as_str).unwrap_or("GET");
    let url = match data.get("url") {
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.get("raw")?.as_str()?.to_owned(),
        None => return None,
    };
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Untitled Request");
    let mut result = ApiRequest::new(method, name, url);
    if let Some(headers) = data.get("header").and_then(Value::as_array) {
        for header in headers {
            let key = header.get("key").and_then(Value::as_str).unwrap_or("");
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
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(
            result.project.collections[0].folders[0].requests[0].name,
            "List"
        );
    }
}
