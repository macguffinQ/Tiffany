# tiffany-loop Engineering Plan

This document is the working engineering plan for `tiffany-loop`.

The product is a lightweight native terminal orchestration shell. It should let
one user talk in one TUI, route work to registered roles, run Claude Code,
Codex, Gemini, and compatible tools with different providers/models, show the
worker process transparently, and preserve enough session state to continue in
either Tiffany or the original native tool.

## North Star

`tiffany-loop` should feel like a small orchestration layer on top of existing
agent CLIs, not a replacement for them.

- One native TUI for conversation, orchestration, queueing, and handoff.
- Roles are first-class: planner, critic, worker, reviewer, and custom workers.
- Each role can bind to its own provider, API model, runtime, and native CLI.
- Claude Code, Codex, Gemini, and future tools keep their own native session
  continuity when possible.
- The user can inspect what happened, what is running, what is queued, what
  failed, and how to resume.
- Output is readable waterfall text by default. Raw JSON stays available only
  in raw/debug views.
- The project remains open-source friendly: clear docs, small install surface,
  reproducible builds, Homebrew packaging, and explicit upstream license
  boundaries.

## Product Contract

- `tiffany-loop` is the primary user-facing command.
- `orchestrator` is the runtime and scripting command.
- `tiffany` is a compatibility alias only.
- `tiffany-loop`, `orchestrator`, and `tiffany` are one multi-call binary; the
  invoked name (argv[0]) selects the entrypoint. There is no cross-binary
  discovery, no PATH coupling, and no "binary not found" failure mode.
- The native TUI is built from the Codex Ratatui fork under
  `tiffany-ui/codex-rs/`.
- New UI work should land in the fork adapter boundary, not in the legacy
  terminal-chat implementation under `src/tui/`.
- Provider and role setup must not require upstream Codex login.
- Tiffany config must stay separate from upstream Codex config:
  `~/.tiffany` for UI state and `~/.orchestrator/config.yaml` for runtime
  routing.
- Provider/model/runtime errors must produce concrete fix actions.
- Worker output must show process visibility without duplicating low-value
  wrapper noise.
- Native sessions must be inspectable, clearable, exportable, and recoverable.

## Architecture

```text
tiffany-loop TUI
  ├─ Codex Ratatui app shell
  ├─ Tiffany command menu and overlays
  ├─ waterfall process/history cells
  └─ native input and pending queue
       │
       ▼
Tiffany orchestrator bridge
  ├─ maps prompts to orchestrator events
  ├─ renders planner/critic/worker/reviewer progress
  ├─ captures worker output and native session ids
  └─ exposes /provider /role /thread /jobs /history /doctor
       │
       ▼
orchestrator runtime
  ├─ route classifier: direct / single / full
  ├─ role registry and provider/model config
  ├─ adapters: claude-code / worker-codex / gemini / direct
  ├─ pipeline: planner -> critic -> worker -> reviewer
  └─ SQLite + JSONL session/job/native-history store
       │
       ▼
native tools
  ├─ Claude Code
  ├─ Codex CLI
  ├─ Gemini CLI
  └─ future agent CLIs
```

## Current State

The project is past prototype but not done.

Working:

- Native `tiffany-loop` TUI starts and routes prompts into the orchestrator.
- Simple conversational requests route directly to one worker.
- Atomic tasks route to one worker without planner/critic/reviewer noise.
- Full implementation tasks can run planner -> critic -> worker -> reviewer.
- `/provider`, `/role`, `/roles`, `/thread`, `/jobs`, `/history`, and
  `/doctor` exist in the native command surface.
- Provider setup supports dropdown-like TUI flows and CLI fallbacks.
- Role registration can bind provider plus API model plus runtime without
  exposing internal model IDs as the main path.
- Worker/native session state is persisted in SQLite/JSONL and can be listed,
  shown, cleared, exported, synced, and recovered.
- Claude Code, Codex, and Gemini adapters have session-id capture/reuse work in
  place.
- The real-runtime adapter harness verifies Claude Code, Codex, and Gemini
  same-role continuity across separate orchestrator processes, including
  persisted jobs, thread export, native-history import, and `/continue open`
  handoff actions.
- Homebrew and release automation exist.

Known gaps:

- The visible process stream still needs stricter de-duplication and cleaner
  event grouping for long tool runs.
- Native handoff still needs a heavier interactive PTY smoke, but the automated
  real-runtime restart/continuity harness now covers Claude Code, Codex, and
  Gemini adapter reuse.
- `/role` and `/provider` setup flows need more operator polish and fewer
  fallback-looking screens.
- Background queue behavior needs stronger UI affordances for running, paused,
  retrying, and failed jobs.
- Release cadence and versioning need to slow down and become milestone-based.
- More docs are needed for installed users, contributors, and provider authors.

