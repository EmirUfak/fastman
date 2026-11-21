# Fastman

**A native, local-first API development client written in Rust and GPUI Kit.**

> Status: **v0.1.0** (early open-source preview). Desktop launch and platform-specific behavior should be verified on your target system.

## Product contract

- Left panel: **project selector at the top**, then collection/folder/request explorer with nested folders.
- Center/right: request tabs; method, URL, inline request name editing, Params, Auth, Headers, Body, Scripts, and Settings sections; Send / Cancel / Save; response area.
- Native UI with no Electron or browser renderer; data stays local unless a request is explicitly sent.
- No required account, cloud sync, advertising, or developer telemetry.
- Postman request-builder compatibility is the **long-term target**, not a claim of full parity today.

## Implemented in v0.1.0

| Area | What exists |
|---|---|
| Workspaces | Workspace switcher and `+ Workspace` creation in the header; default `My workspace` preserved |
| Project selector | GPUI Kit `Select`, create named projects, switch projects |
| Explorer | Collections, recursive folders, requests, expand/collapse, create folder/request |
| Tabs | Multiple closable request tabs within the active project, inline request rename |
| Request editor | Method dropdown, URL, query params, headers, auth, None/JSON/Text/Form URL Encoded body, timeout |
| Request execution | Reqwest async, TLS verification, max 10 redirects, 100–300,000 ms timeout, 2 MiB response cap, cancellation via Tokio task abort |
| Response viewer | Status, duration, byte count, truncation status, header count, final URL, pretty-printed JSON or raw body |
| Storage | Per-project JSON (`schema_version = 1`) in OS local data directory or `FASTMAN_HOME` |
| Postman import | v2.1 subset through CLI; scripts, collection/request auth, multipart, and complex metadata emit warnings and are **not** imported |
| Tests | Core unit and feature tests (nested lookup, secret exclusion, local persistence, interpolation, importer, localhost HTTP) and CLI integration tests |

### Current limitations

Not yet implemented: full Postman feature parity, editable response header/cookie tabs, request tests/assertions and JS runtime, project/folder/request delete or duplicate menus, nested folder creation inside a selected folder, environment editor, OAuth 2.0, Digest, AWS SigV4, multipart/binary uploads, GraphQL tooling, WebSocket/gRPC/SSE, proxy/certificate configuration, collection runner, export formats, request history and search, large-response streaming viewer, keyboard shortcuts, and distribution packaging.

**Editor note:** Params, headers, and form-urlencoded bodies currently use line-oriented key/value text areas (`# ` comments out a row) instead of interactive table rows.

## Usage

On first launch Fastman creates **My API Project**, with an **Examples** folder and an unsent sample GET request. Network requests are made **only** after clicking **Send** (or invoking `fastman-cli send`).

- The desktop header selects the active workspace; use **+ Workspace** to create one by name. Each workspace stores its own projects, while existing projects remain in **My workspace**.
- The CLI currently operates on the default workspace only:

```sh
cargo run -p fastman-cli -- --version
cargo run -p fastman-cli -- list
cargo run -p fastman-cli -- import-postman ./my_collection.json
cargo run -p fastman-cli -- send <project-uuid> <request-uuid>
```

Importing a Postman file creates a new project. Import does **not** execute requests or scripts.

## Build and test

Install Rust stable (`>= 1.88`) and GPUI Kit's native prerequisites for your target OS.

```sh
cargo fmt --all -- --check
cargo test --locked -p fastman-core -p fastman-cli
cargo clippy --locked -p fastman-core -p fastman-cli --all-targets -- -D warnings
cargo test --locked --workspace
cargo run -p fastman-desktop
```

## Layout

```text
fastman/
  apps/
    fastman-desktop/    # GPUI view and retained editor state
    fastman-cli/        # Headless list, import, and send CLI
  crates/
    fastman-core/       # Domain model, storage, HTTP engine, importer + tests
  .github/              # CI workflow and issue/PR templates
```

## Engineering rules

- Core has no GUI dependency. No HTTP, filesystem I/O, or task spawning in `Render`.
- Keep native UI state within GPUI entities; no external global state layer.
- Prefer one direct implementation over compatibility wrappers or speculative abstraction.
- Do not persist `Auth` credentials in project JSON. **Custom headers, query params, bodies, and variable values ARE persisted**, so avoid storing secrets there until secure variables are implemented.
- Treat imported collections and scripts as **untrusted** data; scripts are never executed.
- Build and benchmark on the target OS; do not assume performance without measurement.

## Documentation & community

- [Changelog](CHANGELOG.md)
- [Contributing guide](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

Licensed under the [MIT License](LICENSE). GPUI Kit and third-party dependencies keep their own licenses.
