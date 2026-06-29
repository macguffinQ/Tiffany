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

## F3/F5 — completed locally, combined fallback — 2026-06-28
- Commits:
  - `e3d34d2` Unify tiffany-loop multi-call binary and bridge native session paths
- Build/tests:
  - Probe: `tiffany-cli` depending on the outer `orchestrator` lib built cleanly in this direction; the previous `codex-rmcp-client` type skew did not recur.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-cli --quiet` — green, 18 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-cli --no-default-features --quiet` — green, 18 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop cli_entry --quiet` — green, 20 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 7 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_cli --lib --quiet` — green, 23 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `TIFFANY_QUIET=1 ./scripts/tiffany-build --small` — green.
  - `./scripts/tiffany-install-smoke --smoke` — green; `orchestrator`, `tiffany-loop`, and `tiffany` all resolved in isolated `target/dev-small`.
  - `./scripts/tiffany-check-homebrew-tap-helper` — green.
  - `./scripts/tiffany-release-targets --check` — green.
  - `./scripts/tiffany-check-script-helpers` — green.
  - Cargo metadata now shows only one `tiffany-cli` bin target: `tiffany-loop`.
  - argv0 smoke with symlinked `orchestrator`/`tiffany` verified `orchestrator --help` routes to runtime CLI while `tiffany-loop --help` and `tiffany --help` route to Tiffany Loop.
  - `git diff --check` — green.
  - Known warning cleanup for `src/tui/queue_state.rs` is left outside this commit with F7 groundwork.
- Decisions:
  - Implemented Option A in the feasible direction: `tiffany-ui/codex-rs/tiffany-cli` depends on the outer `tiffany-loop` package as library crate `orchestrator`.
  - Moved the runtime CLI entry from root `src/main.rs` into `src/cli_entry.rs`, exported it from `src/lib.rs`, removed the root `[[bin]] orchestrator`, and deleted root `src/main.rs`.
  - Added argv[0] dispatch in `tiffany-cli/src/main.rs`: `orchestrator` invokes `orchestrator_runtime::cli_entry::run_from_env()`, while `tiffany-loop` and `tiffany` keep the TUI path.
  - Made `codex-tui` an optional dependency behind default feature `tui`; the no-default test path now passes, satisfying the F7 foundation.
  - Deleted `tiffany-cli/src/bin/tiffany.rs` so Cargo does not auto-discover/install a second `tiffany` wrapper binary.
  - Updated release workflow, Homebrew formula template, tap updater, dev/build scripts, release-target checks, and docs to build/package one `tiffany-loop` binary and expose `orchestrator`/`tiffany` aliases.
  - The probe initially hit a SQLite native-link conflict (`rusqlite 0.32` vs the Codex workspace `libsqlite3-sys 0.37` line); aligned root `rusqlite` to `0.39` with bundled sqlite.
  - Did not touch legacy `src/tui/` behavior except through the library entry move.
  - Used the allowed combined fallback instead of two commits: an attempted F3-only fork lockfile generation rolled unrelated upstream dependencies, so freezing that intermediate would have been noisier and riskier than the verified combined state.
  - Moved exactly one pure slice into `tiffany-bridge`: native session runtime detection plus Claude/Codex/Gemini transcript path lookup, Gemini project hashing, and Gemini message counting.
  - Added `tiffany-ui/codex-rs/tiffany-bridge/src/native_session.rs` and re-exported its public API from `tiffany_bridge`.
  - Added bridge dependencies `serde_json` and `sha2`, plus dev-dependency `tempfile` for isolated path tests.
  - Kept transcript parsing, event construction, and Ratatui rendering in `codex-tui`; `tui/src/tiffany_orchestrator.rs` now adapts `TiffanyNativeCliCommand` into `tiffany_bridge::NativeSessionCommand`.
  - `tiffany_orchestrator.rs` dropped from 18,369 lines to 18,151 lines after this slice.
  - Did not touch legacy `src/tui/` behavior.
  - Kept F7 groundwork out of the F3/F5 combined commit; it landed separately afterward.
- Blockers / questions for Claude:
  - No F3/F5 blocker.
  - Full `cargo test -p codex-tui --lib` remains queued as F8.
- Next:
  - Recommended next task: F7, because M0 requires the runtime-only slim build to smoke-pass and F3 already introduced the `tui` feature gate. If continuing extraction instead, move visible-output formatting as the next single slice.

## F7 — completed locally — 2026-06-28
- Commits:
  - this commit: `Add slim runtime-only smoke check`
- Build/tests:
  - `./scripts/tiffany-slim-smoke --quiet` — green; no-default dependency tree did not include `codex-tui`, slim binary built under `target/tiffany-slim/dev-small`, and `orchestrator --help`, `orchestrator status`, `tiffany-loop status --bin`, `tiffany-loop orchestrator --legacy help`, and the explicit no-TUI failure message were verified.
  - `./scripts/tiffany-check-script-helpers` — green.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-cli --no-default-features --quiet` — green, 18 passed.
  - `git diff --check` — green.
- Decisions:
  - Added `scripts/tiffany-slim-smoke` as the M0 guard for the runtime-only multi-call binary.
  - Wired the smoke into `scripts/tiffany-release-preflight` and syntax coverage into `scripts/tiffany-check-script-helpers`.
  - Documented the slim smoke entrypoint in both READMEs.
  - Marked `QueueSnapshot::new` test-only so no-default runtime builds do not carry that warning.
- Blockers / questions for Claude:
  - No F7 blocker.
  - Full `cargo test -p codex-tui --lib` remains queued as F8.
- Next:
  - Recommended next task: F5+ visible-output formatting extraction, then F8 full codex-tui lib-test stabilization.

## F5+ — visible-output formatting extraction completed locally — 2026-06-28
- Commits:
  - this commit: `Extract visible output formatting into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 11 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui output --lib --quiet` — green, 77 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Added `tiffany-ui/codex-rs/tiffany-bridge/src/visible_output.rs` as the next small bridge slice.
  - Moved visible content cleanup, worker output kind selection, native-session recovery detection, dedupe scope/key calculation, and compact plan summary shaping out of `tui/src/tiffany_orchestrator.rs`.
  - Kept Ratatui `Line`/`Span` rendering and history-cell layout in `codex-tui`; the bridge remains UI-renderer-free.
  - Added bridge tests for planner JSON humanization, assistant/result dedupe scope, tool-result seen keys, and event-kind override behavior.
  - Added a `tiffany-ui/UPSTREAM.md` log entry because the fork workspace `Cargo.lock` records the new `tiffany-event-format` bridge dependency.
  - `tiffany_orchestrator.rs` is now 17,918 lines; this slice removed another visible-output decision cluster from the god-file.
- Blockers / questions for Claude:
  - No blocker.
  - Full `cargo test -p codex-tui --lib` remains queued as F8.
- Next:
  - Either continue F5+ with another pure bridge slice, or switch to F8 and stabilize the full codex-tui lib test before release CI depends on it.

## F8 — full codex-tui lib-test stabilization completed locally — 2026-06-28

- Commits:
  - this commit: `Stabilize codex tui lib tests`
- Build/tests:
  - Detected local Claude Code update: `claude --version` reports `2.1.193 (Claude Code)`.
  - `claude --help` still exposes the runtime flags Tiffany depends on: `--print`, `--output-format`, `--resume`, `--continue`, `--fork-session`, `--session-id`, and permission-mode flags.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui --lib --quiet -- --test-threads=1` — green, 3149 passed, 0 failed, 1 ignored, 359.48s.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
  - No `.snap.new` files remain.
- Decisions:
  - Treated no-active-turn interrupt/discard as a handled no-op instead of routing through app-server startup interrupt paths.
  - Added large-stack test wrappers for app-server-heavy TUI tests to avoid stack-overflow aborts in full lib runs.
  - Accepted Tiffany branding/version/menu snapshot drift from the Codex fork.
  - Updated rate-limit and prompt wording from `codex` / `Codex` to `tiffany-loop` where the visible product name is Tiffany.
  - Normalized terminal snapshot helpers to trim trailing row padding before snapshot assertion; this keeps `git diff --check` clean without changing real TUI rendering.
- Blockers / questions for Claude:
  - No blocker for F8.
  - The final full gate is verified in serial mode. Parallel full-test behavior can be a separate hardening task because app-server-heavy tests are still expensive and were historically flaky when run concurrently.
- Next:
  - Continue F5+ with the next small pure bridge slice, or add an explicit CI script for the serial `codex-tui --lib` gate before release preflight starts depending on it.

## F8 follow-up — codex-tui serial gate scripted locally — 2026-06-28

