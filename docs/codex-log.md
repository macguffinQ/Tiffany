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
