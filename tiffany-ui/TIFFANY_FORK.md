# Tiffany Codex Fork

This directory is the Tiffany UI fork of OpenAI Codex.

Rules for this fork:

- Keep OpenAI Codex TUI architecture intact.
- Do not reimplement terminal rendering, selection, copy, paste, IME, scrollback, or resize behavior outside Codex TUI.
- Add Tiffany behavior through small adapter layers.
- Rebase from `openai/codex` regularly instead of copying components back into the legacy orchestrator TUI.

Initial Tiffany changes:

- CLI binary is named `tiffany`.
- CLI help uses `tiffany` as the command name.
- `tiffany orchestrator` starts Codex TUI event mode in idle state and waits for
  the first prompt in the native input box.
- `tiffany orchestrator "..."` starts Codex TUI event mode and immediately
  streams planner, critic, worker, reviewer, and final-result events from
  `orchestrator`.
- In orchestrator mode, native Codex input-box submissions are intercepted and
  routed into the same orchestrator event adapter for multi-turn runs.
- Plain follow-up prompts entered while a run is active stay in Codex's bottom
  pending queue and merge into the next orchestrator run.
- `tiffany orchestrator --legacy ...` bridges to the existing `orchestrator`
  CLI for compatibility checks.
- `orchestrator tui` now delegates to an installed `tiffany` binary when one is
  adjacent to `orchestrator` or available on `PATH`; set
  `ORCHESTRATOR_LEGACY_TUI=1` to force the old compatibility path.

Target integration:

```text
Codex TUI / ChatWidget / BottomPane
        |
        v
Tiffany adapter
        |
        v
orchestrator multi-role runtime
```

Legacy status:

The parent repository's hand-written `src/tui` path is legacy fallback code. New UI work belongs in this fork.
