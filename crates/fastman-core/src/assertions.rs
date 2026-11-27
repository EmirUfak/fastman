use crate::{http::HttpResponse, model::AssertionRule};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionFailure {
    pub rule_index: usize,
    pub reason: AssertionError,
}

impl fmt::Display for AssertionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Response assertion at index {} failed: {}",
            self.rule_index, self.reason
        )
    }
}

impl std::error::Error for AssertionFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionError {
    StatusMismatch,
    MissingHeader,
    HeaderMismatch,
    TruncatedBody,
    MalformedJson,
    MissingJsonField,
    JsonMismatch,
    LatencyExceeded,
}

impl fmt::Display for AssertionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StatusMismatch => f.write_str("status does not match"),
            Self::MissingHeader => f.write_str("header is missing"),
            Self::HeaderMismatch => f.write_str("header value does not match"),
            Self::TruncatedBody => f.write_str("response body was truncated"),
            Self::MalformedJson => f.write_str("response body is not valid JSON"),
            Self::MissingJsonField => f.write_str("JSON field is missing"),
            Self::JsonMismatch => f.write_str("JSON value or type does not match"),
            Self::LatencyExceeded => f.write_str("response latency exceeds limit"),
        }
    }
}

impl std::error::Error for AssertionError {}

/// Evaluate rules in order, stopping at the first failure. Errors contain no response data.
pub fn evaluate_assertions(
    rules: &[AssertionRule],
    response: &HttpResponse,
) -> Result<(), AssertionFailure> {
    let mut json = None;
    for (rule_index, rule) in rules.iter().enumerate() {
        let reason = match rule {
            AssertionRule::Status { expected } if response.status != *expected => {
                AssertionError::StatusMismatch
            }
            AssertionRule::Header { name, expected } => {
                let mut values = response
                    .headers
                    .iter()
                    .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value);
                match values.next() {
                    None => AssertionError::MissingHeader,
                    Some(first) if first == expected || values.any(|value| value == expected) => {
                        continue;
                    }
                    Some(_) => AssertionError::HeaderMismatch,
                }
            }
            AssertionRule::JsonField { .. } if response.truncated => AssertionError::TruncatedBody,
            AssertionRule::JsonField { pointer, expected } => {
                if json.is_none() {
                    json = Some(serde_json::from_str::<Value>(&response.body).map_err(|_| {
                        AssertionFailure {
                            rule_index,
                            reason: AssertionError::MalformedJson,
                        }
                    })?);
                }
                match json.as_ref().and_then(|value| value.pointer(pointer)) {
                    None => AssertionError::MissingJsonField,
                    Some(actual) if actual == expected => continue,
                    Some(_) => AssertionError::JsonMismatch,
                }
            }
            AssertionRule::Latency { max_ms } if response.elapsed_ms > u128::from(*max_ms) => {
                AssertionError::LatencyExceeded
            }
            AssertionRule::Status { .. } | AssertionRule::Latency { .. } => continue,
        };
        return Err(AssertionFailure { rule_index, reason });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response() -> HttpResponse {
        HttpResponse {
            status: 200,
            reason: "OK".into(),
            ttfb_ms: 5,
            download_ms: 20,
            elapsed_ms: 25,
            final_url: "https://example.com".into(),
            headers: vec![
                ("X-Mode".into(), "test".into()),
                ("x-mode".into(), "live".into()),
            ],
            body: r#"{"items":[{"a/b":42}],"empty":null}"#.into(),
            bytes_received: 35,
            truncated: false,
        }
    }

    fn header(name: &str, expected: &str) -> AssertionRule {
        AssertionRule::Header {
            name: name.into(),
            expected: expected.into(),
        }
    }

    fn field(pointer: &str, expected: Value) -> AssertionRule {
        AssertionRule::JsonField {
            pointer: pointer.into(),
            expected,
        }
    }

    #[test]
    fn matches_response_metadata_and_typed_json() {
        assert_eq!(
            evaluate_assertions(
                &[
                    AssertionRule::Status { expected: 200 },
                    header("X-MODE", "live"),
                    field("/items/0/a~1b", json!(42)),
                    field("/empty", Value::Null),
                    AssertionRule::Latency { max_ms: 25 },
                ],
                &response(),
            ),
            Ok(())
        );
    }

    #[test]
    fn reports_first_failure_without_echoing_response_data() {
        use AssertionError::*;
        for (rule, reason) in [
            (AssertionRule::Status { expected: 201 }, StatusMismatch),
            (header("absent", ""), MissingHeader),
            (header("x-mode", "LIVE"), HeaderMismatch),
            (field("/absent", Value::Null), MissingJsonField),
            (field("/items/0/a~1b", json!("42")), JsonMismatch),
            (field("/items/0/a~1b", json!(43)), JsonMismatch),
            (AssertionRule::Latency { max_ms: 24 }, LatencyExceeded),
        ] {
            let failure = evaluate_assertions(
                &[
                    AssertionRule::Status { expected: 200 },
                    rule,
                    AssertionRule::Status { expected: 204 },
                ],
                &response(),
            )
            .unwrap_err();
            assert_eq!(
                failure,
                AssertionFailure {
                    rule_index: 1,
                    reason
                }
            );
            assert!(!failure.to_string().contains("live"));
        }
    }

    #[test]
    fn rejects_malformed_and_truncated_json_only_for_json_rules() {
        let mut response = response();
        let rule = field("", json!({"items": [{"a/b": 42}], "empty": null}));
        assert_eq!(
            evaluate_assertions(std::slice::from_ref(&rule), &response),
            Ok(())
        );
        response.truncated = true;
        // Even a syntactically complete prefix must not satisfy a JSON rule.
        assert_eq!(
            evaluate_assertions(std::slice::from_ref(&rule), &response)
                .unwrap_err()
                .reason,
            AssertionError::TruncatedBody
        );
        response.truncated = false;
        response.body.pop();
        assert_eq!(
            evaluate_assertions(&[rule], &response).unwrap_err().reason,
            AssertionError::MalformedJson
        );
        response.truncated = true;
        assert_eq!(evaluate_assertions(&[], &response), Ok(()));
        assert_eq!(
            evaluate_assertions(&[AssertionRule::Status { expected: 200 }], &response),
            Ok(())
        );
    }
}
