# Getting Started With Tiffany Loop

Install the packaged commands:

```bash
brew tap macguffinQ/tap
brew install tiffany-loop
tiffany-loop setup
tiffany-loop
```

From a source checkout:

```bash
./scripts/tiffany-dev setup
./scripts/tiffany-dev
```

Inside the TUI:

- `/provider` configures provider credentials, endpoint, and provider type.
- `/role` registers planner, critic, worker, and reviewer roles.
- `/roles` shows role routing and health.
- `/doctor` diagnoses missing binaries, keys, models, runtime mismatch, stale
  Homebrew installs, and Codex CLI `exec --cd` compatibility.

The UI runs in Tiffany orchestration mode by default: prompts go through the
planner -> critic -> worker -> reviewer pipeline, with conversational turns
collapsed into direct worker answers and a visible `review skipped` event.
