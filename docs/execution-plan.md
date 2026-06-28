# tiffany-loop Execution Plan

This file turns the Near-Term Order in `docs/engineering-plan.md` into
worker-ready tasks. Working model on this project: **Claude plans, Codex
executes.** Each card below is a self-contained task Codex can pick up. The
planner (Claude) owns scope, ordering, dependencies, and acceptance; the worker
(Codex) owns implementation.

## Active instructions from Claude (updated 2026-06-29 — v0.2 CI failure)

- **⚠ v0.2 CI FAILURE — HOLD the tag move; user decision pending.** v0.2 was
  pushed + tagged; GitHub Actions FAILED before publishing artifacts (no
  artifacts, no Homebrew tap update → no user-facing damage). Two failures:
  (1) `startup_health_actions_report_ok_when_ready` assumed an adjacent standalone
  `orchestrator` binary — a consequence of the F3 merge making `orchestrator` an
  alias; (2) the release workflow's preflight step. Codex's hermetic-test fix
  (temp launchable binaries) + CI/preflight `--lib` alignment is correct. Before
  any re-push: (a) rerun `./scripts/tiffany-release-preflight --full --tag v0.2`
  green; (b) grep for OTHER tests/code with the same ambient-`orchestrator`
  assumption and fix them; (c) confirm the WHOLE release.yml passes, not just the
  one test; (d) **DO NOT move or force-push the v0.2 tag until the user picks
  force-move-v0.2 vs cut-v0.2.1** — force-pushing a tag is destructive and beyond
  the original clean push+tag authorization.

- **PHASE DECISION (supersedes the stale F7-era bullets below): foundation push
  is substantially complete — STOP grinding extraction.** Tree clean, 146 ahead,
  no push. god-file 18,545 → 14,945; the easy pure slices are extracted, the rest
  is rendering/app-server-entangled and not worth forcing (Codex flagged this).
  F1–F8 all done and committed.
- **M0 close-out status:** F9 is implemented locally: CI release builds now run
  through the checked-in build-time regression gate, the baseline table is
  validated by release target checks, and a clean local `tiffany-dist` build was
  measured at 347s against a 1575s default gate limit. The two previously
  unchecked M0 items also have targeted tests: upstream update prompts are
  disabled in Tiffany mode, and Tiffany config/state defaults to `~/.tiffany`
  instead of `~/.codex`.
- **Next priority — pivot to M3 (the differentiator):** Claude/Codex/Gemini
  session capture + resume across restarts with REAL runtimes (the required M3
  gate, not fake-runtime). Provider 429 rate-limiting and slow Claude startup may
  block some live checks — do what is possible now, log what is quota-blocked,
  and make the harness re-runnable when quota recovers.
- **F5+ extraction is PAUSED** (diminishing returns). Resume only if a slice
  unblocks M3 or a later milestone; do not force entangled rendering/app-server
  slices.