- Commits:
  - this commit: `Add codex tui serial lib gate`
- Build/tests:
  - `./scripts/tiffany-codex-tui-lib-test --help` — green.
  - `./scripts/tiffany-check-script-helpers` — green.
  - `./scripts/tiffany-codex-tui-lib-test` — green, 3149 passed, 0 failed, 1 ignored, 360.43s.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
  - No `.snap.new` files remain.
- Decisions:
  - Added `scripts/tiffany-codex-tui-lib-test` as the reusable serial full `codex-tui --lib` gate.
  - Wired the gate into `scripts/tiffany-release-preflight` for `--full`, any `--tag` check, or explicit `TIFFANY_PREFLIGHT_CODEX_TUI_LIB=1`.
  - Kept ordinary untagged `--quick` preflight shorter; the slow gate runs when release confidence matters.
  - Added script syntax coverage to `scripts/tiffany-check-script-helpers`.
  - Documented the script and preflight behavior in README, README.zh-CN, `docs/engineering-plan.md`, and `docs/execution-plan.md`.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Resume F5+ with the next small pure bridge slice from `tui/src/tiffany_orchestrator.rs`.

## F5+ — contextual prompt extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract contextual prompt builder into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 15 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui contextual_prompt --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
  - No `.snap.new` files remain.
- Decisions:
  - Added `tiffany-bridge/src/context_prompt.rs` with a small borrowed `ContextPromptTurn` view.
  - Moved multi-turn contextual prompt assembly and truncation rules out of `tui/src/tiffany_orchestrator.rs`.
  - Kept memory file IO, schema normalization, and `TiffanyOrchestratorTurn` storage inside `codex-tui`; the TUI now adapts turns into bridge views.
  - Added bridge tests for empty history, recent-turn inclusion, six-turn limiting, and long-text truncation.
  - `tiffany_orchestrator.rs` is now 17,887 lines.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another pure helper slice, likely command argument parsing or native-history display shaping.

## Claude Code 2.1.193 compatibility check — completed locally — 2026-06-28

- Commits:
  - this commit: `Guard claude stream json verbose flag`
- Build/tests:
  - `claude --version` — `2.1.193 (Claude Code)`.
  - `claude --help` still exposes `--print`, `--output-format`, `--resume`, `--continue`, `--fork-session`, `--session-id`, `--permission-mode`, and `--setting-sources`.
  - Manual smoke without `--verbose` confirmed the new CLI error: `--output-format=stream-json requires --verbose`.
  - Manual smoke with `--verbose` emitted valid `stream-json` init including `session_id` and `claude_code_version`, then stopped on API `429 rate_limit`; this is provider rate limiting, not CLI protocol failure.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop claude_worker_args --lib --quiet` — green, 3 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop claude_role_args --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop claude --lib --quiet` — green, 27 passed.
  - `git diff --check` — green.
- Decisions:
  - Root Claude worker and role subprocess code already passed `--verbose` with `--output-format stream-json`.
  - Added regression assertions so future refactors do not remove `--verbose`.
  - No push and no release.
- Blockers / questions for Claude:
  - No code blocker.
  - Real provider calls currently hit `429 rate_limit`; retest end-to-end once provider quota recovers.
- Next:
  - Resume F5+ command-argument parser extraction into `tiffany-bridge`.

## F5+ — command-argument parsing extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract command args into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge command_args --quiet` — green, 9 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 24 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui thread_command_args --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_command_args --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui continue_target_role --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui doctor_command_args --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui roles_args --lib --quiet` — green, 3 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui roles_command_args --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added `tiffany-bridge/src/command_args.rs` for pure `/roles`, `/provider`, `/doctor`, `/thread`, `/jobs`, and `/continue` argument mapping.
  - `codex-tui` now imports bridge parsing functions and keeps only command spawning, subprocess output handling, and Ratatui rendering.
  - Moved parser coverage into bridge and kept TUI-side parser tests passing as adapter coverage.
  - `tiffany_orchestrator.rs` is now 17,424 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another pure helper slice, likely native-history display shaping or command-output summary helpers.

## F5+ — native-history command/filter extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract native history commands into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge native_history --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 28 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_command --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_can_filter --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_kind_permission --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_lines --lib --quiet` — green, 3 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_status --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added `tiffany-bridge/src/native_history.rs` for pure `/history` command parsing, filter display, event-view matching, and event-kind alias matching.
  - `codex-tui` now adapts `TiffanyNativeChatEvent` into a bridge `NativeHistoryEventView`; storage loading and Ratatui/Markdown rendering remain local.
  - `tiffany_orchestrator.rs` is now 17,102 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another pure helper slice, likely command-output summary helpers or native-history render text shaping.

## F5+ — command-output parsing extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract command output parsing into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge output_parse --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 32 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_profile_summary_lines --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_summary_lines --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_retry --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added `tiffany-bridge/src/output_parse.rs` for pure role profile row parsing, jobs retry prompt/handoff parsing, recovered jobs detail parsing, and persisted job summary/action shaping.
  - `codex-tui` now keeps Ratatui color/styling and card-line rendering local while delegating output parsing and action derivation to the bridge.
  - `tiffany_orchestrator.rs` is now 16,736 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another pure helper slice, likely provider/role/thread output parsers or native-history render text shaping.

## F5+ — provider/role/thread output parsing extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract role provider thread parsers into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge output_parse --quiet` — green, 7 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui summary_lines --lib --quiet` — green, 17 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 35 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Extended `tiffany-bridge/src/output_parse.rs` to own provider summaries, role summaries, role-option summaries, thread list rows, thread detail fields, and worker-thread title parsing.
  - `codex-tui` now imports those bridge structs/functions and keeps only Ratatui card/waterfall rendering plus native CLI command assembly local.
  - Added bridge-level tests for provider, role, role-option, thread list, and thread field parsing while preserving the existing TUI summary-line tests.
  - `tiffany_orchestrator.rs` is now 16,246 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another small pure helper slice, likely native-history render text shaping or native CLI command derivation if it can stay UI-free.

## F5+ — native CLI handoff parsing extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract native CLI handoff parsing into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge output_parse --quiet` — green, 8 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_cli_command --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 36 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Added `NativeCliHandoff` and `parse_native_cli_handoff` to `tiffany-bridge::output_parse`.
  - Kept `TiffanyNativeCliCommand` local to `codex-tui`; the TUI now adapts bridge handoff output into its existing return/transcript workflow.
  - Bridge parser prefers `native handoff` over `native resume`, preserves Tiffany worker thread id, and refuses `native session: none`.
  - `tiffany_orchestrator.rs` is now 16,212 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another pure helper slice; likely native CLI transcript event parsing is next, but only if it can be separated without pulling Ratatui or app-server state into the bridge.

## F5+ — native transcript parsing extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract native transcript parsing into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge native_transcript --quiet` — green, 3 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_cli_transcript --lib --quiet` — green, 9 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 39 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Added `tiffany-bridge/src/native_transcript.rs` for pure Claude Code JSONL, Codex JSONL, and Gemini chat JSON parsing.
  - The bridge now returns normalized native transcript events with `kind`, `title`, and `content`; `codex-tui` attaches Tiffany role/session metadata and persists local/native history.
  - Preserved existing TUI transcript behavior through the `native_cli_transcript` test set, including tool calls, tool results, patches, diffs, approvals, and Gemini skipped-message handling.
  - `tiffany_orchestrator.rs` is now 15,408 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another small pure slice, or pause extraction and start real-runtime handoff verification for M3 if Claude wants validation over more god-file reduction.

## F5+ — native chat store model extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract native chat store model into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge native_chat_store --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history --lib --quiet` — green, 26 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 40 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Added `tiffany-bridge/src/native_chat_store.rs` for native history schema structs, store normalization, conversation id hashing, and event kind inference for older/local JSON events.
  - `codex-tui` now exposes crate-local type aliases for the bridge store types so existing TUI modules keep their stable `tiffany_orchestrator::TiffanyNativeChatEvent` path.
  - Kept native history file IO, session-db import orchestration, Markdown/graph export, and Ratatui rendering in `codex-tui`.
  - `tiffany_orchestrator.rs` is now 15,224 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with a small pure native-history rendering/search slice, or switch to real-runtime handoff verification if validation is the priority.

