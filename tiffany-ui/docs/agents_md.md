# AGENTS.md And Project Instructions

Tiffany Loop reads project guidance from the orchestrator runtime and from
compatible worker tools.

Supported instruction sources include:

- `AGENTS.md` in the project or `.orchestrator` directories;
- `CLAUDE.md`, Claude Code agents, commands, settings, MCP config, and prior
  Claude sessions when Claude Code workers are configured;
- orchestrator session history stored under `~/.orchestrator/sessions`.

Precedence is handled by the parent orchestrator runtime. Use
`orchestrator doctor`, `/doctor`, and `/roles` when a role appears to ignore the
intended instructions.

The fork still contains upstream `child_agents_md` feature plumbing because it
shares the upstream terminal UI architecture. Tiffany-facing behavior should be
validated through orchestrator integration tests.
