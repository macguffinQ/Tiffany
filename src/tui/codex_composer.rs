// Codex-style composer state helpers for tiffany-loop lightweight terminal runner.
//
// Upstream Codex keeps prompt editing, history navigation, and slash-completion
// behavior under `codex-rs/tui/src/bottom_pane/chat_composer*`. This module
// keeps the same boundary for the local single-line composer: terminal event
// handling asks this module to mutate `InputState`, while rendering stays in
// `codex_bottom_pane` and `codex_command_popup`.

use super::codex_command_popup::CommandPopupSelection;
use super::commands::{complete_slash_argument_at, slash_argument_candidates, SlashArgCandidate};
use super::state::InputState;
use crate::config::Config;
use crate::core::session_store::SessionStore;
use std::sync::Arc;

pub(super) fn clamp_cursor(buffer: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(buffer.len());
    while cursor > 0 && !buffer.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

pub(super) fn set_buffer(input: &mut InputState, text: String) {
    input.buffer = text;
    input.cursor = input.buffer.len();
}

pub(super) fn insert_text_at_cursor(input: &mut InputState, text: &str) {
    input.cursor = clamp_cursor(&input.buffer, input.cursor);
    input.buffer.insert_str(input.cursor, text);
    input.cursor += text.len();
}

pub(super) fn insert_char_at_cursor(input: &mut InputState, c: char) {
    input.cursor = clamp_cursor(&input.buffer, input.cursor);
    input.buffer.insert(input.cursor, c);
    input.cursor += c.len_utf8();
}

pub(super) fn delete_char_before_cursor(input: &mut InputState) {
    input.cursor = clamp_cursor(&input.buffer, input.cursor);
    if input.cursor == 0 {
        return;
    }
    if let Some((idx, _)) = input.buffer[..input.cursor].char_indices().last() {
        input.buffer.drain(idx..input.cursor);
        input.cursor = idx;
    }
}

pub(super) fn delete_char_at_cursor(input: &mut InputState) {
    input.cursor = clamp_cursor(&input.buffer, input.cursor);
    if input.cursor >= input.buffer.len() {
        return;
    }
    let next = input.buffer[input.cursor..]
        .chars()
        .next()
        .map(|c| input.cursor + c.len_utf8())
        .unwrap_or(input.buffer.len());
    input.buffer.drain(input.cursor..next);
}

pub(super) fn move_cursor_left(input: &mut InputState) {
    input.cursor = clamp_cursor(&input.buffer, input.cursor);
    if input.cursor == 0 {
        return;
    }
    if let Some((idx, _)) = input.buffer[..input.cursor].char_indices().last() {
        input.cursor = idx;
    }
}

pub(super) fn move_cursor_right(input: &mut InputState) {
    input.cursor = clamp_cursor(&input.buffer, input.cursor);
    if input.cursor >= input.buffer.len() {
        return;
    }
    if let Some(c) = input.buffer[input.cursor..].chars().next() {
        input.cursor += c.len_utf8();
    }
}

pub(super) fn remember_input(input: &mut InputState, line: &str) {
    if input.input_history.last().map(String::as_str) != Some(line) {
        input.input_history.push(line.to_string());
        if input.input_history.len() > 100 {
            input.input_history.remove(0);
        }
    }
    reset_history_browse(input);
}

pub(super) fn reset_history_browse(input: &mut InputState) {
    input.history_index = None;
    input.history_draft = None;
}

pub(super) fn reset_slash_completion(input: &mut InputState) {
    input.slash_completion_index = 0;
    input.slash_completion_dismissed = false;
}

pub(super) fn navigate_history(input: &mut InputState, delta: isize) -> bool {
    if input.input_history.is_empty() {
        return false;
    }

    if input.history_index.is_none() {
        if delta > 0 {
            return false;
        }
        input.history_draft = Some(input.buffer.clone());
        let next = input.input_history.len() - 1;
        input.history_index = Some(next);
        set_buffer(input, input.input_history[next].clone());
        return true;
    }

    let current = input.history_index.expect("history index exists");
    let next = current as isize + delta;
    if next < 0 {
        return false;
    }

    let next = next as usize;
    if next < input.input_history.len() {
        input.history_index = Some(next);
        set_buffer(input, input.input_history[next].clone());
    } else {
        let draft = input.history_draft.take().unwrap_or_default();
        input.history_index = None;
        set_buffer(input, draft);
    }
    true
}

pub(super) fn terminal_slash_completion_lines(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &InputState,
    terminal_width: usize,
) -> Vec<String> {
    let candidates = visible_slash_completion_candidates(store, config, input);
    if candidates.is_empty() {
        return Vec::new();
    }
    let selected = input
        .slash_completion_index
        .min(candidates.len().saturating_sub(1));
    slash_completion_lines_with_selection(&candidates, Some(selected), terminal_width)
}

#[cfg(test)]
pub(super) fn slash_completion_lines(
    candidates: &[SlashArgCandidate],
    terminal_width: usize,
) -> Vec<String> {
    super::codex_command_popup::render_command_popup(candidates, None, terminal_width)
}

pub(super) fn slash_completion_lines_with_selection(
    candidates: &[SlashArgCandidate],
    selected: Option<usize>,
    terminal_width: usize,
) -> Vec<String> {
    let selection = selected.map(CommandPopupSelection::new);
    super::codex_command_popup::render_command_popup(candidates, selection, terminal_width)
}

#[cfg(test)]
pub(super) fn slash_completion_window_start(total: usize, selected: usize, limit: usize) -> usize {
    super::codex_command_popup::command_popup_window_start(total, selected, limit)
}

pub(super) fn slash_completion_page_size() -> isize {
    CommandPopupSelection::new(0).page_size() as isize
}

pub(super) fn has_visible_slash_completions(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &InputState,
) -> bool {
    !visible_slash_completion_candidates(store, config, input).is_empty()
}

pub(super) fn move_slash_completion_selection(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
    delta: isize,
) -> bool {
    let candidates = visible_slash_completion_candidates(store, config, input);
    if candidates.is_empty() {
        return false;
    }

    let len = candidates.len() as isize;
    let current = input.slash_completion_index.min(candidates.len() - 1) as isize;
    input.slash_completion_index = (current + delta).rem_euclid(len) as usize;
    true
}

pub(super) fn complete_selected_slash_completion(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
) -> bool {
    let candidates = slash_argument_candidates(store, config, input);
    if candidates.is_empty() {
        return false;
    }
    let selected = input
        .slash_completion_index
        .min(candidates.len().saturating_sub(1));
    complete_slash_argument_at(store, config, input, selected)
}

pub(super) fn accept_visible_slash_completion(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
    accept_exact: bool,
) -> bool {
    let candidates = visible_slash_completion_candidates(store, config, input);
    if candidates.is_empty() {
        return false;
    }

    let selected = input
        .slash_completion_index
        .min(candidates.len().saturating_sub(1));
    if !accept_exact && slash_completion_candidate_is_exact(input, &candidates[selected]) {
        return false;
    }

    let completed = complete_slash_argument_at(store, config, input, selected);
    if completed {
        reset_slash_completion(input);
    }
    completed
}

pub(super) fn current_slash_completion_prefix(buffer: &str, cursor: usize) -> Option<&str> {
    if cursor != buffer.len() || !buffer.starts_with('/') {
        return None;
    }
    if buffer.ends_with(char::is_whitespace) {
        return Some("");
    }

    let token = buffer.split_whitespace().last()?;
    Some(token.trim_start_matches('/'))
}

pub(super) fn flatten_paste_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .collect::<Vec<_>>()
        .join(" ")
}

