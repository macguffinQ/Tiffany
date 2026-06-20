# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

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
