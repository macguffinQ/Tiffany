# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Changed

- Replaced upstream Codex placeholder docs in `tiffany-ui/docs` with
  Tiffany-specific setup, config, security, slash-command, and contribution
  guidance.
- Repointed user-visible fork runtime help links for sandbox, config, MCP,
  memories, and Windows sandbox prompts to Tiffany documentation.
- Routed conversational requests directly to worker execution, skipping
  planner, critic, and reviewer noise while keeping full orchestration for
  engineering tasks.
- Marked direct-answer runs with a dedicated progress event in terminal, TUI,
  ACP, and persisted orchestration logs instead of showing them as DAG
  execution.
- Updated the forked `tiffany-loop` run header to label direct-answer runs as
  `flow direct` and show `direct -> worker -> answer`, while implementation
  work still shows the full planner/critic/worker/reviewer flow.
- Moved direct-answer request classification into the shared event-format crate
  so the backend orchestrator and forked TUI use the same direct/full flow
  decision.
- Classified multi-turn contextual prompts by the `Current user request` block,
  preventing older turns containing words like optimize, fix, or commit from
  forcing a simple follow-up question into the full orchestration pipeline.
- Made current-request extraction tolerate CRLF input, inline
  `Current user request: ...` markers, and indented markers for external ACP
  and terminal clients.
- Treated pure URL/link-inspection prompts as direct worker answers while
  keeping creation, code, and implementation requests on the full orchestration
  pipeline.
- Added a middle "single worker" route for atomic execution requests such as
  external scaffolding or standalone diagnostics, avoiding planner/critic
  churn and PR-style reviewer failures when no decomposition is useful.
- Kept multi-turn continuation commands such as "continue" or "继续做" on the
  full pipeline when prior context is clearly about the current Tiffany
  project, TUI, roles, providers, or release work.
- Surfaced non-zero planner/critic/reviewer CLI exits with stderr context so
  runtime, model, and permission failures are explicit in the TUI.
- Suppressed successful planner/critic/reviewer stderr chatter from the live
  workflow stream while still surfacing stderr when a control CLI actually
  fails.
- Made planner and critic subprocess failures non-fatal: Tiffany now shows a
  visible warning, falls back to the original/current plan, and still runs the
  worker instead of discarding useful worker output.
- Promoted planner/critic fallback paths to structured progress events so
  terminal chat, `/process`, ACP clients, and persisted logs render them as
  workflow warnings instead of noisy role output.

## [0.1.25] - 2026-06-22

### Added

- Added `doctor` diagnostics for stale or divergent Homebrew tap checkouts,
  including the tap checkout commit, upstream, ahead/behind state, dirty state,
  and repair commands when local tap history blocks package updates.
- Added post-release check diagnostics that print the same tap repair commands
  when the local Homebrew formula does not reference the expected release URLs.

## [0.1.24] - 2026-06-22

### Fixed

- Kept completed worker output visible when reviewer JSON parsing or reviewer
  subprocess calls fail; these now render as review warnings instead of
  aborting the whole orchestrator run.
- Added `doctor` checks for both PATH-visible command versions and the actual
  launch pair versions used by `orchestrator tui`, catching stale adjacent
  `tiffany-loop` binaries before users hit confusing runtime failures.

## [0.1.23] - 2026-06-22

### Fixed

- Synchronized the packaged `tiffany-loop` UI crate versions with the root
  release version so `tiffany-loop --version`, `tiffany --version`, and
  `orchestrator --version` agree after Homebrew installs.
- Added release checks that fail when root and UI crate versions drift.

## [0.1.22] - 2026-06-22

### Added

- Added `orchestrator doctor` and `/doctor` diagnostics for Codex CLI
  `exec --cd` compatibility so outdated or miswired Codex runtimes are caught
  before planner, critic, reviewer, or `worker-codex` runs fail.

## [0.1.21] - 2026-06-22

### Fixed

- Updated Codex CLI subprocess invocations for planner, critic, reviewer, and
  `worker-codex` to use the current `--cd` workdir flag instead of the removed
  `--cwd` flag, fixing empty planner/reviewer JSON failures with newer Codex
  CLI versions.
- Added Homebrew tap helper assertions that the generated formula installs and
  tests all shipped binaries: `orchestrator`, `tiffany-loop`, and `tiffany`.

## [0.1.20] - 2026-06-22

### Added

- Added Tiffany TUI worker-output type labels (`final`, `tool call`,
  `tool result`, `stderr`, `alert`) so live runs show what the worker is doing
  without exposing raw adapter JSON.
- Added inline TUI repair hints for worker model/auth/permission/runtime/network
  failures, including `[1211]` / `模型不存在` model mapping errors.
- Added Homebrew install visibility diagnostics to `orchestrator doctor`,
  checking the installed package prefix and whether `tiffany-loop` /
  `orchestrator` are visible on `PATH`.

### Fixed

- Made top-level `tiffany-loop setup`, `tiffany-loop doctor`,
  `tiffany-loop config ...`, and other passthrough commands validate the
  `orchestrator` runtime before spawning, producing the same actionable
  missing-runtime message as TUI launches.

