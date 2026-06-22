# Configuration

Tiffany Loop keeps UI state and orchestrator runtime state separate:

- `~/.tiffany` stores UI-specific state for the forked terminal shell.
- `~/.orchestrator/config.yaml` stores providers, models, runtimes, roles, and
  behavior settings.
- `~/.orchestrator/sessions` stores orchestrator session logs and indexes.

Use the guided setup first:

```bash
tiffany-loop setup
tiffany-loop doctor
```

Common scripted commands:

```bash
orchestrator config provider setup anthropic --env ANTHROPIC_API_KEY
orchestrator config provider setup openai --env OPENAI_API_KEY
orchestrator config provider setup minimax --env MINIMAX_API_KEY --endpoint https://api.minimaxi.com/v1

orchestrator roles register planner --provider openai --model-name gpt-4o --runtime codex
orchestrator roles register worker-cc --provider anthropic --model-name claude-sonnet-4-6 --runtime claude-code --agent-teams
orchestrator roles register reviewer --provider openai --model-name gpt-4o-mini --runtime codex

orchestrator status
orchestrator doctor
```

Inside the TUI, `/provider`, `/role`, `/roles`, and `/doctor` expose the same
setup and repair flow with command completion and forms.

## Lifecycle Hooks

The fork still contains upstream hook and managed-config machinery. Admins can
set top-level `allow_managed_hooks_only = true` in `requirements.toml` to ignore
user, project, and session hook configs while still allowing managed hooks from
requirements and managed config layers. This setting is only supported in
`requirements.toml`; putting it in `config.toml` does not enable
managed-hooks-only mode.
