## Summary
Describe what changed and why.

Closes #

## Testing evidence
Tick checks that passed. For checks that failed or were not run, leave unticked and append `— failed: …` or `— not run: <reason>` (e.g. no GPUI toolchain, OS unavailable).

- [ ] `cargo fmt-check`
- [ ] `cargo test-core`
- [ ] `cargo lint-core`
- [ ] Desktop changes: `cargo test-all` and `cargo lint-all`
- [ ] Manual testing (describe OS and steps):

## Breaking changes
- [ ] None
- [ ] Yes — describe impact on CLI output/arguments, persisted project JSON, or user workflows, and the migration path:

## Checklist
- [ ] No secrets, tokens, or personal paths added to fixtures or docs
- [ ] Security-sensitive areas (TLS, redirects, credentials, imports, escaping, file permissions) unchanged, or reasoning explained above
- [ ] Relevant documentation updated (`README.md`, `CHANGELOG.md` `[Unreleased]`, `SECURITY.md`)