## [0.1.19] - 2026-06-21

### Added

- Documented the conversational direct-answer path, including the structured
  `review skipped` JSON event `reason` used by UI adapters and scripts.
- Added release cadence guardrails so small fixes batch under `Unreleased`,
  with an explicit override only for urgent installer, startup, or security
  fixes.

### Fixed

- Made `orchestrator doctor` focus required failures on the active
  role/model/provider chain. Optional providers or example models that are not
  used by current roles now stay as warnings instead of blocking a healthy
  default setup.
- Fixed role diagnostics so a role bound to a model whose provider is missing
  is reported as a required wiring failure instead of an OK role row.
- Injected worker output into reviewer prompts so review decisions can inspect
  the actual response instead of reporting missing worker content.
- Validated the orchestrator runtime before launching the Tiffany TUI, making
  missing local binaries fail with actionable diagnostics.
- Rendered `review skipped` structured events in the Tiffany TUI for
  conversational turns instead of noisy review failure messages.
- Preserved full worker message streams in the Tiffany TUI while keeping
  planner, critic, and reviewer control output summarized.
- Showed fuller worker task prompts and longer failure diagnostics in the
  Tiffany TUI so process details are not hidden behind one-line truncation.

## [0.1.18] - 2026-06-21

### Fixed

- Aligned the vendored Tiffany TUI crate version with the Tiffany release
  version and accepted Tiffany-style `vX.Y.Z` GitHub release tags in the update
  checker.

## [0.1.17] - 2026-06-21

### Fixed

- Repointed Tiffany Loop TUI update checks, release notes, and update commands
  to Tiffany releases and `macguffinQ/tap/tiffany-loop`, removing the remaining
  upstream Codex update/install prompts from the packaged UI.

## [0.1.14] - 2026-06-20

### Fixed

- Handled blank `--bin` and blank `TIFFANY_ORCHESTRATOR_BIN` values so the
  tiffany-loop orchestrator TUI no longer tries to start an empty executable
  path.
- Added a defensive TUI bridge fallback that reports `orchestrator` instead of
  `""` when launch state contains a blank binary path.

### Added

- Added CI and release-preflight coverage for the Homebrew tap formula update
  helper.

## [0.1.13] - 2026-06-20

### Fixed

- Suppressed upstream OpenAI Codex update prompts in `tiffany-loop`
  orchestrator mode.
- Resolved the sibling `orchestrator` binary before falling back to `PATH`, so
  the TUI event bridge works from Homebrew and archived installs even when the
  shell `PATH` is minimal.
- Made release Homebrew tap updates fail loudly when `HOMEBREW_TAP_TOKEN` is
  missing, preventing GitHub Releases and the tap formula from drifting.

### Added

- Added a zero-dependency Python Fibonacci example for validating setup,
  orchestration, worker edits, and test execution from a fresh checkout.
- Added an example smoke-check script and CI coverage for checked-in examples.
- Added a consolidated release preflight script for local quick and full checks.
- Added a `tiffany-clean-targets --trim` mode for pruning rebuildable caches
  while keeping compiled deps and final binaries.
- Added release workflow preflight gating before publishing tagged builds.
- Added release tag checks so tag versions must match the root Cargo package version.
- Added actionable `orchestrator doctor` next steps for first-run setup and repair.
- Added concise `orchestrator status` next/check actions for first-run launch guidance.
- Added provider/model/role/default-worker summaries to `orchestrator status`.
- Added a lightweight internal config health line to `orchestrator status`.
- Made `orchestrator status` prefer setup/doctor repair guidance before launching
  the TUI when provider, model, role, or runtime wiring is unhealthy.
- Added native `/doctor` support in the tiffany-loop orchestrator TUI, routing
  diagnostics through the orchestrator bridge with issue-aware status styling.
- Added concise repair hints to the TUI `/doctor` output so provider, role,
  runtime, and ready-state diagnostics point at the next command to run.
- Improved TUI `/provider` and `/role` command output with status rows and
  next-step hints instead of raw CLI tables.
- Reworked orchestrator progress display into ordered waterfall step lines with
  route/task metadata for worker activity.
- Added `tiffany-loop` as the primary installed UI command while keeping
  `tiffany` as a compatibility alias.

## [0.1.12] - 2026-06-20

### Fixed

- Fixed the macOS Apple Silicon release archive and Homebrew install surface so
  the primary `tiffany-loop` command is included alongside `orchestrator` and
  the `tiffany` compatibility alias.
- Added release packaging checks and archive smoke coverage so future release
  assets fail before publishing if any installed command is missing.

## [0.1.11] - 2026-06-19

### Fixed

- Avoid initializing Tiffany/Codex TUI helper aliases for `tiffany --help`,
  `tiffany --version`, and `tiffany orchestrator --help`, keeping help output
  clean in Homebrew, CI, and temporary-home environments.

### Changed

