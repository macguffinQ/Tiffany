//! Terminal chat interface for orchestration.

pub(super) mod agent_handoff;
pub(super) mod codex_bottom_pane;
pub(super) mod codex_chat_view;
pub(super) mod codex_command_popup;
pub(super) mod codex_composer;
pub(super) mod codex_event_stream;
pub(super) mod codex_frame_rate_limiter;
pub(super) mod codex_frame_requester;
pub(super) mod codex_history_cell;
pub(super) mod codex_inline_surface;
pub(super) mod codex_insert_history;
pub(super) mod codex_keymap;
pub(super) mod codex_process_cell;
pub(super) mod codex_terminal;
pub(super) mod commands;
pub(super) mod context;
pub(super) mod persist;
pub(super) mod process;
pub(super) mod queue_state;
pub(super) mod run_progress_view;
pub(super) mod run_state;
pub(super) mod state;
pub mod terminal;
pub(super) mod util;
