# Security Policy

tiffany-loop is currently preview software. Treat it as a local developer tool
that can start subprocesses and pass prompts, files, and logs to configured
agent runtimes.

## Reporting

Please report security issues privately by contacting the maintainers before opening a public issue.

Include:

- Affected version or commit
- Reproduction steps
- Impact
- Any relevant logs with secrets removed

## Scope

Relevant issues include:

- Secret exposure
- Unsafe command execution
- Incorrect permission handling
- Session log leakage
- Malicious config or prompt injection paths that escape expected boundaries
- Provider credential leakage through TUI, logs, traces, or session replay
- Incorrect subprocess permission handling for Claude Code or Codex-compatible workers

## Local Data

tiffany-loop stores orchestrator state under `~/.orchestrator` by default and UI state under `~/.tiffany` by default. These directories can include session logs, process captures, provider references, TUI history, and local SQLite state. Do not publish those files unless you have reviewed and redacted them.
