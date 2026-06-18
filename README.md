# tiffany-loop

[![CI](https://github.com/macguffinQ/Tiffany/actions/workflows/ci.yml/badge.svg)](https://github.com/macguffinQ/Tiffany/actions/workflows/ci.yml)
[![Release](https://github.com/macguffinQ/Tiffany/actions/workflows/release.yml/badge.svg)](https://github.com/macguffinQ/Tiffany/actions/workflows/release.yml)

[English](README.md) | [简体中文](README.zh-CN.md)

> A Rust-based multi-agent orchestration platform that unifies **Claude Code**, **Codex CLI**, and any other tool-using LLM behind a common adapter, with native **zellij** integration and a Claude Code-style terminal chat interface.

**Status:** Preview / beta. The CLI, config schema, TUI behavior, and release packaging may change before `1.0`.

The product name is **tiffany-loop**. The installed command names remain `tiffany` and `orchestrator` for compatibility with existing scripts and user workflows.

This is an independent community project. It is not an official product of OpenAI, Anthropic, MiniMax, or any model/runtime provider. Third-party names are used only to identify compatible tools, providers, and upstream open-source components.

```
$ ./scripts/tiffany-dev
tiffany-loop orchestrator
tiffany-loop orchestration mode

> implement fib
... planner / critic / Claude worker / reviewer ...
```

---

## What is this?

`tiffany-loop` lets you run multiple AI coding agents (Claude Code, Codex CLI runtimes, or any LLM) **in parallel, with shared context, and with adversarial review** - all from a single CLI, terminal chat, or ACP client.

The core idea: each "agent" is just a CLI subprocess (or a direct API call) wrapped in a common adapter. A planner decomposes your high-level task into sub-tasks, a critic red-teams the plan, workers execute in parallel, a reviewer gates the result, and every agent's session is logged and shared with future agents.

It's designed for software engineering tasks where:
- You want **multiple AI agents** to collaborate, not just one
- You want a **planner → critic → worker → reviewer** pipeline (not just chat)
- You want **shared context** across agents (Claude Code's prior sessions, orchestrator's prior sessions, project rules)
- You want **observability** (who did what, cost, tokens)
- You want to **stay in the terminal** (terminal chat, zellij integration, no web UI)

## Which command should I use?

For normal interactive use, run `tiffany orchestrator` after install, or `./scripts/tiffany-dev` from a source checkout.

| Name | What it is | Use it for |
|---|---|---|
| `tiffany-loop` | Product/package name | Releases, Homebrew, docs, GitHub project identity |
| `tiffany` | Primary TUI binary | Interactive chat, provider setup, role setup, multi-turn orchestration |
| `orchestrator` | Runtime/control binary | Config, events, ACP server, non-interactive runs, scripting |
| `./scripts/tiffany-dev` | Source checkout helper | Local development without installing binaries |

The split is intentional: `tiffany` owns the terminal experience, while
`orchestrator` owns provider config, role routing, event streaming, and runtime
execution. `orchestrator tui` delegates to `tiffany orchestrator` when the TUI
binary is installed.

## UI direction: tiffany-loop UI first

The active UI path is now a full tiffany-loop UI under [`tiffany-ui/`](tiffany-ui/). New terminal UI work should happen there, keeping the upstream Ratatui/Crossterm architecture, resize handling, copy/selection behavior, history cells, bottom pane, overlays, and exit rendering intact.

The legacy [`src/tui`](src/tui/) path remains only as a compatibility bridge for the existing orchestrator runtime. Do not add new hand-written terminal rendering there unless it is a narrow bug fix needed before the tiffany-loop UI adapter replaces it.

Development entrypoints:

- `./scripts/tiffany-dev` - run the tiffany-loop orchestrator in tiffany-loop orchestration mode, using the local debug binary and waiting for input.
- `./scripts/tiffany-dev --help` - explain the source-checkout entrypoints without building or launching the UI.
- `./scripts/tiffany-dev config ...` - run the local orchestrator config command directly. It does not launch the TUI or require a UI login; use it for scriptable provider setup before opening the TUI.
- `./scripts/tiffany-dev orchestrator "..."` - run the orchestrator pipeline immediately with an initial prompt; native mode defaults worker execution to Claude Code (`worker-cc`), and later input-box submissions are routed into the same orchestrator pipeline.
- `./scripts/tiffany-dev orchestrator --orchestrator-config /path/to/config.yaml` - use a specific orchestrator config while keeping tiffany-loop UI config isolated under `TIFFANY_HOME`.
- `./scripts/tiffany-dev orchestrator --legacy ...` - compatibility bridge to the old orchestrator CLI.
- `./scripts/tiffany-build [cargo-build-args...]` - build both the parent orchestrator binary and the tiffany-loop UI in the shared `./target` directory. Use `--fast-release` for a faster distributable build.
- `./scripts/tiffany-check` - build, format-check the fork, and verify the legacy bridge plus event-stream entrypoint.

tiffany-loop keeps fork state separate from upstream UI source. The fork uses `TIFFANY_HOME`, defaulting to `~/.tiffany`, and maps UI-internal config reads to `~/.tiffany/config.toml` instead of the upstream default config directory. SQLite state defaults to the same root through `TIFFANY_SQLITE_HOME`. Override with `TIFFANY_HOME=/path/to/tiffany-home`.

When running the source helper or fork binary directly, set `TIFFANY_ORCHESTRATOR_BIN=/path/to/orchestrator` or pass `tiffany orchestrator --bin /path/to/orchestrator`.
Installed builds include both `orchestrator` and `tiffany`; `orchestrator tui` delegates to `tiffany orchestrator` when the `tiffany` binary is installed, and falls back to the legacy terminal chat only when it is missing. Set `ORCHESTRATOR_LEGACY_TUI=1` to force the fallback.

---

## Features

### 6 roles, 4 providers, infinite combinations

| Role | Default | Configurable to |
|---|---|---|
| **Planner** | GPT-4o (via Codex CLI) | Any model + Claude/Codex CLI runtime |
| **Critic** | Claude Opus (via CC) | Any model + Claude/Codex CLI runtime |
| **Worker (CC)** | Claude Sonnet (Agent Teams) | Any model + runtime |
| **Worker (Codex)** | GPT-4o | Any model + runtime |
| **Reviewer** | GPT-4o-mini | Any model + Claude/Codex CLI runtime |
| **A/B Judge** | Built-in (tests + diff size) | Custom rule |

Providers: **Anthropic**, **OpenAI**, **Google Gemini**, **Ollama** (local), or any OpenAI-compatible endpoint.

### 7+1 layer architecture

```
1. Worker runtime       CC CLI / Codex CLI / direct API
2. MCP tool pool        fs / git / github / slack / ... (shared across workers)
3. Adapter layer        ModelProvider + WorkerRuntime — config-driven
4. Shared state         SQLite task queue + git worktree pool
4.5 Session log         JSONL + SQLite index — injected into new agents
5. Orchestrator core    Plan → Critique → Route → Execute → Review
6. Observability        terminal chat + structured JSON logs
7. Entry layer          CLI / terminal chat / ACP / webhook (axum)
```

### Pipeline

```
                  ┌─ your prompt ─┐
                  └───────┬───────┘
                          ▼
              ┌─────────────────────┐
              │ 0a. Plan (LLM)      │  decomposes into sub-tasks
              └──────────┬──────────┘
                         ▼
              ┌─────────────────────┐
              │ 0b. Critique (LLM)  │  red-teams, forces re-plan on reject
              └──────────┬──────────┘
                         ▼
              ┌─────────────────────┐
              │ 1. Route            │  CLI flag > task tag > config default
              └──────────┬──────────┘
                         ▼
       ┌─────────────────┴─────────────────┐
       ▼                                   ▼
   ┌────────┐                          ┌────────┐
   │2a. CC  │                          │2b. Codex CLI│
   │Agent   │                          │subproc │
   │Teams   │                          │        │
   └────┬───┘                          └───┬────┘
        └─────────────────┬───────────────┘
                          ▼
              ┌─────────────────────┐
              │ 0c. Review (LLM)    │  gates the merge
              └──────────┬──────────┘
                         ▼
                    merged
                         │
                         ▼
       Layer 4.5: Session log (consumed by next agent)
```

### 3-tier role resolution

| Priority | Source | Example |
|---|---|---|
| 1 (highest) | CLI flag | `--worker worker-cc` |
| 2 | Task tag | `--tag refactor` → `worker-cc` |
| 3 (lowest) | `config.yaml` default | `roles.worker-cc.model = sonnet` |

### Shared session log

Every agent's session is persisted to `~/.orchestrator/sessions/<uuid>.jsonl` with a SQLite index. New agents automatically **read prior sessions as injected context** — both orchestrator's own sessions and Claude Code's `~/.claude/projects/<slug>/sessions/*.jsonl`.

### Reads your existing config

- **Claude Code's** `CLAUDE.md`, `settings.json`, `agents/*.md`, `commands/*.md`, `.mcp.json`, prior sessions
- **Orchestrator's own** `AGENTS.md` (`~/.orchestrator/AGENTS.md` + `<project>/.orchestrator/AGENTS.md` + `<project>/AGENTS.md`)

All injected as system-prompt sections, in priority order: **AGENTS.md > CLAUDE.md > history > CC prior sessions**.

### Terminal Chat

- The active UI direction is the full tiffany-loop UI under `tiffany-ui/`.
- `tiffany orchestrator "..."` streams planner, critic, worker, reviewer, and final-result events into tiffany-loop history cells.
- In orchestrator mode, the tiffany-loop input box submits follow-up prompts to the orchestrator adapter instead of the normal tiffany-loop model turn.
- The legacy runner keeps a single conversation view with normal terminal scrollback.
- Native text selection, copy, paste, and mouse scrolling are handled by your terminal.
- The legacy runner still has upstream-derived terminal primitives, but new UI behavior belongs in the fork.
- Follow-up messages entered during a run stay in tiffany-loop bottom pending queue; plain prompts merge into the next orchestrator run when the current run finishes.
- Final worker output is shown as a plain selectable result block.
- Slash commands cover routing, context memory, process capture, sessions, logs, queue control, and usage.
- In tiffany-loop orchestrator mode, `/provider` is native to the tiffany-loop TUI: `/provider` opens a setup form with separate provider/type/env/key/endpoint fields, `/provider edit openai` edits an existing provider with prefilled values, `/provider list` shows config, `/provider env openai OPENAI_API_KEY` stores an env-var reference, and `/provider endpoint openai https://api.openai.com/v1` stores the endpoint.
- In tiffany-loop orchestrator mode, `/role` opens a dedicated role-registration form with role/provider/model/runtime/team fields; `/roles` remains the list/CLI-style command surface.
- Typing `/` opens a command menu; `Up`/`Down` selects and `Enter` confirms.

### zellij-native

- `orchestrator tui` runs in the current terminal by default.
- `orchestrator tui --new-tab` opens a dedicated zellij tab when already inside zellij.
- `orchestrator tui --detach` runs terminal chat in the background outside zellij (PID file in `~/.orchestrator/tui.pid`).

---

## Install

Recommended for users:

```bash
brew tap macguffinQ/tap
brew install tiffany-loop
tiffany orchestrator
```

The Homebrew package installs both commands:

- `tiffany` - the primary terminal UI. Start with `tiffany orchestrator`.
- `orchestrator` - config, roles, event streaming, ACP, and non-interactive runs.

After install:

```bash
orchestrator init
tiffany orchestrator
```

Inside the TUI, use `/provider` to configure providers and `/role` to register
planner, critic, worker, and reviewer roles.

Prebuilt macOS Apple Silicon binaries are published on the GitHub Releases page after each `v*` tag. Archives include both commands. Linux, Windows, and Intel Mac users can install from source while additional prebuilt targets are being staged.

Source checkout for contributors:

```bash
git clone https://github.com/macguffinQ/Tiffany.git ~/code/orchestrator
cd ~/code/orchestrator
./scripts/tiffany-build
./scripts/tiffany-dev
```

Manual source install, if you want the commands in `PATH` without Homebrew:

```bash
cargo install --path . --profile tiffany-dist
cargo install --path tiffany-ui/codex-rs/cli --profile tiffany-dist
```

```bash
# Example: install a downloaded archive
tar -xzf tiffany-loop-v0.1.5-aarch64-apple-darwin.tar.gz
cd tiffany-loop-v0.1.5-aarch64-apple-darwin
chmod +x orchestrator tiffany
./orchestrator status
./tiffany orchestrator
```

The `tiffany-loop` package installs both `orchestrator` and `tiffany`; the current GitHub repository name remains `Tiffany`.
Tag releases prioritize the macOS Apple Silicon archive used by Homebrew; Intel Macs, Linux, and Windows can install from source for now.
Public Homebrew installs require the `macguffinQ/Tiffany` repository and release assets to be public.

## Quickstart

```bash
# 1. Initialize
orchestrator init
# writes ~/.orchestrator/config.yaml

# 2. Set API keys (one of these is enough)
export ANTHROPIC_API_KEY=sk-...
export OPENAI_API_KEY=sk-...

# 3. Run a task
cd ~/your-project
orchestrator run "implement fibonacci in src/fib.rs"

# 4. With tag-based routing
orchestrator run "refactor the auth module" --tag refactor

# 5. Override model
orchestrator run "fix typo" --planner opus --worker codex

# 6. A/B dual-run
orchestrator run "optimize the query" --ab

# 7. Open the tiffany-loop UI from source
./scripts/tiffany-dev

# Or, after install
tiffany orchestrator
orchestrator tui

# 8. Browse past sessions
orchestrator sessions list
orchestrator sessions show <id>
orchestrator sessions grep "rate limit"

# 9. See everything the orchestrator loaded
orchestrator config

# 10. Import your existing CC sessions
cd ~/your-project  # where you used CC before
orchestrator sessions import-cc
```

---

## Configuration

tiffany-loop has two config roots:

- Orchestrator runtime: `~/.orchestrator/config.yaml`
- tiffany-loop UI: `~/.tiffany/config.toml` by default, or `$TIFFANY_HOME/config.toml`; SQLite state uses `$TIFFANY_SQLITE_HOME` or the same root

`~/.orchestrator/config.yaml` — three priority levels:

```yaml
providers:
  anthropic: { type: anthropic, api_key: ${ANTHROPIC_API_KEY} }
  openai:    { type: openai,    api_key: ${OPENAI_API_KEY} }
  minimax:   { type: openai,    api_key: ${MINIMAX_API_KEY}, base_url: https://api.minimaxi.com/v1 }
  google:    { type: google,    api_key: ${GOOGLE_API_KEY} }
  ollama:    { type: ollama,    base_url: http://localhost:11434 }

models:
  - { id: opus,        provider: anthropic, name: claude-opus-4-6 }
  - { id: sonnet,      provider: anthropic, name: claude-sonnet-4-6 }
  - { id: gpt4o,       provider: openai,    name: gpt-4o }
  - { id: gpt4o-mini,  provider: openai,    name: gpt-4o-mini }
  - { id: minimax-m3-claude, provider: minimax, name: MiniMax-M3 }
  - { id: minimax-m3-codex,  provider: minimax, name: MiniMax-M3 }

roles:
  planner:    { model: gpt4o-mini, runtime: codex }
  critic:     { model: opus,       runtime: claude-code }
  worker-cc:  { model: sonnet,     runtime: claude-code, agent_teams: true }
  worker-codex: { model: gpt4o,    runtime: codex }
  reviewer:   { model: gpt4o-mini,  runtime: codex }

overrides:                            # tag-based routing
  - { tag: refactor,    role: worker-cc }
  - { tag: boilerplate, role: worker-codex }

behavior:
  worktree_base:   ~/.orchestrator/worktrees
  db_path:         ~/.orchestrator/state.db
  session_log_dir: ~/.orchestrator/sessions
  mux:             zellij              # zellij | tmux | none
  enable_critic:   true
  enable_reviewer: true
  max_replan:      2
  cc_bypass_permissions: true          # Claude Code roles/workers run non-interactively
```

Missing env vars → empty string (no error). You can run `orchestrator status` before setting any keys.
`cc_bypass_permissions` passes `--permission-mode bypassPermissions` to spawned Claude Code processes. Set it to `false` only when you want Claude Code to stop for manual permission prompts; tiffany-loop TUI `/permissions` controls the tiffany-loop UI itself, not these Claude subprocesses.

---

## Commands

| Command | What it does |
|---|---|
| `./scripts/tiffany-dev` | Run the tiffany-loop UI |
| `./scripts/tiffany-dev orchestrator` | Bridge from the tiffany-loop fork into the existing orchestrator runtime |
| `./scripts/tiffany-build [args]` | Build both binaries in shared `./target`; forwards args such as `--release --locked`, or use `--fast-release --locked` |
| `./scripts/tiffany-check` | Run the local fork/bridge verification |
| `./scripts/tiffany-clean-targets` | Remove the old `tiffany-ui/codex-rs/target` build cache |
| `tiffany orchestrator` | Open the primary tiffany-loop UI after install |
| `orchestrator init` | Generate `~/.orchestrator/config.yaml` |
| `orchestrator run "..."` | Run a task (with optional `--planner`, `--critic`, `--worker`, `--reviewer`, `--tag`, `--ab`, `--no-critic`, `--no-reviewer`) |
| `orchestrator config provider` | Open the guided provider selector |
| `orchestrator config provider setup <provider>` | Configure a provider from built-in presets |
| `orchestrator config provider delete <provider>` | Delete one configured provider |
| `orchestrator config provider list|presets` | Inspect configured providers or built-in presets |
| `orchestrator roles list` | List registered planner/critic/worker/reviewer roles |
| `orchestrator roles register <role> --model <id> --runtime <runtime>` | Register or update a role binding |
| `orchestrator tui` | Open the primary tiffany-loop UI when `tiffany` is installed; otherwise fall back to legacy terminal chat |
| `orchestrator tui --ratatui` | Legacy compatibility flag; kept only for old scripts |
| `orchestrator tui --new-tab` | Open terminal chat in a new zellij tab |
| `orchestrator tui --detach` | Run terminal chat in background outside zellij (PID file at `~/.orchestrator/tui.pid`) |
| `orchestrator acp` | Run an Agent Client Protocol server over stdio |
| `orchestrator sessions list` | List recent sessions |
| `orchestrator sessions show <id>` | Show a session's event log |
| `orchestrator sessions grep <pattern>` | Grep across all session logs |
| `orchestrator sessions import-cc` | Import your existing CC sessions from `~/.claude/projects/<slug>/sessions/` |
| `orchestrator config` | Show everything the orchestrator loaded (config + AGENTS.md + CC config) |
| `orchestrator status` | Show paths, mux, env |
| `orchestrator doctor` | Diagnose config, runtime binaries, API keys, role wiring, and local tools |

---

## Terminal UI

The target terminal UI is the full tiffany-loop UI in [`tiffany-ui/`](tiffany-ui/). From a source checkout:

```bash
./scripts/tiffany-dev
```

After install:

```bash
tiffany orchestrator
orchestrator tui
```

To pass explicit orchestrator arguments during migration:

```bash
./scripts/tiffany-dev orchestrator
```

`orchestrator tui` now delegates to `tiffany orchestrator` when it can find the `tiffany` binary next to `orchestrator` or on `PATH`. If `tiffany` is missing it falls back to the old compatibility runner. Set `ORCHESTRATOR_LEGACY_TUI=1` to force the fallback.

Forward explicit orchestrator arguments after the subcommand:

```bash
./scripts/tiffany-dev orchestrator status
./scripts/tiffany-dev orchestrator -- --help
```

Useful commands:

- `/help` - show commands
- `/doctor` - diagnose config, runtimes, API keys, role wiring, and local tools
- `/provider [setup|edit <provider>]|list|delete <provider>|env <provider> <ENV>|key <provider> <value>|endpoint <provider> <url>` - open or edit a provider setup form, inspect config, delete a provider, or configure orchestrator providers
- `/role [<role>|register <role> --model <id> --runtime <runtime>]` - open the role-registration form or register one role
- `/roles show|route|use <role>|snippet <role> <model> <runtime>|save <role> <model> <runtime>` - inspect configured commander/critic/executor/reviewer roles, select a worker route, print a snippet, or write a role to config
- `/workflow` - show the active planner -> critic -> worker -> reviewer pipeline
- `/agent claude|codex|auto` - route future worker tasks
- `/context compact|full|off|clear` - control multi-turn memory
- `/process summary|full|200` - inspect captured run events without raw JSON noise
- `/trace full` - show more live process detail in the chat
- `/queue pause|resume|run|edit n text` - hold, resume, run, or edit pending follow-up messages
- `/diff summary|stat|full|cached` - inspect current git changes
- `/tests suggest|quick|run quick|status|tail` - show, run, and inspect background test commands; `quick` runs the focused TUI + ACP + runtime checks
- `/handoff claude|codex` - save a markdown context package for continuing in another CLI
- `/handoff open claude|codex` - save a handoff package and open the target CLI inline
- `/continue claude|codex` - save a handoff package, suspend orchestrator input mode, and open the target CLI inline
- `/graph compact|full|mermaid|save` - compress recent conversation history into text or Mermaid flow graphs
- `/acp status|claude|codex` - show Agent Client Protocol server command and client setup hints
- `/checkpoint` and `/rollback last` - save and reverse-apply a tracked git patch checkpoint
- `/result` and `/copy result` - replay or copy the last final result as plain text
- `/usage today|week|month|all` - show token/cost usage
- `/o` - fold or expand history summaries
- `/resume last` - restore the last terminal chat snapshot

The bottom status line follows tiffany-loop-style run HUD behavior: it keeps one compact line for stage, elapsed time, worker route, context mode, queued message count, detail fold state, process filter, and review/worker issue counters.

Troubleshooting first:

- Run `/doctor` or `orchestrator doctor` when a worker exits early, a provider says `model not found`, `模型不存在`, `401/403`, or an API key appears unset.
- Doctor checks env-var key references without printing secrets, verifies `role -> model -> provider -> runtime`, catches duplicate/missing models, and warns when Claude Code workers may pause for manual permission prompts.
- For model errors, confirm the role's internal model id points to the intended provider API model name: `/role <role>` or `orchestrator roles register <role> --model <id> --provider <provider> --model-name <api-model> --runtime <runtime>`.

Input behavior:

- Press `Enter` to send.
- In `tiffany orchestrator "..."`, pressing `Enter` starts another orchestrator run from the same tiffany-loop TUI session.
- While a run is active, normal messages are queued at the bottom, merged into the next batch, and run together when the current task finishes.
- The bottom queue previews the next 4 items; `/queue show` prints the full queued batch.
- Use `/queue pause` to hold queued messages after the current run; `/queue resume` or `/queue run` starts them when idle.
- Use `Up`/`Down` to recall prompt history.
- Type `/` to open the command menu. Use `Up`/`Down` to select and `Enter` to confirm; continuing to type filters the menu.
- Use `Ctrl+C` to cancel an active run; with no active run, it exits.

Provider setup notes:

- Configure provider credentials first, then bind roles to models/runtimes with `/role`.
- `/provider` opens an OpenClaw-style setup panel inside the tiffany-loop bottom pane, with separate provider, type, env, key, and endpoint fields.
- The panel shows a preset summary, auth status (`set`/`unset` for env refs, literal-key warning, or no-auth for Ollama), and the exact `config provider setup ...` write that will be executed.
- Fields marked with `▾` have scrollable choices with hints: press `Space` or `F4`, use `Up`/`Down` or number keys, then `Enter` to apply. Picking a provider fills its default type/env/endpoint.
- Selection fields such as provider/type and role/provider/model/runtime are locked to choices; free text remains available for key/env/endpoint. In `/role`, users choose only `API Model`; the internal orchestrator model id is generated automatically. Built-in choices are filtered by provider, while `custom`/`none` allows manual model text.
- `/provider edit minimax` opens the form prefilled from `~/.orchestrator/config.yaml` when the provider already exists.
- Literal existing keys are not displayed; `Key: <unchanged>` keeps the stored value.
- `/provider list` prints the current orchestrator config.
- `/provider delete minimax` removes one provider from `~/.orchestrator/config.yaml`.
- `/provider env openai OPENAI_API_KEY` writes `api_key: ${OPENAI_API_KEY}` without expanding or leaking the key.
- `/provider key openai sk-...` writes a literal key when you intentionally want the config file to hold it.
- `/provider endpoint openai https://api.openai.com/v1` sets an OpenAI-compatible endpoint.
- `/provider openai $OPENAI_API_KEY https://api.openai.com/v1` is a shortcut for key plus endpoint.

The same provider config can be written before opening the TUI with the OpenClaw-style setup helper:

```bash
./scripts/tiffany-dev config provider
./scripts/tiffany-dev config provider ui --dry-run
./scripts/tiffany-dev config provider presets
./scripts/tiffany-dev config provider setup minimax
./scripts/tiffany-dev config provider delete minimax
./scripts/tiffany-dev config provider setup custom --env CUSTOM_API_KEY --endpoint https://llm.example.com/v1
./scripts/tiffany-dev config provider list
```

`config provider` and `config provider ui` open a guided selector for setup, delete, or list. Setup mode exposes provider/type/auth/env/endpoint choices. `config provider setup ...` and `config provider delete ...` are the non-interactive forms for scripts and CI.

Role routing notes:

- Register roles from the TUI with `/role`; use `/roles` for list/show and CLI-style role commands. The tiffany-loop UI reads `~/.orchestrator/config.yaml` when it starts.
- `/role` opens a dedicated form with role, provider, `API Model`, runtime, and agent-team mode.
- `/role worker-codex` opens the same form prefilled for that role; `/role register ...` dispatches directly to the role command.
- `/roles use <role>` applies to future worker tasks in the current terminal chat.
- Planner, critic, reviewer, and worker roles all use their configured `runtime` and resolved model name.
- You can register multiple Claude Code workers. `worker-cc` is only the default example; roles such as `worker-cc-minimax`, `worker-cc-sonnet`, or `executor-ui` work as long as their `runtime` is `claude-code`.
- `/roles save <role> <model> <runtime>` writes a role into `~/.orchestrator/config.yaml`; restart terminal chat after changing planner/critic/reviewer so the execution pipeline is rebuilt.
- In the tiffany-loop UI, use `/role`, `/roles`, `/roles show <role>`, or `/roles register <role> --model <id> --runtime <runtime>` directly from the input box.

Role registration examples:

```bash
orchestrator roles register planner --model gpt4o --runtime codex
orchestrator roles register critic --model glm51 --provider openai --model-name glm-5.1 --runtime codex
orchestrator roles register worker-cc --model minimax-m3 --provider openai --model-name minimax-m3 --runtime claude-code --agent-teams
orchestrator roles register worker-cc-sonnet --model sonnet --runtime claude-code --agent-teams
```

---

## zellij integration

The terminal chat stays in the current pane by default. Use `orchestrator tui --new-tab` when you want a dedicated zellij tab. Outside zellij, `orchestrator tui --detach` daemonizes the terminal chat and writes its PID to `~/.orchestrator/tui.pid`.

---

## Customizing behavior

### Your platform rules: `~/.orchestrator/AGENTS.md`

```markdown
# My orchestrator rules

Default worker: sonnet
Default critic: opus
This project uses uv not pip
Always run `pytest` before declaring done
For refactors, prefer /docs/style.md conventions
```

This is the **highest-priority** system prompt section injected into every worker.

### Your project rules: `<project>/CLAUDE.md` (already used by CC)

The orchestrator reads this and passes it through to CC workers — so you don't have to duplicate your CC project rules.

### Custom CC subagents: `<project>/.claude/agents/*.md`

```markdown
---
name: reviewer
description: Reviews PRs
tools: [Read, Grep]
model: sonnet
---
You are a careful code reviewer.
```

Orchestrator picks these up automatically for context.

### MCP servers: `<project>/.mcp.json`

Orchestrator auto-merges these into its MCP pool, so workers have access to `github`, `slack`, etc. without any extra config.

---

## Architecture in depth

See `docs/architecture.md` for the full layered architecture.

### Why Rust?

- **Speed**: 18 MB single binary, sub-100ms startup
- **zellij-grade reliability**: matches the tool's ecosystem
- **Async-first**: tokio for subprocess management + event streaming
- **No GC pauses**: stays responsive in terminal chat mode
- **Mature crates**: `crossterm`, `axum`, `rusqlite`, `git2`, `reqwest`, `tokio` - all battle-tested

### Why subprocess-based CC integration (not the SDK)?

- The `claude-agent-sdk` is Python/TS only — we'd lose single-binary
- CC's own session JSONL files (`~/.claude/projects/.../sessions/*.jsonl`) are the source of truth
- CC's CLI is the most stable API — SDK changes don't affect us
- We read CC's events from the JSONL stream in real time

### Why a thin top-level instead of a custom DAG scheduler?

Agent Teams (CC's experimental multi-agent feature) already does shared task lists, mailboxes, and hooks. We use it as-is and add a thin shim for: cross-agent session sharing, A/B judging, capability-based routing, and terminal chat.

---

## Development

```bash
# Build
cargo build              # debug, ~30s incremental
cargo build --release    # release, ~5-8min first time
./scripts/tiffany-build   # orchestrator + tiffany-loop UI
./scripts/tiffany-build --release --locked
./scripts/tiffany-build --fast-release --locked

# Test
cargo test               # run unit and integration tests

# Lint
cargo clippy
cargo fmt

# Run from source
./scripts/tiffany-dev
cargo run -- run "hello"
cargo run -- config
```

### Layout

```
orchestrator/
├── Cargo.toml
├── config.example.yaml
├── docs/architecture.md
├── src/
│   ├── main.rs                  # CLI entry
│   ├── cli.rs                   # command dispatch
│   ├── config.rs                # YAML config + types
│   ├── core/
│   │   ├── types.rs             # Task, Session, Event, Role
│   │   ├── worker.rs            # WorkerAdapter trait
│   │   ├── provider.rs          # ModelProvider trait
│   │   └── session_store.rs     # JSONL + SQLite index
│   ├── adapters/
│   │   ├── claude_code.rs       # CC via subprocess
│   │   ├── codex_cli.rs         # Codex CLI via subprocess
│   │   └── direct_api.rs        # via LLM SDK
│   ├── providers/               # Anthropic, OpenAI, Google, Ollama
│   ├── roles/
│   │   ├── planner.rs / critic.rs / reviewer.rs   # stubs
│   │   ├── llm.rs               # REAL LLM-backed implementations
│   │   ├── router.rs            # 3-tier capability routing
│   │   └── ab_judge.rs          # pick winner
│   ├── pipeline/orchestrator.rs # main pipeline
│   ├── storage/                 # worktree pool, task queue
│   ├── mux/                     # zellij + fallback
│   ├── tui/                     # terminal chat
│   ├── mcp/                     # MCP server pool
│   ├── webhook/                 # axum
│   ├── agent_md.rs              # orchestrator's own AGENTS.md reader
│   ├── cc_config.rs             # CC's CLAUDE.md / settings / agents reader
│   └── cc_session_import.rs     # CC session JSONL reader
└── tests/
```

### Adding a new provider

1. Add a `ProviderConfig` to `config.yaml` with a new `type: myprovider`
2. Create `src/providers/myprovider.rs` implementing `ModelProvider`
3. Register in `src/providers/mod.rs`
4. Add to `build_provider` in `src/cli.rs`

### Adding a new role

1. Create `src/roles/myrole.rs` with a struct + `async_trait` impl
2. Register in `src/roles/mod.rs`
3. Wire into `build_orchestrator` in `src/cli.rs`

---

## Roadmap

- [x] 7+1 layer architecture
- [x] 3 capability providers + 3 worker runtimes
- [x] Subagent / Critic / Reviewer / Planner (LLM-backed)
- [x] Shared session log (orchestrator + CC)
- [x] Reads CC's CLAUDE.md / settings / agents / MCP / sessions
- [x] Reads orchestrator's own AGENTS.md
- [x] 3-tier role priority (CLI > tag > config)
- [x] Single-pane terminal chat
- [x] Real-time process progress in terminal chat
- [x] Pauseable/editable terminal follow-up queue
- [x] Final result replay/copy
- [x] Terminal `/diff` and `/tests` helpers
- [x] Role registry view and per-session role selection
- [x] Claude/Codex CLI handoff packages
- [x] Conversation flow graph summaries
- [x] Patch checkpoint and rollback commands
- [x] Optional zellij tab + --detach
- [x] A/B judge
- [x] Token/cost usage command
- [x] Webhook server (axum)
- [x] Streaming process summaries in terminal chat
- [x] Full tiffany-loop fork under `tiffany-ui/`
- [x] tiffany-loop CLI binary in the tiffany-loop UI
- [x] `tiffany orchestrator "..."` native tiffany-loop TUI event adapter
- [x] Multi-turn orchestrator submissions from the native tiffany-loop input box
- [x] `tiffany orchestrator --legacy ...` bridge to the existing runtime
- [x] Vendored tiffany-loop TUI source snapshot under `third_party/openai-codex/codex-rs/tui`
- [x] Native tiffany-loop adapter inside the tiffany-loop TUI app/session layer
- [x] Make the tiffany-loop UI the default `orchestrator tui` command path when `tiffany` is installed
- [x] Release/Homebrew packaging installs both `orchestrator` and `tiffany`
- [ ] Remove copied partial TUI modules after the fork adapter is stable
- [ ] Full-token streaming final responses in terminal chat
- [ ] Background task mode (`orchestrator run --detach` + `orchestrator attach`)
- [ ] CC agent invocation from orchestrator (`--agent reviewer`)
- [ ] Session export (markdown / HTML)
- [ ] Cost budget alerts
- [ ] VS Code extension

---

## License

MIT