## Milestones

### M0: Stabilize The Shell

Goal: the installed `tiffany-loop` command opens reliably and never exposes
upstream Codex-only login/update flows.

Acceptance:

- `tiffany-loop` works from Homebrew without extra path flags.
- `tiffany-loop`, `orchestrator`, and `tiffany` are one multi-call binary;
  invoking any name works with no PATH coupling and no discovery step. This
  replaces the old "adjacent binary discovery" requirement.
- A runtime-only slim build of the single binary exists and smoke-passes:
  building with the TUI feature off produces a working orchestrator-mode binary
  (`--help`, `status`) without the Codex TUI compiled in — the hedge against the
  one-binary merge raising build cost.
- The Codex TUI is a feature-gated dependency of the single binary
  (`optional = true`, default feature), so the slim path cannot regress silently.
- CI tracks clean release-build time for the default (full) binary and fails on a
  regression beyond a set threshold versus the pre-merge baseline; the one-binary
  merge ships only if build weight stays bounded.
- Upstream update prompts are disabled in Tiffany mode.
- `tiffany-ui/UPSTREAM.md` exists, names the frozen upstream commit, and lists
  the Tiffany-owned file set versus the vendored-untouched set.
- Tiffany config/state is isolated from `~/.codex`.
- Resize, mouse scroll, selection, copy/paste, and IME remain inherited from the
  upstream Ratatui/Crossterm path.

### M1: Complete Provider And Role Setup

Goal: users can configure a provider and bind roles entirely in the native TUI.

Acceptance:

- `/provider` lists configured providers and next actions.
- `/provider <name>` opens provider detail with auth, endpoint, models, and
  role bindings.
- `/provider edit <name>` can set type, env/key, endpoint, and delete safely.
- `/role <name>` opens a role editor with provider/model/runtime choices.
- `/roles` shows every role plus its provider/API model/runtime/health.
- Worker roles show native session readiness and handoff actions.
- `/doctor` reports missing provider auth, missing endpoints, missing models,
  and missing runtimes with one-line fixes.

### M2: Make Execution Transparent

Goal: the user can see what each agent is doing without reading JSON or
duplicated control lines.

Acceptance:

- Route header clearly shows `direct`, `single`, or `full`.
- Planner/critic/reviewer control events collapse into compact timeline rows.
- Worker answer/final chunks stream as readable assistant output.
- Tool calls, tool results, diffs, approvals, stderr, and file updates render as
  labeled waterfall cells.
- Raw JSON never appears in normal mode unless the worker intentionally outputs
  JSON as the answer.
- `/o` toggles compact/full process history.
- `/process`, `/history kind ...`, and `/raw` expose the full trace when needed.

### M3: Session Continuity And Handoff

Goal: the same role in the same Tiffany conversation can continue using the
same native Claude/Codex/Gemini session, and the user can jump into the native
tool manually.

Acceptance:

- Worker thread identity is stable per TUI session/project/role.
- Native session IDs are captured from Claude Code, Codex, and Gemini.
- Busy or missing native session IDs recover with a clear retry/fresh-session
  policy.
- `/thread` shows all worker roles, thread IDs, native IDs, and last Tiffany
  sessions.
- `/thread <role>` shows native resume and handoff commands.
- `/continue open <role>` opens or resumes the native worker session.
- `/thread export <role>` writes a handoff bundle with selectable history.
- Native history can sync into the session DB and be queried with `/history`.

### M4: Queue And Background Work

Goal: queued follow-up prompts and detached jobs are understandable and safe.

Acceptance:

- Prompts entered during a run stay visibly queued at the bottom until they run.
- Queue entries are not merged accidentally unless explicitly intended.
- `/queue` shows, pauses, resumes, clears, and starts queued prompts.
- `/jobs` shows persisted background jobs with status-specific actions.
- `/jobs retry`, `/jobs cancel`, and `/jobs recover` work from the native TUI.
- Failed jobs show model/provider/runtime errors in human text.

### M5: Open-Source Release Quality

Goal: other people can install, understand, run, and contribute without private
context.

Acceptance:

- README and README.zh-CN explain install, setup, first run, roles, providers,
  and common errors.
- `docs/architecture.md`, this engineering plan, Homebrew docs, and open-source
  checklist stay in sync.
- GitHub Actions build release artifacts for supported platforms.
- Homebrew formula installs `tiffany-loop`, `orchestrator`, and `tiffany` alias
  consistently.
- Release tags are cut only after preflight, fake-runtime e2e, multi-runtime
  e2e, and open-source audit pass.
- Version bumps are milestone-based, not every small UI tweak.

### M6: Prove The Pipeline

