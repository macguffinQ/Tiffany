# Third-Party Notices

## OpenAI Codex CLI

This project includes terminal UI strategy and implementation code adapted from OpenAI Codex CLI's Rust TUI (`codex-rs/tui`), including:

- terminal scrollback for finalized history
- stable live input/status redraw behavior inspired by Codex's custom terminal and insert-history strategy
- inline terminal surface and resize cleanup inspired by Codex's custom terminal strategy
- asynchronous terminal event handling inspired by Codex's event stream
- composer editing/history/slash state inspired by Codex's chat composer
- key routing between composer-local edits and terminal-level actions inspired by Codex's BottomPane/ChatComposer split
- slash command popup windowing/rendering inspired by Codex's command popup
- transcript/history cell rendering inspired by Codex's history cells
- process/execution cell rendering inspired by Codex's exec cells
- resize-reflow behavior inspired by Codex's inline viewport strategy
- bracketed paste/raw-mode setup and restore discipline
- avoiding mouse capture so terminal selection and copy remain native

Full fork:

- `tiffany-ui/` is a working fork of OpenAI Codex CLI used as the primary
  tiffany-loop UI direction.
- The fork preserves upstream Apache-2.0 licensing and notice files.
- tiffany-loop-specific changes should stay small and adapter-oriented so the fork
  can be rebased from upstream OpenAI Codex.

Copied/adapted source files:

- `src/tui/codex_terminal.rs`, adapted from `codex-rs/tui/src/custom_terminal.rs`
- `src/tui/codex_insert_history.rs`, adapted from `codex-rs/tui/src/insert_history.rs`
- `src/tui/codex_inline_surface.rs`, adapted from the component boundary around Codex custom terminal and insert-history usage
- `src/tui/codex_frame_requester.rs`, adapted from `codex-rs/tui/src/tui/frame_requester.rs`
- `src/tui/codex_frame_rate_limiter.rs`, adapted from `codex-rs/tui/src/tui/frame_rate_limiter.rs`
- `src/tui/codex_event_stream.rs`, adapted from `codex-rs/tui/src/tui/event_stream.rs`
- `src/tui/codex_composer.rs`, adapted from the component boundary of `codex-rs/tui/src/bottom_pane/chat_composer*`
- `src/tui/codex_keymap.rs`, adapted from the input-routing boundary of `codex-rs/tui/src/bottom_pane/` and `codex-rs/tui/src/bottom_pane/chat_composer*`
- `src/tui/codex_command_popup.rs`, adapted from `codex-rs/tui/src/bottom_pane/command_popup.rs`
- `src/tui/codex_history_cell.rs`, adapted from the component boundary of `codex-rs/tui/src/history_cell/`
- `src/tui/codex_process_cell.rs`, adapted from the component boundary of `codex-rs/tui/src/exec_cell/`

Vendored upstream source:

- `third_party/openai-codex/codex-rs/tui/`
- `third_party/openai-codex/LICENSE`
- `third_party/openai-codex/NOTICE`
- `third_party/openai-codex/UPSTREAM.txt`

OpenAI Codex CLI is licensed under the Apache License, Version 2.0.

Source: https://github.com/openai/codex

This project is not affiliated with, sponsored by, or endorsed by OpenAI.

Project-owned code in this repository remains under this project's MIT license unless a file header or vendored upstream notice states otherwise. The `tiffany-ui/` fork and `third_party/openai-codex/` snapshot preserve upstream Apache-2.0 licensing and notice files.