## F5+ — native history graph rendering extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract native history graph rendering into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge native_history_render --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_graph --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_mermaid --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history --lib --quiet` — green, 26 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 42 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Added `tiffany-bridge/src/native_history_render.rs` for pure native history text graph rendering, Mermaid rendering, event summaries, and compact tool-event labels.
  - `codex-tui` now calls bridge renderers from `/history graph` and `/history mermaid`, while retaining local file IO, export path handling, Ratatui `Line` construction, and Markdown export.
  - Kept `/history compact` behavior by reusing the bridge tool-event label helper instead of duplicating JSON compaction in the TUI.
  - `tiffany_orchestrator.rs` is now 15,018 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker. Claude Code 2.1.193 is installed locally; `--help` confirms `stream-json`, `--verbose`, `--resume`, and `--session-id` semantics remain available. A live minimal `claude --print` smoke command produced no output within the short timeout and was cancelled, so this slice did not rely on live Claude network behavior.
- Next:
  - Continue F5+ with another small pure native-history/search/export slice, or switch to real-runtime Claude/Codex/Gemini handoff verification if validation is now the priority.

## F5+ — native history Markdown export rendering extraction completed locally — 2026-06-28

- Commits:
  - this commit: `Extract native history markdown rendering into tiffany bridge`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge native_history_markdown --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history_export --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_history --lib --quiet` — green, 26 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge --quiet` — green, 43 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Extended `tiffany-bridge/src/native_history_render.rs` with `NativeHistoryMarkdownTurn` and pure Markdown body rendering for `/history export`.
  - Kept filtering, export path selection, directory creation, file writes, and Ratatui status lines in `codex-tui`.
  - Preserved worker role and kind filtered exports through existing TUI tests.
  - `tiffany_orchestrator.rs` is now 14,945 lines.
  - No push and no release.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue F5+ with another small pure native-history slice, likely search hit extraction or compact storyline shaping, unless Claude prioritizes real-runtime handoff validation.

## F9 — release build-time regression gate completed locally — 2026-06-28

- Commits:
  - `4b813ea` Add release build-time regression gate
- Build/tests:
  - `bash -n scripts/tiffany-build-time-gate` — green.
  - `./scripts/tiffany-build-time-gate --self-test` — green.
  - `./scripts/tiffany-build-time-gate --check-baselines` — green.
  - `./scripts/tiffany-release-targets --check` — green.
  - `./scripts/tiffany-check-script-helpers` — green.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_runtime_marker_disables_upstream_update_checks --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-cli default_tiffany_home_is_not_dot_codex --quiet` — green, 1 passed.
  - Clean build-time baseline: `TIFFANY_CARGO_TARGET_DIR=$(mktemp -d) ./scripts/tiffany-build-time-gate --target default -- ./scripts/tiffany-build --fast-release --locked` built the full `tiffany-dist` binary in 5m46s and the gate reported `elapsed=347s`, `limit=1575s`, `threshold=175%`. The outer cleanup wrapper used zsh's read-only `status` name after the successful gate output, so cleanup was done manually; the build and gate result were green.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build --quiet` — green.
  - `git diff --check` — green.
- Decisions:
  - Added `scripts/tiffany-build-time-gate` and `scripts/tiffany-build-time-baselines.tsv`.
  - Release workflow build jobs now run the full `tiffany-cli` release build through the build-time gate from the repo root.
  - `scripts/tiffany-release-targets --check` now verifies every documented release asset target has a build-time baseline.
  - `scripts/tiffany-release-preflight` and `scripts/tiffany-check-script-helpers` run the gate self-test so script drift is caught locally.
  - README and README.zh-CN document the local build-time gate command and baseline file.
  - The previously unchecked M0 items are now verified locally: upstream Codex update prompts are disabled in Tiffany mode, and Tiffany config/state defaults to `~/.tiffany`, not `~/.codex`.
- Blockers / questions for Claude:
  - No F9 blocker. Claude's updated plan authorizes push + `v0.2` only after the pending F9 commit exists and `./scripts/tiffany-release-preflight --full --tag v0.2` passes.
- Next:
  - Commit F9, run the full `v0.2` preflight gate, then push/tag only if it is green.

## v0.2 release version alignment in progress — 2026-06-28

- Commits:
  - pending commit: `Prepare v0.2 release`
- Build/tests:
  - `./scripts/tiffany-release-preflight --quick --tag v0.2` initially passed
    version checks after normalizing `v0.2` to package version `0.2.0`, then
    exposed clippy issues and version snapshot drift.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt` — green.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui update_available_history_cell_snapshot --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui session_info_availability_nux_tooltip_snapshot --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui status_snapshot --lib --quiet` — green, 24 passed.
  - `bash -n scripts/tiffany-release-preflight` — green.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_orchestrator --lib --quiet` — green, 243 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui slash_commands --lib --quiet` — green, 194 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui update_action --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_runtime_marker_disables_upstream_update_checks --lib --quiet` — green, 1 passed.
  - `git diff --check` — green.
- Decisions:
  - Bumped root `tiffany-loop`, `tiffany-cli`, and `codex-tui` package versions to `0.2.0` and refreshed both lockfiles.
  - Added `CHANGELOG.md` release entry `## [0.2.0] - 2026-06-28`.
  - Allowed `scripts/tiffany-release-preflight --tag v0.2` to match Cargo package version `0.2.0` by normalizing short `major.minor` release tags.
  - Fixed release-preflight clippy findings by grouping Claude worker args and worker recovery progress fields, merging identical continue-command branches, and applying the suggested `unwrap_or` / needless-borrow cleanups.
  - Accepted version-only TUI snapshots from `0.1.33` to `0.2.0`.
  - Updated the release preflight targeted `codex-tui` tests to pass `--lib`, after the full preflight reached those checks and stalled in unnecessary binary/test target linking.
- Blockers / questions for Claude:
  - None yet. Next required gate remains `./scripts/tiffany-release-preflight --full --tag v0.2`; push/tag only if green.

## v0.2 CI/release retry fix in progress — 2026-06-28

- Commits:
  - pending commit: `Fix v0.2 CI release gate`
- Build/tests:
  - GitHub Actions first `v0.2` attempt failed before publishing artifacts: CI failed in `codex-tui tiffany_orchestrator`, and Release failed during `Run preflight`.
  - Failure was `startup_health_actions_report_ok_when_ready`: CI did not have an adjacent `orchestrator` binary, so the test saw an extra runtime repair action.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui startup_health_actions_report_ok_when_ready --lib --quiet` — green, 1 passed.
  - `rg` audit for ambient `orchestrator` assumptions found the remaining startup-readiness tests and they were made hermetic; remaining hits are command construction, explicit runtime-status behavior, or role/provider setup prefill tests that do not resolve startup readiness.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui startup_health_actions --lib --quiet` — green, 3 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui intro_lines_show --lib --quiet` — green, 5 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_orchestrator --lib --quiet` — green, 243 passed.
  - `./scripts/tiffany-release-preflight --full --tag v0.2` — green on the local fix commit, including root tests, shared formatter tests, examples, open-source audit, Homebrew helper, release target matrix, build-time gate, script helpers, slim smoke, targeted `codex-tui` tests, serial full `codex-tui --lib`, dist install smoke, fake-runtime e2e, multi-runtime e2e, real-runtime adapter harness, and examples.
  - External damage check: `gh release view v0.2` reports release not found; `gh release list` still shows `v0.1.33` as latest; the failed release run uploaded no artifacts and skipped build/publish/Homebrew jobs; `macguffinQ/homebrew-tap` formula still reports version `0.1.33`.
- Decisions:
  - Made the ready-health test fully hermetic by using temporary launchable `orchestrator` and `claude` binaries.
  - Made the other startup-readiness tests use a temporary launchable `orchestrator` binary so CI/PATH state cannot add unrelated runtime warnings.
  - Aligned CI workflow targeted `codex-tui` commands with release preflight by passing `--lib`.
- Blockers / questions for Claude:
  - Local full preflight is green, but the failed remote `v0.2` tag still points at `1211d9b`. Do not move or force-push the tag until the user chooses force-move `v0.2` vs cut `v0.2.1`.

## M3 real-runtime continuity gate tightened — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `bash -n scripts/tiffany-real-runtime-check` — green.
  - `./scripts/tiffany-real-runtime-check --self-test` — green.
  - `./scripts/tiffany-real-runtime-check --check --runtime all` — green; detected Claude Code, Codex, Gemini binaries and existing transcript stores.
  - `./scripts/tiffany-check-script-helpers` — green.
  - `git diff --check` — green.
  - `claude --version && ./scripts/tiffany-real-runtime-check --adapter-run --runtime claude` — green with Claude Code `2.1.193`.
  - `./scripts/tiffany-real-runtime-check --adapter-run --runtime codex` — green.
  - `./scripts/tiffany-real-runtime-check --adapter-run --runtime gemini` — green.
  - `./scripts/tiffany-real-runtime-check --adapter-run --runtime all` — green; covered Claude Code, Codex, and Gemini in one isolated state.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
