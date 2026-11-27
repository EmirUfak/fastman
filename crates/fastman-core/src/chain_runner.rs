use crate::captures::{MAX_CAPTURE_NAME_BYTES, MAX_CAPTURES};
use crate::security::is_valid_http_method;
use crate::template::inspect_request;
use crate::{Auth, CaptureSource, ChainInputTarget, ChainStep, RequestChain};
use crate::{
    CaptureFailure, ChainRunContext, ChainRunError, HttpEngine, HttpError, Project, SecretStore,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::{Future, pending};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};

pub const MAX_CHAIN_STEPS: usize = 128;
const MAX_RUN_TIME: Duration = Duration::from_secs(300);

/// Check the saved links independently of the execution environment.
pub fn validate_chain_inputs(chain: &RequestChain) -> Result<(), ChainExecutionError> {
    input_sources(chain).map(|_| ())
}

fn input_sources(chain: &RequestChain) -> Result<BTreeSet<(String, String)>, ChainExecutionError> {
    let mut step_ids = BTreeSet::new();
    let mut earlier: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut sources = BTreeSet::new();
    for (index, step) in chain.steps.iter().enumerate() {
        let invalid = |reason| ChainExecutionError::InvalidStep { index, reason };
        if step.id.is_empty() || !step_ids.insert(step.id.as_str()) {
            return Err(invalid("missing or duplicate step ID"));
        }
        if !step.enabled {
            continue;
        }
        let mut targets = BTreeSet::new();
        for input in &step.inputs {
            if !targets.insert(&input.target) || targets.len() > MAX_CAPTURES {
                return Err(invalid("duplicate or too many input targets"));
            }
            if let ChainInputTarget::Variable { name } = &input.target
                && !valid_capture_name(name)
            {
                return Err(invalid("invalid input variable name"));
            }
            let Some(captures) = earlier.get(input.source_step_id.as_str()) else {
                return Err(invalid("input must come from an earlier enabled node"));
            };
            if !captures.contains(input.capture_name.as_str()) {
                return Err(invalid("input refers to a missing output"));
            }
            sources.insert((input.source_step_id.clone(), input.capture_name.clone()));
            if sources.len() > MAX_CAPTURES {
                return Err(invalid("too many linked outputs"));
            }
        }
        if targets.contains(&ChainInputTarget::BearerToken)
            && (targets.contains(&ChainInputTarget::BasicUsername)
                || targets.contains(&ChainInputTarget::BasicPassword))
        {
            return Err(invalid("choose either bearer or basic authentication"));
        }
        earlier.insert(
            step.id.as_str(),
            step.captures
                .iter()
                .map(|rule| rule.name.as_str())
                .collect::<BTreeSet<_>>(),
        );
    }
    Ok(sources)
}

fn valid_capture_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CAPTURE_NAME_BYTES
        && name.trim() == name
        && !name.contains(['{', '}'])
}

/// Existing Basic credentials can supply the other field in desktop sessions.
fn bind_auth(auth: &mut Auth, target: &ChainInputTarget, value: String) {
    match target {
        ChainInputTarget::BearerToken => *auth = Auth::Bearer { token: value },
        ChainInputTarget::BasicUsername | ChainInputTarget::BasicPassword => {
            if !matches!(auth, Auth::Basic { .. }) {
                *auth = Auth::Basic {
                    username: String::new(),
                    password: String::new(),
                };
            }
            if let Auth::Basic { username, password } = auth {
                if *target == ChainInputTarget::BasicUsername {
                    *username = value;
                } else {
                    *password = value;
                }
            }
        }
        ChainInputTarget::Variable { .. } => {}
    }
}

#[derive(Debug)]
pub enum ChainExecutionError {
    UnknownChain,
    InvalidLength,
    InvalidStep { index: usize, reason: &'static str },
    Context(ChainRunError),
    Session,
}

impl fmt::Display for ChainExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownChain => f.write_str("chain does not exist or its ID is ambiguous"),
            Self::InvalidLength => {
                write!(
                    f,
                    "chain must contain an enabled step and at most {MAX_CHAIN_STEPS} steps"
                )
            }
            Self::InvalidStep { index, reason } => {
                write!(f, "invalid chain step at index {index}: {reason}")
            }
            Self::Context(err) => err.fmt(f),
            Self::Session => f.write_str("could not initialize chain HTTP session"),
        }
    }
}

