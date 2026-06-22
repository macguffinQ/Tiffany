# Security Policy

This directory contains Tiffany Loop's terminal UI fork. Security reporting for
this repository follows the root [`SECURITY.md`](../SECURITY.md).

Tiffany Loop is a local developer tool that can start configured worker
runtimes, pass project context into them, and store process/session logs. Treat
provider credentials, runtime auth files, session logs, SQLite databases, and
local config as sensitive.

## Reporting

Please report security issues privately to the maintainers before opening a
public issue. Include the affected version or commit, reproduction steps,
impact, and redacted logs.

Relevant issues include:

- provider credential leakage through UI, logs, traces, session replay, or
  handoff packages;
- unsafe command execution or incorrect approval handling;
- unexpected access to files outside the intended project;
- leakage from `~/.tiffany`, `~/.orchestrator`, Claude Code state, or
  Codex-compatible runtime state.

Run `/doctor` or `orchestrator doctor` to inspect local runtime wiring before
sharing diagnostics.
