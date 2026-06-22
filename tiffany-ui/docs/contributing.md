# Contributing To Tiffany UI

Tiffany UI is the terminal UI fork used by `tiffany-loop`. It keeps the upstream
Ratatui/Crossterm architecture while routing prompts and event streams into the
parent orchestrator runtime.

For project-wide contribution rules, see the root
[`CONTRIBUTING.md`](../../CONTRIBUTING.md).

## UI Development Rules

- Keep terminal rendering, resize behavior, selection, copy, paste, scrollback,
  IME input, history cells, bottom pane, and overlays inside the forked UI
  architecture.
- Add Tiffany behavior through adapter layers instead of rebuilding terminal UI
  behavior in the parent legacy `src/tui` path.
- Preserve upstream Apache-2.0 notices and attribution.
- Prefer Tiffany-facing command names in user documentation: `tiffany-loop` for
  the UI, `orchestrator` for runtime/config/events, and `tiffany` only as a
  compatibility alias.
- Do not commit local `~/.tiffany`, `~/.orchestrator`, session logs, SQLite
  databases, API keys, generated worktrees, or plaintext debug logs.

## Focused Checks

From the repository root:

```bash
./scripts/tiffany-check --smoke
./scripts/open-source-audit
```

For UI-only changes:

```bash
cd tiffany-ui/codex-rs
cargo fmt --check
cargo test -p codex-tui tiffany_orchestrator
cargo test -p codex-tui slash_commands
cargo test -p codex-cli update_does_not_start_interactive_prompt
```