impl std::error::Error for ChainExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Context(err) => err.source(),
            _ => None,
        }
    }
}

impl From<ChainRunError> for ChainExecutionError {
    fn from(err: ChainRunError) -> Self {
        Self::Context(err)
    }
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
    run_chain_observed(
        project,
        chain_id,
        env_id,
        secret_store,
        options,
        cancel,
        |report, _| {
            progress(report);
        },
    )
    .await
}

/// Like `run_chain_with_progress`, with an in-process view of the live response.
/// Callers must keep response data out of logs and persisted reports.
pub async fn run_chain_observed(
    project: &Project,
    chain_id: &str,
    env_id: Option<&str>,
    secret_store: &dyn SecretStore,
    options: ChainRunOptions<'_>,
    cancel: impl Future<Output = ()>,
    mut progress: impl FnMut(&ChainStepReport, Option<&crate::HttpResponse>),
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
    let sources = input_sources(chain)?;
    context.retain_sources(sources.clone());
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
        let mut inspection_request = request.clone();
        let mut step_variables = available.clone();
        if step
            .inputs
            .iter()
            .any(|input| !matches!(input.target, ChainInputTarget::Variable { .. }))
        {
            inspection_request
                .headers
                .retain(|header| !header.key.trim().eq_ignore_ascii_case("authorization"));
        }
        for input in &step.inputs {
            if let ChainInputTarget::Variable { name } = &input.target {
                step_variables.insert(name.clone(), "fastman-capture".into());
            } else {
                bind_auth(
                    &mut inspection_request.auth,
                    &input.target,
                    "fastman-capture".into(),
                );
            }
        }
        if let Auth::Basic { username, password } = &inspection_request.auth
            && !step.inputs.is_empty()
            && !matches!(request.auth, Auth::Basic { .. })
            && (username.is_empty() || password.is_empty())
        {
            return Err(invalid(
                "bind both Basic fields when the saved request has no Basic auth",
            ));
        }
        let inspection = inspect_request(&inspection_request, &step_variables);
        if inspection.unclosed || !inspection.missing_variables.is_empty() {
            return Err(invalid("request has unresolved or malformed variables"));
        }
        if !is_valid_http_method(&request.method) {
            return Err(invalid("invalid HTTP method"));
        }
        // A placeholder permits path/query captures while keeping the approved
        // origin fixed. Check that origin again after substituting real values.
        let url = engine
            .resolve_url(request, &step_variables, &Default::default())
            .map_err(|_| invalid("invalid or unapproved request destination"))?;
        let mut step = step.clone();
        step.captures.retain(|rule| {
            !matches!(rule.source, CaptureSource::LinkedJsonField { .. })
                || sources.contains(&(step.id.clone(), rule.name.clone()))
        });
        let mut names = BTreeSet::new();
        for rule in &step.captures {
            if !valid_capture_name(&rule.name) || !names.insert(&rule.name) {
                return Err(invalid("invalid or duplicate capture name"));
            }
            capture_names.insert(rule.name.clone());
            if rule.required && !matches!(rule.source, CaptureSource::LinkedJsonField { .. }) {
                available.insert(rule.name.clone(), "fastman-capture".into());
            }
        }
        if capture_names.len() > MAX_CAPTURES {
            return Err(invalid("chain exceeds capture limit"));
        }
        plan.push((
            step,
            request.clone(),
            Some(url.origin().ascii_serialization()),
        ));
    }
    drop(available);

    let mut stopped = false;
    let mut aborted = false;
    let mut outcomes = Vec::with_capacity(plan.len());
    for (step, request, origin) in plan {
        let started = Instant::now();
        let mut failure = None;
        let mut preview = None;
        let mut captures = Vec::new();
        let mut received = None;
        let outcome = match origin {
            None => ChainStepOutcome::Disabled,
            Some(_) if aborted || (stopped && !step.always_run) => ChainStepOutcome::Skipped,
            Some(origin) => {
                let send = async {
                    if Instant::now() >= deadline {
                        return Err(ChainFailure::RunTimeout);
                    }
                    send_step(&engine, &context, request, &step, &origin).await
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
                        let status = response.status;
                        received = Some(response);
                        match checked {
                            _ if Instant::now() >= deadline => {
                                failure = Some(ChainFailure::RunTimeout);
                                ChainStepOutcome::RequestFailed
                            }
                            Ok(()) => {
                                captures.extend(
                                    chain.steps[outcomes.len()]
                                        .captures
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(index, rule)| {
                                            let active = !matches!(
                                                rule.source,
                                                CaptureSource::LinkedJsonField { .. }
                                            ) || sources
                                                .contains(&(step.id.clone(), rule.name.clone()));
                                            (active
                                                && context.has_step_capture(&step.id, &rule.name))
                                            .then_some(index)
                                        }),
                                );
                                ChainStepOutcome::Passed { status }
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
        if matches!(
            failure,
            Some(ChainFailure::Cancelled | ChainFailure::RunTimeout)
        ) {
            aborted = true;
        }
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
        progress(&report, received.as_ref());
        outcomes.push(report);
    }
    Ok(outcomes)
}

async fn send_step(
    engine: &HttpEngine,
    context: &ChainRunContext,
    mut request: crate::ApiRequest,
    step: &ChainStep,
    origin: &str,
) -> Result<crate::HttpResponse, ChainFailure> {
    let mut resolved = context.resolved_variables();
    let mut next_key = 0;
    for input in &step.inputs {
        let key = match &input.target {
            ChainInputTarget::Variable { name } => name.clone(),
            target => loop {
                let key = format!("__fastman_chain_auth_{next_key}");
                next_key += 1;
                if !resolved.values.contains_key(&key)
                    && !resolved.unresolved_secrets.contains_key(&key)
                    && !step.inputs.iter().any(|input| matches!(&input.target, ChainInputTarget::Variable { name } if name == &key))
                {
                    bind_auth(&mut request.auth, target, format!("{{{{{key}}}}}"));
                    break key;
                }
            },
        };
        context
            .bind_capture(&mut resolved, input, key)
            .ok_or(ChainFailure::Capture)?;
    }
    let url = engine
        .resolve_url(&request, &resolved.values, &resolved.unresolved_secrets)
        .map_err(|_| ChainFailure::Policy)?;
    if url.origin().ascii_serialization() != origin {
        return Err(ChainFailure::Policy);
    }
    let isolated;
    let engine = if step.isolate_cookies {
        isolated = engine
            .isolated_cookies()
            .map_err(|_| ChainFailure::Policy)?;
        &isolated
    } else {
        engine
    };
    engine
        .send_with_secrets(&request, &resolved)
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
    use crate::{ApiRequest, AssertionRule, CaptureRule, CaptureSource, MemorySecretStore};
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
                inputs: vec![],
                isolate_cookies: false,
                always_run: false,
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
            format!("{origin}/posts/{{{{comment_post}}}}/comments"),
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
            inputs: Vec::new(),
            isolate_cookies: false,
            always_run: false,
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
        first.captures.push(CaptureRule {
            name: "login_bearer".into(),
            source: CaptureSource::LinkedJsonField {
                pointer: "/auth/token".into(),
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
        create_step.captures.push(CaptureRule {
            name: "token".into(),
            source: CaptureSource::Header {
                name: "X-Token".into(),
            },
            required: true,
            sensitive: true,
        });
        create_step.captures.push(CaptureRule {
            name: "username".into(),
            source: CaptureSource::Header {
                name: "X-Username".into(),
            },
            required: true,
            sensitive: true,
        });
        let mut comment_step = step(3, &comment);
        for (capture, target) in [
            (
                "post_id",
                ChainInputTarget::Variable {
                    name: "comment_post".into(),
                },
            ),
            ("username", ChainInputTarget::BasicUsername),
            ("token", ChainInputTarget::BasicPassword),
        ] {
            comment_step.inputs.push(crate::ChainInput {
                source_step_id: create_step.id.clone(),
                capture_name: capture.into(),
                target,
            });
        }
        let mut verify_step = step(4, &verify);
        verify_step.isolate_cookies = true;
        verify_step.inputs.push(crate::ChainInput {
            source_step_id: first.id.clone(),
            capture_name: "login_bearer".into(),
            target: ChainInputTarget::BearerToken,
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
                comment_step,
                verify_step,
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
        project.chains[0].steps.swap(0, 4);
        assert!(matches!(
            run_chain(&project, "chain", None, &store, &origin, &[]).await,
            Err(ChainExecutionError::InvalidStep { index: 0, .. })
        ));
        project.chains[0].steps.swap(0, 4);
        let server = tokio::spawn(async move {
            for (start, reply, body, bearer) in [
                (
                    "POST /login HTTP/",
                    "HTTP/1.1 200 OK\r\nSet-Cookie: session=fixture\r\nX-Token: fixture-token",
                    r#"{"auth":{"token":"fixture-token"}}"#,
                    false,
                ),
                ("GET /profile HTTP/", "HTTP/1.1 200 OK", "", true),
                (
                    "POST /posts HTTP/",
                    "HTTP/1.1 201 Created\r\nX-Token: replacement-token\r\nX-Username: fixture-user",
                    r#"{"id":42}"#,
                    false,
                ),
                (
                    "POST /posts/42/comments HTTP/",
                    "HTTP/1.1 201 Created",
                    "",
                    false,
                ),
                (
                    "GET /posts/42 HTTP/",
                    "HTTP/1.1 200 OK\r\nSet-Cookie: isolated=fixture",
                    "",
                    true,
                ),
                ("GET /profile HTTP/", "HTTP/1.1 403 Forbidden", "", false),
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
                if reply.contains("403") {
                    assert!(request.contains("authorization: Bearer replacement-token\r\n"));
                }
                if start.contains("/comments") {
                    assert!(request.contains(
                        "authorization: Basic Zml4dHVyZS11c2VyOnJlcGxhY2VtZW50LXRva2Vu\r\n"
                    ));
                }
                if start.starts_with("GET /posts/42 HTTP/") {
                    assert!(!request.contains("cookie:"));
                } else if !start.starts_with("POST /login") {
                    assert!(request.contains("cookie: session=fixture\r\n"));
                    assert!(!request.contains("isolated=fixture"));
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

    #[tokio::test]
    async fn always_run_cleanup_runs_after_assertion_failure_and_validates_step_ids() {
        let mut invalid = RequestChain::new("Invalid");
        let mut s1 = ChainStep::new("req");
        s1.id = "dup".into();
        let mut s2 = ChainStep::new("req");
        s2.id = "dup".into();
        invalid.steps = vec![s1, s2];
        assert!(validate_chain_inputs(&invalid).is_err());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut project = Project::new("CRUD");
        let create = ApiRequest::new("POST", "Create", format!("{origin}/items"));
        let check = ApiRequest::new("GET", "Check", format!("{origin}/items/{{{{item_id}}}}"));
        let delete = ApiRequest::new(
            "DELETE",
            "Cleanup",
            format!("{origin}/items/{{{{item_id}}}}"),
        );
        project.collections[0]
            .requests
            .extend([create.clone(), check.clone(), delete.clone()]);

        let mut step_create = ChainStep::new(create.id.clone());
        step_create.captures.push(CaptureRule {
            name: "item_id".into(),
            source: CaptureSource::JsonField {
                pointer: "/id".into(),
            },
            required: true,
            sensitive: false,
        });
        let mut step_check = ChainStep::new(check.id.clone());
        step_check
            .assertions
            .push(AssertionRule::Status { expected: 200 });
        let mut step_delete = ChainStep::new(delete.id.clone());
        step_delete.always_run = true;
        step_delete
            .assertions
            .push(AssertionRule::Status { expected: 204 });

        project.chains.push(RequestChain {
            id: "crud".into(),
            name: "CRUD".into(),
            steps: vec![step_create, step_check, step_delete],
        });

        let server = tokio::spawn(async move {
            for (expected_prefix, response) in [
                (
                    "POST /items HTTP/",
                    "HTTP/1.1 201 Created\r\nConnection: close\r\nContent-Length: 10\r\n\r\n{\"id\":777}",
                ),
                (
                    "GET /items/777 HTTP/",
                    "HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                ),
                (
                    "DELETE /items/777 HTTP/",
                    "HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                ),
            ] {
                let (mut conn, _) = listener.accept().await.unwrap();
                let mut req = Vec::new();
                while !req.ends_with(b"\r\n\r\n") {
                    req.push(conn.read_u8().await.unwrap());
                }
                assert!(
                    std::str::from_utf8(&req)
                        .unwrap()
                        .starts_with(expected_prefix)
                );
                conn.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let outcomes = run_chain(
            &project,
            "crud",
            None,
            &MemorySecretStore::new(),
            &origin,
            &[],
        )
        .await
        .unwrap();
        assert!(matches!(
            outcomes.as_slice(),
            [
                ChainStepOutcome::Passed { status: 201 },
                ChainStepOutcome::CheckFailed(_),
                ChainStepOutcome::Passed { status: 204 },
            ]
        ));
        server.await.unwrap();
    }
}
