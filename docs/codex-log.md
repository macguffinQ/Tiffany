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
