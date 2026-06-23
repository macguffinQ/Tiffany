# tiffany-loop architecture

tiffany-loop is a preview-stage multi-agent orchestration shell. The public
product name is `tiffany-loop`; the installed commands remain `orchestrator`
and `tiffany` for compatibility.

## Layers (7 + session log)

1. **Worker runtime** — `claude` CLI, `codex` CLI, direct API
2. **MCP tool pool** — shared fs / git / github / slack servers
3. **Adapter layer** — `WorkerAdapter` trait + 3 implementations (CC, Codex, direct)
4. **Shared state** — SQLite task queue + git worktree pool
4.5. **Session log** — JSONL per session + SQLite index; new agents can read prior sessions
5. **Orchestrator core** — route every turn to direct worker, single worker, or full Plan → Critique → Execute → Review
6. **Observability** — terminal chat + structured JSON logs (tracing)
7. **Entry layer** — CLI (clap) / terminal chat / ACP / webhook (axum)

## 6 Roles

- **Planner** (default: Codex via direct API or CLI) — decomposes full-pipeline implementation work into a DAG
- **Critic** (default: CC) — red-teams the plan, forces re-plan on rejection
- **Router** (built-in) — 3-tier resolution: CLI flag > task tag > config default
- **Worker** (CC / Codex / direct) — executes a routed worker run or one planned DAG task
- **Reviewer** (default: Codex cheap model) — gates implementation worker output before merge; direct and single-worker routes do not invoke it
- **A/B Judge** (built-in) — picks a winner from two configured worker-route runs using success status, diff size, and session-log size fallback

## Pipeline

```
User task
   ↓
0. Route classifier
   ├─ direct-answer → one worker answer
   ├─ single-worker → one worker run
   └─ full-pipeline
        ↓
     1a. Plan (Planner)
        ↓
     1b. Critique (Critic) ← reject → loop to 1a
        ↓
     2.  Workers (CC + Codex + direct, parallel via DAG)
        ↓
     3.  Review (Reviewer) ← implementation results only
        ↓
     Result
   ↓
4.5. Session log (consumed by next agent)
```

Conversational prompts such as greetings, simple Q&A, or explanations still
produce run events, but they route directly to one worker answer and do not
invoke planner, critic, or reviewer. External links, research, diagnostics, and
scaffolding can route to a single worker with the same route/worker/done
visibility. Only implementation work runs the full planner → critic → worker →
reviewer path. Older session logs may contain `ReviewSkipped` events from the
previous routing model; UI code keeps those readable for compatibility.

## Configuration

`~/.orchestrator/config.yaml` — see `config.example.yaml`. Three priority levels:

1. **CLI flag** (e.g. `--worker codex`)
2. **Task tag** mapping (e.g. `tag: refactor → worker-cc`)
3. **Default** in `roles:` section

## File layout

```
~/code/orchestrator/
├── Cargo.toml
├── config.example.yaml
├── README.md
├── LICENSE
├── scripts/                  # tiffany-loop dev/build/check entrypoints
├── tiffany-ui/               # primary tiffany-loop TUI fork
├── src/
│   ├── main.rs                # CLI entry
│   ├── lib.rs                 # library re-exports
│   ├── cli.rs                 # command dispatch
│   ├── config.rs              # YAML config + types
│   ├── core/                  # types, traits, session store
│   ├── adapters/              # claude_code, codex_cli, direct_api
│   ├── providers/             # anthropic, openai, google, ollama
│   ├── roles/                 # planner, critic, reviewer, router, ab_judge
│   ├── pipeline/orchestrator.rs
│   ├── storage/               # worktree, queue
│   ├── mux/                   # zellij + fallback
│   ├── tui/                   # legacy terminal chat fallback
│   ├── mcp/                   # MCP server pool
│   └── webhook/               # axum
└── tests/
```

## Building

```bash
./scripts/tiffany-build --locked
./scripts/tiffany-build --dev --locked
./scripts/tiffany-build --fast-release --locked
```

The build scripts use a shared Cargo target directory. Source-checkout builds
default to the smaller `dev-small` profile, so `./scripts/tiffany-build --locked`
writes local runnable binaries to `target/dev-small/`. Use
`./scripts/tiffany-build --dev --locked` only when you need Cargo's normal debug
profile and full debug symbols in `target/debug/`.

Normal release builds use `target/release/`. Fast distributable builds use
`target/tiffany-dist/` and install the root `orchestrator` binary next to the
primary `tiffany-loop` UI and the compatibility `tiffany` alias. The final
binaries are small relative to the build cache; use
`./scripts/tiffany-build --fast-release --locked --prune-dist-cache` or
`./scripts/tiffany-clean-targets --dist-cache` when you want to keep the runnable
dist binaries but remove rebuildable dist internals.

## Terminal chat

The primary terminal UI is the tiffany-loop UI:

```bash
./scripts/tiffany-dev      # source checkout
tiffany-loop               # installed binary
orchestrator tui           # delegates to tiffany-loop when installed
```

`orchestrator tui` falls back to the legacy terminal chat only when the
`tiffany-loop` binary is unavailable, or when `ORCHESTRATOR_LEGACY_TUI=1` is
set.

- The tiffany-loop UI preserves upstream Ratatui/Crossterm rendering, resize,
  history cells, bottom pane, overlays, and exit rendering behavior.
- Planner, critic, worker, reviewer, and final result events stream through the
  tiffany-loop orchestrator adapter.
- Follow-up prompts entered during a run stay queued at the bottom until they execute.
- Slash commands expose provider setup, role registration, sessions, logs,
  process capture, routing, context memory, and usage.
- Typing `/` opens a command menu. `Up`/`Down` select and `Enter` confirms.

## Webhook

The crate contains an axum webhook module, but the public CLI subcommand is not wired yet. The intended shape is:

```json
POST /run
{
  "prompt": "implement X",
  "tags": ["refactor"]
}
```

## zellij

If `ZELLIJ` env var is set, tiffany-loop auto-detects and uses zellij for tab/pane management. Otherwise it falls back to plain subprocesses.
