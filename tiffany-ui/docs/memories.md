# Memories

Tiffany Loop can preserve context through orchestrator sessions and through
worker runtimes that support their own memory features.

Important storage locations:

- `~/.orchestrator/sessions` stores orchestrator conversations, worker events,
  and indexed session metadata.
- `~/.tiffany` stores UI state for the terminal shell.
- Claude Code and Codex-compatible runtimes may keep their own local history or
  memory state.

Use `/sessions`, `/trace`, `/process`, `/result`, and `/workflow` to inspect
what Tiffany recorded for a run. Use `/doctor` when a role appears to miss prior
context.

Do not publish memory, session, or UI state files without reviewing and
redacting private content.
