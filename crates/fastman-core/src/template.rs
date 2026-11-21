use crate::model::{ApiRequest, Auth, HttpBody, KeyValue};
use crate::security::is_sensitive_var_name;
use std::collections::BTreeMap;

pub const MAX_INTERPOLATED_BYTES: usize = 4 * 1024 * 1024;
const MASKED_SECRET: &str = "••••";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateInspection {
    pub preview: String,
    pub used_variables: Vec<String>,
    pub missing_variables: Vec<String>,
    pub unclosed: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestTemplateInspection {
    pub url_preview: String,
    pub url_has_templates: bool,
    pub url_resolved_count: usize,
    pub missing_variables: Vec<String>,
    pub unclosed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSpan {
    pub range: std::ops::Range<usize>,
    pub name: String,
}

/// Find UTF-8 byte spans of `{{variable}}` placeholders in `input` that resolve in `variables`.
pub fn find_resolved_spans(input: &str, variables: &BTreeMap<String, String>) -> Vec<TemplateSpan> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(rel_start) = input[offset..].find("{{") {
        let start = offset + rel_start;
        let after_open = &input[start + 2..];
        let Some(rel_end) = after_open.find("}}") else {
            break;
        };
        let end = start + 2 + rel_end + 2;
        let name = after_open[..rel_end].trim();
        if variables.contains_key(name) {
            spans.push(TemplateSpan {
                range: start..end,
                name: name.to_owned(),
            });
        }
        offset = end;
    }
    spans
}

/// Resolve each token once. `name` is absent for an unclosed token.
pub(crate) fn expand<'a, E>(
    input: &'a str,
    mut resolve: impl FnMut(Option<&'a str>, &'a str) -> Result<&'a str, E>,
    too_large: impl Fn() -> E,
) -> Result<String, E> {
    let mut output = String::with_capacity(input.len().min(MAX_INTERPOLATED_BYTES));
    let mut push = |piece: &str| {
        if piece.len() > MAX_INTERPOLATED_BYTES - output.len() {
            return Err(too_large());
        }
        output.push_str(piece);
        Ok(())
    };
    let mut remainder = input;
    while let Some(start) = remainder.find("{{") {
        push(&remainder[..start])?;
        let raw = &remainder[start..];
        let after_open = &raw[2..];
        let Some(end) = after_open.find("}}") else {
            push(resolve(None, raw)?)?;
            return Ok(output);
        };
        let (token, rest) = raw.split_at(end + 4);
        push(resolve(Some(after_open[..end].trim()), token)?)?;
        remainder = rest;
    }
    push(remainder)?;
    Ok(output)
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|existing| existing == item) {
        list.push(item.to_owned());
    }
}

/// Inspect `{{variable}}` placeholders in a template string without failing on missing
/// or unclosed expressions. Optionally masks values of sensitive variable names.
pub fn inspect(
    input: &str,
    variables: &BTreeMap<String, String>,
    mask_sensitive: bool,
) -> TemplateInspection {
    let mut used_variables = Vec::new();
    let mut missing_variables = Vec::new();
    let mut unclosed = false;

    let expanded = expand(
        input,
        |name, raw| {
            let Some(name) = name else {
                unclosed = true;
                return Ok::<_, ()>(raw);
            };
            if let Some(value) = variables.get(name) {
                push_unique(&mut used_variables, name);
                if mask_sensitive && is_sensitive_var_name(name) && !value.is_empty() {
                    Ok(MASKED_SECRET)
                } else {
                    Ok(value.as_str())
                }
            } else {
                push_unique(&mut missing_variables, name);
                Ok(raw)
            }
        },
        || (),
    );

    match expanded {
        Ok(preview) => TemplateInspection {
            preview,
            used_variables,
            missing_variables,
            unclosed,
            truncated: false,
        },
        Err(()) => TemplateInspection {
            preview: String::new(),
            used_variables,
            missing_variables,
            unclosed,
            truncated: true,
        },
    }
}

fn scan_field(
    input: &str,
    variables: &BTreeMap<String, String>,
    missing: &mut Vec<String>,
    unclosed: &mut bool,
) {
    if !input.contains("{{") {
        return;
    }
    let result = inspect(input, variables, false);
    for name in result.missing_variables {
        push_unique(missing, &name);
    }
    *unclosed |= result.unclosed;
}

fn scan_pairs(
    items: &[KeyValue],
    variables: &BTreeMap<String, String>,
    missing: &mut Vec<String>,
    unclosed: &mut bool,
) {
    for item in items {
        if !item.enabled || item.key.trim().is_empty() {
            continue;
        }
        scan_field(&item.key, variables, missing, unclosed);
        scan_field(&item.value, variables, missing, unclosed);
    }
}

