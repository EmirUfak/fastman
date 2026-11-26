use crate::captures::{MAX_CAPTURE_NAME_BYTES, MAX_CAPTURES};
use crate::security::is_valid_http_method;
use crate::template::inspect_request;
use crate::{
    CaptureFailure, ChainRunContext, ChainRunError, HttpEngine, HttpError, Project, SecretStore,
};
use std::collections::BTreeSet;
use std::future::{Future, pending};
use std::time::Duration;
use thiserror::Error;
use tokio::time::{Instant, sleep_until};

pub const MAX_CHAIN_STEPS: usize = 128;
const MAX_RUN_TIME: Duration = Duration::from_secs(300);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainStepOutcome {
    Passed { status: u16 },
    RequestFailed,
    CheckFailed(ChainRunError),
    Disabled,
    Skipped,
}

pub struct ChainRunOptions<'a> {
    pub primary_origin: &'a str,
    pub trusted_origins: &'a [String],
    /// Budget for the whole run, capped at five minutes. Zero expires immediately.
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainFailure {
    Transport,
    Assertion,
    Capture,
    Policy,
    RequestTimeout,
    RunTimeout,
    Cancelled,
}

/// Metadata-only preview. Server-controlled text is always withheld, including
/// previously unknown credentials in response bodies and headers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChainResponsePreview {
    pub status: u16,
    pub bytes_received: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainStepReport {
    pub outcome: ChainStepOutcome,
    pub failure: Option<ChainFailure>,
    pub elapsed: Duration,
    pub response: Option<ChainResponsePreview>,
    /// Indices into the step's configured capture rules. No values are retained.
    pub captures: Vec<usize>,
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
    Ok(run_chain_with_report(
        project,
        chain_id,
        env_id,
        secret_store,
        ChainRunOptions {
            primary_origin,
            trusted_origins,
            timeout: MAX_RUN_TIME,
        },
        pending(),
    )
    .await?
    .into_iter()
    .map(|step| step.outcome)
    .collect())
}

/// Outcomes align with definition indices. Resolving `cancel` stops the active
/// request and skips subsequent enabled steps. Dropping the run future also
/// releases its session. Neither action can undo server-side effects.
pub async fn run_chain_with_report(
    project: &Project,
    chain_id: &str,
    env_id: Option<&str>,
    secret_store: &dyn SecretStore,
    options: ChainRunOptions<'_>,
    cancel: impl Future<Output = ()>,
) -> Result<Vec<ChainStepReport>, ChainExecutionError> {
    run_chain_with_progress(
        project,
        chain_id,
        env_id,
        secret_store,
        options,
        cancel,
        |_| {},
    )
    .await
}

/// Report each finished step in order, including disabled and skipped steps.
pub async fn run_chain_with_progress(
    project: &Project,
    chain_id: &str,
    env_id: Option<&str>,
    secret_store: &dyn SecretStore,
    options: ChainRunOptions<'_>,
    cancel: impl Future<Output = ()>,
    mut progress: impl FnMut(&ChainStepReport),
) -> Result<Vec<ChainStepReport>, ChainExecutionError> {
    let deadline = Instant::now() + options.timeout.min(MAX_RUN_TIME);
    let deadline_timer = sleep_until(deadline);
    tokio::pin!(cancel, deadline_timer);
    let mut matches = project.chains.iter().filter(|chain| chain.id == chain_id);
    let chain = matches.next().ok_or(ChainExecutionError::UnknownChain)?;
    if matches.next().is_some() {
        return Err(ChainExecutionError::UnknownChain);
    }
    if chain.steps.len() > MAX_CHAIN_STEPS || !chain.steps.iter().any(|step| step.enabled) {
        return Err(ChainExecutionError::InvalidLength);
    }
    let mut context = ChainRunContext::new(project, env_id, secret_store)?;
    let engine = HttpEngine::for_chain(options.primary_origin, options.trusted_origins)
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
        plan.push((
            step.clone(),
            request.clone(),
            Some(url.origin().ascii_serialization()),
        ));
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
        let started = Instant::now();
        let mut failure = None;
        let mut preview = None;
        let mut captures = Vec::new();
        let outcome = match origin {
            None => ChainStepOutcome::Disabled,
            Some(_) if stopped => ChainStepOutcome::Skipped,
            Some(origin) => {
                let send = async {
                    if Instant::now() >= deadline {
                        return Err(ChainFailure::RunTimeout);
                    }
                    send_step(&engine, &context, &request, &origin).await
                };
                let result = tokio::select! {
                    biased;
                    () = &mut cancel => Err(ChainFailure::Cancelled),
                    () = &mut deadline_timer => Err(ChainFailure::RunTimeout),
                    result = send => result,
                };
                match result {
                    Ok(response) => {
                        preview = Some(ChainResponsePreview {
                            status: response.status,
                            bytes_received: response.bytes_received,
                            truncated: response.truncated,
                        });
                        let checked = context.apply_step_response(&step, &response);
                        match checked {
                            _ if Instant::now() >= deadline => {
                                failure = Some(ChainFailure::RunTimeout);
                                ChainStepOutcome::RequestFailed
                            }
                            Ok(()) => {
                                captures.extend(step.captures.iter().enumerate().filter_map(
                                    |(index, rule)| {
                                        (context.capture_origin(&rule.name)
                                            == Some(step.id.as_str()))
                                        .then_some(index)
                                    },
                                ));
                                ChainStepOutcome::Passed {
                                    status: response.status,
                                }
                            }
                            Err(error) => {
                                failure = Some(match &error {
                                    ChainRunError::Capture(CaptureFailure::Assertion(_)) => {
                                        ChainFailure::Assertion
                                    }
                                    _ => ChainFailure::Capture,
                                });
                                ChainStepOutcome::CheckFailed(error)
                            }
                        }
                    }
                    Err(reason) => {
                        failure = Some(reason);
                        ChainStepOutcome::RequestFailed
                    }
                }
            }
        };
        stopped |= failure.is_some();
        let report = ChainStepReport {
            elapsed: if matches!(
                outcome,
                ChainStepOutcome::Disabled | ChainStepOutcome::Skipped
            ) {
                Duration::ZERO
            } else {
                started.elapsed()
            },
            outcome,
            failure,
            response: preview,
            captures,
        };
        progress(&report);
        outcomes.push(report);
    }
    Ok(outcomes)
}

