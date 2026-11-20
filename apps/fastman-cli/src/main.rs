use anyhow::{Context, Result, bail};
use fastman_core::{HttpEngine, ProjectStore, import::postman_v21};

fn usage() {
    eprintln!(
        "Fastman CLI v{}\n\n  fastman-cli list\n  fastman-cli import-postman <export.json>\n  fastman-cli send <project-id> <request-id>\n  fastman-cli --version\n  fastman-cli --help\n\nProject data: FASTMAN_HOME or the OS local data directory /fastman",
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
            for project in store.list()? {
                println!("{}\t{}", project.id, project.name);
            }
        }
        "import-postman" => {
            let path = args.next().context("Missing Postman JSON filename")?;
            let bytes = std::fs::read_to_string(&path).with_context(|| format!("Read {path}"))?;
            let report = postman_v21(&bytes)?;
            store.save(&report.project)?;
            println!(
                "Imported {} request(s) into {} ({})",
                report.requests_imported, report.project.name, report.project.id
            );
            for warning in report.warnings {
                eprintln!("Warning: {warning}");
            }
        }
        "send" => {
            let id = args.next().context("Missing project ID")?;
            let request_id = args.next().context("Missing request ID")?;
            let project = store.load(&id)?;
            let request = project
                .find_request(&request_id)
                .context("Request not found")?;
            let result = HttpEngine::new()?.send(request, &project.variables).await?;
            println!(
                "HTTP {} {} · {} ms · {} bytes{}\n{}",
                result.status,
                result.reason,
                result.elapsed_ms,
                result.bytes_received,
                if result.truncated { " (truncated)" } else { "" },
                result.body
            );
        }
        _ => {
            usage();
            bail!("Unknown command: {command}");
        }
    }
    Ok(())
}
