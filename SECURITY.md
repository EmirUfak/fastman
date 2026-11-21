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
- TLS verification is enabled; imported Postman collections and scripts are treated as untrusted data and never executed.
