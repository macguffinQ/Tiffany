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
5. **Orchestrator core** — Plan → Critique → Route → Execute → Review pipeline
6. **Observability** — terminal chat + structured JSON logs (tracing)
7. **Entry layer** — CLI (clap) / terminal chat / ACP / webhook (axum)

## 6 Roles

- **Planner** (default: Codex via direct API or CLI) — decomposes a task into a DAG
- **Critic** (default: CC) — red-teams the plan, forces re-plan on rejection
- **Router** (built-in) — 3-tier resolution: CLI flag > task tag > config default
- **Worker** (CC / Codex / direct) — executes a sub-task
- **Reviewer** (default: Codex cheap model) — gates worker output before merge
- **A/B Judge** (built-in) — picks winner from two parallel runs

## Pipeline

```
User task
   ↓
0a. Plan (Planner)
   ↓
0b. Critique (Critic)  ← reject → loop to 0a
   ↓
1.  Route (Router)
   ↓
2.  Workers (CC + Codex, parallel via DAG)
   ↓
0c. Review (Reviewer)  ← reject → loop to 2
   ↓
   Merge
   ↓
4.5. Session log (consumed by next agent)
```

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
cargo build --release
./scripts/tiffany-build --fast-release --locked
```

The build scripts use a shared Cargo target directory. The root release binary
lands at `target/release/orchestrator` for a normal release build, and the
primary UI binary lands next to it as `target/release/tiffany`. Fast
distributable builds use `target/tiffany-dist/`. The final binaries are small
relative to the build cache; use `./scripts/tiffany-build --fast-release --locked
--prune-dist-cache` or `./scripts/tiffany-clean-targets --dist-cache` when you
want to keep the runnable dist binaries but remove rebuildable dist internals.

## Terminal chat

The primary terminal UI is the tiffany-loop UI:

```bash
./scripts/tiffany-dev      # source checkout
tiffany orchestrator       # installed binary
orchestrator tui           # delegates to tiffany when installed
```

`orchestrator tui` falls back to the legacy terminal chat only when the `tiffany`
binary is unavailable, or when `ORCHESTRATOR_LEGACY_TUI=1` is set.

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
