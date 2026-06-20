# Open Source Checklist

Use this before making the repository public.

## Required

- [x] Create the public repository.
- [x] Add the real repository URL to `Cargo.toml`.
- [x] Confirm `LICENSE` holder text is correct.
- [x] Review `README.md` and `README.zh-CN.md` for public-facing install and development accuracy.
- [x] Add preview-stage and non-affiliation language.
- [x] Preserve Codex fork license and notice files.
- [x] Configure or manually update the Homebrew tap for the current release.
  `HOMEBREW_TAP_TOKEN` enables automatic tap updates for future tagged releases;
  without it, update the tap manually after release. See [docs/homebrew.md](homebrew.md).
- [ ] Run full release preflight before publishing the next tag:

```bash
./scripts/tiffany-install-smoke --smoke
./scripts/tiffany-release-preflight --full --tag v0.1.15
./scripts/tiffany-clean-targets --sizes
```

- [ ] Review all files under `~/.orchestrator` before sharing logs or examples.
- [ ] Review all files under `~/.tiffany` before sharing UI history, logs, or config.
- [ ] Confirm no API keys, private prompts, session logs, database files, or generated worktrees are committed.
- [x] Run a final secret scan before pushing. `./scripts/open-source-audit` covers tracked local state, untracked suspicious files, large files, and common secret patterns. Treat every reported path as a blocker unless it is an intentional fixture with a documented exception. If you run manual scans, investigate every match:

```bash
git grep -nE 'sk-[A-Za-z0-9_-]{20,}|ANTHROPIC_API_KEY|OPENAI_API_KEY|GITHUB_TOKEN|BEGIN (RSA|OPENSSH|PRIVATE) KEY' -- . ':!Cargo.lock'
find . -path './target' -prune -o -path './tiffany-ui/codex-rs/target' -prune -o -type f -size +50M -print
```

- [x] Push CI workflow and verify it passes on GitHub.

## Recommended

- [x] Add release binaries after the first tagged release.
- [ ] Add screenshots or terminal recordings for `tiffany-loop`.
- [x] Add a minimal example project under `examples/`.
- [ ] Decide whether to publish to crates.io.
- [x] Add a `CHANGELOG.md` before version `0.2.0`.
- [x] Add Homebrew tap documentation and formula template.

## Release Commands

```bash
./scripts/tiffany-release-preflight --full --tag v0.1.15
./scripts/tiffany-build --fast-release --locked --prune-dist-cache
./scripts/tiffany-install-smoke --dist
```

Run `cargo package --allow-dirty` only when preparing a crates.io package.
Release from a clean tree.
