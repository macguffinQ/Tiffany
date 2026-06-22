# Sandbox And Approvals

Tiffany Loop is a local developer tool. It starts configured worker runtimes
such as Claude Code or Codex CLI and passes them prompts, project context,
worker output, and session history.

Security boundaries depend on the runtime you choose:

- Claude Code permissions are controlled by Claude Code and by Tiffany's
  `behavior.cc_bypass_permissions` setting.
- Codex CLI permissions are controlled by the installed Codex CLI runtime and
  its `exec` behavior.
- Direct API providers do not execute shell commands unless paired with a tool
  runtime.

Before publishing logs or bug reports, redact:

- API keys and provider tokens;
- private repository paths or contents;
- `~/.orchestrator` sessions and SQLite databases;
- `~/.tiffany` UI state;
- Claude Code or Codex CLI local auth files.

Use `/doctor` or `orchestrator doctor` to surface permission and runtime issues
before a long run.
