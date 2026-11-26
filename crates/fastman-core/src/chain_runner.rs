use crate::captures::{MAX_CAPTURE_NAME_BYTES, MAX_CAPTURES};
use crate::security::is_valid_http_method;
use crate::template::inspect_request;
use crate::{ChainRunContext, ChainRunError, HttpEngine, Project, SecretStore};
use std::collections::BTreeSet;
use thiserror::Error;

const MAX_CHAIN_STEPS: usize = 128;

#[derive(Debug, Error)]
pub enum ChainExecutionError {
    #[error("chain does not exist or its ID is ambiguous")]
    UnknownChain,
    #[error("chain must contain an enabled step and at most {MAX_CHAIN_STEPS} steps")]
    InvalidLength,
    #[error("invalid chain step at index {index}: {reason}")]
    InvalidStep { index: usize, reason: &'static str },
    #[error(transparent)]
    Context(#[from] ChainRunError),
    #[error("could not initialize chain HTTP session")]
    Session,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChainStepOutcome {
    Passed { status: u16 },
    RequestFailed,
    CheckFailed(ChainRunError),
    Disabled,
    Skipped,
}

/// Execute a project chain in definition order. Outcomes align with step indices.
/// Destinations must be known before execution; captures may fill URL paths or
/// queries, but must not be needed to construct the origin.
pub async fn run_chain(
    project: &Project,
    chain_id: &str,
    env_id: Option<&str>,
    secret_store: &dyn SecretStore,
    primary_origin: &str,
    trusted_origins: &[String],
) -> Result<Vec<ChainStepOutcome>, ChainExecutionError> {
    let mut matches = project.chains.iter().filter(|chain| chain.id == chain_id);
    let chain = matches.next().ok_or(ChainExecutionError::UnknownChain)?;
    if matches.next().is_some() {
        return Err(ChainExecutionError::UnknownChain);
    }
    if chain.steps.len() > MAX_CHAIN_STEPS || !chain.steps.iter().any(|step| step.enabled) {
        return Err(ChainExecutionError::InvalidLength);
    }
    let mut context = ChainRunContext::new(project, env_id, secret_store)?;
    let engine = HttpEngine::for_chain(primary_origin, trusted_origins)
        .map_err(|_| ChainExecutionError::Session)?;
    let mut available = context.resolved_variables().values;
    let mut step_ids = BTreeSet::new();
    let mut capture_names = BTreeSet::new();
    let mut plan = Vec::with_capacity(chain.steps.len());
    for (index, step) in chain.steps.iter().enumerate() {
        let invalid = |reason| ChainExecutionError::InvalidStep { index, reason };
        if step.id.is_empty() || !step_ids.insert(&step.id) {
            return Err(invalid("missing or duplicate step ID"));
        }
        let request = project
            .find_request(&step.request_id)
            .ok_or_else(|| invalid("request reference does not exist"))?;
        if !step.enabled {
            plan.push((step.clone(), request.clone(), None));
            continue;
        }
        let inspection = inspect_request(request, &available);
        if inspection.unclosed || !inspection.missing_variables.is_empty() {
            return Err(invalid("request has unresolved or malformed variables"));
        }
        if !is_valid_http_method(&request.method) {
            return Err(invalid("invalid HTTP method"));
        }
        // A placeholder permits path/query captures while keeping the approved
        // origin fixed. Check that origin again after substituting real values.
        let url = engine
            .resolve_url(request, &available, &Default::default())
            .map_err(|_| invalid("invalid or unapproved request destination"))?;
        plan.push((step.clone(), request.clone(), Some(url.origin())));
        let mut names = BTreeSet::new();
        for rule in &step.captures {
            if rule.name.is_empty()
                || rule.name.len() > MAX_CAPTURE_NAME_BYTES
                || rule.name.trim() != rule.name
                || rule.name.contains(['{', '}'])
                || !names.insert(&rule.name)
            {
                return Err(invalid("invalid or duplicate capture name"));
            }
            capture_names.insert(&rule.name);
            if rule.required {
                available.insert(rule.name.clone(), "fastman-capture".into());
            }
        }
        if capture_names.len() > MAX_CAPTURES {
            return Err(invalid("chain exceeds capture limit"));
        }
    }
    drop(available);

    let mut stopped = false;
    let mut outcomes = Vec::with_capacity(plan.len());
    for (step, request, origin) in plan {
        let outcome = match origin {
            None => ChainStepOutcome::Disabled,
            Some(_) if stopped => ChainStepOutcome::Skipped,
            Some(origin) => {
                let resolved = context.resolved_variables();
                match engine.resolve_url(&request, &resolved.values, &resolved.unresolved_secrets) {
                    Ok(url) if url.origin() == origin => {
                        match engine.send_with_secrets(&request, &resolved).await {
                            Ok(response) => match context.apply_step_response(&step, &response) {
                                Ok(()) => ChainStepOutcome::Passed {
                                    status: response.status,
                                },
                                Err(error) => ChainStepOutcome::CheckFailed(error),
                            },
                            Err(_) => ChainStepOutcome::RequestFailed,
                        }
                    }
                    _ => ChainStepOutcome::RequestFailed,
                }
            }
        };
        stopped |= matches!(
            outcome,
            ChainStepOutcome::CheckFailed(_) | ChainStepOutcome::RequestFailed
        );
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ApiRequest, AssertionRule, Auth, CaptureRule, CaptureSource, ChainStep, MemorySecretStore,
        RequestChain,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn validates_then_runs_in_order_and_stops_on_failed_assertion() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut project = Project::new("API");
        let login = ApiRequest::new("POST", "Login", format!("{origin}/login"));
        let mut profile = ApiRequest::new("GET", "Profile", format!("{origin}/profile"));
        profile.auth = Auth::Bearer {
            token: "{{token}}".into(),
        };
        let create = ApiRequest::new("POST", "Create post", format!("{origin}/posts"));
        let comment = ApiRequest::new(
            "POST",
            "Comment",
            format!("{origin}/posts/{{{{post_id}}}}/comments"),
        );
        let verify = ApiRequest::new(
            "GET",
            "Verify post",
            format!("{origin}/posts/{{{{post_id}}}}"),
        );
        let step = |index: usize, request: &ApiRequest| ChainStep {
            id: index.to_string(),
            request_id: request.id.clone(),
            enabled: true,
            assertions: Vec::new(),
            captures: Vec::new(),
        };
        let mut first = step(0, &login);
        first.captures.push(CaptureRule {
            name: "token".into(),
            source: CaptureSource::Header {
                name: "X-Token".into(),
            },
            required: true,
            sensitive: true,
        });
        let mut create_step = step(2, &create);
        create_step.captures.push(CaptureRule {
            name: "post_id".into(),
            source: CaptureSource::JsonField {
                pointer: "/id".into(),
            },
            required: true,
            sensitive: false,
        });
        let mut denied = step(5, &profile);
        denied
            .assertions
            .push(AssertionRule::Status { expected: 200 });
        let mut disabled = step(6, &profile);
        disabled.enabled = false;
        project.chains.push(RequestChain {
            id: "chain".into(),
            name: "Workflow".into(),
            steps: vec![
                first,
                step(1, &profile),
                create_step,
                step(3, &comment),
                step(4, &verify),
                denied,
                disabled,
                step(7, &profile),
            ],
        });
        project.collections[0].requests.push(login);
        let store = MemorySecretStore::new();
        assert!(matches!(
            run_chain(&project, "chain", None, &store, &origin, &[]).await,
            Err(ChainExecutionError::InvalidStep { index: 1, .. })
        ));
        project.collections[0]
            .requests
            .extend([profile, create, comment, verify]);
        let server = tokio::spawn(async move {
            for (start, reply, body, bearer) in [
                (
                    "POST /login HTTP/",
                    "HTTP/1.1 200 OK\r\nSet-Cookie: session=fixture\r\nX-Token: fixture-token",
                    "",
                    false,
                ),
                ("GET /profile HTTP/", "HTTP/1.1 200 OK", "", true),
                (
                    "POST /posts HTTP/",
                    "HTTP/1.1 201 Created",
                    r#"{"id":42}"#,
                    false,
                ),
                (
                    "POST /posts/42/comments HTTP/",
                    "HTTP/1.1 201 Created",
                    "",
                    false,
                ),
                ("GET /posts/42 HTTP/", "HTTP/1.1 200 OK", "", false),
                ("GET /profile HTTP/", "HTTP/1.1 403 Forbidden", "", true),
            ] {
                let (mut conn, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(conn.read_u8().await.unwrap());
                }
                let request = std::str::from_utf8(&request).unwrap();
                assert!(request.starts_with(start));
                if bearer {
                    assert!(request.contains("authorization: Bearer fixture-token\r\n"));
                }
                if !start.starts_with("POST /login") {
                    assert!(request.contains("cookie: session=fixture\r\n"));
                }
                conn.write_all(
                    format!(
                        "{reply}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            }
        });
        let outcomes = run_chain(&project, "chain", None, &store, &origin, &[])
            .await
            .unwrap();
        assert!(matches!(
            outcomes.as_slice(),
            [
                ChainStepOutcome::Passed { status: 200 },
                ChainStepOutcome::Passed { status: 200 },
                ChainStepOutcome::Passed { status: 201 },
                ChainStepOutcome::Passed { status: 201 },
                ChainStepOutcome::Passed { status: 200 },
                ChainStepOutcome::CheckFailed(_),
                ChainStepOutcome::Disabled,
                ChainStepOutcome::Skipped,
            ]
        ));
        server.await.unwrap();
    }
}