- Decisions:
  - Strengthened `scripts/tiffany-real-runtime-check --adapter-run`: after the first real adapter run it now captures `/thread show <role>`, after the second same-role run it captures the thread again, and it fails if the Tiffany worker thread id or native session id changed.
  - Kept the existing `/thread` checks for native resume, native handoff, `/continue open <role>`, and persisted job/native/result output.
  - Added `/thread export <role> --out ...` verification so the real-runtime gate proves a handoff file can be written and includes the worker result.
  - Added native history import/query verification: the gate writes a minimal Tiffany native history file using the current worker thread/native session, runs `orchestrator sessions import-native --path ...`, then checks `orchestrator sessions native-history --format text --role <role>` returns the worker result, thread id, native id, and `/continue open <role>` hint.
  - Made the script compatible with the single multi-call binary by creating a temporary `orchestrator` symlink to a local `tiffany-loop` build when no standalone `orchestrator` path exists; this preserves argv[0] runtime dispatch.
  - Updated README, README.zh-CN, and `docs/engineering-plan.md` to describe the stronger adapter-run gate.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - M3 is improved but not complete. Remaining proof needed: a full human-interactive PTY smoke for `/continue open <role>`; the non-interactive shell execution seam is now covered separately.
- Next:
  - Add a `/continue open <role>` check that exercises the native TUI handoff path without hanging CI, likely by using a fake PTY/runtime shim for interaction semantics plus keeping the real-runtime adapter continuity gate for native session proof.

## M3 TUI native handoff execution seam covered — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_cli_handoff_command_runs_through_default_shell --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui continue_open_output --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml --all` — completed; same stable-rust `imports_granularity = Item` warning as before.
- Decisions:
  - Extracted the `/continue open` shell execution path in `tui/src/app/input.rs` into `run_native_cli_handoff_command`.
  - Kept behavior unchanged: `open_tiffany_native_cli` still restores the terminal fully, runs the handoff command through the user's default shell, captures transcript/git state after return, and records native history.
  - Added a non-interactive unit test proving the handoff command is actually executed through the default shell. This covers the TUI execution seam without trying to automate an interactive Claude/Codex/Gemini session.
- Blockers / questions for Claude:
  - Full interactive PTY behavior is still best validated manually or with a future fake PTY shim; current automated coverage now proves parsing, event emission, shell execution, return capture, thread export, and native-history DB query.
- Next:
  - Continue M2/M3 hardening: either add fake PTY coverage for `/continue open` return behavior, or switch back to worker waterfall de-dup/noise cleanup.

## Engineering plan status refreshed — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `./scripts/tiffany-install-smoke --smoke` — green; verified `tiffany-loop`, `orchestrator`, and `tiffany` aliases from `target/dev-small`.
  - `./scripts/tiffany-slim-smoke --quiet` — green; verified runtime-only no-TUI multi-call binary under `target/tiffany-slim/dev-small`.
- Decisions:
  - Updated `docs/engineering-plan.md` Near-Term Order so it no longer asks Codex to implement the already-landed single multi-call binary.
  - Updated M0 status from stale "single multi-call binary not yet implemented" to "mostly done locally"; kept the remaining caveats explicit: exact upstream commit TODO and external `v0.2` release/tag handling still blocked on user decision.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - None for the status refresh.
- Next:
  - Continue with M2 worker waterfall de-duplication / JSON humanization.

## M2 question waterfall dedupe regression covered — 2026-06-29

- Commits:
  - this commit: `Cover question waterfall dedupe`
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui bridge_state_dedupes_question_tool_call_and_error --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added a BridgeState-level regression test for the visible AskUserQuestion path so a tool-call event plus the matching `tool error: Answer questions?` event produces one human waterfall prompt, not duplicated raw wrapper lines.
  - Verified the normal view keeps the actionable `/continue open worker-cc` hint while hiding raw `AskUserQuestion` and `tool error` text.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 by adding more focused coverage for long worker tool runs: JSON reviewer/planner leakage, duplicate assistant/tool-result chunks, and final-answer completeness.

## M2 unclosed JSON fence leakage fixed — 2026-06-29

- Commits:
  - this commit: `Humanize unclosed JSON fences`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes_unclosed_json_fence_without_fence_leakage --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui humanizes_unclosed_json_fence_without_raw_leakage --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes --quiet` — green, 11 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Updated shared `tiffany-event-format` JSON-ish humanization so incomplete Markdown JSON fences no longer leak ` ```json ` lines into normal planner/critic/reviewer waterfall output.
  - Allowed a single JSON line left after fence filtering to be summarized, and stripped JSON fence-only prefix/suffix context around embedded JSON values.
  - Added both formatter-level and native TUI visible-content regressions for unclosed planner JSON fences.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 with remaining normal-view cleanup: raw/debug-only trace separation, reviewer issue display completeness, and duplicate worker answer/result stream handling.

## M2 reviewer unavailable status humanized — 2026-06-29

- Commits:
  - this commit: `Humanize reviewer unavailable status`
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui reviewer_status_lines_track_worker_review_lifecycle --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added a TUI control-status reason label so reviewer-unavailable statuses no longer expose protocol phrasing like `no JSON found in CLI response` or parser internals like `expected value at line 1 column 1` in the normal waterfall.
  - Normal view now says `returned plain text`; raw diagnostics remain available through process/doctor paths.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 by tightening raw/debug-only trace separation and verifying reviewer issue/suggestion detail stays visible when it changes the result.

## M2 Codex controller stderr humanized — 2026-06-29

- Commits:
  - this commit: `Humanize Codex controller stderr`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes_codex_exec_argument_mismatch_for_normal_view --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui controller_stderr_humanizes_codex_exec_argument_mismatch --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes --quiet` — green, 12 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Normal waterfall now maps Codex `unexpected argument '--cwd'` / `--cd` compatibility stderr into a concise `Codex CLI compatibility issue` line instead of showing raw `codex stderr`, usage text, and internal argument wording.
  - Failure diagnostics still preserve the raw stderr and existing `/doctor` hint path; this change only affects normal visible agent output.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 with more raw/debug separation and then move to M1 role/provider/session display polish once the common waterfall noise paths are covered.

## M2 control fallback compatibility text humanized — 2026-06-29

- Commits:
  - this commit: `Humanize control fallback compatibility text`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format formats_control_fallbacks_for_user_visible_surfaces --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop acp_progress_text_shows_control_fallback_as_status --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop formats_control_fallback_warning_without_fake_issue_count --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes --quiet` — green, 12 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Extended the shared control-fallback formatter so `codex exec --cd unsupported` renders as the same user-facing Codex CLI compatibility message used by normal stderr waterfall output.
  - Updated ACP and session-display expectations so all visible control fallback surfaces avoid the raw `unsupported` implementation phrase.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 with raw/debug-only trace separation, then reassess whether M1 provider/role polish should become the next active slice.

## M1 doctor endpoint repair step preserved with failures — 2026-06-29

