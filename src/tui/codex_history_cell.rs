// Codex-style history cell renderers for tiffany-loop lightweight terminal runner.
//
// The upstream UI source implementation lives under:
// third_party/openai-codex/codex-rs/tui/src/history_cell/
//
// This module keeps the same boundary for the local non-fullscreen runner:
// transcript entries render themselves into finalized history lines, while the
// terminal loop only inserts those lines into scrollback.

use super::state::ChatMsg;
use super::util::truncate_chars;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const TIFFANY: &str = "\x1b[38;2;129;216;208m";
const CYAN: &str = TIFFANY;
const GREEN: &str = TIFFANY;
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = TIFFANY;

pub(super) fn message_lines(msg: &ChatMsg) -> Vec<String> {
    let mut lines = match msg.role.as_str() {
        "user" => user_message_lines(msg),
        "assistant" => assistant_message_lines(msg),
        "tool" => tool_message_lines(msg),
        "trace" => trace_message_lines(msg),
        _ => system_lines(&msg.content),
    };
    lines.push(String::new());
    lines
}

pub(super) fn system_lines(content: &str) -> Vec<String> {
    let mut lines = vec![format!("{DIM}system{RESET}")];
    lines.extend(indented_lines(content, "  "));
    lines
}

pub(super) fn queue_receipt_lines(count: usize, paused: bool, last: &str) -> Vec<String> {
    let queue_state = if paused { "paused batch" } else { "next batch" };
    let mut lines = vec![format!(
        "{CYAN}↳{RESET} {CYAN}{BOLD}queued{RESET} {DIM}{count} item(s), {queue_state}, runs together{RESET}",
    )];
    lines.extend(indented_lines(&truncate_chars(last, 180), "  "));
    lines.push(String::new());
    lines
}

pub(super) fn queue_batch_start_lines(count: usize) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    vec![format!(
        "{CYAN}▶{RESET} {CYAN}{BOLD}queued batch{RESET} {DIM}starting {count} item(s) together{RESET}",
    )]
}

pub(super) fn assistant_content_lines(content: &str) -> Vec<String> {
    super::codex_chat_view::assistant_content_history_lines(content)
}

pub(super) fn status_icon(status: &str) -> (&'static str, &'static str) {
    match status {
        "complete" => ("✓", GREEN),
        "error" => ("✗", RED),
        "thinking" => ("●", YELLOW),
        "warning" => ("⚠", YELLOW),
        "queued" => ("↳", CYAN),
        "queued_batch" => ("▶", CYAN),
        "removed" => ("○", DIM),
        _ => ("·", DIM),
    }
}

fn user_message_lines(msg: &ChatMsg) -> Vec<String> {
    let header = match msg.status.as_str() {
        "queued" => format!("{CYAN}↳{RESET} {CYAN}{BOLD}queued message{RESET}"),
        "queued_batch" => format!("{CYAN}▶{RESET} {CYAN}{BOLD}queued batch{RESET}"),
        "removed" => format!("{DIM}○ removed queued message{RESET}"),
        _ => format!("{CYAN}{BOLD}you{RESET}"),
    };
    let mut lines = vec![header];
    lines.extend(indented_lines(&msg.content, "  "));
    lines
}

fn assistant_message_lines(msg: &ChatMsg) -> Vec<String> {
    let (icon, color) = status_icon(&msg.status);
    let mut lines = vec![format!(
        "{color}{icon}{RESET} {GREEN}{BOLD}orchestrator{RESET}"
    )];
    lines.extend(assistant_content_lines(&msg.content));
    lines
}

fn tool_message_lines(msg: &ChatMsg) -> Vec<String> {
    let mut lines = vec![format!("{DIM}↳ tool{RESET}")];
    lines.extend(indented_lines(&msg.content, "  "));
    lines
}

fn trace_message_lines(msg: &ChatMsg) -> Vec<String> {
    let mut lines = vec![format!("{MAGENTA}{BOLD}trace{RESET}")];
    lines.extend(indented_lines(&msg.content, "  "));
    lines
}

fn indented_lines(content: &str, prefix: &str) -> Vec<String> {
    if content.is_empty() {
        return vec![prefix.to_string()];
    }
    content
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn msg(role: &str, status: &str, content: &str) -> ChatMsg {
        ChatMsg {
            role: role.into(),
            status: status.into(),
            content: content.into(),
            ts: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn renders_user_and_queue_cells() {
        let user = message_lines(&msg("user", "complete", "hello"));
        assert!(user[0].contains("you"));
        assert!(user.iter().any(|line| line.ends_with("hello")));

        let queued = message_lines(&msg("user", "queued", "later"));
        assert!(queued[0].contains("queued message"));
        assert!(queued.iter().any(|line| line.ends_with("later")));
    }

    #[test]
    fn renders_assistant_plain_final_result_without_box() {
        let assistant = message_lines(&msg(
            "assistant",
            "complete",
            "✓ done\n\nFinal result:\nhello",
        ));
        let joined = assistant.join("\n");

        assert!(joined.contains("orchestrator"));
        assert!(joined.contains("Final result:"));
        assert!(!joined.contains("╭─ Final result:"));
    }

    #[test]
    fn renders_system_and_trace_cells() {
        let system = message_lines(&msg("system", "complete", "saved"));
        assert!(system[0].contains("system"));
        assert!(system.iter().any(|line| line.ends_with("saved")));

        let trace = message_lines(&msg("trace", "complete", "worker output"));
        assert!(trace[0].contains("trace"));
        assert!(trace.iter().any(|line| line.ends_with("worker output")));

        let tool = message_lines(&msg("tool", "complete", "Bash(cargo test)"));
        assert!(tool[0].contains("tool"));
        assert!(tool.iter().any(|line| line.ends_with("Bash(cargo test)")));
    }

    #[test]
    fn renders_queue_receipt_and_start_lines() {
        let receipt = queue_receipt_lines(2, false, "next task");
        assert!(receipt[0].contains("2 item(s)"));
        assert!(receipt[0].contains("runs together"));
        assert!(receipt.iter().any(|line| line.ends_with("next task")));

        let start = queue_batch_start_lines(3);
        assert_eq!(start.len(), 1);
        assert!(start[0].contains("starting 3 item(s) together"));
    }

    #[test]
    fn status_icons_match_terminal_states() {
        assert_eq!(status_icon("complete").0, "✓");
        assert_eq!(status_icon("error").0, "✗");
        assert_eq!(status_icon("thinking").0, "●");
        assert_eq!(status_icon("warning").0, "⚠");
        assert_eq!(status_icon("queued").0, "↳");
        assert_eq!(status_icon("removed").0, "○");
    }
}
