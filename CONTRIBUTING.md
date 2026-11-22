# Contributing to Fastman

Thanks for your interest in Fastman! This guide explains how to suggest changes, which checks to run, and the rules the code follows.

By contributing, you agree that your contributions are licensed under the project's [MIT License](LICENSE). There is no CLA.

## Workflow

1. **Find or open an issue.** Search [existing issues](https://github.com/EmirUfak/fastman/issues) first. Small fixes such as typos, obvious bugs, or test gaps can go straight to a pull request. For new features, UI changes, or anything listed under "Changes that need discussion first" below, open an issue and agree on the approach before writing code. Report security vulnerabilities through [SECURITY.md](SECURITY.md), not public issues.
2. **Fork and branch.** Fork the repository and create a topic branch from `main`, for example `fix/redirect-downgrade` or `feat/env-switcher`.
3. **Make a focused change.** Keep each pull request to one logical change. Add or update tests for bug fixes and new core behavior. Update docs (`README.md`, `CHANGELOG.md` under `[Unreleased]`, `SECURITY.md`) whenever behavior or a security posture changes.
4. **Commit using [Conventional Commits](https://www.conventionalcommits.org/)**, for example `feat(core): ...`, `fix(desktop): ...`, or `docs: ...`. Use the commit body for details.
5. **Open a pull request** against `main` and fill in the [template](.github/PULL_REQUEST_TEMPLATE.md). Link the issue, describe how you tested the change, and declare any breaking changes.

### Review and acceptance

- Pull requests are reviewed as time allows, so there is no fixed response time. A friendly ping after two weeks is fine.
- A pull request is merged when CI passes, review comments are resolved, and the change fits the project's scope and [engineering rules](#engineering-rules).
- You may be asked to split a large pull request. Changes that are out of scope may be closed. That isn't a judgement of your work.

### Changes that need discussion first

Open an issue before starting any of these:

- **Breaking changes:** CLI arguments or output format, keyboard shortcuts, or removed features. Describe the migration path for users.
- **Persistence schema changes:** anything touching the serialized `Project` model. New fields must use `#[serde(default)]` so existing `schema_version = 1` files still load. If old files can no longer be read, bump `schema_version` and add a migration with tests.
- **New dependencies:** explain why the standard library or an existing dependency isn't enough. Check the license (MIT, Apache-2.0, or similarly permissive) and keep default features minimal. CI runs `cargo audit`.
- **Security-sensitive code:** TLS, redirects, credential handling, import parsing, cURL generation, terminal output escaping, and file permissions. Explain your threat-model reasoning in the pull request.

## Local checks

Install Rust stable (`>= 1.100`). For the desktop app, also install GPUI's native build prerequisites for your OS (see [README: Quick start](README.md#quick-start-from-source)).

The check commands are short Cargo aliases defined in [`.cargo/config.toml`](.cargo/config.toml). CI uses the same aliases, so local and CI checks stay identical.

**Required for every pull request.** These run on any OS and need no GUI toolchain:

```sh
cargo fmt-check   # formatting (run `cargo fmt` to fix)
cargo test-core   # tests for fastman-core and fastman-cli
cargo lint-core   # Clippy for fastman-core and fastman-cli, warnings as errors
```

**Required when you change `apps/fastman-desktop`.** These cover the whole workspace and need GPUI's native prerequisites:

```sh
cargo test-all    # tests across the whole workspace
cargo lint-all    # Clippy across the whole workspace, warnings as errors
cargo check-all   # compile check across all workspace targets (used by MSRV CI)
```

**Checks that depend on the OS.** CI runs the full workspace checks on **Windows** only, and `cargo audit` on Linux. CI also runs `cargo check-all` with the declared minimum Rust version (1.100). If you can't run the desktop checks locally, or you only tested on Linux or macOS, say so in the pull request. Write "not run" rather than ticking the box.

### Desktop save and navigation smoke test

The automated project-switch test checks saving and conflict handling without opening a native window. Use this manual test to also check the visible editor and the error notification.

1. Set `FASTMAN_HOME` to a new temporary directory and start the desktop app with `cargo run --locked -p fastman-desktop`. Use test data only.
2. Check that the first project has no sample requests. Create and save a test request, then create a second project. In the first project, change the request URL without saving, switch projects, then switch back. Restart the app and check that the new URL is still there.
3. Open two instances that use the same `FASTMAN_HOME`. In the first instance, change the request URL without saving. In the second instance, change the same request to a different URL and save it.
4. In the first instance, try switching projects. The app should report a save conflict, stay on the current project and request, and still show the unsaved URL. Restart the second instance and check that its saved URL is still on disk.

## Engineering rules

1. **Keep `fastman-core` GUI-free.** Do not add GPUI or windowing dependencies to `crates/fastman-core`. CI enforces this.
2. **Keep `render()` pure.** Never make HTTP requests, do filesystem I/O, or spawn background tasks inside GPUI `render()`.
3. **Keep UI state in GPUI entities.** Do not add an external global state layer.
4. **Keep it direct.** Prefer one direct implementation over compatibility wrappers or speculative abstractions.
5. **Preserve the security invariants:**
   - Never disable TLS verification silently. The only opt-out is the explicit per-request setting.
   - Never run imported requests or scripts automatically.
   - Keep `Auth` credentials out of serialized project JSON (`#[serde(skip)]`) until Fastman integrates with the OS credential store.
   - Never put real credentials or tokens in tests or fixtures.
6. **Pin GPUI Kit.** Check APIs against `gpui-kit = "=0.10.0"` and don't mix GPUI forks.
7. **Test your changes.** Add focused unit or integration tests. Avoid `#[allow(...)]` unless it is narrowly justified.
8. **Measure performance.** Build and benchmark on the target OS rather than assuming.

## Release checklist

1. Bump `version` in the root `Cargo.toml` and run `cargo check --workspace` to update `Cargo.lock`.
2. In `CHANGELOG.md`, move the `[Unreleased]` entries into a new dated version section and update the comparison links at the bottom.
3. Check that the README's Download, Platform support, Features, and Current limitations sections match what the release actually ships.
4. Review third-party license notices for the binary distribution, for example with `cargo about` or `cargo deny check licenses`.
5. Tag `vX.Y.Z` on `main` and push the tag. The [release workflow](.github/workflows/release.yml) checks that the tag matches `fastman-desktop` in `Cargo.toml`, then builds and publishes `fastman-desktop-<tag>-windows-x86_64.exe` and its `.sha256` checksum.
6. Edit the generated release notes so they match the changelog and README, including which features are desktop-only and which are in the CLI.
