use anyhow::{Context, Result, bail};
use fastman_core::{
    ChainFailure, ChainRunOptions, ChainStepOutcome, OsSecretStore, ProjectStore,
    run_chain_with_report,
};
use serde_json::json;
use std::future::pending;
use std::time::Duration;

pub(super) async fn run(mut args: impl Iterator<Item = String>) -> Result<i32> {
    if args.next().as_deref() != Some("run") {
        bail!("Expected `chain run` (see --help)");
    }
    let (mut project, mut chain, mut origin, mut environment, mut workspace) =
        (None, None, None, None, None);
    let mut trusted_origins = Vec::new();
    let mut json_output = false;
    while let Some(flag) = args.next() {
        let target = match flag.as_str() {
            "--project" => &mut project,
            "--chain" => &mut chain,
            "--origin" => &mut origin,
            "--env" => &mut environment,
            "--workspace" => &mut workspace,
            "--trust-origin" => {
                trusted_origins.push(flag_value(&mut args)?);
                continue;
            }
            "--json" if !json_output => {
                json_output = true;
                continue;
            }
            _ => bail!("Unknown or repeated chain option (see --help)"),
        };
        if target.is_some() {
            bail!("Repeated chain option (see --help)");
        }
        *target = Some(flag_value(&mut args)?);
    }
    let project_id = project.context("Missing --project")?;
    let chain_id = chain.context("Missing --chain")?;
    let origin = origin.context("Missing --origin")?;
    let project = ProjectStore::system_default()
        .and_then(|store| store.for_workspace(workspace.as_deref().unwrap_or("default")))
        .and_then(|store| store.load(&project_id))
        .map_err(|_| anyhow::anyhow!("Could not load or validate project"))?;
    let env_id = environment
        .as_deref()
        .map(|name| project.find_environment_by_name_or_id(name))
        .transpose()
        .map_err(|_| anyhow::anyhow!("Environment not found or ambiguous"))?
        .map(|env| env.id.as_str());
    let reports = run_chain_with_report(
        &project,
        &chain_id,
        env_id,
        &OsSecretStore::default(),
        ChainRunOptions {
            primary_origin: &origin,
            trusted_origins: &trusted_origins,
            timeout: Duration::from_secs(300),
        },
        pending(),
    )
    .await?;
    let mut exit_code = 0;
    let mut steps = Vec::new();
    for (index, report) in reports.iter().enumerate() {
        let state = match &report.outcome {
            ChainStepOutcome::Passed { .. } => "passed",
            ChainStepOutcome::RequestFailed | ChainStepOutcome::CheckFailed(_) => "failed",
            ChainStepOutcome::Disabled => "disabled",
            ChainStepOutcome::Skipped => "skipped",
        };
        if let Some(failure) = report.failure {
            exit_code = match failure {
                ChainFailure::Policy => 2,
                ChainFailure::Transport
                | ChainFailure::RequestTimeout
                | ChainFailure::RunTimeout
                | ChainFailure::Cancelled => 3,
                ChainFailure::Assertion => 4,
                ChainFailure::Capture => 5,
            };
        }
        let detail = match &report.outcome {
            ChainStepOutcome::CheckFailed(error) => Some(error.to_string()),
            _ => None,
        };
        if json_output {
            steps.push(json!({
                "index": index, "state": state, "failure": report.failure,
                "elapsed_ms": report.elapsed.as_millis(), "response": report.response, "detail": detail,
            }));
        } else {
            print!(
                "Step {}: {state} · {} ms",
                index + 1,
                report.elapsed.as_millis()
            );
            if let Some(response) = &report.response {
                print!(" · HTTP {}", response.status);
                if response.truncated {
                    print!(" · response truncated");
                }
            }
            if let Some(failure) = report.failure {
                print!(" · {failure:?}");
            }
            if let Some(detail) = detail {
                print!(" · {detail}");
            }
            println!();
        }
    }
    if json_output {
        println!("{}", json!({ "exit_code": exit_code, "steps": steps }));
    }
    Ok(exit_code)
}

fn flag_value(args: &mut impl Iterator<Item = String>) -> Result<String> {
    args.next()
        .filter(|value| !value.starts_with("--") && !value.trim().is_empty())
        .context("Missing chain option value (see --help)")
}
