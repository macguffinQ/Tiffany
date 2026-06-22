# Execution Policy

Tiffany Loop can run tool-capable subprocess workers. Treat every configured
runtime as code that can read files, run shell commands, and write logs within
the permissions you grant.

Practical defaults:

- Run `orchestrator doctor` before long unattended jobs.
- Use environment references for secrets instead of literal keys in committed
  files.
- Keep generated worktrees, session logs, SQLite databases, and local config out
  of git.
- For Claude Code workers, choose whether unattended runs should use manual
  permission prompts or `behavior.cc_bypass_permissions`.
- Codex CLI workers: verify the installed binary supports `codex exec --cd`
  by running `orchestrator doctor`.

The fork still contains upstream `execpolicy` crates and approval plumbing for
compatibility with the terminal UI architecture. User-facing orchestration
policy should be documented and tested through Tiffany commands.
