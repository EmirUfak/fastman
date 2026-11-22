# Security Policy

## Supported Versions

Fastman is currently in pre-1.0 development (`0.2.x`). Security fixes target the latest `0.2.x` release on the default branch.

| Version | Supported |
|---|---|
| 0.2.x | Yes |
| < 0.2.0 | No |

## Reporting a Vulnerability

Please **do not** open a public GitHub issue for suspected security vulnerabilities (credential leaks, TLS bypasses, path traversal, parser crashes, or code execution via imports).

Instead, report it privately through **[GitHub Private Vulnerability Reporting](https://github.com/EmirUfak/fastman/security/advisories/new)**. On the repository's **Security** tab, choose **Report a vulnerability**. Only the maintainer can see the report. Please include:

- A description of the issue and the affected crate (`fastman-core`, `fastman-cli`, or `fastman-desktop`).
- The Fastman version or commit, and your OS.
- Steps to reproduce, or a minimal proof of concept. Use test data only, never real credentials.
- The potential impact and any mitigation you suggest.

### What to expect

- **Acknowledgement:** reports are usually acknowledged within 7 days. This is a goal, not a guarantee.
- **Next steps:** once the issue is confirmed, any follow-up questions and the fix plan are discussed with you in the private advisory.
- **Coordinated disclosure:** please keep the issue private until a fix is released or a disclosure date is agreed. When the fix ships, a GitHub Security Advisory is published and you're credited, unless you'd rather stay anonymous.
- **Out of scope:** risks the security model below already documents, such as plaintext persistence of custom headers or an explicitly disabled TLS check. Reports showing these protections can be bypassed are in scope.

## Security Model & Known Caveats

- `Auth` credentials are excluded from serialized `project.json` (`#[serde(skip)]`).
- Custom headers, query params, bodies, and variable values **are** persisted locally; avoid storing secrets there until secure variable storage is implemented.
- **Imports:** Collection v2.1 JSON and cURL imports validate HTTP methods and `http`/`https` schemes, enforce file-size and nesting caps, and detect sensitive headers/query params/URL userinfo before saving (moving recognized credentials to session-only `Auth` or stripping/requiring explicit opt-in).
- **Redirects & TLS:** TLS certificate verification is enabled by default and never silently disabled by imports (explicit per-request opt-out is available for local/self-signed development endpoints). Redirects do not emit `Referer` headers and stop automatically before cross-origin hops or HTTPS-to-HTTP downgrades.
- **Clipboard & Terminal:** cURL export redacts credentials and keeps `{{variables}}` unexpanded by default (full export requires explicit confirmation), validates/quotes HTTP methods, uses `--url` / `--data-raw`, and escapes control characters. CLI output escapes terminal control sequences unless `--raw` is explicitly passed.
- **Storage permissions:** Directories and files are created with private Unix modes (`0o700` / `0o600`), bounded read limits, save locks, conflict detection, and backup recovery. Filesystem permissions are not encryption at rest.
- Imported collections and scripts are treated as untrusted data and never executed.
