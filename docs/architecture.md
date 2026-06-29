# tiffany-loop Architecture

`tiffany-loop` is a lightweight native terminal orchestration shell for existing
agent CLIs. It is not a replacement for Claude Code, Codex, Gemini, or future
tools; it is a small control layer that routes prompts to registered roles,
captures what each worker did, and keeps enough state to continue either inside
Tiffany or in the original native tool.

This document mirrors the current source of truth in
[`docs/engineering-plan.md`](engineering-plan.md). When behavior changes, update
both files together.

## Product Shape

- `tiffany-loop` is the primary user-facing command.
- `orchestrator` is the runtime and scripting command.
- `tiffany` is a compatibility alias.
- All three names are exposed by one multi-call binary. The invoked name
  (`argv[0]`) selects the entrypoint, so installed users should not need
  `--bin`, PATH probing, or an adjacent standalone `orchestrator` binary.
- The primary TUI is the Codex Ratatui fork under `tiffany-ui/codex-rs/`.
- Tiffany config/state is separate from upstream Codex state:
  `~/.tiffany` for UI state and `~/.orchestrator/config.yaml` for runtime
  routing.

## Runtime Flow

```text
tiffany-loop TUI
  -> Tiffany orchestrator bridge
  -> orchestrator runtime
  -> native tool adapters
  -> Claude Code / Codex CLI / Gemini CLI / direct providers
```

The TUI owns the conversation surface, slash commands, queue preview, waterfall
history cells, and native handoff actions. The runtime owns provider/model/role
routing, pipeline execution, adapter invocation, event streaming, SQLite state,
JSONL logs, and background jobs.

## Main Components

```text
repo root
├── src/
│   ├── cli_entry.rs              # runtime CLI entry used by the multi-call binary
│   ├── cli.rs                    # runtime command dispatch
│   ├── config.rs                 # provider/model/role config
│   ├── doctor.rs                 # setup diagnostics
│   ├── adapters/                 # claude_code, codex_cli, gemini, direct
│   ├── core/                     # workers, tasks, sessions, stores
│   ├── pipeline/orchestrator.rs  # direct/single/full routing pipeline
│   ├── providers/                # Anthropic, OpenAI, Google, Ollama, compatible APIs
│   ├── roles/                    # planner, critic, reviewer, router, judge helpers
│   ├── tui/                      # legacy terminal chat fallback only
│   └── tui_jobs.rs               # persisted job helpers
├── tiffany-cli/
│   └── src/main.rs               # single installed binary entrypoint
├── tiffany-ui/codex-rs/
│   ├── tui/                      # Codex Ratatui fork plus Tiffany UI seams
│   └── tiffany-bridge/           # pure Tiffany parsing/formatting/session helpers
├── docs/
│   ├── engineering-plan.md       # product contract and milestone state
│   ├── execution-plan.md         # worker task cards and current Claude instructions
│   └── codex-log.md              # execution log from Codex worker slices
└── scripts/                      # build, smoke, release, runtime verification
```

New UI work should prefer `tiffany-ui/codex-rs/tiffany-bridge/` for pure logic
and keep fork-side Ratatui files thin. The legacy `src/tui/` path is a
compatibility fallback and should not receive new hand-written UI work except
narrow shims or bug fixes needed by the runtime.

## Role And Provider Model

Users configure roles rather than hard-coding one model everywhere.

```text
provider
  -> API model name
  -> Tiffany model binding
  -> role
  -> runtime adapter
```

Examples:

- `planner` can use Codex CLI with one provider/model.
- `critic` can use Claude Code with another provider/model.
- `worker-cc` can use Claude Code and preserve its native session.
- `worker-codex` can use Codex CLI and preserve its native session.
- `worker-gemini` can use Gemini CLI and preserve its native session.
- `reviewer` can use a cheaper or stricter role-specific model.

Normal UI surfaces should show `provider`, `api model`, and `runtime` first.
Internal model ids remain available for config and scripting, but they should
not be the primary operator concept.

## Routing Pipeline

Every prompt is classified into one of three flows:

```text
user prompt
  -> route classifier
     -> direct: one worker answers conversational/explanatory prompts
     -> single: one worker handles atomic tasks, links, diagnostics, scaffolds
     -> full: planner -> critic -> worker(s) -> reviewer for implementation work
```

The direct and single paths still emit route/worker/done events so the run is
observable, but they avoid planner/critic/reviewer noise. The full path is kept
for implementation work where decomposition and adversarial review can improve
the result.

## Event And Display Contract

Normal TUI output should be readable without inspecting JSON.

- Route headers show `direct`, `single`, or `full` from runtime events.
- Planner, critic, and reviewer control output collapses to compact timeline
  rows unless there is actionable detail.
