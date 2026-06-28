# Codex → Claude log

This file is the Codex→Claude half of the file-based handoff. Claude→Codex
instructions and task cards live in `docs/execution-plan.md`; the shared product
source of truth is `docs/engineering-plan.md`. Codex appends a status entry here
after each task (or when it hits a blocker / question). Claude reads this file to
track progress and plan the next batch. Working model: **Claude plans, Codex
executes, they talk through files — not through chat relay.**

Entry format:

```
## <task id, e.g. F1> — <short status> — <date or "in progress">
- Commits: <list of (hash, subject) or "none">
- Build/tests: <green/red + which commands ran>
- Decisions: <group merges forced by build failures, scope changes, files left uncommitted and why>
- Blockers / questions for Claude: <...>
- Next: <what Codex proposes to pick up>
```

Hard rules while executing anything from `docs/execution-plan.md`:
- Do NOT `git push` or cut release tags — those are gated user steps.
- Do NOT hunk-split the `tiffany_orchestrator.rs` god-file.
- Every commit must pass `cargo +1.95.0 build` (cargo at `~/.cargo/bin/cargo`).
- When in doubt or scope expands, stop and write a question here for Claude
  instead of guessing.

---

## F1 — completed locally — 2026-06-28
- Commits:
  - `d76b007` Revise engineering plan and add Codex execution plan
  - `cf26cd9` Expand release scripts and workflow with e2e and real-runtime checks
  - `467b6ba` Capture and reuse native sessions across claude, codex, gemini adapters
  - `5853f9b` Extend planner critic worker reviewer pipeline and tiffany events
  - `850ee30` Add doctor jobs provider role config and acp surface
  - `03564c3` Sync legacy TUI shims with native TUI and core changes
  - `3797449` Refactor slash-command dispatch and command menu
  - `5476f0c` Add chat composer pending preview and user message flow
  - `8f6cfe0` Add role and provider setup views
  - `1a8b3cd` Render worker sessions history sync and run views in orchestrator bridge
  - `f4d0a78` Update CHANGELOG and README for native TUI roles and jobs
- Build/tests:
  - `cargo +1.95.0 build` was run after every F1 commit and returned green.
  - Final fast gate green:
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — 45 passed, 0 failed.
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — 6 passed, 0 failed.
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — 2 passed, 0 failed.
    - `git diff --check` — green.
  - Known warning: `src/tui/queue_state.rs` has unused `QueueSnapshot::new`.
- Decisions:
  - Executed Claude's G0 → G4 grouping directly.
  - G2 and G3 sub-splits all built green, so no merge-forward commit was forced.
  - Kept `tiffany_orchestrator.rs` whole in G3d as instructed.
  - Reverted the earlier F3 `Cargo.toml` partial edit before starting F1.
  - Left `tiffany-ui/UPSTREAM.md` uncommitted because it belongs to F2, not F1.
  - Did not push and did not cut any release.
- Blockers / questions for Claude:
  - No F1 blocker.
  - Confirm whether Codex should commit the existing F2 `tiffany-ui/UPSTREAM.md` next, or wait for a more detailed F2/F3/F4 card.
- Next:
  - Recommended next task: finish F2 by reviewing and committing `tiffany-ui/UPSTREAM.md`, then continue F3 single multi-call binary once its install-surface details are approved.

## F2 — completed locally — 2026-06-28
- Commits:
  - `4b7848e` Document upstream fork boundary
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green after commit.
  - Final fast gate green before commit:
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — 45 passed, 0 failed.
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — 6 passed, 0 failed.
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — 2 passed, 0 failed.
    - `git diff --check` — green.
  - Known warning: `src/tui/queue_state.rs` has unused `QueueSnapshot::new`.
- Decisions:
  - Created `tiffany-ui/UPSTREAM.md` with the vendor-freeze policy, Tiffany-owned file set, vendored-untouched default, and non-owned vendored edit log.
  - Recorded the exact upstream commit as TODO because the vendor tree has no nested `.git` metadata and local Cargo/package markers do not include an upstream commit hash.
  - Updated `docs/engineering-plan.md` so the source of truth no longer says `UPSTREAM.md` is missing.
  - Did not push and did not cut any release.
- Blockers / questions for Claude:
  - No F2 blocker.
  - F4 is now unblocked, but F3 still looks like the lower-risk next step because it removes the installed binary-discovery failure mode before bridge extraction starts.
- Next:
  - Recommended next task: F3 single multi-call binary, including Cargo, entry dispatch tests, release workflow, Homebrew tap script, and formula/symlink install surface.

## F4 — completed locally — 2026-06-28
- Commits:
  - `15c50d2` Extract config summary into tiffany bridge
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui startup_readiness --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_orchestrator --lib --quiet` — green, 243 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green after commit.
  - Final fast gate green:
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — 45 passed, 0 failed.
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — 6 passed, 0 failed.
    - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — 2 passed, 0 failed.
    - `git diff --check` — green.
  - Known warning: `src/tui/queue_state.rs` has unused `QueueSnapshot::new`.
- Decisions:
  - Added `tiffany-ui/codex-rs/tiffany-bridge/` as a codex-rs workspace member.
  - Extracted exactly one pure slice from `tui/src/tiffany_orchestrator.rs`: orchestrator config YAML parsing plus default worker readiness summary.
  - Kept file IO, path expansion, runtime probing, and Ratatui rendering inside `codex-tui`; `tiffany-bridge` has no dependency back on `codex-tui`.
  - Updated `tiffany-ui/UPSTREAM.md` to record the corrected bridge path and the non-owned vendored `tui/Cargo.toml` dependency edit.
  - Stopped a stuck full `codex-tui --lib` verification run and removed generated `.snap.new` files; the failure mode was unrelated snapshot drift / app-test hang outside the F4 extraction slice.
  - F3 recon note: the earlier root-to-Codex-TUI direct link probe should not be resumed as-is. It pulled in the nested Codex workspace and hit a `codex-rmcp-client` type mismatch (`InitializeResult` vs `Arc<InitializeResult>`). Keep the locked Option A direction, but implement it later from `tiffany-cli` toward the outer `orchestrator` lib and feature-gate the TUI dependency.
- Blockers / questions for Claude:
  - No F4 blocker.
  - The next extraction should stay small: native session path helpers or visible-output formatting, not a broad bridge rewrite.
- Next:
  - Recommended next task: F3 Step 0 recon only, then either implement the `tiffany-cli` single-binary dispatch path if feasible, or start F5 by moving the next pure helper cluster into `tiffany-bridge`.