- Commits:
  - this commit: `Preserve doctor endpoint repair step`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop report_next_steps_include_openai_compatible_endpoint_fix_with_failures --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/doctor` now keeps the OpenAI-compatible endpoint repair hint even when required failures such as missing provider auth are also present.
  - The normal next-step path now tells users to run `/provider endpoint <provider> <url>` instead of losing the endpoint fix behind other failures.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M1 provider/role setup polish or switch back to M2 worker waterfall cleanup depending on the next plan update.

## M3 real-runtime handoff self-test strengthened — 2026-06-29

- Commits:
  - this commit: `Cover real runtime handoff self-test`
- Build/tests:
  - `./scripts/tiffany-real-runtime-check --self-test` — green.
  - `./scripts/tiffany-check-script-helpers` — green.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Extended the quota-free real-runtime self-test so it now covers `/thread` detail handoff assertions for Claude Code, Codex, and Gemini role sessions.
  - Wired `tiffany-real-runtime-check --self-test` into the script helper gate, so regressions in native resume/TUI resume/thread-detail wording are caught before live runtime checks.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M3 by running or hardening `--adapter-run` per runtime where local auth/quota permits, then return to M2 waterfall cleanup for remaining real-output noise.

## M2 worker start prompt compacted — 2026-06-29

- Commits:
  - this commit: `Compact worker start prompt details`
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_started_task --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Worker start rows now keep normal view compact by shortening long task prompts to six lines and truncating very long single lines.
  - Injected multi-turn wrapper text still resolves to the current user request before compaction, so normal waterfall no longer expands previous-turn boilerplate on worker start.
  - Full worker prompt remains in the event stream and can be inspected through `/process full`.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 on remaining worker waterfall noise, especially grouping long tool runs and raw/debug separation.

## M4 queued follow-up preview numbered — 2026-06-29

- Commits:
  - this commit: `Number queued follow-up preview items`
- Build/tests:
  - From `tiffany-ui/codex-rs`: `INSTA_UPDATE=always /Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui pending_input_preview --lib --quiet` — green, 14 passed; snapshots updated.
  - From `tiffany-ui/codex-rs`: `INSTA_UPDATE=always /Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui queued_messages --lib --quiet` — green, 9 passed; snapshots updated.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui pending_input_preview --lib --quiet` — green, 14 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui queued_messages --lib --quiet` — green, 9 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Native bottom-pane queued follow-up preview now shows item counts and numbers each queued message, making execution order explicit.
  - Tiffany queued batch mode now reads as `Queued batch: N item(s), runs together after current orchestration`.
  - Pending steers keep the existing arrow display; only ordinary queued user messages became numbered.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M4 by tightening queue run/pause/clear affordances, or return to M2 worker waterfall noise if queue behavior is stable enough for now.

## M4 queue command affordances tightened — 2026-06-29

- Commits:
  - this commit: `Clarify queue command affordances`
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_queue --lib --quiet` — green, 7 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/queue show` now exposes a `next` line so users can see whether the queue is empty, paused, waiting on the current orchestration, ready to run a retry-first item, or ready to merge normal queued prompts into one batch.
  - `/queue run` during an active Tiffany orchestration now reports that the queue is armed instead of silently falling back to a generic status panel; the queued prompts still submit together after the current run finishes.
  - `/queue pause` and `/queue resume` now include queued item counts and distinguish immediate start from "armed after current orchestration".
  - `/queue clear` now reports the real queued item count instead of double-counting internal history records.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M4 with persisted queue/job status visibility, or switch to M2 worker waterfall grouping/raw-debug separation.

## M4 jobs cancel result made explicit — 2026-06-29

- Commits:
  - this commit: `Clarify jobs cancel results`
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_cancel --lib --quiet` — green, 2 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_ --lib --quiet` — green, 13 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/jobs cancel <id>` now gets the same explicit top-level result treatment as `/jobs retry` and `/jobs recover`.
  - Successful cancel output starts with `cancelled · <id>` and points back to `/jobs` to refresh persisted status.
  - No-op cancel output such as `already failed` points users to `/jobs retry <id>` before the normal failed-job card.
  - Existing status cards, actions, and humanized model/provider/runtime errors remain visible below the result line.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M4 by improving `/jobs` failed-job repair hints, or switch back to M2 waterfall grouping/raw-debug separation.

## M4 failed job repair hints added — 2026-06-29

- Commits:
  - this commit: `Add jobs repair hints`
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge job_repair --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_summary_lines --lib --quiet` — green, 4 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_ --lib --quiet` — green, 13 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added a pure `tiffany-bridge` repair-hint classifier for common failed-job error families: model missing/invalid, provider auth, endpoint/network, runtime binary, and provider rate limit.
  - Normal `/jobs` cards now render a `fix` line below the humanized error when a concrete repair path is known.
  - Model errors such as `[1211][模型不存在] invalid model` now point to `/role <role>` plus `/doctor` instead of only showing retry.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 waterfall grouping/raw-debug separation, or keep tightening M1 `/doctor` one-line fixes.

## M2 process-exit wrapper humanized — 2026-06-29

- Commits:
  - this commit: `Humanize worker process exits`
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format classifies_visible_agent_output_for_ui_surfaces --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes --quiet` — green, 12 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall_normalizes_claude_tool_status_wrappers --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Failed native worker `process_exit` lines are now normalized for normal waterfall output; `claude-code process_exit: claude exited with status exit status: 1` renders as `claude process exited with status 1`.
  - Successful native process exits still remain hidden to avoid low-value process noise.
  - The raw process event remains available through full/raw process history; this only changes normal visible output.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 with more raw/debug-only separation for worker status/control wrappers, or switch to M1 provider/role setup polish.

## M2 run intro route prediction removed — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui run_intro_defers_route_to_runtime_events --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui timeline_compacts_stale_route_selected_when_route_updates --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - The TUI run intro no longer predicts `direct`, `single`, or `full` using a local prompt classifier before the runtime starts.
  - Normal run headers now show `flow runtime` and `route waiting for orchestrator runtime classification`, then rely on actual `RouteSelected` / `RouteUpdated` events for the real flow.
  - This removes misleading combinations such as an intro showing `flow single` while the pipeline later emits `route selected · full-pipeline`.
  - Existing timeline compaction still hides stale `route selected · full` when the runtime downgrades to `single-worker` after planner fallback.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 on remaining waterfall noise/raw-debug separation, especially controller fallback and long tool-run grouping.

## M2 control fallback reasons humanized — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format formats_control_fallbacks_for_user_visible_surfaces --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes --quiet` — green, 12 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui reviewer_status_lines_track_worker_review_lifecycle --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui run_intro_defers_route_to_runtime_events --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Shared control-fallback formatting now normalizes internal parse phrases before display.
  - `no JSON found`, `expected value at line`, `invalid json`, and `malformed json` render as `returned plain text`.
  - `no_sub_tasks`, `returned no_sub_tasks`, and no-worker-run variants render as `planner returned no worker runs`, even when the raw reason also contains `parse failed`.
  - TUI review-unavailable/control reason labels use the same no-worker-run ordering so normal waterfall output does not leak parser jargon.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M2 on long tool-run grouping/raw-debug separation, or move to M1 `/doctor` one-line fixes.

## M2 orchestrator failure details humanized — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui failure_lines_humanize_plain_text_parse_failures --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui failure_lines --lib --quiet` — green, 7 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format classifies_common_agent_failures --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format humanizes --quiet` — green, 12 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
- Decisions:
  - Normal orchestrator failure detail lines now apply the same control-reason humanization as timeline fallback rows.
  - `no JSON found in CLI response` and parser detail lines render as `returned plain text` instead of leaking internal parser jargon into the waterfall.
  - `no_sub_tasks` / no-worker-run variants continue to render as `planner returned no worker runs`.
  - The shared agent-failure classifier now treats `no json found` and `no_sub_tasks` as parse failures, so the TUI can show the parse repair hint pointing users to `/process full`.
  - Raw diagnostics remain available through full process/debug surfaces; only normal failure display is changed.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Pivot to the current Claude priority: M3 real-runtime session continuity and handoff checks across Claude Code, Codex, and Gemini, unless the user asks to package the current M2 slice first.

## M3 real adapter runtime continuity verified — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `./scripts/tiffany-real-runtime-check --self-test` — green.
  - `./scripts/tiffany-real-runtime-check --check-installed --runtime all` — green.
  - `./scripts/tiffany-real-runtime-check --adapter-run --runtime all` — green; Claude Code, Codex, and Gemini adapter probes passed.
- Decisions:
  - Verified the current installed local native CLIs are present: `claude`, `codex`, and `gemini`.
  - Verified transcript stores are discoverable for all three runtimes.
  - The real adapter probe completed two same-role runs for each runtime and validated thread detail, jobs, thread export, native history import, and native history query paths.
  - This is the strongest current M3 signal: real adapter continuity works on this machine for Claude Code, Codex, and Gemini.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker from the real adapter continuity check.
- Next:
  - Decide whether to commit the current M2+M3 verification slice, then continue M3 with native TUI `/continue open <role>` manual smoke and restart recovery documentation.

## M3 native TUI handoff dry-run seam added — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui native_cli_handoff_command_ --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added a hidden `TIFFANY_NATIVE_CLI_DRY_RUN_LOG` execution seam for the native TUI `/continue open <role>` path.
  - Normal production behavior is unchanged: Tiffany still restores the terminal and runs the parsed native handoff command.
  - Dry-run mode records the exact command that would be executed and returns success without opening Claude Code, Codex, or Gemini.
  - This closes the automated evidence gap between "thread detail exposes `/continue open`" and "the TUI command execution seam can safely receive and execute a native handoff command"; full human-interactive PTY smoke remains optional future coverage.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M3 restart/recovery documentation or move to M1 provider/role polish.