- Worker answers stream as normal assistant text.
- Tool calls, tool results, diffs, approvals, stderr, file updates, and native
  session recovery render as labeled waterfall cells.
- Raw JSON remains available through raw/process/history views, not normal
  conversation output.
- `/o` toggles compact/full process history.
- `/process`, `/history kind ...`, and `/raw` expose the full trace when needed.

Most formatting and de-duplication lives in:

```text
crates/tiffany-event-format/src/lib.rs
tiffany-ui/codex-rs/tiffany-bridge/src/visible_output.rs
tiffany-ui/codex-rs/tiffany-bridge/src/output_parse.rs
```

The Ratatui fork adapts those pure results into history cells rather than
duplicating parser logic in rendering code.

## Persistence

Tiffany persists both its own orchestration state and native worker continuity.

- SQLite stores indexed sessions, events, tasks, worker threads, and jobs.
- JSONL stores append-only full event/session traces.
- `~/.tiffany/tiffany-orchestrator/native-sessions.json` mirrors local native
  history for TUI-side continuity.
- `/thread` shows worker roles, Tiffany worker thread ids, native session ids,
  and handoff actions.
- `/history` can list, filter, export, graph, compact, and sync native history.
- `/continue open <role>` restores the terminal and opens the original native
  CLI session when enough native session data exists.

## Queue And Jobs

Tiffany has two related work queues:

- The bottom input queue stores follow-up prompts typed while a run is active.
  These prompts stay visible until they execute. In orchestrator mode, normal
  follow-ups are submitted as a numbered batch rather than silently merging.
- Persisted jobs store detached or recoverable work. `/jobs` exposes
  status-specific next actions such as retry, cancel, recover, inspect history,
  or open the native worker session.

The key invariant is that queued work should not disappear. If work is paused,
armed, retrying, failed, or running, the UI should say so with a concrete next
action.

## Slash Command Surface

In Tiffany orchestrator mode the slash menu intentionally hides upstream
Codex-only commands that are not wired for this product surface. Supported
native commands include:

```text
/provider  /role      /roles     /queue     /thread
/jobs      /continue  /history   /approvals /compact
/process   /o         /doctor    /status    /help
/copy      /raw       /diff      /clear     /exit
```

Known hidden upstream or legacy commands should return a specific explanation
and point to the Tiffany equivalent. Unknown commands can remain normal chat
input.

## Native Runtime Adapter Contract

Each runtime adapter should provide:

- launch command and compatible CLI arguments;
- provider/model/runtime error normalization;
- visible event summaries for tool calls, tool results, approvals, stderr, and
  answers;
- native session id capture when the tool supports it;
- same-role session reuse across orchestrator processes;
- a busy/missing session recovery policy;
- handoff information for `/thread`, `/continue open`, `/thread export`, and
  `/history`.

The real-runtime harness verifies Claude Code, Codex, and Gemini adapter
continuity:

```bash
./scripts/tiffany-real-runtime-check --adapter-run --runtime all
```

It runs two same-role prompts per runtime and checks Tiffany worker-thread
reuse, native session reuse, persisted jobs, thread export, native-history
import, and history query paths. It may consume model quota.

## Build And Release Shape

The installed release should provide one binary with three names:

```text
tiffany-loop  # primary UI
orchestrator  # runtime/scripting alias
tiffany       # compatibility alias
```

Important gates:

```bash
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib
git diff --check
```

Pre-release gates include the full Codex-fork TUI library suite, fake runtime
e2e, multi-runtime e2e, open-source audit, and release preflight:

```bash
./scripts/tiffany-codex-tui-lib-test
./scripts/tiffany-e2e-fake-runtime
./scripts/tiffany-e2e-multi-runtime
./scripts/open-source-audit
./scripts/tiffany-release-preflight --full --tag vX.Y.Z
```

Existing release tags must not be moved unless the user explicitly chooses that
path and the tagged preflight is rerun with the documented override. The safer
path after a failed tag is usually a new patch tag.

## Current Milestone State

The foundation is working but not complete:

- M0 shell stabilization is mostly done locally, but the external release/tag
  decision is still held.
- M1 provider/role setup is partial and needs more operator polish.
- M2 execution display is partial and still needs stricter de-duplication and
  JSON/raw-debug separation.
- M3 session continuity is partial but has strong real-runtime adapter evidence.
- M4 queue/background jobs are partial and need clearer paused/running/retry
  states.
- M5 release/open-source quality is partial and depends on docs, packaging, and
  milestone-based versioning discipline.
- M6 pipeline efficacy is not started; it needs evidence that the full pipeline
  improves outcomes enough to justify its cost.