async fn send_step(
    engine: &HttpEngine,
    context: &ChainRunContext,
    request: &crate::ApiRequest,
    origin: &str,
) -> Result<crate::HttpResponse, ChainFailure> {
    let resolved = context.resolved_variables();
    let url = engine
        .resolve_url(request, &resolved.values, &resolved.unresolved_secrets)
        .map_err(|_| ChainFailure::Policy)?;
    if url.origin().ascii_serialization() != origin {
        return Err(ChainFailure::Policy);
    }
    engine
        .send_with_secrets(request, &resolved)
        .await
        .map_err(|error| match error {
            HttpError::Transport(error) if error.is_timeout() => ChainFailure::RequestTimeout,
            HttpError::Transport(error) if !error.is_redirect() => ChainFailure::Transport,
            _ => ChainFailure::Policy,
        })
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

    fn two_step_project(origin: &str, timeout_ms: u64) -> Project {
        let mut project = Project::new("API");
        let mut request =
            ApiRequest::new("GET", "Fixture", format!("{origin}/?key=fixture-secret"));
        request.timeout_ms = timeout_ms;
        let steps = (0..2)
            .map(|index| ChainStep {
                id: index.to_string(),
                request_id: request.id.clone(),
                enabled: true,
                assertions: vec![],
                captures: vec![],
            })
            .collect();
        project.collections[0].requests.push(request);
        project.chains.push(RequestChain {
            id: "fixture".into(),
            name: "Fixture".into(),
            steps,
        });
        project
    }

    #[tokio::test]
    async fn cancellation_and_deadlines_close_the_active_response_and_skip_the_next_step() {
        for (expected, timeout_ms, budget) in [
            (ChainFailure::Cancelled, 5_000, Duration::from_secs(5)),
            (ChainFailure::RunTimeout, 5_000, Duration::from_millis(250)),
            (ChainFailure::RequestTimeout, 250, Duration::from_secs(5)),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let project = two_step_project(&origin, timeout_ms);
            let (sent, received) = tokio::sync::oneshot::channel();
            let server = async {
                let (mut conn, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(conn.read_u8().await.unwrap());
                }
                conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nfixture-secret")
                    .await
                    .unwrap();
                sent.send(()).unwrap();
                assert_eq!(conn.read(&mut [0]).await.unwrap(), 0);
            };
            let cancel_run = expected == ChainFailure::Cancelled;
            let cancel = async {
                received.await.unwrap();
                if !cancel_run {
                    pending::<()>().await;
                }
            };
            let store = MemorySecretStore::new();
            let run = run_chain_with_report(
                &project,
                "fixture",
                None,
                &store,
                ChainRunOptions {
                    primary_origin: &origin,
                    trusted_origins: &[],
                    timeout: budget,
                },
                cancel,
            );
            let (report, ()) =
                tokio::time::timeout(Duration::from_secs(6), async { tokio::join!(run, server) })
                    .await
                    .unwrap();
            let report = report.unwrap();
            assert!(!format!("{report:?}").contains("fixture-secret"));
            assert_eq!(report[0].failure, Some(expected));
            assert_eq!(report[1].outcome, ChainStepOutcome::Skipped);
        }
    }

    #[tokio::test]
    async fn oversized_response_keeps_only_metadata_after_partial_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut project = two_step_project(&origin, 5_000);
        project.chains[0].steps[1]
            .assertions
            .push(AssertionRule::Status { expected: 204 });
        let server = tokio::spawn(async move {
            for body in [String::new(), "fixture-secret".repeat(200_000)] {
                let (mut conn, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(conn.read_u8().await.unwrap());
                }
                conn.write_all(format!(
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nSet-Cookie: token=fixture-secret\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                ).as_bytes()).await.unwrap();
                // The client may close as soon as it reaches the response cap.
                let _ = conn.write_all(body.as_bytes()).await;
            }
        });
        let report = run_chain_with_report(
            &project,
            "fixture",
            None,
            &MemorySecretStore::new(),
            ChainRunOptions {
                primary_origin: &origin,
                trusted_origins: &[],
                timeout: Duration::from_secs(5),
            },
            pending(),
        )
        .await
        .unwrap();
        assert_eq!(report[0].outcome, ChainStepOutcome::Passed { status: 200 });
        assert_eq!(report[1].failure, Some(ChainFailure::Assertion));
        assert_eq!(
            report[1].response,
            Some(ChainResponsePreview {
                status: 200,
                bytes_received: 2 * 1024 * 1024,
                truncated: true,
            })
        );
        assert!(!format!("{report:?}").contains("fixture-secret"));
        server.await.unwrap();
    }

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