## M3 native session recovery docs corrected — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Updated README and README.zh-CN to describe the real busy/missing native-session policy.
  - Busy native sessions now document the continuity-preserving behavior: wait and retry with the same native session.
  - Missing/unresumable native sessions now document the fresh-session fallback: clear the stale native id and retry once.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M1 provider/role polish or M4 queue/jobs visual states; M3 now has stronger real-runtime, dry-run, and user-doc evidence but still lacks a full interactive PTY smoke.

## M1 role summary prioritizes provider/API model — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - `git diff --check` — green.
- Decisions:
  - `/roles` and `/role <name>` cards now put `provider` and `api model` before the internal model id.
  - The internal model id remains visible as `internal id` for scripting/debugging, but normal role setup reads as provider/API-model/runtime first.
  - Single-role output without provider metadata now shows `provider  unbound`, making the missing binding explicit instead of implying the model id is the primary setup target.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M1 provider detail/editor polish or move to M4 queue/jobs visual states.

## M1 provider summary renders actionable cards — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_summary_lines --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/provider` summaries now render each provider as a compact card instead of a dense table row.
  - Provider cards show type, endpoint, auth state, model bindings, role bindings, and health in separate scan-friendly lines.
  - Action rows now point directly at `/provider <name>` for healthy providers, `/provider edit <name>` for missing auth or missing providers, `/role` or `/roles` depending on role binding state, and `/doctor`.
  - Existing parser support for config-show output, provider registry output, and provider detail output is preserved.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M1 by tightening the provider editor/delete path, or move to M4 queue/jobs visual states.

## M4 jobs cards show primary next steps — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_summary_lines --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/jobs` cards now show a status-specific `next` line before lower-level metadata.
  - Queued jobs point to `/queue run`; running jobs point to `/process 200` plus `/jobs recover`; failed jobs point to `/jobs retry <id>`.
  - Completed native jobs point to `/continue open <role>`; completed Tiffany-only jobs point to `/history session <id>` or `/jobs show <id>`.
  - This keeps the existing action row but makes the primary recovery/handoff action visible without scanning the whole card.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M4 queue affordances or return to the updated Claude priority: M3 restart/handoff checks with real native runtimes.

## M4 queue show explains execution order and merge mode — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_queue_show --lib --quiet` — green, 2 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_queue --lib --quiet` — green, 8 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Native TUI `/queue show` now adds `order`, `mode`, and `auto` lines.
  - `order` makes retry-first-vs-normal queue execution explicit.
  - `mode` tells the user whether queued prompts will merge into one numbered follow-up or run unchanged.
  - `auto` tells the user whether the queue is paused, armed behind the current orchestration, ready to run now, or idle.
  - This directly addresses the previous confusion that queued messages might be executed separately or silently merged without explanation.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M4 by improving queue state visibility while a run is active, or switch to the M3 real-runtime restart/handoff gate.

## M3 real-runtime restart continuity harness strengthened — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `bash -n scripts/tiffany-real-runtime-check` — green.
  - `./scripts/tiffany-real-runtime-check --self-test` — green.
  - `./scripts/tiffany-real-runtime-check --adapter-run --runtime all` — green; Claude Code, Codex, and Gemini all passed.
- Decisions:
  - The real-runtime adapter harness now prints explicit restart-continuity evidence when the same role reuses the same Tiffany worker thread and native session across separate orchestrator processes.
  - The adapter-run jobs check now asserts the persisted job output contains the reused native session id.
  - The thread-export check now asserts the export command output names the same Tiffany worker thread that was captured after the second run.
  - This makes the M3 gate stronger: the harness verifies runtime continuity, persisted jobs, thread export, native-history import/query, and `/continue open <role>` handoff actions for Claude Code, Codex, and Gemini.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker. Full interactive PTY smoke for `/continue open <role>` remains future coverage, but the dry-run seam and real adapter harness now cover the non-interactive proof points.
- Next:
  - Continue M3 by adding an optional interactive PTY smoke, or move to M4 active-run queue visibility and recovery affordances.

## M4 active-run queue preview is regression-covered — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_orchestrator_shell_uses_batch_queue_preview --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui render_tiffany_batch_messages --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_queue --lib --quiet` — green, 8 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Confirmed the native Tiffany shell already switches the bottom pending-input preview into queued-batch mode.
  - Added focused regression coverage so entering Tiffany orchestrator mode keeps queued follow-ups displayed as a batch, and leaving the mode restores the normal preview semantics.
  - This protects the M4 invariant that prompts entered during a run remain visibly queued at the bottom until the batch executes, without adding duplicate history noise.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - No blocker.
- Next:
  - Continue M4 by improving active queue pause/resume affordances, or move to the M5 open-source release docs once the v0.2 tag decision is made.

## M4 queue help card and v0.2 CI failure check — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_queue --lib --quiet` — green, 9 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui startup_health_actions --lib --quiet` — green, 3 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_orchestrator --lib --quiet` — green, 247 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Read Claude's updated active instruction: hold external `v0.2` tag movement until the user chooses force-move `v0.2` versus cut `v0.2.1`.
  - Added native `/queue help` output so the TUI itself explains numbered batch merging, retry-first ordering, bottom-preview retention, and the main `/queue` actions.
  - Confirmed the CI-failure test area now uses temp launchable runtimes: `startup_health_actions` and the full `tiffany_orchestrator --lib` filtered suite pass locally.
  - Did not run the full release preflight and did not push or move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - If the user picks a tag path, run `./scripts/tiffany-release-preflight --full --tag <chosen-tag>` before any push/tag action.
  - Otherwise continue M4 active queue pause/resume affordances or M2 waterfall de-duplication.

## M4 queue pause/resume state stays visible — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui tiffany_queue --lib --quiet` — green, 11 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui render_tiffany_paused_batch_messages --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui pending_input_preview --lib --quiet` — green, 15 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/queue pause` now renders the same structured queue status card as `/queue show`, instead of a one-off info line.
  - `/queue show` adds a `hold` line while paused so the user sees that queued prompts remain in the bottom preview until `/queue resume` or `/queue run`.
  - The bottom pending-input preview now carries an explicit paused state: queued Tiffany batches render as `Queued batch paused: ...`, and retry-first messages no longer imply they will auto-submit at end of turn while autosend is paused.
  - `/queue resume` and `/queue run` clear the bottom paused marker immediately; if the current orchestration is still running, the queue card shows the batch is armed rather than submitting early.
  - Added regressions proving paused queued prompts do not submit when the active orchestration finishes, and resume arms then drains the merged numbered batch.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Continue M4 with failed/retry job recovery visibility, or switch to M2 worker waterfall de-duplication and JSON cleanup.

## M2 worker waterfall filters pip notice noise — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format normalizes_claude_tool_status_wrappers --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall_normalizes_claude_tool_status_wrappers --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall --lib --quiet` — green, 8 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui output --lib --quiet` — green, 78 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Normal-mode worker output now drops low-value pip upgrade notices (`[notice] A new release of pip is available` / `pip install --upgrade pip`) after stripping runtime/tool-result wrappers.
  - Python PEP 668 / externally-managed-environment output now renders as one actionable line: create a virtual environment and install dependencies there.
  - Added formatter-level and native TUI waterfall regressions so raw `externally-managed-environment`, PEP links, and pip notice lines do not leak into normal process output.
  - Kept this in `tiffany-event-format` so renderer behavior stays shared and raw/debug traces can still preserve original event content.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Continue M2 with the next noisy wrapper class in worker waterfall output, especially duplicate `tool result`/`user` wrappers and long tool-result grouping.

## M2 worker waterfall strips repeated tool-result wrappers — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format visible_agent_output --quiet` — green, 2 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_output_body_uses_native_kind_markers --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall --lib --quiet` — green, 8 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui output --lib --quiet` — green, 78 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Normal-mode worker output now strips secondary wrappers per line, including repeated `claude-code user: tool result:`, `tool result:`, `tool_output:`, `assistant:`, `result:`, and `final:` prefixes.
  - Multi-line tool results now keep the useful content while removing repeated transport labels from subsequent lines.
  - The native TUI waterfall regression asserts rendered worker lines no longer expose `claude-code user:` or `tool result:` wrapper text in normal mode.
  - Raw/debug trace behavior is unchanged so `/raw` and trace views can still preserve original event payloads.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Continue M2 with long tool-result grouping, or switch to M4 failed/retry job recovery visibility.

