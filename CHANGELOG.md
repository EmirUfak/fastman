# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-11-21

### Changed
- Raised the minimum build toolchain to Rust 1.100 and configured the workspace to use stable Rust.
- Updated dependencies to their latest compatible stable releases and removed unused dependency features.

## [0.2.0] - 2026-11-21

### Added
- Redesigned desktop workbench with light and dark themes, keyboard shortcuts, searchable explorer, and project variables editor.
- Interactive parameter, header, and form tables; JSON validation and formatting; and a searchable response inspector with body, headers, and cookies.
- Desktop Collection v2.1 JSON and cURL import, plus Copy as cURL with secrets redacted by default.
- Explorer context menus and drag-and-drop for organizing requests and folders.

### Changed
- Fresh desktop workspaces start without sample folders or requests, and new requests have a blank URL.
- HTTP redirects are now restricted to the same origin. The CLI escapes terminal control characters by default; use `send --raw` for unescaped output.
- CLI collection imports containing detected secrets now require an explicit `--strip-secrets` or `--allow-plaintext-secrets` choice.
- Storage now detects concurrent save conflicts, retains a previous project copy for recovery, and isolates unreadable projects from workspace listings.
- Large responses are processed off the UI thread, and response views render only visible rows.

### Fixed
- Failed tree saves now keep unsaved moves visible and prevent project switches from silently discarding them.

## [0.1.0] - 2026-11-20

### Added
- Initial open-source release of Fastman (`fastman-core`, `fastman-cli`, `fastman-desktop`).
- **Core (`fastman-core`)**:
  - UUID-identified `Project`, `Collection`, `Folder`, `ApiRequest`, `KeyValue`, `HttpBody`, and session-only `Auth` domain model (`schema_version = 1`).
  - Atomic-style local JSON storage with path/ID validation and workspace isolation (`FASTMAN_HOME` or OS local data directory).
  - Reusable Reqwest HTTP engine with TLS verification, 10-redirect cap, clamped timeouts (100–300,000 ms), 2 MiB response buffer cap, and single-pass `{{var}}` interpolation.
  - Best-effort Collection v2.1 JSON importer that warns on unsupported features and never executes scripts.
- **Desktop (`fastman-desktop`)**:
  - Native GPUI Kit (`0.10.0`) workspace shell with workspace switcher/creator, project selector/creator, recursive collection/folder explorer, and closable request tabs.
  - Request editor with inline request rename, method dropdown, URL input, Params, session-only Auth, Headers, Body (`None`, `JSON`, `Text`, `Form URL Encoded`), and timeout settings.
  - Async request execution on a Tokio runtime with cancellation (`AbortHandle`) and stale-response generation fencing.
- **CLI (`fastman-cli`)**:
  - Commands for `list`, `import-collection`, `send`, `--version`, and `--help`.
- **Documentation & CI**:
  - Architecture, build, testing/benchmarking, security model, schema format, roadmap, and GitHub Actions CI for headless core and CLI.
