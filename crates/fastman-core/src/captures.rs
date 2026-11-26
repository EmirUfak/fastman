use crate::{AssertionFailure, CaptureSource, ChainStep, HttpResponse, evaluate_assertions};
use serde_json::Value;
use thiserror::Error;

pub(crate) const MAX_CAPTURES: usize = 64;
pub(crate) const MAX_CAPTURE_NAME_BYTES: usize = 128;
const MAX_CAPTURE_VALUE_BYTES: usize = 16 * 1024;

/// Runtime-only value. Deliberately does not implement `Debug` or serialization.
pub struct CapturedValue {
    pub name: String,
    pub value: Value,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CaptureFailure {
    #[error(transparent)]
    Assertion(#[from] AssertionFailure),
    #[error("capture rule count exceeds {MAX_CAPTURES}")]
    TooManyRules,
    #[error("Response capture at index {rule_index} failed: {reason}")]
    Extraction {
        rule_index: usize,
        reason: CaptureError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CaptureError {
    #[error("required value is missing")]
    MissingValue,
    #[error("response body was truncated")]
    TruncatedBody,
    #[error("response body is not valid JSON")]
    MalformedJson,
    #[error("capture name or value exceeds its size limit")]
    TooLarge,
}

/// Stage captures for an executed step. Return nothing unless all assertions and
/// captures succeed. Optional rules skip missing values, not unreadable JSON.
pub fn capture_step_response(
    step: &ChainStep,
    response: &HttpResponse,
) -> Result<Vec<CapturedValue>, CaptureFailure> {
    evaluate_assertions(&step.assertions, response)?;
    if step.captures.len() > MAX_CAPTURES {
        return Err(CaptureFailure::TooManyRules);
    }
    let mut captured = Vec::new();
    let mut json = None;
    for (rule_index, rule) in step.captures.iter().enumerate() {
        let failure = |reason| CaptureFailure::Extraction { rule_index, reason };
        let value = match &rule.source {
            CaptureSource::Status => Some(Value::from(response.status)),
            CaptureSource::Header { name } => response
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| Value::String(value.clone())),
            CaptureSource::JsonField { pointer } => {
                if response.truncated {
                    return Err(failure(CaptureError::TruncatedBody));
                }
                if json.is_none() {
                    json = Some(
                        serde_json::from_str::<Value>(&response.body)
                            .map_err(|_| failure(CaptureError::MalformedJson))?,
                    );
                }
                json.as_ref()
                    .and_then(|value| value.pointer(pointer))
                    .cloned()
            }
        };
        match value {
            Some(value) => {
                if rule.name.len() > MAX_CAPTURE_NAME_BYTES
                    || value.to_string().len() > MAX_CAPTURE_VALUE_BYTES
                {
                    return Err(failure(CaptureError::TooLarge));
                }
                captured.push(CapturedValue {
                    name: rule.name.clone(),
                    value,
                    sensitive: rule.sensitive,
                });
            }
            None if rule.required => return Err(failure(CaptureError::MissingValue)),
            None => {}
        }
    }
    Ok(captured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssertionError, AssertionRule};
    use serde_json::json;

    fn login() -> (ChainStep, HttpResponse) {
        let step = serde_json::from_value(json!({
            "id": "login",
            "request_id": "request",
            "assertions": [{"kind": "status", "expected": 200}],
            "captures": [
                {"name": "token", "source": {"kind": "header", "name": "X-Token"}},
                {"name": "user_id", "source": {"kind": "json_field", "pointer": "/user/id"},
                 "sensitive": false},
                {"name": "status", "source": {"kind": "status"}},
                {"name": "nickname", "source": {"kind": "json_field", "pointer": "/nickname"},
                 "required": false}
            ]
        }))
        .unwrap();
        let body = r#"{"user":{"id":42}}"#.to_owned();
        let response = HttpResponse {
            status: 200,
            reason: "OK".into(),
            ttfb_ms: 5,
            download_ms: 20,
            elapsed_ms: 25,
            final_url: "https://example.com/login".into(),
            headers: vec![("x-token".into(), "fixture-token".into())],
            bytes_received: body.len(),
            body,
            truncated: false,
        };
        (step, response)
    }

    #[test]
    fn captures_token_typed_user_id_and_status() {
        let (step, response) = login();
        let values = capture_step_response(&step, &response).unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0].name, "token");
        assert_eq!(values[0].value, json!("fixture-token"));
        assert!(values[0].sensitive);
        assert_eq!(values[1].name, "user_id");
        assert_eq!(values[1].value, json!(42));
        assert!(!values[1].sensitive);
        assert_eq!(values[2].value, json!(200));
    }

    #[test]
    fn returns_no_partial_values_on_failure() {
        let (mut step, mut response) = login();
        response.body = "{}".into();
        assert_eq!(
            capture_step_response(&step, &response).err(),
            Some(CaptureFailure::Extraction {
                rule_index: 1,
                reason: CaptureError::MissingValue,
            })
        );
        step.assertions = vec![AssertionRule::Status { expected: 201 }];
        assert_eq!(
            capture_step_response(&step, &response).err(),
            Some(CaptureFailure::Assertion(AssertionFailure {
                rule_index: 0,
                reason: AssertionError::StatusMismatch,
            }))
        );
    }

    #[test]
    fn optional_capture_still_requires_readable_json() {
        let (mut step, mut response) = login();
        step.captures.drain(..3);
        response.body = "{".into();
        for (truncated, reason) in [
            (false, CaptureError::MalformedJson),
            (true, CaptureError::TruncatedBody),
        ] {
            response.truncated = truncated;
            assert_eq!(
                capture_step_response(&step, &response).err(),
                Some(CaptureFailure::Extraction {
                    rule_index: 0,
                    reason
                })
            );
        }
    }
}