- **Push + `v0.2` AUTHORIZED (user, 2026-06-28; supersedes the older "Gated /
  user-only" text further down):** after the F9 commit exists and a full
  `./scripts/tiffany-release-preflight --full --tag v0.2` passes, push `main`,
  then tag `v0.2` and push the tag — the tag fires the release workflow (build
  artifacts + Homebrew tap update). Do NOT ask again before pushing; the green
  gates ARE the authorization. If ANY gate fails, STOP and report; do not push or
  tag. Real-runtime check is NOT required for v0.2 (that is the M3 / v0.5 gate).

- **F1, F2, F4 ✅ done and verified.** F1 (G0–G4) and F2 (`4b7848e`) as before;
  F4 landed as `15c50d2` Extract config summary into tiffany bridge.
  `tiffany-bridge` is a codex-rs workspace member at
  `tiffany-ui/codex-rs/tiffany-bridge/`; one pure slice moved (config YAML
  parsing + default worker readiness summary); no dep back on `codex-tui`;
  build + fast gate green. Extraction pattern proven. No push (~127 ahead).
- **F3 ✅ done and verified.** Landed in combined fallback commit `e3d34d2`
  with F5. The feasible
  direction was `tiffany-cli` → outer `orchestrator` lib. The probe did not hit
  the `codex-rmcp` skew in this direction; it hit a SQLite native-link mismatch
  instead, fixed by aligning root `rusqlite` to the Codex workspace's
  `libsqlite3-sys` line. Current local tree builds one Cargo bin
  (`tiffany-loop`) and exposes `orchestrator` / `tiffany` as package/dev aliases.
  `tiffany-cli` dispatches by argv[0], and `codex-tui` is optional behind the
  default `tui` feature. No push, no release tag.
- **F5 ✅ done and verified.** Landed in combined fallback commit `e3d34d2`
  with F3. Moved the
  native-session path helper slice into `tiffany-bridge` as
  `src/native_session.rs`: runtime detection, Claude/Codex/Gemini transcript
  path lookup, Gemini project hash, and Gemini message count. `codex-tui`
  now uses a thin adapter in `tiffany_orchestrator.rs`; transcript parsing and
  rendering stayed in the TUI. One slice only.
- **Commit directive resolved with the allowed combined fallback; push still
  gated.** F3 + F5 landed together because the F3-only split forced a fresh
  fork lockfile generation that rolled unrelated upstream dependencies. Do not
  freeze that noisy intermediate. Original target was TWO logical commits if
  each builds green —
    - F3 first: `Unify tiffany-loop, orchestrator, and tiffany into one
      multi-call binary` (delete root `src/main.rs`, add `src/cli_entry.rs`,
      argv[0] dispatch in `tiffany-cli/src/main.rs`, drop
      `tiffany-cli/src/bin/tiffany.rs`, `codex-tui` optional behind the `tui`
      feature, the install-surface files, and the SQLite alignment). The commit
      body MUST note the new coupling — outer `rusqlite` now tracks the vendored
      codex-rs `libsqlite3-sys` line — and add a one-line
      `tiffany-ui/UPSTREAM.md` Non-Owned Vendored Edit Log entry for it.
    - F5 second: `Extract native session path helpers into tiffany bridge`.
  The applied fallback is ONE combined commit with a body covering both. Still
  DO NOT push.
- **F7 ✅ implemented locally and verified.** Added the
  runtime-only no-TUI smoke script, wired it into release preflight and script
  helper checks, documented it in both READMEs, and cleaned the no-default warning
  path.
- **F5+ visible-output formatting slice ✅ implemented locally and verified.**
  A new `tiffany-bridge/src/visible_output.rs` owns visible content cleanup,
  worker output kind selection, dedupe scope/key calculation, and compact plan
  summary shaping. `codex-tui` now adapts `TiffanyProgressEvent` into the bridge
  view and keeps Ratatui rendering local.
- **F5+ command-argument parsing slice ✅ implemented locally and verified.**
  A new `tiffany-bridge/src/command_args.rs` owns pure `/roles`, `/provider`,
  `/doctor`, `/thread`, `/jobs`, and `/continue` argument mapping. `codex-tui`
  imports the bridge API and keeps command spawning/rendering local.
- **F5+ native-history command/filter slice ✅ implemented locally and verified.**
  A new `tiffany-bridge/src/native_history.rs` owns pure `/history` command
  parsing, native-history filter display, event-view matching, and event-kind
  alias matching. `codex-tui` keeps local JSON/session-store loading and
  Ratatui/Markdown rendering.
- **F5+ command-output parsing slice ✅ implemented locally and verified.**
  A new `tiffany-bridge/src/output_parse.rs` owns pure role-profile row parsing,
  jobs retry prompt/handoff parsing, recovered-job detail parsing, and persisted
  job summary/action shaping. `codex-tui` keeps Ratatui line styling local.
- **F5+ provider/role/thread output parsing slice ✅ implemented locally and
  verified.** `tiffany-bridge/src/output_parse.rs` now also owns provider
  summary parsing, role/role-option summary parsing, thread list/detail field
  parsing, and worker-thread title parsing. `codex-tui` keeps Ratatui card and
  waterfall rendering local.
- **F5+ native CLI handoff parsing slice ✅ implemented locally and verified.**
  `tiffany-bridge/src/output_parse.rs` now owns parsing `/thread` detail output
  into a native CLI handoff command, including native session id, runtime,
  Tiffany worker thread id, command, and worktree. `codex-tui` adapts the bridge
  result into its local return/transcript workflow.
- **F5+ native transcript parsing slice ✅ implemented locally and verified.**
  A new `tiffany-bridge/src/native_transcript.rs` owns pure Claude Code JSONL,
  Codex JSONL, and Gemini chat JSON parsing into normalized native transcript
  events. `codex-tui` only attaches Tiffany role/session metadata and persists
  the events.
- **F5+ native chat store model slice ✅ implemented locally and verified.**
  A new `tiffany-bridge/src/native_chat_store.rs` owns the native history store
  schema, conversation/turn/event structs, conversation id hashing, and
  normalization. `codex-tui` keeps file IO, Ratatui rendering, and session-db
  import orchestration local.
- **F5+ native history graph rendering slice ✅ implemented locally and
  verified.** A new `tiffany-bridge/src/native_history_render.rs` owns pure
  native history text graph and Mermaid rendering plus compact tool-event graph
  labels. `codex-tui` keeps file IO, export path handling, Ratatui lines, and
  Markdown rendering local.
- **F5+ native history Markdown export rendering slice ✅ implemented locally and
  verified.** `tiffany-bridge/src/native_history_render.rs` now also owns pure
  native history Markdown body rendering and event metadata formatting.
  `codex-tui` keeps filtering, export path handling, file IO, and user-facing
  status lines local.
- **F8 ✅ implemented locally and verified.** Full `codex-tui --lib` now passes
  serially after stabilizing no-active-turn app-server paths, no-turn side-thread
  discard behavior, large-stack test wrappers, Tiffany snapshot drift, and
  trailing-whitespace-free terminal snapshot helpers.
- **Next:** continue F5+ extraction with another small pure slice, then decide
  whether to make the remaining app-server-heavy tests parallel-stable or keep
  the full TUI gate documented as serial.
- **F7 was M0-required and is now satisfied locally:** the slim runtime-only
  build exists and smoke-passes before M0 / `v0.2`; full release preflight still
  remains gated before tagging.
- **Still no push, no release tag.**

Toolchain note for every task: cargo lives at `/Users/allendred/.cargo/bin/cargo`
(not on the default PATH), toolchain `+1.95.0`. Fast validation gate to run
before claiming any task done:

```bash
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib
/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib
git diff --check
```

## Dependency graph

```
F1 (commit hygiene)        ── independent
F2 (UPSTREAM.md)           ── independent, unblocks F4
F3 (multi-call binary)     ── done in `e3d34d2`
F4 (bridge crate + first   ── blocked by F2
     extraction slice)
F5 (native session paths)  ── done in `e3d34d2`
F7 (slim smoke)            ── local slice verified
PUSH (origin/main)         ── GATED, user-only, only after F1 grouping approved
```

Recommended order: F2 → F3 → F4 (scaffold only) in parallel where possible, F1
running throughout. Do not start the big `tiffany_orchestrator.rs` extraction
beyond the scaffold until F2 lands.

---

## F1 — Commit hygiene

- **Goal:** get the working tree under control without losing or blindly
  lumping the +18,692-line uncommitted diff, and stage the 113 unpushed commits
  for review.
- **Files:** whole working tree; especially `tiffany-ui/codex-rs/tui/`.
- **Grouping (Claude has pre-grouped; execute in order):**
  - **G0** docs (commit first): `docs/engineering-plan.md`, `docs/execution-plan.md` → `Revise engineering plan and add Codex execution plan`.
  - **G1** release tooling (shell, builds alone): `.github/workflows/release.yml`, all modified `scripts/tiffany-*` + `scripts/open-source-audit`, new scripts `tiffany-check-legacy-tui-shims`, `tiffany-e2e-fake-runtime`, `tiffany-e2e-multi-runtime`, `tiffany-real-runtime-check`, `tiffany-release-targets`, `scripts/tests/` → `Expand release scripts and workflow with e2e and real-runtime checks`.
  - **G2** core runtime (src/ + event-format; try sub-split, merge on build failure): G2a adapters (`src/adapters/*`, `src/core/session_store.rs`, `src/core/worker.rs`) → `Capture and reuse native sessions across claude, codex, gemini adapters`; G2b pipeline+events (`src/pipeline/orchestrator.rs`, `src/tiffany_events.rs`, `crates/tiffany-event-format/src/lib.rs`) → `Extend planner/critic/worker/reviewer pipeline and tiffany events`; G2c CLI surface (`src/cli.rs`, `src/main.rs`, `src/lib.rs`, `src/config.rs`, `src/doctor.rs`, `src/acp.rs`, `src/tui_jobs.rs`) → `Add doctor, jobs, provider/role config, and acp surface`; G2d legacy shims (`src/tui/*.rs`) → `Sync legacy TUI shims with native TUI and core changes`. Merged msg: `Add native sessions, roles, jobs, doctor, and pipeline to runtime`.
  - **G3** native TUI fork (after G2; try sub-split): G3a slash dispatch (`chatwidget/slash_dispatch.rs`, `chatwidget/tests/slash_commands.rs`, `bottom_pane/slash_commands.rs`, `bottom_pane/command_popup.rs` + snapshots, `slash_command.rs`, `chatwidget.rs`, `chatwidget/constructor.rs`, `chatwidget/input_flow.rs`) → `Refactor slash-command dispatch and command menu`; G3b composer (`bottom_pane/chat_composer.rs`, `chatwidget/user_messages.rs`, `chatwidget/tests/composer_submission.rs`, `bottom_pane/pending_input_preview.rs`, `bottom_pane/mod.rs`, `app/input.rs`, `app/event_dispatch.rs`, `app.rs`, `app_event.rs`) → `Add chat composer, pending preview, and user message flow`; G3c role views (`bottom_pane/role_setup_view.rs`, `bottom_pane/role_profile_setup_view.rs`) → `Add role and provider setup views`; G3d god-file (ALWAYS whole, ALWAYS last: `tiffany_orchestrator.rs`, `cli/src/tiffany.rs`, `tiffany-cli/src/main.rs`) → `Render worker sessions, history sync, and run views in orchestrator bridge`. Merged msg: `Add worker sessions, slash dispatch, composer, role setup to native TUI`.
  - **G4** docs last: `CHANGELOG.md`, `README.md`, `README.zh-CN.md` → `Update CHANGELOG and README for native TUI, roles, and jobs`.
- **Execution rules:** commit G0 → G1 → G2 → G3 → G4; `cargo +1.95.0 build` after every commit (sub-split only if each subset builds green, else merge forward); G3d is never hunk-split; run the fast gate at the end. Do NOT `git push` — that is a gated user step. After finishing, append status (commits made, merges forced by build failures, test results, blockers, questions for Claude) to `docs/codex-log.md`.
- **Acceptance:** working tree clean or only flagged WIP; every commit builds; subjects match the existing Title-Case-imperative log style; status written to `docs/codex-log.md`; push has NOT happened.
- **Deps:** none.
- **Risk:** the diff may contain scratch/WIP — if a file looks like throwaway, log it in `docs/codex-log.md` and leave it uncommitted rather than freezing it. G2 and G3d are unavoidably large because the layers are mutually dependent; do not force a split that breaks the build.

## F2 — `tiffany-ui/UPSTREAM.md` — ✅ DONE

- **Status:** complete. Landed as commit `4b7848e Document upstream fork
  boundary`. The file records the frozen upstream point (`openai/codex`,
  `tiffany-ui/codex-rs/`, freeze date 2026-06-28, best-available version
  markers), correctly leaves the exact upstream commit as TODO rather than
  fabricating one, states the vendor-freeze policy, enumerates the Tiffany-owned
  files/seams, and seeds a Non-Owned Vendored Edit Log (with a forward entry for
  F4 bridge wiring). Verified by Claude; no changes needed.

## F3 — Single multi-call binary — ✅ DONE

- **Status:** implemented and verified in combined fallback commit `e3d34d2`
  with F5. No push or release has happened.
- **Previous reality:** the three command names used to be separate binaries:
  root `orchestrator`, plus `tiffany-cli`'s `tiffany-loop` and `tiffany`.
- **Current implementation:** `tiffany-cli` is now the single Cargo binary. It
  depends on the outer `orchestrator` library and dispatches by argv[0] basename:
  `tiffany-loop` / `tiffany` → TUI, `orchestrator` → runtime CLI. The root package
  keeps `[lib]` and drops the explicit `[[bin]] orchestrator`; the old
  `src/bin/tiffany.rs` wrapper is removed so Cargo metadata exposes only
  `tiffany-loop`.
- **Option A (recommended, honors the locked contract):** make `tiffany-cli`
  the single binary. It gains a path dep on the outer `orchestrator` lib and its
  `main()` dispatches by `argv[0]` basename — `tiffany-loop`/`tiffany` → TUI,
  `orchestrator` → runtime CLI (delegate to the orchestrator lib's CLI entry).
  Drop the top-level `[[bin]] orchestrator` (keep `[lib]`). Release builds ONE
  artifact and installs `orchestrator`/`tiffany` as symlinks to it. Files:
  `tiffany-cli/src/main.rs`, top-level `Cargo.toml`, `tiffany-cli/Cargo.toml`,
  `.github/workflows/release.yml`, `packaging/homebrew/tiffany-loop.rb`,
  `scripts/tiffany-update-homebrew-tap`, `scripts/tiffany-release-targets`.
- **Option B (fallback, smaller, contradicts the contract):** keep two binaries,
  add a robust resolver (known install location / PATH probe with an exact
  repair message), and amend the Product Contract to drop "one binary."
- **Value (concrete):** eliminates the installed-user failure class — symptoms
  like "event bridge failed" and "can't find orchestrator" — by removing the
  cross-binary discovery/coexistence problem entirely. This is the M0 win.
- **Cost & mandatory follow-up (user judgment):** linking the full Codex TUI
  into the single root binary raises compile cost noticeably. The trade is
  accepted because it kills the install failure class, but slimming becomes
  mandatory afterward — via feature-gating the TUI (see queued F7) and/or ongoing
  F4+ bridge extraction. Design the merge so the TUI is a feature-gated
  dependency from the start (`optional = true`, default feature); retrofitting
  that later is expensive.
- **Step 0 ✅ DONE (recon result):** the "link TUI into root" direction is blocked
  by `codex-rmcp-client` type skew (`InitializeResult` vs `Arc<InitializeResult>`)
  and nested-workspace pull-in. The feasible direction is `tiffany-cli` → outer
  `orchestrator` lib.
- **Acceptance:** one binary; all three names dispatch correctly and are
  unit-tested; `cargo +1.95.0 build` + fast gate green; release.yml / formula /
  tap / release-targets all consistent; legacy `src/tui/` untouched.
- **Deps:** none.
- **Risk:** the fork→outer dependency direction (`tiffany-cli` depending on the
  outer `orchestrator` lib) is a new coupling; `tiffany-cli` is Tiffany-owned so
  it is acceptable, but record it. Five install-surface files must change in
  lockstep or release breaks.

## F4 — Scaffold `tiffany-bridge` crate + first extraction slice — ✅ DONE

- **Status:** complete (commit `15c50d2`). `tiffany-bridge` created as a codex-rs
  workspace member at `tiffany-ui/codex-rs/tiffany-bridge/`; one pure slice moved
  (config YAML parsing + default worker readiness summary); no dep back on
  `codex-tui`; UPSTREAM.md updated; build + fast gate green (tiffany-bridge 2
  tests, codex-tui `tiffany_orchestrator` 243 tests). Pattern proven. Further
  extraction is queued as F5+.
- **Goal:** stand up the crate boundary that `tiffany_orchestrator.rs` (~18.5k
  lines) migrates into, and prove the pattern by moving ONE self-contained
  slice. Do NOT move more than one slice in this task.
- **Placement (corrected):** add `tiffany-bridge` as a MEMBER of the
  `tiffany-ui/codex-rs` workspace at `tiffany-ui/codex-rs/tiffany-bridge/` — NOT
  at `tiffany-ui/tiffany-bridge/` (there is no `tiffany-ui` workspace root and
  Cargo workspaces cannot nest; same-workspace membership is required so
  dependency versions align with `codex-tui`). Update the `tiffany-ui/UPSTREAM.md`
  "Future bridge crate" line accordingly to `tiffany-ui/codex-rs/tiffany-bridge/**`.
- **Dependency direction (critical):** `codex-tui` depends on `tiffany-bridge`
  (the TUI calls the bridge). `tiffany-bridge` MUST NOT depend back on
  `codex-tui` (that is circular). So the first slice must be PURE: only `std`,
  `serde`, and types co-located inside `tiffany_orchestrator.rs` — no ratatui,
  no `codex-tui` items.
- **First slice (pick one, verify purity before moving):**
  - native-session-path resolution — the `*_session_*_path` and
    `find_*_path_by_id` helpers (~lines 1706–1900: claude/codex/gemini
    JSONL/rollout/chat path math); or
  - config parsing — the `RawTiffany*Config` / `TiffanyOrchestratorConfig`
    structs and serde impls (~lines 218–312).
  Verify the chosen slice's types are all defined in `tiffany_orchestrator.rs`
  and it imports nothing from ratatui / codex-tui; if it does, pick the other.
- **Steps:** (1) create `tiffany-ui/codex-rs/tiffany-bridge/{Cargo.toml,src/lib.rs}`
  mirroring workspace `edition`/`license`/`[lints]`; (2) add `tiffany-bridge` to
  `tiffany-ui/codex-rs/Cargo.toml` `members`; (3) move the slice's types+fns
  verbatim into the bridge, re-export from `lib.rs`; (4) add
  `tiffany-bridge = { path = "../tiffany-bridge" }` to
  `tiffany-ui/codex-rs/tui/Cargo.toml` and update call sites to
  `tiffany_bridge::...`; (5) `cargo +1.95.0 build` + `cargo test -p tiffany-bridge`
  + `-p codex-tui` green; (6) update the UPSTREAM.md owned path. Stop after one
  slice; queue the rest as F5+.
- **Acceptance:** workspace builds; `tiffany-bridge` compiles standalone with a
  passing test; one slice moved out of the god-file; `codex-tui` reaches it via
  public API; `tiffany_orchestrator.rs` line count dropped; UPSTREAM.md path
  corrected.
- **Deps:** F2 ✅ (UPSTREAM.md exists and already lists the future bridge crate).
- **Risk:** highest-risk task. A slice that secretly depends on `codex-tui`
  internals hits the circular-dependency wall — which is why purity is verified
  before moving. Never big-bang; one slice, build green, then stop.

## F5 — Native session path helpers — ✅ DONE

- **Status:** implemented and verified in combined fallback commit `e3d34d2`
  with F3. No push or release has happened.
- **Moved slice:** `tiffany-ui/codex-rs/tiffany-bridge/src/native_session.rs`
  now owns runtime detection plus native transcript path lookup for Claude Code,
  Codex rollout JSONL, and Gemini chat JSON. It also owns Gemini project hashing
  and message counting.
- **TUI seam:** `tiffany-ui/codex-rs/tui/src/tiffany_orchestrator.rs` keeps
  transcript parsing/rendering but delegates path resolution through a thin
  `NativeSessionCommand` adapter.
- **Acceptance:** `tiffany-bridge` compiles and tests independently; `codex-tui`
  native CLI transcript tests pass; only one pure helper slice was moved.
- **Risk:** more native-history code remains in the god-file. Do not move
  transcript parsers until path helpers stay stable across real-runtime checks.

---

## Gated (user-only, not auto-executed)

- **PUSH to `origin/main`:** 113 local commits + whatever F1 commits. Outward-
  facing and hard to reverse. Codex must surface the proposed push and wait for
  explicit user go-ahead. Not part of any worker task above.
- **`v0.2` release cut (M0):** only after F1–F4/F7 land and the full preflight
  (`./scripts/tiffany-release-preflight --full --tag v0.2`) passes.

## Queued (after F3/F4)

- F5+: continue `tiffany_orchestrator.rs` extraction into `tiffany-bridge`,
  one module per task, build green each time, until the god-file is empty and
  deleted. Completed slices: config summary, native session paths, visible
  output formatting/dedupe, contextual prompt assembly, command argument
  parsing, native-history command/filter parsing, command-output parsing.
- F6: prune vendored `codex-rs` crates that Tiffany does not use (per the
  pruning goal in the Upstream Fork Strategy), guided by `UPSTREAM.md`.
- F7 ✅ done locally: the Codex TUI is behind the default `tui` feature and
  `./scripts/tiffany-slim-smoke --quiet` verifies the runtime-only no-TUI binary.
- F8 ✅ done locally: the full `cargo test -p codex-tui --lib` gate passes
  serially, and `./scripts/tiffany-codex-tui-lib-test` makes that slow gate
  reusable from full/tagged release preflight.
- Then M1 (provider/role polish), M2 (waterfall de-dup), M3 (real-runtime
  handoff — now a required gate), M4 (queue/jobs UI), M6 (pipeline efficacy).
