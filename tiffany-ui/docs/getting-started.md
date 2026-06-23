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

The UI runs in Tiffany orchestration mode by default. Each prompt first selects
a flow: `direct` for chat/Q&A, `single` for atomic worker tasks, or `full` for
planner -> critic -> worker -> reviewer implementation work. `/workflow`,
`/process`, and `/session flow` show the selected flow and route reason.
