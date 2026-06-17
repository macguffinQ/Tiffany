// Codex-style key routing for tiffany-loop lightweight terminal composer.
//
// Upstream Codex routes keys through BottomPane/ChatComposer first, then lets
// the parent widget decide process-level actions such as interrupt or quit.
// This module follows that boundary: it mutates only composer state and returns
// explicit high-level actions for the terminal runner to execute.

use super::codex_composer::{
    accept_visible_slash_completion, complete_selected_slash_completion, delete_char_at_cursor,
    delete_char_before_cursor, flatten_paste_text, has_visible_slash_completions,
    insert_char_at_cursor, insert_text_at_cursor, move_cursor_left, move_cursor_right,
    move_slash_completion_selection, navigate_history, reset_history_browse,
    reset_slash_completion, slash_completion_page_size,
};
use super::state::InputState;
use crate::config::Config;
use crate::core::session_store::SessionStore;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TerminalKeyAction {
    None,
    Submit(String),
    CancelOrExit,
    ExitRequested,
}

pub(super) fn handle_composer_key(
    key: KeyEvent,
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
) -> TerminalKeyAction {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return TerminalKeyAction::None;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c' | 'C') if ctrl => TerminalKeyAction::CancelOrExit,
        KeyCode::Char('d' | 'D') if ctrl && input.buffer.is_empty() => {
            TerminalKeyAction::ExitRequested
        }
        KeyCode::Char('u' | 'U') if ctrl => {
            input.buffer.clear();
            input.cursor = 0;
            reset_history_browse(input);
            reset_slash_completion(input);
            TerminalKeyAction::None
        }
        KeyCode::Char('p' | 'P') if ctrl => {
            move_slash_or_history(store, config, input, -1);
            TerminalKeyAction::None
        }
        KeyCode::Char('n' | 'N') if ctrl => {
            move_slash_or_history(store, config, input, 1);
            TerminalKeyAction::None
        }
        KeyCode::Enter => submit_or_accept_completion(store, config, input),
        KeyCode::Backspace => {
            delete_char_before_cursor(input);
            reset_after_edit(input);
            TerminalKeyAction::None
        }
        KeyCode::Delete => {
            delete_char_at_cursor(input);
            reset_after_edit(input);
            TerminalKeyAction::None
        }
        KeyCode::Left => {
            move_cursor_left(input);
            TerminalKeyAction::None
        }
        KeyCode::Right => {
            move_cursor_right(input);
            TerminalKeyAction::None
        }
        KeyCode::Home => {
            input.cursor = 0;
            TerminalKeyAction::None
        }
        KeyCode::End => {
            input.cursor = input.buffer.len();
            TerminalKeyAction::None
        }
        KeyCode::Up => {
            move_slash_or_history(store, config, input, -1);
            TerminalKeyAction::None
        }
        KeyCode::Down => {
            move_slash_or_history(store, config, input, 1);
            TerminalKeyAction::None
        }
        KeyCode::PageUp => {
            move_slash_completion_selection(store, config, input, -slash_completion_page_size());
            TerminalKeyAction::None
        }
        KeyCode::PageDown => {
            move_slash_completion_selection(store, config, input, slash_completion_page_size());
            TerminalKeyAction::None
        }
        KeyCode::Tab => {
            if complete_selected_slash_completion(store, config, input) {
                reset_history_browse(input);
                reset_slash_completion(input);
            }
            TerminalKeyAction::None
        }
        KeyCode::Esc => {
            if has_visible_slash_completions(store, config, input) {
                input.slash_completion_dismissed = true;
            }
            TerminalKeyAction::None
        }
        KeyCode::Char(c) if !ctrl => {
            insert_char_at_cursor(input, c);
            reset_after_edit(input);
            TerminalKeyAction::None
        }
        _ => TerminalKeyAction::None,
    }
}

pub(super) fn handle_paste_text(input: &mut InputState, text: &str) {
    insert_text_at_cursor(input, &flatten_paste_text(text));
    reset_history_browse(input);
    reset_slash_completion(input);
}

fn submit_or_accept_completion(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
) -> TerminalKeyAction {
    if accept_visible_slash_completion(store, config, input, false) {
        reset_history_browse(input);
        return TerminalKeyAction::None;
    }

    let line = input.buffer.clone();
    input.buffer.clear();
    input.cursor = 0;
    reset_history_browse(input);
    reset_slash_completion(input);
    if line.trim().is_empty() {
        TerminalKeyAction::None
    } else {
        TerminalKeyAction::Submit(line)
    }
}

fn move_slash_or_history(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
    delta: isize,
) {
    if !move_slash_completion_selection(store, config, input, delta) {
        navigate_history(input, delta);
        reset_slash_completion(input);
    }
}

fn reset_after_edit(input: &mut InputState) {
    reset_history_browse(input);
    reset_slash_completion(input);
}

#[cfg(test)]
mod tests {
    use super::super::codex_composer::{remember_input, set_buffer};
    use super::*;

    fn store_and_config() -> (tempfile::TempDir, Arc<SessionStore>, Arc<Config>) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite"))
                .expect("store"),
        );
        (tmp, store, Arc::new(Config::default()))
    }

    #[test]
    fn enter_submits_and_clears_draft() {
        let (_tmp, store, config) = store_and_config();
        let mut input = InputState::default();
        set_buffer(&mut input, "你好".into());

        let action = handle_composer_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &store,
            &config,
            &mut input,
        );

        assert_eq!(action, TerminalKeyAction::Submit("你好".into()));
        assert!(input.buffer.is_empty());
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn slash_enter_accepts_partial_command_before_submit() {
        let (_tmp, store, config) = store_and_config();
        let mut input = InputState::default();
        set_buffer(&mut input, "/ro".into());

        let action = handle_composer_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &store,
            &config,
            &mut input,
        );

        assert_eq!(action, TerminalKeyAction::None);
        assert_eq!(input.buffer, "/roles ");
    }

    #[test]
    fn ctrl_p_and_ctrl_n_follow_codex_popup_navigation() {
        let (_tmp, store, config) = store_and_config();
        let mut input = InputState::default();
        set_buffer(&mut input, "/".into());

        handle_composer_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &store,
            &config,
            &mut input,
        );
        assert_eq!(input.slash_completion_index, 1);

        handle_composer_key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &store,
            &config,
            &mut input,
        );
        assert_eq!(input.slash_completion_index, 0);
    }

    #[test]
    fn history_navigation_restores_draft() {
        let (_tmp, store, config) = store_and_config();
        let mut input = InputState::default();
        remember_input(&mut input, "first");
        remember_input(&mut input, "second");
        set_buffer(&mut input, "draft".into());

        handle_composer_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &store,
            &config,
            &mut input,
        );
        assert_eq!(input.buffer, "second");

        handle_composer_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &store,
            &config,
            &mut input,
        );
        assert_eq!(input.buffer, "draft");
    }

    #[test]
    fn paste_flattens_multiline_text_and_resets_browse_state() {
        let mut input = InputState {
            history_index: Some(0),
            slash_completion_index: 3,
            slash_completion_dismissed: true,
            ..InputState::default()
        };

        handle_paste_text(&mut input, "a\r\nb\nc");

        assert_eq!(input.buffer, "a b c");
        assert_eq!(input.cursor, input.buffer.len());
        assert_eq!(input.history_index, None);
        assert_eq!(input.slash_completion_index, 0);
        assert!(!input.slash_completion_dismissed);
    }
}
