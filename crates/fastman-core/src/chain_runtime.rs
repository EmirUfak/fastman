use crate::captures::{CaptureFailure, CapturedValue, MAX_CAPTURES, capture_step_response};
use crate::{ChainStep, HttpResponse, Project, ResolvedVariables, SecretStore};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Private state for one run. Never serialized, logged, or shared between runs.
pub struct ChainRunContext {
    base: ResolvedVariables,
    captures: BTreeMap<String, RunCapture>,
}

struct RunCapture {
    source_step_id: String,
    captured: CapturedValue,
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum ChainRunError {
    #[error("selected environment does not exist in this project")]
    UnknownEnvironment,
    #[error(transparent)]
    Capture(#[from] CaptureFailure),
    #[error("run capture count exceeds {MAX_CAPTURES}")]
    TooManyCaptures,
    #[error("step captured the same variable more than once")]
    DuplicateCapture,
}

impl ChainRunContext {
    /// Snapshot base variables and the explicitly selected environment. `None` uses project only.
    pub fn new(
        project: &Project,
        env_id: Option<&str>,
        secret_store: &dyn SecretStore,
    ) -> Result<Self, ChainRunError> {
        if env_id.is_some_and(|id| project.find_environment(id).is_none()) {
            return Err(ChainRunError::UnknownEnvironment);
        }
        Ok(Self {
            base: project.resolve_variables(env_id, secret_store),
            captures: BTreeMap::new(),
        })
    }

    /// Build inputs for the next request without altering the run snapshot.
    pub fn resolved_variables(&self) -> ResolvedVariables {
        let mut resolved = self.base.clone();
        for (name, entry) in &self.captures {
            let value = &entry.captured.value;
            let text = match value.as_str() {
                Some(text) => text.to_owned(),
                None => value.to_string(),
            };
            resolved.values.insert(name.clone(), text);
            resolved.unresolved_secrets.remove(name);
            if entry.captured.sensitive {
                resolved.secret_names.insert(name.clone());
            } else {
                resolved.secret_names.remove(name);
            }
        }
        resolved
    }

    pub fn capture_origin(&self, name: &str) -> Option<&str> {
        self.captures
            .get(name)
            .map(|entry| entry.source_step_id.as_str())
    }

    /// Commit one executed step's captures atomically, only after assertions pass.
    pub fn apply_step_response(
        &mut self,
        step: &ChainStep,
        response: &HttpResponse,
    ) -> Result<(), ChainRunError> {
        let staged = capture_step_response(step, response)?;
        let mut names = BTreeSet::new();
        let mut total = self.captures.len();
        for capture in &staged {
            if !names.insert(&capture.name) {
                return Err(ChainRunError::DuplicateCapture);
            }
            total += usize::from(!self.captures.contains_key(&capture.name));
        }
        if total > MAX_CAPTURES {
            return Err(ChainRunError::TooManyCaptures);
        }
        for captured in staged {
            self.captures.insert(
                captured.name.clone(),
                RunCapture {
                    source_step_id: step.id.clone(),
                    captured,
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CaptureRule, CaptureSource, Environment, MemorySecretStore, SecretRef};

    fn step(names: impl IntoIterator<Item = String>) -> ChainStep {
        ChainStep {
            id: "login".into(),
            request_id: "request".into(),
            enabled: true,
            assertions: vec![],
            captures: names
                .into_iter()
                .map(|name| CaptureRule {
                    name,
                    source: CaptureSource::Header {
                        name: "token".into(),
                    },
                    required: true,
                    sensitive: true,
                })
                .collect(),
        }
    }

    fn response(token: &str) -> HttpResponse {
        HttpResponse {
            status: 200,
            reason: "OK".into(),
            ttfb_ms: 1,
            download_ms: 0,
            elapsed_ms: 1,
            final_url: "https://example.com/".into(),
            headers: vec![("Token".into(), token.into())],
            body: String::new(),
            bytes_received: 0,
            truncated: false,
        }
    }

    #[test]
    fn snapshots_variables_and_overrides_unresolved_secrets() {
        let mut project = Project::new("API");
        project.variables.insert("host".into(), "base".into());
        project.secret_variables.insert(
            "token".into(),
            SecretRef::Keychain {
                key: "fixture-key".into(),
            },
        );
        let mut env = Environment::new("Test");
        env.variables.insert("host".into(), "environment".into());
        let env_id = env.id.clone();
        project.environments.push(env);
        let store = MemorySecretStore::new();
        let mut run = ChainRunContext::new(&project, Some(&env_id), &store).unwrap();
        project.environments.clear();
        assert_eq!(run.resolved_variables().values["host"], "environment");
        assert!(
            run.resolved_variables()
                .unresolved_secrets
                .contains_key("token")
        );
        run.apply_step_response(
            &step(["host".into(), "token".into()]),
            &response("captured"),
        )
        .unwrap();
        let variables = run.resolved_variables();
        assert_eq!(variables.values["host"], "captured");
        assert_eq!(variables.values["token"], "captured");
        assert!(variables.unresolved_secrets.is_empty());
        assert!(variables.secret_names.contains("token"));
        assert_eq!(run.capture_origin("token"), Some("login"));
        assert_eq!(
            ChainRunContext::new(&project, Some(&env_id), &store).err(),
            Some(ChainRunError::UnknownEnvironment)
        );
    }

    #[test]
    fn isolates_parallel_runs_and_projects() {
        let mut first = Project::new("First");
        first.variables.insert("token".into(), "first base".into());
        let mut second = Project::new("Second");
        second
            .variables
            .insert("token".into(), "second base".into());
        let store = MemorySecretStore::new();
        std::thread::scope(|scope| {
            for (project, token) in [(&first, "a"), (&first, "b"), (&second, "c")] {
                let store = &store;
                scope.spawn(move || {
                    let mut run = ChainRunContext::new(project, None, store).unwrap();
                    run.apply_step_response(&step(["token".into()]), &response(token))
                        .unwrap();
                    assert_eq!(run.resolved_variables().values["token"], token);
                });
            }
        });
        for project in [&first, &second] {
            let run = ChainRunContext::new(project, None, &store).unwrap();
            assert_eq!(run.resolved_variables().values, project.variables);
            assert_eq!(run.capture_origin("token"), None);
        }
    }

    #[test]
    fn rejects_oversized_and_duplicate_batches_atomically() {
        let project = Project::new("API");
        let mut run = ChainRunContext::new(&project, None, &MemorySecretStore::new()).unwrap();
        let full = step((0..MAX_CAPTURES).map(|i| format!("token_{i}")));
        run.apply_step_response(&full, &response("original"))
            .unwrap();
        assert_eq!(
            run.apply_step_response(&step(["token_0".into(), "extra".into()]), &response("new")),
            Err(ChainRunError::TooManyCaptures)
        );
        assert_eq!(
            run.apply_step_response(
                &step(["token_0".into(), "token_0".into()]),
                &response("new")
            ),
            Err(ChainRunError::DuplicateCapture)
        );
        assert!(
            run.apply_step_response(&step(["token_0".into()]), &response(&"a".repeat(17 * 1024)))
                .is_err()
        );
        assert_eq!(run.resolved_variables().values["token_0"], "original");
        assert_eq!(run.capture_origin("extra"), None);
    }
}