- Clarified the TUI alignment document so future maintainers treat
  `tiffany-ui/codex-rs/tiffany-cli` as the published UI entrypoint instead of
  the full upstream-style `codex-rs/cli`.

## [0.1.10] - 2026-06-19

### Changed

- Build the published `tiffany` binary from a new lightweight `tiffany-cli`
  crate instead of the full Codex CLI entrypoint.
- Keep the top-level `tiffany --help` surface focused on Tiffany orchestration
  rather than exposing Codex login, model, sandbox, and profile flags.
- Route `tiffany`, `tiffany "prompt"`, and `tiffany orchestrator ...` through
  the orchestrator-native TUI by default.
- Updated development, release, Homebrew fallback, and source-install commands
  to build `tiffany-ui/codex-rs/tiffany-cli`.

### Fixed

- Fixed `./scripts/tiffany-dev -- <raw args>` so the raw-argument separator is
  stripped before invoking the local Tiffany binary.

## [0.1.9] - 2026-06-19

### Added

- Expanded `./scripts/tiffany-clean-targets` with `--top`, `--top-deep`, and
  `--incremental` diagnostics/cleanup modes for managing large local target
  directories.

### Changed

- Redirected direct Cargo commands inside the `tiffany-ui/codex-rs` fork to the
  shared root `./target` directory, so local development no longer creates a
  second large fork-local target cache.
- Reduced debug/test artifact growth with limited debug info and disabled Cargo
  incremental builds in CI and Tiffany Integration.

## [0.1.8] - 2026-06-19

### Changed

- Made the heavy Codex code-mode/V8 runtime optional and disabled it in the
  default `tiffany` build; set `TIFFANY_CODE_MODE_RUNTIME=1` to include it.
- Stripped final distributable binaries after build instead of stripping
  intermediate proc-macro artifacts.
- Improved release cache diagnostics and target cleanup commands.
- Updated the published release and Homebrew tap to `v0.1.8`.

## [0.1.7] - 2026-06-19

### Added

- Added a fast Tiffany smoke check for local and CI verification.
- Split Tiffany Integration into its own workflow.
- Added richer doctor diagnostics for local install, toolchain, Homebrew, and
  worker CLI discovery.

### Changed

- Improved release workflow reliability and build timing diagnostics.

## [0.1.6] - 2026-06-19

### Added

- Added the full `tiffany` UI fork as the primary terminal experience for orchestrator runs.
- Added native `tiffany orchestrator` streaming for planner, critic, worker, reviewer, and final result events.
- Added `/provider`, `/role`, and `/roles` flows for configuring providers and role routing from the TUI.
- Added top-level `orchestrator setup` and `./scripts/tiffany-dev setup` for first-run provider/model/role setup.
- Added install diagnostics to `orchestrator status` and `orchestrator doctor`, including binary discovery, config roots, and the exact bridge command.

### Changed

- Renamed the public product to `tiffany-loop`.
- Updated release archive and Homebrew formula naming to `tiffany-loop`.
- Added preview-stage and non-affiliation language for open-source publication.
- Clarified that `orchestrator` and `tiffany` remain installed command names for compatibility.
- Updated TUI visible branding, docs, and snapshots to use `tiffany-loop`.
- Made `tiffany orchestrator` and `orchestrator tui` use the tiffany-loop UI path by default when installed.
- Switched release and developer builds to shared Cargo target directories to reduce duplicate build artifacts.
- Reduced `orchestrator` binary build warning noise by removing duplicate module compilation and stale direct-provider code.

### Fixed

- Humanized planner, critic, worker, and reviewer JSON-like output before showing it in terminal history.
- Reduced duplicate worker/process messages in the TUI event stream.
- Kept final results as plain selectable text instead of framed output.
- Improved queue and multi-turn follow-up handling while an orchestrator run is active.

## [0.1.5] - 2026-06-15

### Changed

- Hardened Homebrew release checksum generation by using authenticated GitHub APIs.
- Updated Homebrew tap documentation and formula template with published checksums.

## [0.1.4] - 2026-06-15

### Added

- Homebrew tap automation for `macguffinQ/homebrew-tap`.

### Changed

- Release packaging publishes the macOS Apple Silicon archive used by Homebrew.
  Intel macOS, Linux, and Windows use the source build path until additional
  prebuilt target archives are added.

## [0.1.3] - 2026-06-15

Initial open-source release.

### Added

- Claude Code-style terminal chat with native scrollback, selection, copy, paste, and IME-friendly input.
- Multi-agent orchestration pipeline: planner, critic, router, workers, reviewer, and A/B judge.
- Claude Code and Codex CLI subprocess runtimes.
- Agent Client Protocol stdio server.
- Process capture with readable summaries and expandable detail.
- Queued follow-up messages while a run is active.
- Multi-turn context memory with compact, full, off, and clear modes.
- Final result replay as plain selectable text.
- Handoff packages for continuing work in Claude Code or Codex.
- Conversation flow graph summaries.
- GitHub issue templates, pull request template, CI, and open-source documentation.