## M2 worker model errors render as actionable text — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml --all` — green; stable rustfmt emitted the existing `imports_granularity` warning.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml -p codex-tui` — green; same existing warning.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format keeps_actionable_stderr_visible --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_error_output_adds_actionable_fix_line --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format --quiet` — green, 45 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui output --lib --quiet` — green, 78 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Normal-mode worker stderr now turns provider model failures such as `API Error: 400 [1211][模型不存在]` into `model not found: 模型不存在 (1211)`.
  - The worker waterfall keeps the actionable fix line (`fix model not found` plus `/role` and `/doctor` guidance) while hiding raw `API Error` noise from the main body.
  - This directly covers the previously reported `[1211][模型不存在]` failure class without changing raw/debug traces.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Continue M2 with remaining worker waterfall grouping/noise, or move to M4 failed/retry job recovery visibility.

## M4 failed jobs reuse humanized model errors — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml --all` — green; stable rustfmt emitted the existing `imports_granularity` warning.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml -p tiffany-bridge -p codex-tui` — green; same existing warning.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format keeps_actionable_stderr_visible --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge parses_jobs_and_actions_without_raw_cli_next_actions --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_summary_lines_render_queue_cards_without_raw_stdout --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-bridge jobs --quiet` — green, 2 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_summary_lines --lib --quiet` — green, 4 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_retry --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added a shared `humanize_agent_status_text` helper so stored job errors can reuse the same normal-mode status cleanup as worker waterfall output.
  - `/jobs` failed-job cards now render model errors such as `{"message":"[1211][模型不存在] invalid model"}` as `model not found: 模型不存在 (1211)` instead of exposing raw provider payload text.
  - The existing failed-job repair line remains intact: `check model binding with /role <role>, then run /doctor`.
  - Result text is intentionally still parsed with the generic JSON humanizer, not the status-error humanizer.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Continue M4 with failed/retry job recovery visibility, especially retry prompt queue placement, or return to M2 remaining waterfall grouping/noise.

## M4 jobs retry handoff names bottom queue visibility — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path tiffany-ui/codex-rs/Cargo.toml -p codex-tui` — green; stable rustfmt emitted the existing `imports_granularity` warning.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_retry_handoff_renders_current_tui_queue_card --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_retry --lib --quiet` — green, 2 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_summary_lines --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/jobs retry` success cards now explicitly say the restored prompt stays in the bottom queue until it runs.
  - This makes the handoff match the M4 invariant: queued prompts stay visible instead of seeming to disappear into the job subsystem.
  - The existing `/queue run`, `/queue show`, and `/jobs show <id>` actions remain unchanged.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Continue M4 by reducing duplicate `/jobs retry` acknowledgement noise, or return to M2 remaining waterfall grouping/noise.

## M4 jobs retry hides the already-queued failed job card — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml -p codex-tui` — green; stable rustfmt emitted the existing `imports_granularity` warning.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_retry_handoff_renders_current_tui_queue_card --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_retry --lib --quiet` — green, 2 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_summary_lines --lib --quiet` — green, 4 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `/jobs retry <id>` success output now keeps the retry handoff card and filters the original failed job `<id>` out of the appended jobs summary.
  - Other persisted jobs still render below the handoff card, with recalculated active/shown counts.
  - This removes the confusing duplicate state where Tiffany said the retry was queued while also showing the same failed job with another `/jobs retry <id>` next action.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Continue M4 queue/job polish or return to M2 remaining waterfall grouping/noise.

## v0.2 release workflow uses the full preflight gate — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml"); puts "release.yml yaml ok"'` — green.
  - `bash -n scripts/tiffany-release-preflight scripts/tiffany-check scripts/tiffany-slim-smoke scripts/tiffany-real-runtime-check` — green.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui startup_health_actions --lib --quiet` — green, 3 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Read Claude's updated v0.2 CI-failure instruction and kept the external tag hold in place.
  - Changed `.github/workflows/release.yml` so a pushed release tag runs `./scripts/tiffany-release-preflight --full --tag "${GITHUB_REF_NAME}"`, not the weaker quick gate.
  - Updated `docs/homebrew.md` to match the new release workflow and local release rule.
  - Rechecked the `startup_health_actions` area; the tests now use temporary launchable runtime binaries, so CI/PATH state should not add the old ambient runtime warning.
  - Did not run a full release preflight in this slice, and did not push or move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - If a release path is chosen, run `./scripts/tiffany-release-preflight --full --tag <chosen-tag>` on the exact commit before any push/tag action.

## M0 e2e scripts accept tiffany-loop-only binary dirs — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `bash -n scripts/lib/tiffany-binaries.sh scripts/tiffany-e2e-fake-runtime scripts/tiffany-e2e-multi-runtime scripts/tiffany-check-script-helpers` — green.
  - `./scripts/tiffany-check-script-helpers` — green.
  - `./scripts/tiffany-e2e-fake-runtime` — green.
  - `./scripts/tiffany-e2e-multi-runtime` — green.
  - `tmp_bin=$(mktemp -d); ln -s target/dev-small/tiffany-loop "$tmp_bin/tiffany-loop"; ./scripts/tiffany-e2e-fake-runtime --bin-dir "$tmp_bin"` — green; proved the script works with no `orchestrator` file in the supplied bin dir.
  - `tmp_bin=$(mktemp -d); ln -s target/dev-small/tiffany-loop "$tmp_bin/tiffany-loop"; ./scripts/tiffany-e2e-multi-runtime --bin-dir "$tmp_bin"` — green; proved the multi-runtime script works with no `orchestrator` file in the supplied bin dir.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Added `scripts/lib/tiffany-binaries.sh` with a small runtime alias resolver.
  - `tiffany-e2e-fake-runtime` and `tiffany-e2e-multi-runtime` now prefer an existing `orchestrator` alias but can create a temporary `orchestrator` argv[0] alias from `tiffany-loop` when the bin dir only contains the primary binary.
  - This removes another release-script assumption that `orchestrator` must be a standalone file already present in the bin dir, while preserving the argv[0] dispatch contract.
  - Added helper-script coverage for the tiffany-loop-only alias path.
  - Did not run a full release preflight in this slice, and did not push or move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` or cut `v0.2.1`.
- Next:
  - Remaining release proof is the expensive gate: `./scripts/tiffany-release-preflight --full --tag <chosen-tag>` on the exact release commit.

## M0 tagged preflight blocks stale existing tags — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `bash -n scripts/tiffany-release-preflight` — green.
  - `TIFFANY_RELEASE_ALLOW_FREQUENT=1 ./scripts/tiffany-release-preflight --quick --tag v0.2` — expected failure before expensive checks: existing `v0.2` points at `1211d9b...` while HEAD is `d3cd973...`; output says the tag does not point at HEAD.
  - `TIFFANY_RELEASE_ALLOW_FREQUENT=1 TIFFANY_RELEASE_ALLOW_TAG_REWRITE=1 ./scripts/tiffany-release-preflight --quick --tag v0.2` — rerun exposed stale tests, then after fixes passed all steps up to the full serial `codex-tui --lib` gate; the first failure there was the expected queue-preview snapshot drift.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop shared_visible_output_classifier_keeps_role_errors_only --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop status_view_updates_stage_and_assistant_text --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format keeps_actionable_stderr_visible --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui jobs_parser_keeps_result_and_error_fields_separate --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui shows_actionable_stderr_instead_of_hiding_model_errors --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui chatwidget_tall --lib --quiet` — green, 1 passed.
  - `./scripts/tiffany-codex-tui-lib-test` — green, 3162 passed, 1 ignored.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - `scripts/tiffany-release-preflight --tag <tag>` now fails fast when the tag already exists but does not point at HEAD.
  - This prevents a false local green where HEAD passes checks but GitHub Actions would still run the old tagged commit.
  - Added explicit `TIFFANY_RELEASE_ALLOW_TAG_REWRITE=1` override wording for the rare force-move path; this should only be used after the user explicitly chooses to force-move an existing tag.
  - Updated README, README.zh-CN, and Homebrew docs with the existing-tag rewrite guard.
  - Fixed stale root/TUI tests that were still expecting pre-humanization parser/model-error text.
  - Improved shared model-error formatting so bare `[1211] 模型不存在` renders as `model not found: 模型不存在 (1211)`, while already-human text like `model not found with several words` is not over-collapsed.
  - Accepted the `chatwidget_tall` snapshot update for the numbered queue preview (`Queued follow-up inputs: 30 item(s)` with indexed rows).
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` with explicit override, or cut `v0.2.1` after bumping versions/changelog.
- Next:
  - If the user chooses `v0.2.1`, bump versions and changelog, then run `./scripts/tiffany-release-preflight --full --tag v0.2.1`.
  - If the user chooses force-moving `v0.2`, run `TIFFANY_RELEASE_ALLOW_TAG_REWRITE=1 ./scripts/tiffany-release-preflight --full --tag v0.2` on the exact intended commit before touching the tag.

