# Skills

Tiffany Loop can inherit capabilities from the worker runtime you choose, but
role orchestration is configured in Tiffany's own provider/model/role graph.

Use skills or runtime-specific extensions when they are part of your selected
worker:

- Claude Code workers can use Claude Code's local configuration, commands,
  agents, MCP servers, and prior sessions when available.
- Codex CLI workers can use Codex-compatible runtime features when configured
  and authenticated separately.
- Tiffany's orchestrator records worker sessions and can inject prior context
  into later workers.

Use `/roles`, `/role`, `/provider`, and `/doctor` to verify that a role is
actually wired to the runtime and provider you expect.
