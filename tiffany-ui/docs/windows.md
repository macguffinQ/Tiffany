# Windows Sandbox

Tiffany Loop inherits the forked terminal UI's Windows sandbox prompts and
runtime checks.

Use the sandbox prompts and `/doctor` output as the source of truth for the
current machine. In general:

- keep worker runtimes updated;
- run `orchestrator doctor` after changing permissions or runtime binaries;
- review commands before granting broad file or network access;
- redact `~/.tiffany`, `~/.orchestrator`, provider credentials, and worker
  runtime auth files from shared logs.

If Windows sandbox setup fails, include the Tiffany version, Windows version,
terminal, runtime route, and redacted `/doctor` output in the bug report.
