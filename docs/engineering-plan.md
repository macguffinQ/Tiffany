# tiffany-loop Engineering Plan

This project optimizes for a lightweight orchestration shell, not a second full
agent IDE. The primary UI stays on the forked Ratatui terminal app in
`tiffany-ui/`; the root `orchestrator` binary owns runtime control, session
storage, roles, providers, and diagnostics.

## Product Contract

- `tiffany-loop` is the user-facing command.
- `orchestrator` is the runtime and scripting command.
- `tiffany` is only a compatibility alias.
- Tiffany mode shows only commands that are wired in Tiffany mode.
- Provider and role setup must not require account login from the upstream UI.
- Custom OpenAI-compatible providers must require an explicit endpoint before
  saving, so first-run setup cannot create a provider that immediately fails at
  runtime.
- Built-in role presets should default to the neutral configured provider path
  used by Tiffany examples, while still allowing users to select any supported
  provider manually.
- Worker output is rendered as readable waterfall text, not raw JSON.
- Worker sessions are stable per role and can be inspected or cleared with
  `/thread`.
- Worker session history can be exported from the native TUI with
  `/thread export <role>` for handoff or manual continuation.
- `/roles` and `/thread` summaries expose inline next actions so users can edit
  role bindings, inspect sessions, export handoffs, or clear native session ids
  without guessing subcommands.
- Orchestration control events are grouped into compact run timeline cells,
  while worker/tool/stderr output remains visible as waterfall text.
- Role profiles can save multiple planner, critic, worker, and reviewer
  bindings in one command; the native TUI can call that through `/roles profile`.
- `/roles profile` opens a native dropdown form for common planner, critic,
  worker, and reviewer bindings; existing inline CLI arguments still work.
- `tiffany-loop` checks that the `orchestrator` runtime is launchable before
  entering the native TUI, and reports concrete repair commands when it is not.

## Current Priorities

1. **TUI polish**
   - Keep upstream Ratatui/Crossterm resize, IME, selection, copy, paste, and
     scrollback behavior.
   - Add Tiffany behavior through adapter and history-cell boundaries.
   - Keep `/` command surface small: provider, role, roles, thread, doctor,
     status, help, copy, raw, diff, clear, exit.

2. **Orchestration clarity**
   - Route simple chat to direct worker answers.
   - Route links, research, diagnostics, and scaffolding to single-worker runs.
   - Reserve planner, critic, and reviewer for implementation work.
   - Collapse planner/critic/reviewer JSON and control events into compact human
     timeline summaries.

3. **Session continuity**
   - Reuse the same worker thread for the same role.
   - Reuse native Claude Code / Codex CLI sessions only when the native session
     id is real and current.
   - If a native session is busy, clear it and retry once with a fresh native
     session.
   - Keep `/thread <role>` and `/thread clear <role>` as the operator controls.

4. **Release discipline**
   - Avoid release churn for cosmetic changes.
   - Use `./scripts/tiffany-release-preflight --full --tag vX.Y.Z` before tags.
   - Keep Homebrew, release assets, and installed binary versions in sync.

## Near-Term Backlog

- Trim any remaining upstream-only command affordances that can still appear in
  Tiffany mode during queued or edge-case dispatch.
- Add a denser session inspector popup if inline `/thread` summaries become too
  wide for small terminals.
- Add upstream sync notes for each pulled Codex UI update.
