# Authentication

Tiffany Loop does not require a separate UI login for normal orchestration
runs. Credentials belong to the provider and runtime you register.

Recommended setup:

```bash
tiffany-loop setup
tiffany-loop
```

In the TUI, use `/provider` to add or edit provider credentials. For scripted
setup, use the runtime command:

```bash
orchestrator config provider setup anthropic --env ANTHROPIC_API_KEY
orchestrator config provider setup minimax --env MINIMAX_API_KEY --endpoint https://api.minimaxi.com/v1
orchestrator roles register worker-cc --provider minimax --model-name MiniMax-M3 --runtime claude-code --agent-teams
orchestrator doctor
```

Provider keys can be stored as literal values or as environment references such
as `${ANTHROPIC_API_KEY}`. Do not commit generated config files containing
literal secrets.

Claude Code and Codex CLI may also have their own authentication state. Tiffany
uses those tools as worker runtimes when configured, but Tiffany's orchestrator
config remains separate under `~/.orchestrator`, and UI state remains under
`~/.tiffany`.
