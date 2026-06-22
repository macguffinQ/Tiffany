# Non-Interactive Runs

Use `orchestrator run` when you want the pipeline without opening the TUI:

```bash
orchestrator run "Add tests for the fibonacci example and run them."
```

To stream Tiffany event JSON for adapters or tests:

```bash
orchestrator events "Explain this repository in one paragraph."
```

The packaged UI can start with an initial prompt:

```bash
tiffany-loop orchestrator "Fix the failing tests and summarize the diff."
```

From a source checkout:

```bash
./scripts/tiffany-dev orchestrator "Fix the failing tests and summarize the diff."
```

Use `orchestrator doctor` first if non-interactive runs fail because of missing
provider keys, stale Homebrew binaries, missing `claude`/`codex` executables, or
Codex CLI `exec --cd` compatibility.
