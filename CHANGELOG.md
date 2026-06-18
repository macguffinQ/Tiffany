# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

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

- Release packaging now publishes Linux, Windows, and macOS Apple Silicon binaries while Intel macOS uses the Homebrew source build path.

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
