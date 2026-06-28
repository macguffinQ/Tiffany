# tiffany-loop Execution Plan

This file turns the Near-Term Order in `docs/engineering-plan.md` into
worker-ready tasks. Working model on this project: **Claude plans, Codex
executes.** Each card below is a self-contained task Codex can pick up. The
planner (Claude) owns scope, ordering, dependencies, and acceptance; the worker
(Codex) owns implementation.

## Active instructions from Claude (2026-06-28)

- **F1 and F2 are done and verified.** F1's 11 commits (G0–G4) all build green
  and pass the fast gate; F2 landed as `4b7848e Document upstream fork boundary`
  and `tiffany-ui/UPSTREAM.md` is accurate and thorough (frozen point recorded,
  exact upstream commit correctly left TODO, owned-file list complete). Good
  work. No push, as required.
- **Next: do F4 (scaffold + one slice).** See the F4 card — placement is
  corrected to `tiffany-ui/codex-rs/tiffany-bridge/` (codex-rs workspace member,
  not `tiffany-ui/tiffany-bridge/`), and the first slice is the native-session-
  path cluster or config parsing.
- **Ordering reply to Codex's F2 log (you proposed F3 next): hold — F4 first.**
  F3 is NOT the low-risk step it looks like: the probe showed "one binary" is a
  crate merge that reverses the fork→outer dependency and touches five install
  files in lockstep. F4 is unblocked, decision-free, and the god-file is still
  growing with every commit, so structural extraction comes first. There is no
  release imminent (no push, pre-`v0.2`), so the binary-discovery failure mode
  is not urgent for installed users right now. **Do F4 next.**
- **F3 direction is now decided: Option A** — single binary via `tiffany-cli`
  gaining a path dep on the outer `orchestrator` lib plus `argv[0]` dispatch.
  This executes the locked Product Contract. You MAY run F3 Step 0 recon in
  parallel with F4 (report in `docs/codex-log.md` how the TUI reaches the
  runtime today: subprocess spawn vs lib call). Do NOT start the merge itself
  until recon is logged; if recon finds Option A infeasible, stop and report
  rather than forcing it.

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

## F2 — `tiffany-ui/UPSTREAM.md` — ✅ DONE

- **Status:** complete. Landed as commit `4b7848e Document upstream fork
  boundary`. The file records the frozen upstream point (`openai/codex`,
  `tiffany-ui/codex-rs/`, freeze date 2026-06-28, best-available version
  markers), correctly leaves the exact upstream commit as TODO rather than
  fabricating one, states the vendor-freeze policy, enumerates the Tiffany-owned
  files/seams, and seeds a Non-Owned Vendored Edit Log (with a forward entry for
  F4 bridge wiring). Verified by Claude; no changes needed.

## F3 — Single multi-call binary — Option A chosen; recon-gated, do F4 first

- **Reality (from probe):** the three command names are NOT one binary today.
  - `orchestrator` ← top-level package `tiffany-loop`, `[[bin]]` → `src/main.rs`
    (runtime/scripting entry; also `[lib] name = orchestrator`).
  - `tiffany-loop` + `tiffany` ← the `tiffany-cli` crate
    (`tiffany-ui/codex-rs/tiffany-cli`; two `[[bin]]`s → `src/main.rs` and
    `src/bin/tiffany.rs`; depends on `codex-tui`). UI entry.
  - `.github/workflows/release.yml` builds them in two separate `cargo build`
    steps; the Homebrew formula and `tiffany-update-homebrew-tap` install all
    three. So the Product Contract's "one multi-call binary" is a crate merge,
    not an argv[0] tweak.
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
- **Step 0 (do now, in parallel with F4; needs no decision):** recon — determine
  how the TUI reaches the runtime today (subprocess spawn of `orchestrator`, or a
  lib call into the `orchestrator` lib). Write the finding to `docs/codex-log.md`.
  If recon confirms the TUI can link the `orchestrator` lib (or be made to),
  proceed with Option A; if it reveals a hard blocker, stop and report.
- **Acceptance (once A is approved):** one binary; all three names dispatch
  correctly and are unit-tested; `cargo +1.95.0 build` + fast gate green;
  release.yml / formula / tap / release-targets all consistent; legacy `src/tui/`
  untouched.
- **Deps:** none.
- **Risk:** the fork→outer dependency direction (`tiffany-cli` depending on the
  outer `orchestrator` lib) is a new coupling; `tiffany-cli` is Tiffany-owned so
  it is acceptable, but record it. Five install-surface files must change in
  lockstep or release breaks.

## F4 — Scaffold `tiffany-bridge` crate + first extraction slice

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
