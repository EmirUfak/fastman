## Summary
Describe what changed and why.

## Checklist
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo test --locked -p fastman-core -p fastman-cli` passes
- [ ] `cargo clippy --locked -p fastman-core -p fastman-cli --all-targets -- -D warnings` passes
- [ ] Desktop changes verified with `cargo test --locked --workspace` (if applicable)
- [ ] No secrets, tokens, or personal paths added to fixtures or docs
- [ ] Relevant documentation updated (`README.md`, `CHANGELOG.md`, `docs/`)
