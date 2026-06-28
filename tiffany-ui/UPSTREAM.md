# Tiffany UI Upstream Boundary

This directory vendors the OpenAI Codex monorepo UI/runtime tree under
`tiffany-ui/codex-rs/` and layers Tiffany-owned orchestration behavior on top.
The goal is a stable fork boundary: Tiffany can evolve quickly without making
future upstream backports impossible to review.

## Frozen Upstream Point

- Upstream project: `openai/codex`
- Vendored root: `tiffany-ui/codex-rs/`
- Freeze date recorded here: 2026-06-28
- Best available local identifiers:
  - `codex-tui` crate version: `0.1.33`
  - `tiffany-cli` crate version: `0.1.33`
  - `codex-core` crate version: workspace `0.0.0`
  - npm package marker: `@openai/codex` version `0.0.0-dev`
- Exact upstream commit: TODO. The current vendor drop does not contain a
  recoverable upstream commit hash: there is no nested `.git` metadata under
  `tiffany-ui/codex-rs/`, and the local Cargo/package markers above do not name
  a source commit. Do not fabricate one; fill this in only after matching the
  vendor tree against an upstream Codex commit.

## Freeze Policy

- This is a vendor-freeze fork with manual backports, not a scheduled rebase.
- Upstream Ratatui/Crossterm app architecture should remain intact.
- Tiffany-specific product logic should move toward
  `tiffany-ui/tiffany-bridge/` one module at a time.
- Fork-side Codex files should become thin seams that delegate to Tiffany-owned
  bridge code.
- Do not do a big-bang extraction of `tui/src/tiffany_orchestrator.rs`; each
  extraction slice must build and test green before the next slice starts.

## Tiffany-Owned Files And Seams

These files or file regions are Tiffany-owned. Edits here do not need a
separate vendor-edit log entry, but should still preserve upstream UI behavior
unless a Tiffany requirement explicitly changes it.

- `tiffany-ui/codex-rs/tiffany-cli/**`
- `tiffany-ui/codex-rs/cli/src/tiffany.rs`
- `tiffany-ui/codex-rs/tui/src/tiffany_orchestrator.rs`
- `tiffany-ui/codex-rs/tui/src/app_event.rs` Tiffany orchestrator events
- `tiffany-ui/codex-rs/tui/src/app.rs` Tiffany startup/session wiring
- `tiffany-ui/codex-rs/tui/src/app/event_dispatch.rs` Tiffany event dispatch
- `tiffany-ui/codex-rs/tui/src/app/input.rs` Tiffany prompt routing hooks
- `tiffany-ui/codex-rs/tui/src/slash_command.rs` Tiffany command entries
- `tiffany-ui/codex-rs/tui/src/bottom_pane/provider_setup_view.rs`
- `tiffany-ui/codex-rs/tui/src/bottom_pane/role_setup_view.rs`
- `tiffany-ui/codex-rs/tui/src/bottom_pane/role_profile_setup_view.rs`
- `tiffany-ui/codex-rs/tui/src/bottom_pane/pending_input_preview.rs`
- `tiffany-ui/codex-rs/tui/src/bottom_pane/slash_commands.rs`
- `tiffany-ui/codex-rs/tui/src/bottom_pane/command_popup.rs` Tiffany command
  visibility and command-menu rows
- `tiffany-ui/codex-rs/tui/src/bottom_pane/chat_composer.rs` Tiffany slash and
  queue input affordances
- `tiffany-ui/codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- `tiffany-ui/codex-rs/tui/src/chatwidget/input_flow.rs` Tiffany queue drain
  behavior
- `tiffany-ui/codex-rs/tui/src/chatwidget/user_messages.rs` Tiffany queue
  history/merge behavior
- `tiffany-ui/codex-rs/tui/src/chatwidget/constructor.rs` Tiffany shell setup
- `tiffany-ui/codex-rs/tui/src/chatwidget/tests/*` tests for Tiffany command,
  provider, role, queue, and orchestration behavior
- `tiffany-ui/codex-rs/tui/src/bottom_pane/snapshots/*command_popup*` Tiffany
  command-menu snapshots
- Future bridge crate: `tiffany-ui/tiffany-bridge/**`

## Vendored-Untouched Default

Everything else under `tiffany-ui/codex-rs/` is treated as upstream-vendored by
default. Examples include the core Codex runtime crates, app-server crates,
protocol crates, auth/login crates, model-provider crates, sandbox crates,
generic history-cell rendering, terminal/event-loop infrastructure, and
workspace-wide dependency metadata.

If a Tiffany change must edit a non-owned vendored file, add a one-line entry
to the log below before making the change.

## Non-Owned Vendored Edit Log

Format: `YYYY-MM-DD - path - reason`.

- 2026-06-28 - `tiffany-ui/codex-rs/Cargo.toml` - planned F4 bridge wiring will
  add `tiffany-bridge` as a local workspace dependency; no other workspace-wide
  dependency edits are covered by this entry.