Goal: the planner/critic/worker/reviewer pipeline is shown to change outcomes,
not just produce more output. A product whose pitch is adversarial review has to
show the review catching things.

Acceptance:

- A small fixed task set is run both `direct` and `full`. For each task, the
  full path's critic or reviewer surfaces at least one concrete defect the
  direct path would have shipped, captured in a repeatable report.
- Critic "reject" is exercised on at least one planted-bad plan and shown to
  force a re-plan rather than no-op.
- Token and wall-clock cost of `full` versus `direct` is reported per task, so
  the user can see what the extra stages buy and what they cost.
- A short qualitative write-up records where the pipeline helped and where it
  added noise with no signal.

## Versioning

- Releases are milestone-based. Target mapping: M0 to `v0.2`, M1 to `v0.3`,
  M2 to `v0.4`, M3 to `v0.5`, M4 to `v0.6`, M5 to `v0.7`, M6 to `v1.0` (first
  stable). The mapping is a target, not a date promise: a milestone ships when
  its acceptance criteria pass.
- No more `0.1.x` feature releases. The `0.1.x` line (through `0.1.33`) is
  frozen; only security or crash hotfixes backport to it.
- A release tag is cut only after preflight, fake-runtime e2e, multi-runtime
  e2e, open-source audit, and (from M3 on) the real-runtime check all pass.

## Workstreams

### Native TUI

- Keep upstream Ratatui event/state/render architecture intact.
- Tiffany behavior lives in a separate bridge crate
  (`tiffany-ui/codex-rs/tiffany-bridge/`) that stays inside the Codex fork
  workspace and avoids a dependency back on `codex-tui`. Pure Tiffany parsing,
  formatting, and session helpers move there; fork-side Ratatui code adapts
  those results into UI. The current `tiffany_orchestrator.rs` remains a
  migration target, but broad extraction is paused unless a slice directly
  unblocks M2, M3, or M4.
- Fork-side seams (app dispatch hooks, command menu items, history-cell
  formatting) stay thin: they delegate into the bridge crate and hold no Tiffany
  business logic of their own.
- Do not rebuild terminal selection, copy/paste, IME, mouse scroll, or resize in
  the legacy TUI.
- Keep command menu small and only show working Tiffany commands.

### Runtime And Routing

- Maintain three execution flows:
  - `direct`: conversational answer, one worker.
  - `single`: atomic task, one worker.
  - `full`: planner -> critic -> worker -> reviewer.
- Make route decisions explicit in the run header.
- Keep planner/critic/reviewer out of simple chat.
- Preserve full pipeline for implementation tasks that need decomposition and
  review.

### Providers, Models, Roles

- Providers own API type, key/env, and endpoint.
- Models bind provider plus API model name.
- Roles bind model plus runtime plus optional native-team behavior.
- Users should normally think in `provider/model@runtime`, not internal model
  IDs.
- Internal IDs remain stable for config and scripting.

### Native Tool Adapters

- Claude Code adapter must support native resume, provider isolation, approval
  surfacing, and real session capture.
- Codex adapter must use compatible `codex exec` flags for installed versions
  and capture resumable session IDs.
- Gemini adapter must parse stream JSON for real `session_id`.
- Future adapters should implement the same session and visible-event contract.

### Upstream Fork Strategy

- `tiffany-ui/codex-rs/` is a vendored snapshot of upstream Codex's Rust
  workspace, not a submodule and not auto-synced. It is frozen at a recorded
  upstream commit; the commit hash, freeze date, and the Tiffany-owned vs
  vendored-untouched file list live in `tiffany-ui/UPSTREAM.md`.
- Policy is vendor-freeze with manual backport. Security fixes and critical
  terminal-primitive fixes are cherry-picked from upstream on demand; there is
  no scheduled rebase. This matches the "lightweight layer, not a replacement"
  North Star and avoids silent rot.
- Tiffany owns a narrow surface inside the fork: the bridge crate, command-menu
  items, history-cell formatting, and app dispatch hooks. Editing any other
  vendored file requires a one-line entry in `UPSTREAM.md` explaining why.
- Pruning goal: vendor only what the TUI shell needs (the `tui` crate and its
  dependencies), not the entire Codex CLI engine, unless a concrete Tiffany
  feature depends on `core`, `exec`, or `mcp`. Vendored-but-unused code is
  carry-weight and a future-sync liability.
- Any future upstream sync must happen after Tiffany-owned code has moved into
  the bridge crate. Merging upstream into a tree where Tiffany logic is
  interleaved with vendored code is explicitly out of scope.

### Persistence

- SQLite stores indexed sessions, events, tasks, worker threads, and jobs.
- JSONL keeps full append-only session/event logs.
- `~/.tiffany/tiffany-orchestrator/native-sessions.json` mirrors local native
  history for TUI-side continuity.