fn visible_slash_completion_candidates(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &InputState,
) -> Vec<SlashArgCandidate> {
    if input.slash_completion_dismissed {
        return Vec::new();
    }
    slash_argument_candidates(store, config, input)
}

fn slash_completion_candidate_is_exact(input: &InputState, candidate: &SlashArgCandidate) -> bool {
    current_slash_completion_prefix(&input.buffer, input.cursor)
        .map(|prefix| prefix == candidate.value)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_editing_preserves_utf8_boundaries() {
        let mut input = InputState::default();
        set_buffer(&mut input, "a你b".into());

        move_cursor_left(&mut input);
        delete_char_before_cursor(&mut input);

        assert_eq!(input.buffer, "ab");
        assert_eq!(input.cursor, 1);

        insert_char_at_cursor(&mut input, '好');
        assert_eq!(input.buffer, "a好b");
        assert_eq!(input.cursor, "a好".len());
    }

    #[test]
    fn history_navigation_restores_draft() {
        let mut input = InputState::default();
        remember_input(&mut input, "first");
        remember_input(&mut input, "second");
        input.buffer = "draft".into();
        input.cursor = input.buffer.len();

        assert!(navigate_history(&mut input, -1));
        assert_eq!(input.buffer, "second");
        assert!(navigate_history(&mut input, -1));
        assert_eq!(input.buffer, "first");
        assert!(navigate_history(&mut input, 1));
        assert_eq!(input.buffer, "second");
        assert!(navigate_history(&mut input, 1));
        assert_eq!(input.buffer, "draft");
    }

    #[test]
    fn paste_flattens_multiline_text_for_single_line_editor() {
        assert_eq!(flatten_paste_text("a\r\nb\nc"), "a b c");
    }
}
