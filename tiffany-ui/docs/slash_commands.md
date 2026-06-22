# Slash Commands

Tiffany Loop exposes orchestration commands from the input box. Type `/` to open
the command menu and use the arrow keys to select an entry.

Core commands:

- `/provider` opens provider setup, edit, list, env, endpoint, and delete flows.
- `/role` opens role registration for planner, critic, worker, and reviewer.
- `/roles` shows current role routing, health, snippets, and save helpers.
- `/doctor` diagnoses provider, model, runtime, Homebrew, and worker CLI issues.
- `/agent` selects the worker route.
- `/workflow` shows the active planner -> critic -> worker -> reviewer flow.
- `/sessions`, `/trace`, `/process`, and `/result` inspect prior runs.
- `/handoff` and `/continue` prepare continuation in Claude Code or Codex CLI.
- `/acp` shows Agent Client Protocol server/client setup hints.

The command menu intentionally routes Tiffany orchestration commands through the
parent `orchestrator` runtime. Generic upstream UI commands may still exist in
the fork where they support terminal behavior, but Tiffany setup should use the
commands above.
