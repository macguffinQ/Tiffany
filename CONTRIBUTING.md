# Contributing

Thanks for helping improve tiffany-loop.

## Development Setup

Requirements:

- Rust stable toolchain
- `cargo`
- Optional: `claude`, `codex`, `zellij`

Run checks before opening a pull request:

```bash
cargo fmt -- --check
cargo test --all
cargo build --locked
```

For tiffany-loop UI work:

```bash
cd tiffany-ui/codex-rs
cargo fmt -p codex-tui -p codex-cli -- --check
cargo test -p codex-tui tiffany_orchestrator
cargo test -p codex-tui slash_commands
cargo build --locked -p codex-cli --bin tiffany
```

## Pull Requests

- Keep changes focused.
- Add or update tests for behavior changes.
- Do not commit local config, session logs, database files, API keys, or generated worktrees.
- Prefer human-readable terminal output over raw provider JSON in user-facing UI.
- Keep `tiffany-ui/` changes adapter-oriented and preserve upstream Apache-2.0 notices.
- Keep the public product name `tiffany-loop`; `tiffany` and `orchestrator` remain command names for compatibility.

## Release Cadence

- Do not tag a new release for every small fix. Keep fixes in `CHANGELOG.md`
  under `Unreleased` and batch them into the next planned release.
- A same-day follow-up release should be reserved for installer breakage,
  startup failures, security fixes, or other issues that block normal use.
- `./scripts/tiffany-release-preflight --tag ...` enforces a 24-hour release
  cooldown by default. Override only for urgent releases with
  `TIFFANY_RELEASE_ALLOW_FREQUENT=1`.

## Security

Do not open a public issue for secrets, credential leaks, or command-execution vulnerabilities. See `SECURITY.md`.
