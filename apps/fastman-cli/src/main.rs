use anyhow::{Context, Result, bail};
use fastman_core::{
    HttpEngine, ProjectStore,
    import::{collection_v21, read_collection_file},
    security::{sanitize_terminal_body, sanitize_terminal_line, summarize_sensitive_findings},
};
use std::path::Path;

fn usage() {
    eprintln!(
        "Fastman CLI v{}\n\n  fastman-cli list\n  fastman-cli import-collection [--strip-secrets | --allow-plaintext-secrets] <export.json>\n  fastman-cli send [--raw] <project-id> <request-id>\n  fastman-cli --version\n  fastman-cli --help\n\nProject data: FASTMAN_HOME or the OS local data directory /fastman",
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        usage();
        return Ok(());
    };
    if matches!(command.as_str(), "--version" | "-V") {
        println!("fastman-cli {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if matches!(command.as_str(), "--help" | "-h") {
        usage();
        return Ok(());
    }
    let store = ProjectStore::system_default()?;
    match command.as_str() {
        "list" => {
            let (projects, warnings) = store.list_resilient()?;
            for warning in warnings {
                eprintln!("Warning: {}", sanitize_terminal_line(&warning));
            }
            for project in projects {
                println!("{}\t{}", project.id, sanitize_terminal_line(&project.name));
            }
        }
        "import-collection" => {
            let mut strip_secrets = false;
            let mut allow_plaintext = false;
            let mut path: Option<String> = None;
            for arg in args {
                match arg.as_str() {
                    "--strip-secrets" => strip_secrets = true,
                    "--allow-plaintext-secrets" => allow_plaintext = true,
                    s if s.starts_with('-') => bail!("Unknown option for import-collection: {s}"),
                    _ if path.is_none() => path = Some(arg),
                    extra => bail!("Unexpected argument for import-collection: {extra}"),
                }
            }
            if strip_secrets && allow_plaintext {
                bail!("--strip-secrets and --allow-plaintext-secrets cannot be combined");
            }
            let path = path.context("Missing collection JSON filename")?;
            let json = read_collection_file(Path::new(&path))?;
            let mut report = collection_v21(&json)?;
            if !report.sensitive_fields.is_empty() {
                if strip_secrets {
                    let removed = report.sanitize_credentials();
                    eprintln!(
                        "Notice: moved {} recognized auth field(s) to session-only memory (not saved) and stripped {} sensitive field(s) before saving.",
                        removed.moved_to_session_auth, removed.stripped_fields
                    );
                } else {
                    let summary_text = sanitize_terminal_line(&summarize_sensitive_findings(
                        &report.sensitive_fields,
                    ));
                    let count = report.sensitive_fields.len();
                    if !allow_plaintext {
                        bail!(
                            "Refusing to persist {count} sensitive field(s) in plaintext ({summary_text}). Re-run with --strip-secrets or --allow-plaintext-secrets."
                        );
                    }
                    eprintln!(
                        "Warning: persisting {count} sensitive field(s) in plaintext project.json: {summary_text}"
                    );
                }
            }
            store.save(&report.project)?;
            println!(
                "Imported {} request(s) into {} ({})",
                report.requests_imported,
                sanitize_terminal_line(&report.project.name),
                report.project.id
            );
            for warning in report.warnings {
                eprintln!("Warning: {}", sanitize_terminal_line(&warning));
            }
        }
        "send" => {
            let mut raw_output = false;
            let mut positional = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "--raw" => raw_output = true,
                    s if s.starts_with('-') => bail!("Unknown option for send: {s}"),
                    _ => positional.push(arg),
                }
            }
            let mut pos_iter = positional.into_iter();
            let id = pos_iter.next().context("Missing project ID")?;
            let request_id = pos_iter.next().context("Missing request ID")?;
            if let Some(extra) = pos_iter.next() {
                bail!("Unexpected argument for send: {extra}");
            }
            let project = store.load(&id)?;
            let request = project
                .find_request(&request_id)
                .context("Request not found")?;
            if !request.verify_tls {
                eprintln!("Warning: TLS certificate verification is disabled for this request.");
            }
            let result = HttpEngine::new()?.send(request, &project.variables).await?;
            let body_out = if raw_output {
                result.body
            } else {
                sanitize_terminal_body(&result.body)
            };
            println!(
                "HTTP {} {} · {} ms (TTFB {} ms · download {} ms) · {} bytes{}\n{}",
                result.status,
                sanitize_terminal_line(&result.reason),
                result.elapsed_ms,
                result.ttfb_ms,
                result.download_ms,
                result.bytes_received,
                if result.truncated { " (truncated)" } else { "" },
                body_out
            );
        }
        _ => {
            usage();
            bail!("Unknown command: {}", sanitize_terminal_line(&command));
        }
    }
    Ok(())
}
