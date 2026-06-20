# tiffany-loop

[![CI](https://github.com/macguffinQ/Tiffany/actions/workflows/ci.yml/badge.svg)](https://github.com/macguffinQ/Tiffany/actions/workflows/ci.yml)
[![Release](https://github.com/macguffinQ/Tiffany/actions/workflows/release.yml/badge.svg)](https://github.com/macguffinQ/Tiffany/actions/workflows/release.yml)

[English](README.md) | [简体中文](README.zh-CN.md)

> A Rust-based multi-agent orchestration platform that unifies **Claude Code**, **Codex CLI**, and any other tool-using LLM behind a common adapter, with native **zellij** integration and a Claude Code-style terminal chat interface.

**Status:** Preview / beta. The CLI, config schema, TUI behavior, and release packaging may change before `1.0`.

The product name and primary installed command are **tiffany-loop**. The older `tiffany` UI command and the lower-level `orchestrator` runtime command remain for compatibility and scripting.

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

For normal interactive use, run `tiffany-loop setup` once, then start the UI with `tiffany-loop`. From a source checkout, use `./scripts/tiffany-dev setup` and `./scripts/tiffany-dev`.

The shortest path is:

```bash
# Installed with Homebrew or a release archive:
tiffany-loop setup
tiffany-loop

# Source checkout:
./scripts/tiffany-dev setup
./scripts/tiffany-dev
```

Inside the UI, configure credentials with `/provider`, register roles with
`/role`, inspect role wiring with `/roles`, and use `/doctor` when a worker
fails because of a provider, model, key, or runtime mismatch.

| Name | What it is | Use it for |
|---|---|---|
| `tiffany-loop` | Primary UI command / package name | Interactive chat, provider setup, role setup, multi-turn orchestration |
| `tiffany` | Compatibility UI alias | Existing scripts that still call `tiffany orchestrator` |
| `orchestrator` | Runtime/control binary | Config, events, ACP server, non-interactive runs, scripting |
| `./scripts/tiffany-dev` | Source checkout helper | Local development without installing binaries |

The split is intentional: `tiffany-loop` is the user-facing shell, while
`orchestrator` owns provider config, role routing, event streaming, and runtime
execution. Common runtime commands are available at the top level, so
`tiffany-loop setup`, `tiffany-loop status`, `tiffany-loop doctor`, and
`tiffany-loop config ...` forward to the matching `orchestrator` commands.
`orchestrator tui` delegates to `tiffany-loop orchestrator` when the UI binary is
installed.

## Try The Minimal Example

After setup, use the zero-dependency example project to verify that the UI,
role routing, worker execution, and test-running loop are wired correctly:

```bash
cd examples/python-fibonacci
python3 -m unittest discover -s tests
tiffany-loop
```

Then ask:

```text
Add a lucas(n) function to fibonacci.py, add unit tests, and run python3 -m unittest discover -s tests.
```

The same task can be run non-interactively:

```bash
orchestrator run "Add a lucas(n) function to fibonacci.py, add unit tests, and run python3 -m unittest discover -s tests."
```

## UI direction: tiffany-loop UI first

The active UI path is now a full tiffany-loop UI under [`tiffany-ui/`](tiffany-ui/). New terminal UI work should happen there, keeping the upstream Ratatui/Crossterm architecture, resize handling, copy/selection behavior, history cells, bottom pane, overlays, and exit rendering intact.

The legacy [`src/tui`](src/tui/) path remains only as a compatibility bridge for the existing orchestrator runtime. Do not add new hand-written terminal rendering there unless it is a narrow bug fix needed before the tiffany-loop UI adapter replaces it.

Development entrypoints:

- `./scripts/tiffany-dev` - run the tiffany-loop orchestrator in tiffany-loop orchestration mode, reusing `./target/dev-small/orchestrator` and `./target/dev-small/tiffany-loop` after the first build, then waiting for input. Set `TIFFANY_DEV_PROFILE=dev` when you need full debug symbols.
- `./scripts/tiffany-dev --help` - explain the source-checkout entrypoints without building or launching the UI.
- `./scripts/tiffany-dev setup` - run the local first-run setup wizard without installing binaries.
- `./scripts/tiffany-dev config ...` - run the local orchestrator config command directly. It does not launch the TUI or require a UI login; use it for scriptable provider setup before opening the TUI.
- `./scripts/tiffany-dev orchestrator "..."` - run the orchestrator pipeline immediately with an initial prompt; native mode defaults worker execution to Claude Code (`worker-cc`), and later input-box submissions are routed into the same orchestrator pipeline.
- `./scripts/tiffany-dev orchestrator --orchestrator-config /path/to/config.yaml` - use a specific orchestrator config while keeping tiffany-loop UI config isolated under `TIFFANY_HOME`.
- `./scripts/tiffany-dev orchestrator --legacy ...` - compatibility bridge to the old orchestrator CLI.
- `./scripts/tiffany-build [cargo-build-args...]` - build both the parent orchestrator binary and the tiffany-loop UI in the shared `./target` directory. It defaults to `--small` for source-checkout local binaries; use `--dev` for Cargo's normal debug profile or `--fast-release` for a faster distributable build. Final dist binaries are stripped unless `TIFFANY_NO_STRIP=1`. Add `--prune-dist-cache` when you want to keep only the final dist binaries after a successful build.
- `./scripts/tiffany-check --smoke` - small debug-build, format-check the fork, verify the isolated install surface, legacy bridge, event-stream entrypoint, and example smoke tests.
- `./scripts/tiffany-check --dist` - run the same checks with the distributable `tiffany-dist` profile before a release.
- `./scripts/tiffany-install-smoke --smoke|--dist` - verify `orchestrator`, `tiffany-loop`, and the `tiffany` alias in a temporary HOME without touching real user config.
- `./scripts/tiffany-check-examples` - run only the checked-in example project tests.
- `./scripts/tiffany-release-preflight --quick|--full [--tag vX.Y.Z]` - run the consolidated release-readiness checks; use `--full --tag vX.Y.Z` before tagging.
- `./scripts/tiffany-update-homebrew-tap --tag vX.Y.Z [--tap-dir ../homebrew-tap] [--commit --push]` - generate the Homebrew formula from a published release asset and source archive checksums.
- `./scripts/tiffany-clean-targets --sizes|--top|--top-deep|--trim|--incremental|--dist-cache|--dist|--debug|--small` - inspect build-cache size, find large artifacts, trim rebuildable caches while keeping compiled deps/final binaries, or remove larger build outputs when `target/` grows too large.

Direct Cargo commands inside `tiffany-ui/codex-rs` are also redirected to the root `./target` through the fork's Cargo config. If an older checkout already has `tiffany-ui/codex-rs/target`, it is a legacy duplicate cache and can be removed with `./scripts/tiffany-clean-targets`.

For source-checkout development, `./scripts/tiffany-dev` builds missing `dev-small` binaries once and then execs the cached binary directly for faster repeat startup and smaller local build caches. Set `TIFFANY_DEV_PROFILE=dev` when you need full debug symbols, or `TIFFANY_DEV_CARGO_RUN=1` only when you specifically want Cargo's `cargo run` wrapper.

tiffany-loop keeps fork state separate from upstream UI source. The fork uses `TIFFANY_HOME`, defaulting to `~/.tiffany`, and maps UI-internal config reads to `~/.tiffany/config.toml` instead of the upstream default config directory. SQLite state defaults to the same root through `TIFFANY_SQLITE_HOME`. Override with `TIFFANY_HOME=/path/to/tiffany-home`.

When running the source helper or fork binary directly, set `TIFFANY_ORCHESTRATOR_BIN=/path/to/orchestrator` or pass `tiffany-loop orchestrator --bin /path/to/orchestrator`.
Installed builds include `tiffany-loop`, `orchestrator`, and the compatibility alias `tiffany`; `orchestrator tui` delegates to `tiffany-loop orchestrator` when the UI binary is installed, and falls back to the legacy terminal chat only when it is missing. Set `ORCHESTRATOR_LEGACY_TUI=1` to force the fallback.

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
| **A/B Judge** | Built-in success + diff/log-size heuristic | Custom rule |

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
- `tiffany-loop "..."` streams planner, critic, worker, reviewer, and final-result events into tiffany-loop history cells.
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
tiffany-loop setup
tiffany-loop
```

The Homebrew package installs three commands:

- `tiffany-loop` - the primary terminal UI. Start here.
- `tiffany` - compatibility alias for older scripts.
- `orchestrator` - config, roles, event streaming, ACP, and non-interactive runs.

After install:

```bash
tiffany-loop init
tiffany-loop setup
tiffany-loop doctor
tiffany-loop
```

Inside the TUI, use `/provider` to configure providers and `/role` to register
planner, critic, worker, and reviewer roles.

Prebuilt macOS Apple Silicon binaries are published on the GitHub Releases page after each `v*` tag. Archives include `tiffany-loop`, `orchestrator`, and the compatibility `tiffany` alias. Linux, Windows, and Intel Mac users can install from source while additional prebuilt targets are being staged.

Source checkout for contributors:

```bash
git clone https://github.com/macguffinQ/Tiffany.git ~/code/orchestrator
cd ~/code/orchestrator
./scripts/tiffany-build     # defaults to target/dev-small
./scripts/tiffany-dev setup
./scripts/tiffany-dev
```

On macOS source builds, run `orchestrator doctor` if compilation fails after an Xcode update or when `xcode-select` points at `/Applications/Xcode-beta.app`; doctor reports the selected developer directory, `clang`, `xcodebuild`, and the exact `sudo xcode-select -s ...` command to switch to stable Xcode or Command Line Tools.

Manual source install, if you want the commands in `PATH` without Homebrew:

```bash
cargo install --path . --profile tiffany-dist
cargo install --path tiffany-ui/codex-rs/tiffany-cli --profile tiffany-dist
strip "$(command -v orchestrator)" "$(command -v tiffany-loop)" "$(command -v tiffany)" 2>/dev/null || true
```

```bash
# Example: install a downloaded archive
tar -xzf tiffany-loop-v0.1.14-aarch64-apple-darwin.tar.gz
cd tiffany-loop-v0.1.14-aarch64-apple-darwin
chmod +x orchestrator tiffany-loop tiffany
./tiffany-loop setup
./tiffany-loop doctor
./tiffany-loop
```

The `tiffany-loop` package installs `tiffany-loop`, `orchestrator`, and the compatibility alias `tiffany`; the current GitHub repository name remains `Tiffany`.
Tag releases prioritize the macOS Apple Silicon archive used by Homebrew; Intel Macs, Linux, and Windows can install from source for now.
Public Homebrew installs require the `macguffinQ/Tiffany` repository and release assets to be public.

## Quickstart

```bash
# 1. Initialize
tiffany-loop init
# writes ~/.orchestrator/config.yaml

# 2. Configure providers, models, and roles
tiffany-loop setup
# Or use the TUI forms:
tiffany-loop
# then run /provider and /role

# 3. Check the full provider -> model -> role -> runtime wiring
tiffany-loop doctor

# 4. Run interactively
cd ~/your-project
tiffany-loop

# 5. Or run one non-interactive task
orchestrator run "implement fibonacci in src/fib.rs"

# 6. With tag-based routing
orchestrator run "refactor the auth module" --tag refactor

# 7. Override worker route
orchestrator run "fix typo" --planner opus --worker codex
orchestrator run "review the diff" --worker worker-cc --agent reviewer

# 8. Watch the live pipeline as readable text
orchestrator events "fix typo" --format text

# 9. Run in the background and attach later
orchestrator run "fix the flaky test" --detach
orchestrator attach

# 10. Source checkout UI
./scripts/tiffany-dev

# 11. Browse past sessions
orchestrator sessions list
orchestrator sessions show                    # latest session, human-readable
orchestrator sessions show <id|prefix|last>          # human-readable by default
orchestrator sessions show <id|prefix|last> --raw    # original JSONL
orchestrator sessions show <id|prefix|last> --tree   # parent/child run tree
orchestrator sessions show <id|prefix|last> --flow   # readable orchestration/worker waterfall
orchestrator sessions export <id|prefix|last> --format html
orchestrator sessions grep "rate limit" --limit 10

# 12. See everything the orchestrator loaded
orchestrator config

# 13. Import your existing CC sessions
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
  token_plan:
    enabled: false
    warn_at_percent: 80
    daily_limit: 200000
    monthly_limit_usd: 50.0
    per_provider:
      claude: 120000
      gpt: 80000
```

Missing env vars → empty string (no error). You can run `orchestrator status` before setting any keys.
`cc_bypass_permissions` passes `--permission-mode bypassPermissions` to spawned Claude Code processes. Set it to `false` only when you want Claude Code to stop for manual permission prompts; tiffany-loop TUI `/permissions` controls the tiffany-loop UI itself, not these Claude subprocesses.
Set `behavior.token_plan.enabled: true` to show daily token, monthly cost, and per-provider budget alerts in `orchestrator usage` and `/usage`.

---

## Commands

| Command | What it does |
|---|---|
| `./scripts/tiffany-dev` | Run the tiffany-loop UI |
| `./scripts/tiffany-dev setup` | Run the first-run setup wizard from source |
| `./scripts/tiffany-dev orchestrator` | Bridge from the tiffany-loop fork into the existing orchestrator runtime |
| `./scripts/tiffany-build [args]` | Build runtime and UI binaries in shared `./target`; defaults to `--small`, use `--dev --locked` for normal debug, or use stripped `--fast-release --locked`; add `--prune-dist-cache` to keep only final dist binaries |
| `./scripts/tiffany-check --smoke` | Run the fast local fork/install/bridge/example verification |
| `./scripts/tiffany-check --dist` | Run the release-profile fork/install/bridge/example verification |
| `./scripts/tiffany-install-smoke --smoke|--dist` | Verify installed command behavior in an isolated temporary HOME |
| `./scripts/tiffany-check-examples` | Run only checked-in example tests |
| `./scripts/tiffany-release-preflight --quick|--full [--tag vX.Y.Z]` | Run consolidated local release-readiness checks: format, clippy, tests, examples, audit, and dist checks in full mode |
| `./scripts/tiffany-update-homebrew-tap --tag vX.Y.Z [--commit --push]` | Generate and optionally commit/push the Homebrew tap formula for a published release |
| `./scripts/tiffany-clean-targets --sizes|--top|--top-deep|--trim|--incremental|--dist-cache|--dist|--debug|--small` | Inspect or trim shared and legacy Cargo build caches; default removes only the old fork-local target |
| `tiffany-loop` | Open the primary tiffany-loop UI after install |
| `tiffany-loop init` | Generate `~/.orchestrator/config.yaml` through the primary command |
| `tiffany-loop setup` | Guided first-run setup for providers, models, and roles |
| `tiffany-loop status` | Show installed commands, config roots, bridge command, mux, and logs |
| `tiffany-loop doctor` | Diagnose UI discovery, config, runtime binaries, API keys, role wiring, and local tools |
| `tiffany-loop config ...` | Inspect and edit orchestrator configuration through the primary command |
| `orchestrator run "..."` | Run a task (with optional `--planner`, `--critic`, `--worker`, `--agent`, `--reviewer`, `--tag`, `--ab`, `--no-critic`, `--no-reviewer`) |
| `orchestrator run "..." --detach` | Run a task in the background and write a readable event log under `~/.orchestrator/runs/` |
| `orchestrator attach [id|prefix|last]` | Print the latest detached run status and log tail |
| `orchestrator events "..."` | Stream progress events; default JSONL is stable for UI adapters/scripts, `--format text` prints a readable planner/critic/worker/reviewer waterfall |
| `orchestrator config provider` | Open the guided provider selector |
| `orchestrator config provider setup <provider>` | Configure a provider from built-in presets |
| `orchestrator config provider delete <provider>` | Delete one configured provider |
| `orchestrator config provider list|presets` | Inspect configured providers or built-in presets |
| `orchestrator roles list` | List registered planner/critic/worker/reviewer roles |
| `orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime>` | Register or update a role; Tiffany derives the internal model id |
| `orchestrator tui` | Open the primary tiffany-loop UI when `tiffany-loop` is installed; otherwise fall back to legacy terminal chat |
| `orchestrator tui --ratatui` | Legacy compatibility flag; kept only for old scripts |
| `orchestrator tui --new-tab` | Open terminal chat in a new zellij tab |
| `orchestrator tui --detach` | Run terminal chat in background outside zellij (PID file at `~/.orchestrator/tui.pid`) |
| `orchestrator acp` | Run an Agent Client Protocol server over stdio |
| `orchestrator sessions list` | List recent sessions with role labels, parent/child hints, and copyable open commands |
| `orchestrator sessions show <id|prefix|last>` | Show a human-readable orchestration or worker session log (`--raw` keeps JSONL, `--tree` shows parent/child links, `--flow` shows the readable run waterfall) |
| `orchestrator sessions export <id|prefix|last> --format markdown\|html` | Export a readable session report to Markdown or HTML |
| `orchestrator sessions grep <pattern> --limit 10` | Search session logs and print readable event summaries with copyable open commands |
| `orchestrator sessions import-cc` | Import your existing CC sessions from `~/.claude/projects/<slug>/sessions/` |
| `orchestrator config` | Inspect and edit orchestrator configuration |
| `orchestrator status` | Show installed commands, config roots, bridge command, mux, and logs |
| `orchestrator doctor [--format text\|json]` | Diagnose tiffany UI discovery, config, runtime binaries, API keys, role wiring, and local tools |

---

## Terminal UI

The target terminal UI is the full tiffany-loop UI in [`tiffany-ui/`](tiffany-ui/). From a source checkout:

```bash
./scripts/tiffany-dev
```

After install:

```bash
tiffany-loop
orchestrator tui
```

To pass explicit orchestrator arguments during migration:

```bash
./scripts/tiffany-dev orchestrator
```

`orchestrator tui` now delegates to `tiffany-loop orchestrator` when it can find the `tiffany-loop` binary next to `orchestrator` or on `PATH`. If the UI binary is missing it falls back to the old compatibility runner. Set `ORCHESTRATOR_LEGACY_TUI=1` to force the fallback.

Forward explicit orchestrator arguments after the subcommand:

```bash
./scripts/tiffany-dev orchestrator status
./scripts/tiffany-dev orchestrator -- --help
```

Useful commands:

- `/help` - show commands
- `/doctor` - diagnose config, runtimes, API keys, role wiring, and local tools
- `/provider [setup|edit <provider>]|list|delete <provider>|env <provider> <ENV>|key <provider> <value>|endpoint <provider> <url>` - open or edit a provider setup form, inspect config, delete a provider, or configure orchestrator providers
- `/role [<role>|register <role> --provider <provider> --model-name <api-model> --runtime <runtime>]` - open the role-registration form or register one role; `--model <id>` remains supported for existing model ids
- `/roles show|route|use <role>|snippet <role> <model> <runtime>|save <role> --provider <provider> --model-name <api-model> --runtime <runtime>` - inspect configured commander/critic/executor/reviewer roles, select a worker route, print a snippet, or write a role to config
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
- `/usage today|week|month|all` - show token/cost usage and configured budget alerts
- `/o` - fold or expand history summaries
- `/resume last` - restore the last terminal chat snapshot

The bottom status line follows tiffany-loop-style run HUD behavior: it keeps one compact line for stage, elapsed time, worker route, context mode, queued message count, detail fold state, process filter, and review/worker issue counters.

Troubleshooting first:

- Run `orchestrator status` when you are unsure which `tiffany-loop` / `orchestrator` binaries or config roots are being used.
- `orchestrator status` prints targeted next actions, for example `/provider env minimax <ENV_NAME>`, `/provider endpoint minimax <url>`, or `orchestrator roles register worker-cc ...` when provider/model/runtime wiring is incomplete.
- Run `/doctor` or `orchestrator doctor` when a worker exits early, a provider says `model not found`, `模型不存在`, `401/403`, or an API key appears unset.
- Doctor checks env-var key references without printing secrets, verifies `role -> model -> provider -> runtime`, catches duplicate/missing models, and reports the local install/toolchain surface: Homebrew tap/package, Rust/cargo, Xcode/CLT, and worker CLI binaries.
- Use `orchestrator doctor --format json` from scripts, CI, or UI bridges that need stable `status`, `issue_count`, `issue_summary`, `next_steps`, and diagnostic lines without parsing human text.
- On macOS, doctor also calls out Xcode beta selections and gives the `xcode-select` command to switch to stable Xcode or Command Line Tools when source builds fail.
- For model errors, confirm the role points to the intended provider API model name: `/role <role>` or `orchestrator roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime>`. Use `--model <id>` only when binding to an existing internal model id.
- When a run fails with `model not found`, `模型不存在`, or `[1211]`, `/process` and the failure summary include a copyable repair template such as `/roles save worker-codex --provider openai --model-name <api-model> --runtime codex`.

`orchestrator run "..." --ab` runs the task through two configured worker roles, for example `worker-cc` and `worker-codex`. If `--worker <role>` is supplied, that route becomes side A and Tiffany picks another configured worker for side B. The built-in judge prefers a successful side; when both succeed or both fail, it prefers the smaller diff, falling back to session-log size when a diff is unavailable.

Input behavior:

- Press `Enter` to send.
- In `tiffany-loop "..."`, pressing `Enter` starts another orchestrator run from the same tiffany-loop TUI session.
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
- `/roles save <role> --provider <provider> --model-name <api-model> --runtime <runtime>` writes both the model and role into `~/.orchestrator/config.yaml`; `/roles save <role> <model> <runtime>` remains supported for existing model ids. Restart terminal chat after changing planner/critic/reviewer so the execution pipeline is rebuilt.
- In the tiffany-loop UI, use `/role`, `/roles`, `/roles show <role>`, or `/roles register <role> --provider <provider> --model-name <api-model> --runtime <runtime>` directly from the input box.

Role registration examples:

```bash
orchestrator roles register planner --model gpt4o --runtime codex
orchestrator roles register critic --provider openai --model-name glm-5.1 --runtime codex
orchestrator roles register worker-cc --provider minimax --model-name MiniMax-M3 --runtime claude-code --agent-teams
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

Use one explicitly with:

```bash
orchestrator run "review this change" --worker worker-cc --agent reviewer
orchestrator events "review this change" --worker worker-cc --agent reviewer --format text
tiffany-loop orchestrator --worker worker-cc --agent reviewer
```

`--worker` selects the orchestrator worker route; `--agent` is passed only to Claude Code as `claude --agent <name>`.

### MCP servers: `<project>/.mcp.json`

Orchestrator auto-merges these into its MCP pool, so workers have access to `github`, `slack`, etc. without any extra config.

---

## Architecture in depth

See `docs/architecture.md` for the full layered architecture.

### Why Rust?

- **Fast control plane**: `orchestrator` stays small and quick to start, while
  the heavier `tiffany-loop` TUI carries the full terminal UI stack.
- **Terminal reliability**: matches the tool's ecosystem
- **Async-first**: tokio for subprocess management + event streaming
- **No GC pauses**: stays responsive in terminal chat mode
- **Mature crates**: `crossterm`, `axum`, `rusqlite`, `git2`, `reqwest`, `tokio` - all battle-tested

### Why subprocess-based CC integration (not the SDK)?

- The `claude-agent-sdk` is Python/TS only; using the CLI keeps runtime
  integration language-agnostic and easy to package.
- CC's own session JSONL files (`~/.claude/projects/.../sessions/*.jsonl`) are the source of truth
- CC's CLI is the most stable API — SDK changes don't affect us
- We read CC's events from the JSONL stream in real time

### Why a thin top-level instead of a custom DAG scheduler?

Agent Teams (CC's experimental multi-agent feature) already does shared task lists, mailboxes, and hooks. We use it as-is and add a thin shim for: cross-agent session sharing, A/B judging, capability-based routing, and terminal chat.

---

## Development

```bash
# Build
cargo build              # normal debug profile
cargo build --release    # release, ~5-8min first time
./scripts/tiffany-build   # orchestrator + tiffany-loop UI, defaults to dev-small
./scripts/tiffany-build --small --locked
./scripts/tiffany-build --dev --locked
./scripts/tiffany-build --release --locked
./scripts/tiffany-build --fast-release --locked
./scripts/tiffany-build --fast-release --locked --prune-dist-cache
./scripts/tiffany-install-smoke --smoke
./scripts/tiffany-release-preflight --quick
./scripts/tiffany-release-preflight --full --tag v0.1.14   # before tagging

# Direct cargo commands inside tiffany-ui/codex-rs also use ./target.
# Use ./scripts/tiffany-clean-targets --top to find target bloat,
# ./scripts/tiffany-clean-targets --top-deep for slower file-level detail, and
# ./scripts/tiffany-clean-targets --trim to remove rebuildable caches while
# keeping compiled deps and final binaries warm.

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
- [x] A/B judge for two configured worker routes
- [x] Token/cost usage command
- [x] Webhook server (axum)
- [x] Streaming process summaries in terminal chat
- [x] Full tiffany-loop fork under `tiffany-ui/`
- [x] `tiffany-loop` CLI binary in the tiffany-loop UI
- [x] `tiffany-loop "..."` native tiffany-loop TUI event adapter
- [x] Multi-turn orchestrator submissions from the native tiffany-loop input box
- [x] `tiffany-loop orchestrator --legacy ...` bridge to the existing runtime
- [x] Vendored tiffany-loop TUI source snapshot under `third_party/openai-codex/codex-rs/tui`
- [x] Native tiffany-loop adapter inside the tiffany-loop TUI app/session layer
- [x] Make the tiffany-loop UI the default `orchestrator tui` command path when `tiffany-loop` is installed
- [x] Release/Homebrew packaging installs `tiffany-loop`, `orchestrator`, and the compatibility `tiffany` alias
- [x] Session export (markdown / HTML)
- [x] Cost budget alerts
- [x] Background task mode (`orchestrator run --detach` + `orchestrator attach`)
- [x] CC agent invocation from orchestrator (`--agent reviewer`)
- [ ] Remove copied partial TUI modules after the fork adapter is stable
- [ ] Full-token streaming final responses in terminal chat
- [ ] VS Code extension

---

## License

MIT
