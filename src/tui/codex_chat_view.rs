// Codex-style chat history view helpers for tiffany-loop lightweight terminal runner.
//
// The upstream UI source implementation uses HistoryCell renderers under:
// third_party/openai-codex/codex-rs/tui/src/history_cell/
//
// This module starts that split locally by owning assistant transcript line
// rendering instead of keeping it inside the terminal event loop.

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const TIFFANY: &str = "\x1b[38;2;129;216;208m";
const GREEN: &str = TIFFANY;
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";

pub(super) fn assistant_content_history_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return vec!["  ".into()];
    }

    let mut lines = Vec::new();
    for line in content.lines() {
        if is_final_result_border(line) || line.contains("Final result:") {
            lines.push(format!("  {GREEN}{BOLD}{line}{RESET}"));
        } else if let Some(result_line) = line.strip_prefix("│ ") {
            if let Some(result_line) = result_line.strip_suffix(" │") {
                lines.push(format!(
                    "  {GREEN}│{RESET} {BOLD}{result_line}{RESET} {GREEN}│{RESET}"
                ));
            } else {
                lines.push(format!("  {GREEN}│{RESET} {BOLD}{result_line}{RESET}"));
            }
        } else if line.starts_with("Details:") || line.starts_with("Actions:") {
            lines.push(format!("  {DIM}{line}{RESET}"));
        } else if line.starts_with("✓ done") {
            lines.push(format!("  {GREEN}{BOLD}{line}{RESET}"));
        } else if is_panel_heading(line) {
            lines.push(format!("  {GREEN}{BOLD}{line}{RESET}"));
        } else if line.trim_start().starts_with("✓ ") {
            lines.push(format!("  {GREEN}{line}{RESET}"));
        } else if line.trim_start().starts_with("✗ ") {
            lines.push(format!("  {RED}{line}{RESET}"));
        } else if line.trim_start().starts_with("⚠ ") {
            lines.push(format!("  {YELLOW}{line}{RESET}"));
        } else if is_step_line(line) {
            lines.push(format!("  {DIM}{line}{RESET}"));
        } else {
            lines.push(format!("  {line}"));
        }
    }
    lines
}

fn is_final_result_border(line: &str) -> bool {
    line.starts_with("╭─") || line.starts_with("╰─")
}

fn is_panel_heading(line: &str) -> bool {
    matches!(
        line,
        "Agent routing"
            | "Agent route"
            | "Role registry"
            | "Role selected"
            | "Role flow"
            | "Workflow status"
            | "Working"
            | "Routes:"
            | "Worker roles:"
            | "Pipeline roles:"
            | "Pipeline"
            | "Worker"
            | "Role config snippet:"
            | "Token usage"
    )
}

fn is_step_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    matches!(trimmed.chars().next(), Some('1' | '2' | '3' | '4' | '5'))
        && trimmed.chars().nth(1) == Some('.')
}
