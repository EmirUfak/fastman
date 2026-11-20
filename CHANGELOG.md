# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-20

### Added
- Initial open-source release of Fastman (`fastman-core`, `fastman-cli`, `fastman-desktop`).
- **Core (`fastman-core`)**:
  - UUID-identified `Project`, `Collection`, `Folder`, `ApiRequest`, `KeyValue`, `HttpBody`, and session-only `Auth` domain model (`schema_version = 1`).
  - Atomic-style local JSON storage with path/ID validation and workspace isolation (`FASTMAN_HOME` or OS local data directory).
  - Reusable Reqwest HTTP engine with TLS verification, 10-redirect cap, clamped timeouts (100–300,000 ms), 2 MiB response buffer cap, and single-pass `{{var}}` interpolation.
  - Best-effort Postman Collection v2.1 importer that warns on unsupported features and never executes scripts.
- **Desktop (`fastman-desktop`)**:
  - Native GPUI Kit (`0.10.0`) workspace shell with workspace switcher/creator, project selector/creator, recursive collection/folder explorer, and closable request tabs.
  - Request editor with inline request rename, method dropdown, URL input, Params, session-only Auth, Headers, Body (`None`, `JSON`, `Text`, `Form URL Encoded`), and timeout settings.
  - Async request execution on a Tokio runtime with cancellation (`AbortHandle`) and stale-response generation fencing.
- **CLI (`fastman-cli`)**:
  - Commands for `list`, `import-postman`, `send`, `--version`, and `--help`.
- **Documentation & CI**:
  - Architecture, build, testing/benchmarking, security model, schema format, roadmap, and GitHub Actions CI for headless core and CLI.