- Import/sync commands keep local native history and DB history comparable.

### Observability

- Normal output is human-readable.
- Raw traces are available but not the default.
- Errors should identify the failing stage, role, provider/model/runtime, and
  next action.
- Tests should cover formatter behavior because display regressions are product
  regressions.

## Near-Term Order

1. **Hold external release decisions.** Local M0 gates are green, but the failed
   remote `v0.2` tag still points at the older commit. Do not move the tag or
   push a replacement release until the user chooses force-move `v0.2` versus
   cut `v0.2.1`.
2. Tighten worker waterfall de-duplication and JSON humanization.
3. Finish role/provider/session display polish in the native TUI.
4. Keep hardening `/continue open <role>` evidence: the native command execution
   seam now has a dry-run proof, while a full human-interactive PTY smoke remains
   optional future coverage.
5. Improve `/jobs` and `/queue` visual states for paused/running/retry/recover.
6. Keep README and README.zh-CN aligned with the current install and setup flow.
7. Continue small `tiffany-bridge` extractions only when they directly unblock
   M2/M3/M4; broad extraction is paused.

## Definition Of Done

A change that touches orchestration UX is done only when:

- It works from the native `tiffany-loop` TUI, not only the legacy CLI.
- It has focused tests for parser/formatter/command routing behavior.
- It does not expose unsupported upstream commands in Tiffany mode.
- It does not require users to read JSON for normal operation.
- It preserves native terminal behavior inherited from the Codex fork.
- It updates docs or changelog when user-facing behavior changes.

## Validation Gates

Fast local checks:

```bash
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib
git diff --check
```

Pre-release checks:

```bash
./scripts/tiffany-codex-tui-lib-test
./scripts/tiffany-e2e-fake-runtime
./scripts/tiffany-e2e-multi-runtime
./scripts/open-source-audit
./scripts/tiffany-release-preflight --full --tag vX.Y.Z
```

Required for M3 sign-off (session continuity is the differentiating feature;
fake-runtime proves nothing about real handoff, so this gate is mandatory):

```bash
./scripts/tiffany-real-runtime-check --adapter-run --runtime all
```

The adapter gate runs two same-role prompts per selected native runtime and then
asserts that the second run keeps the same Tiffany worker thread and the same
native session id while `/thread` still exposes native resume, native handoff,
`/continue open <role>` actions, and `/thread export <role>` can write a
handoff file containing the worker result. It also imports a Tiffany native
history file into the session DB and verifies the CLI `/history` query path via
`orchestrator sessions native-history --format text --role <role>`.

## Milestone Status

Status is tracked against milestone acceptance criteria, not a percentage.
Markers: done, partial, not started. These are self-reported from the Current
State section above and must be re-confirmed by running the Validation Gates
before any release tag.

- M0 Stabilize Shell — mostly done locally. The single multi-call binary,
  installed aliases, runtime-only slim build, upstream-update suppression, and
  `~/.tiffany` config isolation are verified locally; the exact upstream commit
  in `tiffany-ui/UPSTREAM.md` is still TODO, and external `v0.2` release/tag
  handling remains on hold pending user decision.
- M1 Provider/Role Setup — partial. `/role` and `/provider` flows need operator
  polish; `/doctor` one-line fixes incomplete.
- M2 Execution Display — partial. Worker waterfall de-duplication and JSON
  humanization still needed.
- M3 Session Continuity — partial. Real-runtime adapter continuity now verifies
  same-role Tiffany/native session reuse for Claude Code, Codex, and Gemini
  locally, checks `/thread export` handoff output, and proves native history can
  import into/query from the session DB. `/continue open` has unit proof for the
  TUI handoff command execution seam plus dry-run proof that the exact native
  handoff command can be recorded without opening an interactive CLI; a full
  human-interactive PTY smoke is still not automated.
- M4 Queue/Background Jobs — partial. `/jobs` and `/queue` visual states for
  paused/retry/recover still needed.
- M5 Release/Open-Source — partial. Milestone-based versioning not enforced;
  installed-user and contributor docs thin.
- M6 Prove The Pipeline — not started. No efficacy evidence yet.

Cross-cutting foundation gaps owned by no single milestone:

- Upstream fork strategy is partially documented: `tiffany-ui/UPSTREAM.md`
  exists with a freeze date and owned-file list, but the exact upstream commit
  still needs to be matched against upstream Codex.
- `tiffany_orchestrator.rs` (roughly 18k lines) must be decomposed into the
  `tiffany-bridge` crate before it grows further and before any upstream sync.

The basic idea is proven. The remaining work is integration hardening, display
quality, real-runtime validation of the headline feature, fork-boundary
discipline, and demonstrating that the pipeline actually improves outcomes.
