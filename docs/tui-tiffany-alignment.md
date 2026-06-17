# tiffany-loop TUI Plan

This project now treats a full tiffany-loop UI as the primary terminal UI
path. The fork lives at:

- `tiffany-ui/`

The previous approach of copying selected UI components into
`src/tui/` is legacy. It is useful as migration history, but new UI work should
land in the fork and keep the upstream terminal architecture intact.

## Current State

- `tiffany-ui/` is a clone of `https://github.com/openai/codex`.
- The fork branch is `tiffany-main`.
- The CLI binary in `codex-rs/cli` is named `tiffany`.
- `tiffany orchestrator` starts the tiffany-loop TUI in tiffany-loop orchestrator idle mode and
  waits for the first prompt in the native input box.
- `tiffany orchestrator "..."` starts the tiffany-loop TUI and immediately streams
  orchestrator events into tiffany-loop-native history cells.
- After the first orchestrator prompt, follow-up submissions from the native
  tiffany-loop input box are intercepted and routed into the orchestrator adapter.
- While an orchestrator run is active, plain follow-up prompts stay in tiffany-loop's
  bottom pending queue and merge into the next run when the current run exits.
- `tiffany orchestrator --legacy ...` remains as the compatibility bridge to
  the existing `orchestrator` CLI.
- `./scripts/tiffany-dev` defaults to `tiffany orchestrator --bin
  target/debug/orchestrator`, building the local debug `orchestrator` binary
  first when needed.
- `./scripts/tiffany-dev orchestrator` is the explicit spelling for the same
  native adapter mode.
- `orchestrator tui` delegates to an installed adjacent or PATH `tiffany`
  binary and falls back to the legacy terminal chat only when `tiffany` is not
  available. Set `ORCHESTRATOR_LEGACY_TUI=1` to force the fallback.
- Direct fork runs and `./scripts/tiffany-dev` can set
  `TIFFANY_ORCHESTRATOR_BIN=/path/to/orchestrator` or pass
  `tiffany orchestrator --bin /path/to/orchestrator`.
- tiffany-loop config is isolated from upstream UI source. The fork sets UI-internal
  `CODEX_HOME` to `TIFFANY_HOME`, defaulting to `~/.tiffany`, so UI config,
  history, logs, plugins, and sessions do not use `~/.codex`. It also maps
  `CODEX_SQLITE_HOME` to `TIFFANY_SQLITE_HOME`, defaulting to the same root.
- `src/tui/` remains the old compatibility path.

## Rules

- Do not reimplement terminal rendering, resize behavior, selection, copy,
  paste, IME handling, scrollback, bottom pane, or overlays in the legacy
  `src/tui/` path.
- Keep the upstream Ratatui/Crossterm event, state, and render loop intact.
- Add tiffany-loop behavior through adapter layers around the app/session
  boundaries.
- Rebase `tiffany-ui/` from upstream UI source regularly.
- Preserve Apache-2.0 license and notice files for copied or forked upstream UI code.

## Target Integration

```text
tiffany-loop TUI
├── ChatWidget
├── BottomPane
├── StatusLine
├── Overlay / Modal
├── HistoryCell
└── App / Session event loop
        |
        v
tiffany-loop adapter
        |
        v
orchestrator runtime
├── planner
├── critic
├── worker
├── reviewer
└── session log
```

The adapter translates orchestrator run events into tiffany-loop-native history cells
instead of printing custom terminal blocks. It currently handles planner,
critic, worker, reviewer, errors, a plain final-result replay, native input-box
follow-ups, and pending queue drain for plain prompts.

## Migration Steps

1. Use `./scripts/tiffany-build` to build both binaries.
2. Use `./scripts/tiffany-check` as the local fork/bridge smoke test.
3. Run the native tiffany-loop adapter through `./scripts/tiffany-dev`.
4. Use `tiffany orchestrator "..."` for an initial prompt in the native tiffany-loop
   TUI event adapter.
5. Keep `tiffany orchestrator --legacy ...` only for compatibility checks.
6. Keep release/Homebrew packaging installing both `orchestrator` and `tiffany`.
7. Remove legacy partial upstream copies under `src/tui/codex_*` when the fork path
   is stable.

## Legacy Snapshot

The old partial-copy attempt included modules such as:

- `src/tui/codex_terminal.rs`
- `src/tui/codex_insert_history.rs`
- `src/tui/codex_inline_surface.rs`
- `src/tui/codex_frame_requester.rs`
- `src/tui/codex_frame_rate_limiter.rs`
- `src/tui/codex_event_stream.rs`
- `src/tui/codex_composer.rs`
- `src/tui/codex_keymap.rs`
- `src/tui/codex_command_popup.rs`
- `src/tui/codex_history_cell.rs`
- `src/tui/codex_process_cell.rs`
- `src/tui/codex_bottom_pane.rs`
- `src/tui/codex_chat_view.rs`

Those files are no longer the target architecture.

## Licensing

tiffany-loop's UI fork is based on upstream Apache-2.0 code. The fork keeps
upstream license and notice files in `tiffany-ui/`. The older vendored source snapshot remains under
`third_party/openai-codex/` and is documented in `THIRD_PARTY_NOTICES.md`.
