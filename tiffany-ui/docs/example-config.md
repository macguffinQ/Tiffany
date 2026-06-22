# Sample Configuration

Generate a starter config with:

```bash
tiffany-loop init
tiffany-loop setup
```

Minimal shape:

```yaml
providers:
  anthropic:
    type: anthropic
    api_key: ${ANTHROPIC_API_KEY}
  minimax:
    type: openai
    api_key: ${MINIMAX_API_KEY}
    base_url: https://api.minimaxi.com/v1

runtimes:
  claude-code:
    type: subprocess
    binary: claude
    supports_agent_teams: true
  codex:
    type: subprocess
    binary: codex

models:
  - id: sonnet
    provider: anthropic
    name: claude-sonnet-4-6
  - id: minimax-m3
    provider: minimax
    name: MiniMax-M3

roles:
  planner:
    model: sonnet
    runtime: claude-code
  critic:
    model: sonnet
    runtime: claude-code
  worker-cc:
    model: minimax-m3
    runtime: claude-code
    agent_teams: true
  reviewer:
    model: sonnet
    runtime: claude-code

behavior: {}
```

Validate after editing:

```bash
orchestrator doctor
orchestrator status
```
