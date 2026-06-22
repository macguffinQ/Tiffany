# Installing & Building Tiffany UI

This document covers the Tiffany UI fork under `tiffany-ui/`. For normal usage,
install Tiffany Loop from the parent project instead of installing upstream
Codex packages.

## System Requirements

| Requirement | Details |
|---|---|
| Operating systems | macOS 12+, Ubuntu 20.04+/Debian 10+, or Windows 11 via WSL2 |
| Git | 2.23+ recommended |
| Rust | Stable toolchain with `rustfmt` and `clippy` |
| RAM | 4 GB minimum, 8 GB recommended |

## Install A Release

```bash
brew tap macguffinQ/tap
brew install tiffany-loop
tiffany-loop setup
tiffany-loop
```

The package installs `tiffany-loop`, `tiffany`, and `orchestrator`.

## Build From Source

From the repository root:

```bash
git clone https://github.com/macguffinQ/Tiffany.git
cd Tiffany

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup component add rustfmt clippy

./scripts/tiffany-build --small
./scripts/tiffany-dev setup
./scripts/tiffany-dev
```

For a release-style local build:

```bash
./scripts/tiffany-build --fast-release --locked
./scripts/tiffany-install-smoke --dist
```

## Focused UI Development

The fork's Cargo workspace lives at `tiffany-ui/codex-rs`, but its Cargo config
redirects build artifacts to the parent `target/` directory to avoid duplicate
large caches.

```bash
cd tiffany-ui/codex-rs
cargo fmt --check
cargo test -p codex-tui feedback_view
cargo test -p codex-cli update_does_not_start_interactive_prompt
```

Use parent scripts for integration checks:

```bash
./scripts/tiffany-check --smoke
./scripts/open-source-audit
```

## Logging

Tiffany Loop keeps UI state under `~/.tiffany` by default and orchestrator state
under `~/.orchestrator`.

For a plaintext TUI log during local debugging:

```bash
tiffany-loop -c log_dir=./.tiffany-log
tail -F ./.tiffany-log/codex-tui.log
```

The log filename still contains `codex-tui` because it comes from the upstream
TUI crate. Do not commit generated logs, local config, SQLite databases, or
session exports.
