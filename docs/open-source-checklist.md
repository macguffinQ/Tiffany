# Open Source Checklist

Use this before making the repository public.

## Required

- [x] Create the public repository.
- [x] Add the real repository URL to `Cargo.toml`.
- [x] Confirm `LICENSE` holder text is correct.
- [ ] Review `README.md` for public-facing accuracy.
- [x] Add preview-stage and non-affiliation language.
- [x] Preserve Codex fork license and notice files.
- [ ] Run:

```bash
cargo fmt -- --check
cargo test --all
./scripts/tiffany-check-examples
cargo build --locked
./scripts/open-source-audit
(cd tiffany-ui/codex-rs && cargo fmt -p codex-tui -p tiffany-cli -- --check)
(cd tiffany-ui/codex-rs && cargo test -p codex-tui tiffany_orchestrator)
(cd tiffany-ui/codex-rs && cargo test -p codex-tui slash_commands)
(cd tiffany-ui/codex-rs && cargo build --locked -p tiffany-cli --bin tiffany)
./scripts/tiffany-check --smoke
./scripts/tiffany-check --dist
./scripts/tiffany-clean-targets --sizes
```

- [ ] Review all files under `~/.orchestrator` before sharing logs or examples.
- [ ] Review all files under `~/.tiffany` before sharing UI history, logs, or config.
- [ ] Confirm no API keys, private prompts, session logs, database files, or generated worktrees are committed.
- [ ] Run a final secret scan before pushing. `./scripts/open-source-audit` covers common local-state, large-file, and secret patterns. If you run manual scans, investigate every match:

```bash
git grep -nE 'sk-[A-Za-z0-9_-]{20,}|ANTHROPIC_API_KEY|OPENAI_API_KEY|GITHUB_TOKEN|BEGIN (RSA|OPENSSH|PRIVATE) KEY' -- . ':!Cargo.lock'
find . -path './target' -prune -o -path './tiffany-ui/codex-rs/target' -prune -o -type f -size +50M -print
```

- [ ] Push CI workflow and verify it passes on GitHub.

## Recommended

- [x] Add release binaries after the first tagged release.
- [ ] Add screenshots or terminal recordings for `tiffany orchestrator`.
- [x] Add a minimal example project under `examples/`.
- [ ] Decide whether to publish to crates.io.
- [x] Add a `CHANGELOG.md` before version `0.2.0`.
- [x] Add Homebrew tap documentation and formula template.

## Release Commands

```bash
cargo fmt -- --check
cargo test --all
./scripts/tiffany-check-examples
./scripts/open-source-audit
./scripts/tiffany-check --dist
./scripts/tiffany-build --release --locked
cargo package --allow-dirty
```

Use `--allow-dirty` only for local preflight checks. Release from a clean tree.