/// Inspect all interpolated fields of an [`ApiRequest`] against `variables`.
pub fn inspect_request(
    request: &ApiRequest,
    variables: &BTreeMap<String, String>,
) -> RequestTemplateInspection {
    let url_has_templates = request.url.contains("{{");
    let url_result = inspect(&request.url, variables, true);
    let url_resolved_count = url_result.used_variables.len();
    let mut missing_variables = url_result.missing_variables;
    let mut unclosed = url_result.unclosed;

    scan_pairs(
        &request.params,
        variables,
        &mut missing_variables,
        &mut unclosed,
    );
    scan_pairs(
        &request.headers,
        variables,
        &mut missing_variables,
        &mut unclosed,
    );

    match &request.body {
        HttpBody::None => {}
        HttpBody::Text(text) | HttpBody::Json(text) => {
            scan_field(text, variables, &mut missing_variables, &mut unclosed);
        }
        HttpBody::FormUrlEncoded(rows) => {
            scan_pairs(rows, variables, &mut missing_variables, &mut unclosed);
        }
    }

    match &request.auth {
        Auth::None => {}
        Auth::Basic { username, password } => {
            scan_field(username, variables, &mut missing_variables, &mut unclosed);
            scan_field(password, variables, &mut missing_variables, &mut unclosed);
        }
        Auth::Bearer { token } => {
            scan_field(token, variables, &mut missing_variables, &mut unclosed);
        }
        Auth::ApiKeyHeader { name, value } | Auth::ApiKeyQuery { name, value } => {
            scan_field(name, variables, &mut missing_variables, &mut unclosed);
            scan_field(value, variables, &mut missing_variables, &mut unclosed);
        }
    }

    let url_preview = if url_result.truncated {
        "<preview exceeds 4 MiB limit>".to_owned()
    } else {
        url_result.preview
    };

    RequestTemplateInspection {
        url_preview,
        url_has_templates,
        url_resolved_count,
        missing_variables,
        unclosed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_resolved_spans_matches_defined_placeholders_only() {
        let vars = BTreeMap::from([
            ("prod".into(), "api.example.com".into()),
            ("v".into(), "v2".into()),
        ]);
        let input = "https://{{ prod }}/{{missing}}/{{v}}/{{unclosed";
        let spans = find_resolved_spans(input, &vars);
        assert_eq!(spans.len(), 2);
        assert_eq!(&input[spans[0].range.clone()], "{{ prod }}");
        assert_eq!(spans[0].name, "prod");
        assert_eq!(&input[spans[1].range.clone()], "{{v}}");
        assert_eq!(spans[1].name, "v");
    }

    #[test]
    fn inspect_resolves_masks_sensitive_and_reports_missing() {
        let vars = BTreeMap::from([
            ("host".into(), "https://api.example.com".into()),
            ("api_key".into(), "super_secret_123".into()),
        ]);
        let res = inspect("{{host}}/items?key={{api_key}}&env={{env}}", &vars, true);
        assert_eq!(
            res.preview,
            "https://api.example.com/items?key=••••&env={{env}}"
        );
        assert_eq!(res.used_variables, vec!["host", "api_key"]);
        assert_eq!(res.missing_variables, vec!["env"]);
        assert!(!res.unclosed);
        assert!(!res.truncated);
    }

    #[test]
    fn inspect_request_collects_across_enabled_sections_only() {
        let mut req = ApiRequest::new("POST", "Create", "{{base}}/users");
        req.params.push(KeyValue::new("org", "{{missing_param}}"));
        req.params.push(KeyValue {
            enabled: false,
            key: "ignored".into(),
            value: "{{disabled_var}}".into(),
        });
        req.headers
            .push(KeyValue::new("X-Trace", "{{missing_hdr}}"));
        req.body = HttpBody::Json(r#"{"id":"{{missing_body}}"}"#.into());
        req.auth = Auth::Bearer {
            token: "{{missing_tok".into(),
        };

        let vars = BTreeMap::from([("base".into(), "https://api.example.com".into())]);
        let report = inspect_request(&req, &vars);
        assert!(report.url_has_templates);
        assert_eq!(report.url_preview, "https://api.example.com/users");
        assert_eq!(
            report.missing_variables,
            vec!["missing_param", "missing_hdr", "missing_body"]
        );
        assert!(report.unclosed);
    }
}
