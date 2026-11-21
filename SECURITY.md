# Security Policy

## Supported Versions

Fastman is currently in pre-1.0 development (`0.1.x`). Security fixes target the latest `0.1.x` release on the default branch.

| Version | Supported |
|---|---|
| 0.1.x | Yes |
| < 0.1.0 | No |

## Reporting a Vulnerability

Please **do not** open a public GitHub issue for suspected security vulnerabilities (credential leaks, TLS bypasses, path traversal, parser crashes, or code execution via imports).

Instead, report vulnerabilities privately to the repository maintainers via GitHub Security Advisories (Private Vulnerability Reporting) if enabled on the repository, or contact the maintainers directly with:

- A description of the issue and affected crate (`fastman-core`, `fastman-cli`, or `fastman-desktop`).
- Steps to reproduce or a minimal proof of concept.
- Potential impact and any suggested mitigation.

## Security Model & Known Caveats

- `Auth` credentials are excluded from serialized `project.json` (`#[serde(skip)]`).
- Custom headers, query params, bodies, and variable values **are** persisted locally; avoid storing secrets there until secure variable storage is implemented.
- **Imports:** Postman and cURL imports validate HTTP methods and `http`/`https` schemes, enforce file-size and nesting caps, and detect sensitive headers/query params/URL userinfo before saving (moving recognized credentials to session-only `Auth` or stripping/requiring explicit opt-in).
- **Redirects & TLS:** TLS certificate verification is enabled. Redirects do not emit `Referer` headers and stop automatically before cross-origin hops or HTTPS-to-HTTP downgrades.
- **Clipboard & Terminal:** cURL export redacts credentials and keeps `{{variables}}` unexpanded by default (full export requires explicit confirmation), validates/quotes HTTP methods, uses `--url` / `--data-raw`, and escapes control characters. CLI output escapes terminal control sequences unless `--raw` is explicitly passed.
- **Storage permissions:** Directories and files are created with private Unix modes (`0o700` / `0o600`), bounded read limits, save locks, conflict detection, and backup recovery. Filesystem permissions are not encryption at rest.
- Imported Postman collections and scripts are treated as untrusted data and never executed.
