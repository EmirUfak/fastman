# Contributing to Fastman

Thank you for helping improve Fastman!

## Prerequisites

Install Rust stable (`>= 1.88`) and GPUI Kit's native build prerequisites for your target OS.

## Before submitting changes

Run formatting, tests, and Clippy from the repository root:

```sh
cargo fmt --all -- --check
cargo test --locked -p fastman-core -p fastman-cli
cargo clippy --locked -p fastman-core -p fastman-cli --all-targets -- -D warnings
```

If you modify `apps/fastman-desktop`, also run the full workspace checks on a machine with GPUI Kit's native prerequisites installed:

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

### Desktop save and navigation smoke test

The automated project-switch test covers saving and conflict handling without a native window. Use this smoke test to also check the visible editor and error notification.

1. Set `FASTMAN_HOME` to a new temporary directory and start the desktop app with `cargo run --locked -p fastman-desktop`. Use test data only.
2. Verify the first project has no sample requests. Create and save a test request, then create a second project. In the first project, change the request URL without saving, switch projects, then switch back. Verify the new URL is still present after restarting the app.
3. Open two instances using the same `FASTMAN_HOME`. In the first instance, change the request URL without saving. In the second, change the same request to a different URL and save it.
4. In the first instance, try switching projects. Verify that the app reports a save conflict, stays on the current project and request, and still shows the unsaved URL. Restart the second instance and verify its saved URL is still on disk.

## Engineering guidelines

1. **Keep `fastman-core` GUI-free:** Do not introduce GPUI or windowing dependencies into `crates/fastman-core`.
2. **Keep `render()` pure:** No HTTP requests, filesystem I/O, or background task spawning inside GPUI `render()`.
3. **Preserve security invariants:**
   - Never silently disable TLS verification.
   - Never auto-execute imported requests or scripts.
   - Keep `Auth` credentials excluded from serialized project JSON (`#[serde(skip)]`) until an OS credential-store integration exists.
   - Never include real credentials or tokens in tests or fixtures.
4. **Verify GPUI Kit signatures:** Pin and check against `gpui-kit = "=0.10.0"` rather than mixing GPUI forks.
5. **Test changes:** Add focused unit or integration tests for bug fixes and new core behavior, and avoid `#[allow(...)]` unless narrowly justified.
6. **Commit style:** Use [Conventional Commits](https://www.conventionalcommits.org/) (e.g., `feat(core): ...`, `fix(desktop): ...`, `docs: ...`).
