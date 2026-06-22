# Tiffany UI

This directory contains the Tiffany Loop terminal UI fork. It is based on the
OpenAI Codex TUI codebase and keeps the upstream Ratatui/Crossterm terminal
architecture for resize handling, native selection/copy/paste, IME input,
history cells, overlays, and bottom-pane interactions.

Tiffany-specific behavior is added through small adapter layers that route the
UI into the parent `orchestrator` runtime.

## Use Tiffany Loop

For normal users, install and run Tiffany Loop from the parent project:

```bash
brew tap macguffinQ/tap
brew install tiffany-loop
tiffany-loop setup
tiffany-loop
```

From a source checkout, use the parent helper scripts:

```bash
./scripts/tiffany-dev setup
./scripts/tiffany-dev
```

The installed package provides:

- `tiffany-loop` - primary user-facing UI command.
- `tiffany` - compatibility alias for the forked UI binary.
- `orchestrator` - lower-level runtime/config/events/ACP command.

## Development

Build both the parent runtime and this UI fork from the repository root:

```bash
./scripts/tiffany-build --small
./scripts/tiffany-dev
```

Run focused UI tests from this workspace when changing fork internals:

```bash
cd tiffany-ui/codex-rs
cargo test -p codex-tui feedback_view
cargo test -p codex-cli update_does_not_start_interactive_prompt
```

Run the parent smoke checks before release-facing changes:

```bash
./scripts/tiffany-check --smoke
./scripts/open-source-audit
```

## Fork Policy

See [TIFFANY_FORK.md](TIFFANY_FORK.md). In short:

- Keep the upstream Codex TUI architecture intact.
- Do not reimplement terminal rendering in the legacy parent `src/tui`.
- Add Tiffany orchestration through adapter layers.
- Preserve upstream license and notice files.

This fork is an independent community project. It is not an official OpenAI,
Anthropic, MiniMax, or provider product. OpenAI Codex names remain in package
and module names where they identify upstream code or compatibility surfaces.
