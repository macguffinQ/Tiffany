# tiffany-loop Execution Plan

This file turns the Near-Term Order in `docs/engineering-plan.md` into
worker-ready tasks. Working model on this project: **Claude plans, Codex
executes.** Each card below is a self-contained task Codex can pick up. The
planner (Claude) owns scope, ordering, dependencies, and acceptance; the worker
(Codex) owns implementation.

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
F3 (multi-call binary)     ── independent
F4 (bridge crate + first   ── blocked by F2
     extraction slice)
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

## F2 — Write `tiffany-ui/UPSTREAM.md`

- **Goal:** document the fork boundary so vendored vs Tiffany-owned code is
  explicit. This is the unblock for F4 and the cheapest hedge against fork rot.
- **Files:** new file `tiffany-ui/UPSTREAM.md`.
- **Prompt for Codex:** "Create `tiffany-ui/UPSTREAM.md`. Record: (1) the
  frozen upstream point — find the best available identifier by inspecting
  `tiffany-ui/codex-rs/Cargo.toml` workspace / `codex-core` version and any
  CHANGELOG or version markers; if an exact upstream commit cannot be
  determined, record `codex-core` version + freeze date and mark the exact
  commit as TODO; (2) the freeze policy (vendor-freeze with manual backport,
  no scheduled rebase); (3) the Tiffany-owned file set — derive this from
  `git diff` / recent commits under `tiffany-ui/codex-rs/tui/` (notably
  `tiffany_orchestrator.rs`, `bottom_pane/*`, `chatwidget/slash_dispatch.rs`,
  command-menu and history-cell code); (4) the rule that editing any
  non-owned vendored file requires a one-line entry here explaining why."
- **Acceptance:** file exists; names a frozen upstream identifier; lists owned
  vs untouched files; states the backport policy.
- **Deps:** none.
- **Risk:** exact upstream commit may be unrecoverable from a vendor drop.
  Version + date with a TODO is an acceptable fallback; do not fabricate a
  commit hash.

## F3 — Single multi-call binary

- **Goal:** collapse `tiffany-loop`, `orchestrator`, and `tiffany` into one
  binary dispatched by `argv[0]`, eliminating cross-binary discovery.
- **Files:** `src/main.rs` (entry + dispatch), `Cargo.toml` (`[[bin]]`),
  `scripts/tiffany-update-homebrew-tap`, `.github/workflows/release.yml`, and
  the Homebrew formula — all install-surface files must change in lockstep.
- **Prompt for Codex:** "Make the three command names one multi-call binary.
  At entry, inspect `argv[0]` basename: `tiffany-loop` → default TUI entry,
  `orchestrator` → runtime/scripting entry, `tiffany` → compat alias to the TUI
  entry; unknown/`cargo run` → default. Add unit tests for the dispatch table.
  Update Cargo `[[bin]]` accordingly, and update the Homebrew tap script,
  release workflow, and formula so install creates the two extra symlinks
  pointing at the one binary. Keep the legacy `src/tui/` path untouched."
- **Acceptance:** one binary builds; the three names route to the right
  entrypoints; dispatch is unit-tested; `cargo +1.95.0 build` and the fast gate
  pass; install-surface files are consistent with each other.
- **Deps:** none (independent of F2).
- **Risk:** install surface is spread across Cargo + Homebrew script + release
  workflow + formula. Missing any one breaks release. Acceptance requires all
  four in sync.

## F4 — Scaffold `tiffany-bridge` crate + first extraction slice

- **Goal:** establish the crate boundary that `tiffany_orchestrator.rs`
  (~18k lines) migrates into, and prove the pattern by moving ONE small,
  self-contained module. Do NOT attempt the full extraction in this task.
- **Files:** new crate `tiffany-ui/tiffany-bridge/` (`Cargo.toml`, `src/lib.rs`,
  one extracted module); `tiffany-ui/Cargo.toml` (workspace member);
  `tiffany-ui/codex-rs/tui/Cargo.toml` (add `tiffany-bridge` dep); the thin seam
  in `codex-rs/tui` that calls into the bridge.
- **Prompt for Codex:** "Create `tiffany-bridge` as a workspace crate under
  `tiffany-ui/`. It depends on `codex-tui`'s public API only. Pick the smallest
  self-contained slice currently inside `tiffany_orchestrator.rs` — prefer
  command-menu item definitions or history-cell formatting, nothing with deep
  entanglement — and move it verbatim into `tiffany-bridge`. Wire the original
  call site in `codex-rs/tui` to delegate to the bridge. Build green after the
  move. Stop after one module; the remaining extraction is queued as F5+."
- **Acceptance:** workspace builds; `tiffany-bridge` compiles standalone; one
  module has moved out of the god-file; `codex-tui` reaches it via public API;
  `cargo +1.95.0 test -p tiffany-bridge` and `-p codex-tui` pass.
- **Deps:** F2 (UPSTREAM.md must record `tiffany-bridge` as Tiffany-owned first).
- **Risk:** highest-risk task. Extraction must be incremental — one module,
  build green, repeat. A big-bang move of 18k lines will not review and will
  break the build. This task is deliberately scoped to scaffold + one module.

---

## Gated (user-only, not auto-executed)

- **PUSH to `origin/main`:** 113 local commits + whatever F1 commits. Outward-
  facing and hard to reverse. Codex must surface the proposed push and wait for
  explicit user go-ahead. Not part of any worker task above.
- **`v0.2` release cut (M0):** only after F1–F4 land and the full preflight
  (`./scripts/tiffany-release-preflight --full --tag v0.2`) passes.

## Queued (after F1–F4)

- F5+: continue `tiffany_orchestrator.rs` extraction into `tiffany-bridge`,
  one module per task, build green each time, until the god-file is empty and
  deleted.
- F6: prune vendored `codex-rs` crates that Tiffany does not use (per the
  pruning goal in the Upstream Fork Strategy), guided by `UPSTREAM.md`.
- Then M1 (provider/role polish), M2 (waterfall de-dup), M3 (real-runtime
  handoff — now a required gate), M4 (queue/jobs UI), M6 (pipeline efficacy).