## M5 architecture docs synced to current product shape — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Rewrote `docs/architecture.md` to match the current product contract instead of the older 7-layer prototype description.
  - The architecture doc now names the one multi-call binary shape, current TUI/runtime/adapter layering, `tiffany-ui/codex-rs/tiffany-bridge/`, direct/single/full routing, visible-output contract, persistence, queue/jobs behavior, Tiffany-only slash commands, native runtime adapter contract, and release gates.
  - Corrected the bridge-crate path in `docs/engineering-plan.md`; the source-of-truth now says pure Tiffany helpers live under `tiffany-ui/codex-rs/tiffany-bridge/` inside the Codex fork workspace, with no dependency back on `codex-tui`.
  - This advances M5 documentation quality without changing runtime behavior.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` with explicit override, or cut `v0.2.1` after bumping versions/changelog.
- Next:
  - Continue M2 worker waterfall de-duplication/JSON cleanup, or continue M1 provider/role setup polish.

## M2 worker waterfall filters npm upgrade notice noise — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format normalizes_claude_tool_status_wrappers --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall_normalizes_claude_tool_status_wrappers --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Extended the shared worker visible-output cleanup to drop low-value npm upgrade notices, matching the existing pip upgrade notice filtering.
  - Normal worker waterfall output now hides `npm notice New major version of npm available`, changelog, update command, and standalone `npm notice` lines when they are only package-manager update noise.
  - Kept the rule in `tiffany-event-format` so raw/debug traces can still preserve the original event payload, while normal TUI views avoid another common package-manager wrapper.
  - Added formatter-level and native TUI waterfall regressions.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` with explicit override, or cut `v0.2.1` after bumping versions/changelog.
- Next:
  - Continue M2 with remaining worker waterfall grouping/noise, or move to M1 provider/role setup polish.

## M2 worker waterfall normalizes Claude file operation wrappers — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml --all` — green.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml -p codex-tui` — green; stable rustfmt still prints the existing `imports_granularity = Item` warnings.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format normalizes_claude_tool_status_wrappers --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall_normalizes_claude_tool_status_wrappers --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Extended shared worker visible-output cleanup for Claude Code file-operation wrappers beyond the previous `File created successfully at:` and `The file ... has been updated successfully` cases.
  - Normal waterfall output now renders `File updated successfully at: ...`, `File deleted successfully at: ...`, `The file ... has been deleted successfully`, and related modified/removed aliases as compact `file updated: ...` or `file deleted: ...` lines.
  - This keeps file-operation events in the `file update` display bucket, so the native waterfall title, output scope, and history filters stay consistent instead of showing a raw `tool result` wrapper.
  - Added formatter-level and native TUI waterfall regressions.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` with explicit override, or cut `v0.2.1` after bumping versions/changelog.
- Next:
  - Continue M2 with remaining worker waterfall grouping/noise, or move to M1 provider/role setup polish.

## M2 worker waterfall hides successful Codex exit-code noise — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml --all` — green.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml -p codex-tui` — green; stable rustfmt still prints the existing `imports_granularity = Item` warnings.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-event-format unwraps_codex_rollout_event_msg_fields_without_raw_json --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall_humanizes_native_codex_rollout_events --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Changed Codex rollout `exec_command_end` summaries so successful command results no longer add `exit 0` to normal worker waterfall output.
  - Successful shell results with stdout/stderr now show only the useful output body; successful shell results with no stdout/stderr collapse to low-value `tool result` and are hidden from normal view.
  - Failed shell results still keep the exit code and stderr/stdout body, so actionable failures remain visible.
  - Added formatter-level and native TUI rollout regressions for success-with-output, success-without-output, and failure cases.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` with explicit override, or cut `v0.2.1` after bumping versions/changelog.
- Next:
  - Continue M2 with remaining worker waterfall grouping/noise, or move to M1 provider/role setup polish.

## M2 worker waterfall marks failed tool results clearly — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml -p codex-tui` — green; stable rustfmt still prints the existing `imports_granularity = Item` warnings.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml --all` — green.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall_marks_failed_tool_results_without_success_checkmark --lib --quiet` — green, 1 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui worker_waterfall_humanizes_native_codex_rollout_events --lib --quiet` — green, 1 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Native worker waterfall now detects failed tool results from explicit `tool error` output or non-zero `exit N` content.
  - Failed tool-result cells and grouped tool call/result cells render with a red `✗` title marker instead of the previous gray success checkmark.
  - The first failed result body line also uses `✗ exit N`; successful tool results still keep the existing gray `✓`.
  - This makes Codex/Claude/Gemini tool failures visually distinct without changing raw trace preservation.
  - Added native TUI regressions for grouped failed tool results and Codex rollout failed `exec_command_end` output.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` with explicit override, or cut `v0.2.1` after bumping versions/changelog.
- Next:
  - Continue M2 with remaining worker waterfall grouping/noise, or move to M1 provider/role setup polish.

## M1 role cards flag unbound providers before native handoff actions — 2026-06-29

- Commits:
  - none yet
- Build/tests:
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml -p codex-tui` — green; stable rustfmt still prints the existing `imports_granularity = Item` warnings.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 fmt --manifest-path Cargo.toml --all` — green.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p tiffany-loop provider --quiet` — green, 45 passed plus filtered tail.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui provider_args --lib --quiet` — green, 6 passed.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui role_summary_lines --lib --quiet` — green, 2 passed.
  - `git diff --check` — green.
- Decisions:
  - Native `/role` and `/roles` cards now derive health from missing provider/model bindings when the runtime output does not provide an explicit health failure.
  - Worker roles with an unbound provider are shown as `provider unbound` instead of looking ready.
  - For unbound worker roles, actions now lead to `/role <name>`, `/provider`, and `/doctor` instead of showing `/continue open <role>` or `/history role <role>` before a native worker session can actually exist.
  - Worker roles that already have provider/API model bindings still keep `/thread`, `/continue open`, and `/history role` actions, even when they have a runtime warning and need `/doctor`.
  - Added native TUI role-summary regressions for the unbound worker role path.
  - Did not push and did not move the `v0.2` tag.
- Blockers / questions for Claude:
  - External release remains blocked on the user's tag decision: force-move `v0.2` with explicit override, or cut `v0.2.1` after bumping versions/changelog.
- Next:
  - Continue M1 provider/role setup polish, especially `/doctor` one-line fixes, or return to M2 if more real-output noise appears.

## v0.2.1 release prep and full preflight — 2026-06-29

- Commits:
  - `115e4cb` Harden Tiffany release preflight checks.
  - `0611a9d` Polish Tiffany worker output displays.
  - `aa57acf` Clarify Tiffany queued input flow.
  - `c7d0754` Refresh Tiffany release planning docs.
  - Current HEAD: Prepare v0.2.1 release.
- Build/tests:
  - `/Users/allendred/.cargo/bin/cargo +1.95.0 build` after each local grouping commit — green.
  - Initial `./scripts/tiffany-release-preflight --full --tag v0.2.1` stopped at the expected 24-hour release cadence guard.
  - `TIFFANY_RELEASE_ALLOW_FREQUENT=1 ./scripts/tiffany-release-preflight --full --tag v0.2.1` first exposed version-only TUI snapshot drift from `v0.2.0` to `v0.2.1`; the generated snapshots were accepted and amended into the release commit.
  - From `tiffany-ui/codex-rs`: `/Users/allendred/.cargo/bin/cargo +1.95.0 test -p codex-tui status_snapshot_includes_enterprise_monthly_credit_limit --lib --quiet` — green, 1 passed.
  - Final `TIFFANY_RELEASE_ALLOW_FREQUENT=1 ./scripts/tiffany-release-preflight --full --tag v0.2.1` — green.
- Decisions:
  - Cut the fix release as `v0.2.1`; the existing broken `v0.2` tag remains untouched.
  - Used `TIFFANY_RELEASE_ALLOW_FREQUENT=1` only to bypass the same-day release cadence guard for this urgent release fix.
  - Bumped root `tiffany-loop`, `tiffany-cli`, and `codex-tui` to `0.2.1`, refreshed both lockfiles, updated CHANGELOG, and accepted version-only snapshots.
  - Did not push `main`, did not create `v0.2.1`, and did not move `v0.2`.
- Blockers / questions for Claude:
  - None for local release readiness after full preflight; remote release still requires the explicit push/tag step.
- Next:
  - If the user approves publishing, push `main`, then create and push `v0.2.1`.
