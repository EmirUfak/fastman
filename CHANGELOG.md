# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-11-27

### Added
- **Declarative Request Chains (`fastman-core`, `fastman-cli`, `fastman-desktop`):**
  - Sequential multi-step chain execution with strict origin authorization (`primary_origin` + optional `trusted_origins`), per-run isolated cookie jars, and per-step `isolate_cookies` opt-out.
  - Declarative response checks (`Status`, `Header`, `JsonField`, `Latency`) and redacted captures (`Status`, `Header`, `JsonField`, and auto-linked `LinkedJsonField`) that pass values into later steps as `{{variable}}`, Bearer token, or Basic auth credentials without leaking secrets in logs or reports.
  - Cleanup steps via `ChainStep.always_run`, which still execute after an earlier request or check failure while preserving the original failure exit code.
  - Desktop **Chains** flow editor with a GPU-accelerated horizontal canvas, proportional zoom, orthogonal data-flow wires, and a resizable 2-column bottom step inspector (`Inputs` + `Outputs` and inline `Checks`).
  - CLI subcommands `fastman-cli chain list` and `fastman-cli chain run` with `--env`, `--origin`, `--trusted-origin`, `--json`, and structured exit codes.
  - Local Bun + Express Docker test API (`tools/test-api/`) for end-to-end verification of `HttpOnly` cookie sessions, cookie isolation, linked tokens, and CRUD cleanup chains.

### Fixed
- Guarded chain navigation and request-tab switching when a chain run is active or has unsaved changes.
- Validated chain step IDs against empty or duplicate identifiers before execution.

## [0.3.1] - 2026-11-22

### Changed
- Deferred workspace, project, and UI settings loading off the UI startup path and initialized the Tokio runtime and HTTP client lazily on first request send.
- Cached flattened explorer rows across frames, expanded only the active request's ancestor path on load/switch, and skipped redundant form/input/auth editor updates.
- Skipped directory lock acquisition and recovery scans on clean project directories during `ProjectStore` loads and switched private file writes from `sync_all()` to `sync_data()`.
- Release workflow publishes only the standalone Windows desktop executable (`fastman-desktop-<tag>-windows-x86_64.exe`) and its `.sha256` checksum instead of a zip archive with the CLI.

### Fixed
- Quit the desktop application cleanly when the last window closes, and ensure a trailing blank row when switching body type to `FormUrlEncoded`.

## [0.3.0] - 2026-11-22

### Added
- URL template variable highlighting with resolved-value inspection and unresolved-variable warnings.
- Safe cURL paste with a staged import confirmation that reports unsupported flags.
- Fuzzy command palette (`Ctrl+K` / `Cmd+K`) and persistent response panel placement.
- Project environments (base variables plus per-environment overrides) with header and command-palette switching, scoped variable editing, and CLI `--env <name-or-id>`.
- Typed `Plain`, `Secret` (OS credential store: Windows Credential Manager, macOS Keychain, Linux `secret-tool`), and `Env` variables with masked previews, plaintext draft warnings, and secret redaction across exports and backups.
- Explicit per-request TLS verification opt-out with visible warnings, and TTFB / download / total response timing in desktop and CLI.
- Issue template contact link for private vulnerability reports, and a release checklist.
- Shared Cargo check aliases in `.cargo/config.toml` (`fmt-check`, `test-core`, `lint-core`, `check-all`, `test-all`, `lint-all`) used by local checks, the PR template, and CI.
- Storage scale and explorer viewport benchmark suites.

### Changed
- Desktop project and UI settings saves run off the UI thread via a coalesced background persistence worker.
- Explorer tree rendering is virtualized over flattened visible rows with zero-allocation ASCII case-insensitive filtering and lazy context-menu target resolution.
- CI and release builds target Windows; tagged releases publish a Windows x86_64 archive with SHA-256 checksum. CI also checks the declared MSRV (Rust 1.100).
- README reorganized around download, quick start, platform support, and current limitations; contributing workflow and security reporting process clarified.

### Fixed
- Coordinated storage `load`, `list_resilient`, and `save` under per-project directory locks with lock-token verification and single-pass recovery.
- Preserved URL query percent-decoding, literal `+`/`%2B`, empty values, and `#fragment` suffixes across editor extraction, `HttpEngine`, and cURL export.
- Differentiated cURL data flags (`-d`/`--data`/`--data-ascii`, `--data-raw`, `--data-binary`, `--data-urlencode`) and reported `@file` arguments as unsupported without leaking local paths or reading files.

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

[Unreleased]: https://github.com/EmirUfak/fastman/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/EmirUfak/fastman/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/EmirUfak/fastman/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/EmirUfak/fastman/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/EmirUfak/fastman/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/EmirUfak/fastman/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/EmirUfak/fastman/releases/tag/v0.1.0
