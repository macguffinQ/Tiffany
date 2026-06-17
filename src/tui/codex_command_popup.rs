// Codex-style slash command popup for tiffany-loop lightweight terminal runner.
//
// Adapted from OpenAI Codex CLI concepts in:
// third_party/openai-codex/codex-rs/tui/src/bottom_pane/command_popup.rs
// third_party/openai-codex/codex-rs/tui/src/bottom_pane/popup_consts.rs
//
// OpenAI Codex CLI is licensed under Apache-2.0. This module keeps the same
// component boundary: popup filtering/windowing/rendering lives outside the
// terminal event loop.

use super::commands::SlashArgCandidate;
use unicode_width::UnicodeWidthStr;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const TIFFANY: &str = "\x1b[38;2;129;216;208m";
const MAX_POPUP_ROWS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CommandPopupSelection {
    pub(super) selected: usize,
    pub(super) visible_limit: usize,
}

impl CommandPopupSelection {
    pub(super) fn new(selected: usize) -> Self {
        Self {
            selected,
            visible_limit: MAX_POPUP_ROWS,
        }
    }

    pub(super) fn page_size(self) -> usize {
        self.visible_limit.max(1)
    }
}

pub(super) fn render_command_popup(
    candidates: &[SlashArgCandidate],
    selection: Option<CommandPopupSelection>,
    terminal_width: usize,
) -> Vec<String> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let selected = selection
        .map(|selection| selection.selected.min(candidates.len().saturating_sub(1)))
        .unwrap_or(0);
    let visible_limit = selection
        .map(CommandPopupSelection::page_size)
        .unwrap_or(MAX_POPUP_ROWS)
        .min(candidates.len().max(1));
    let window_start = command_popup_window_start(candidates.len(), selected, visible_limit);
    let window_end = (window_start + visible_limit).min(candidates.len());
    let width = terminal_width.max(24);
    let label_width = visible_label_width(&candidates[window_start..window_end]);
    let description_width = width.saturating_sub(label_width).saturating_sub(8).max(12);

    let mut lines = Vec::new();
    let title = if candidates.len() > 1 {
        format!("commands {}/{}", selected + 1, candidates.len())
    } else {
        "commands".to_string()
    };
    lines.push(format!(
        "{DIM}{title}  Enter accept · Esc close · ↑↓ move{RESET}"
    ));

    if window_start > 0 {
        lines.push(format!("  {DIM}↑ {} more{RESET}", window_start));
    }

    for (idx, candidate) in candidates
        .iter()
        .enumerate()
        .take(window_end)
        .skip(window_start)
    {
        let is_selected = idx == selected;
        let marker = if is_selected {
            format!("{TIFFANY}›{RESET}")
        } else {
            " ".to_string()
        };
        let label = format!("/{}", candidate.label.trim_start_matches('/'));
        let padded_label = pad_display_width(&label, label_width);
        let description = truncate_display_width(&candidate.description, description_width);
        let label_style = if is_selected { BOLD } else { "" };
        if description.is_empty() {
            lines.push(format!(
                " {marker} {TIFFANY}{label_style}{padded_label}{RESET}"
            ));
        } else {
            lines.push(format!(
                " {marker} {TIFFANY}{label_style}{padded_label}{RESET} {DIM}{description}{RESET}"
            ));
        }
    }

    if window_end < candidates.len() {
        lines.push(format!(
            "  {DIM}↓ {} more{RESET}",
            candidates.len() - window_end
        ));
    }

    lines
}

pub(super) fn command_popup_window_start(total: usize, selected: usize, limit: usize) -> usize {
    if total <= limit || limit == 0 {
        return 0;
    }
    selected
        .saturating_add(1)
        .saturating_sub(limit)
        .min(total - limit)
}

fn visible_label_width(candidates: &[SlashArgCandidate]) -> usize {
    candidates
        .iter()
        .map(|candidate| {
            let label = format!("/{}", candidate.label.trim_start_matches('/'));
            UnicodeWidthStr::width(label.as_str())
        })
        .max()
        .unwrap_or(0)
        .min(28)
}

fn pad_display_width(value: &str, target_width: usize) -> String {
    let width = UnicodeWidthStr::width(value);
    if width >= target_width {
        truncate_display_width(value, target_width)
    } else {
        format!("{value}{}", " ".repeat(target_width - width))
    }
}

fn truncate_display_width(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }
    if max_width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut width = 0;
    let ellipsis_width = 1;
    for ch in value.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + char_width + ellipsis_width > max_width {
            break;
        }
        out.push(ch);
        width += char_width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(label: &str) -> SlashArgCandidate {
        SlashArgCandidate {
            value: label.into(),
            label: label.into(),
            description: format!("description for {label}"),
        }
    }

    #[test]
    fn popup_renders_codex_style_selection_without_json() {
        let candidates = vec![candidate("roles"), candidate("queue")];
        let lines = render_command_popup(&candidates, Some(CommandPopupSelection::new(1)), 80);
        let joined = lines.join("\n");

        assert!(joined.contains("commands 2/2"));
        assert!(joined.contains("›"));
        assert!(joined.contains("/queue"));
        assert!(!joined.contains('{'));
    }

    #[test]
    fn popup_scroll_window_keeps_selected_visible() {
        let candidates = (0..14)
            .map(|idx| candidate(&format!("cmd{idx}")))
            .collect::<Vec<_>>();
        let lines = render_command_popup(&candidates, Some(CommandPopupSelection::new(11)), 80);
        let joined = lines.join("\n");

        assert!(joined.contains("commands 12/14"));
        assert!(joined.contains("↑ 4 more"));
        assert!(joined.contains("/cmd11"));
        assert!(joined.contains("↓ 2 more"));
    }

    #[test]
    fn popup_truncates_cjk_description_to_width() {
        let candidates = vec![SlashArgCandidate {
            value: "context".into(),
            label: "context".into(),
            description: "这是一段很长的中文说明，需要被截断但不能破坏字符边界".into(),
        }];

        let lines = render_command_popup(&candidates, Some(CommandPopupSelection::new(0)), 32);

        assert!(lines.join("\n").contains('…'));
    }
}
