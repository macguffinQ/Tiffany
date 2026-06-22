# MCP Tools

Tiffany Loop can use MCP-capable worker runtimes when those runtimes are
configured and authenticated locally.

There are two separate layers:

- Tiffany's orchestrator config controls providers, models, roles, and worker
  runtimes.
- The selected worker runtime controls its MCP server configuration and tool
  permissions.

Run these checks before relying on MCP tools:

```bash
orchestrator doctor
tiffany-loop
```

Inside the TUI, use `/roles` to confirm which runtime a role uses and `/doctor`
to surface missing binaries, credentials, provider links, or runtime
compatibility issues.

Never include MCP credentials, local server tokens, private tool output, or
session logs in public bug reports.
