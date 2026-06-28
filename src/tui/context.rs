use super::state::{ChatMsg, ContextMode, InputState};
use super::util::truncate_chars;

const COMPACT_MESSAGE_LIMIT: usize = 8;
const FULL_MESSAGE_LIMIT: usize = 24;
const COMPACT_TOTAL_CHARS: usize = 4_000;
const FULL_TOTAL_CHARS: usize = 12_000;
const COMPACT_MESSAGE_CHARS: usize = 700;
const FULL_MESSAGE_CHARS: usize = 1_800;
const COMPACT_SUMMARY_TRIGGER_MESSAGES: usize = 10;
const FULL_SUMMARY_TRIGGER_MESSAGES: usize = 28;
const COMPACT_SUMMARY_TARGET_CHARS: usize = 1_200;
const FULL_SUMMARY_TARGET_CHARS: usize = 2_400;

pub(super) struct ContextualPrompt {
    pub(super) prompt: String,
    pub(super) message_count: usize,
    pub(super) context_chars: usize,
}

struct ContextEntry {
    role: &'static str,
    content: String,
}

pub(super) fn build_contextual_prompt(
    input: &mut InputState,
    current_prompt: &str,
) -> ContextualPrompt {
    refresh_context_summary(input);
    let entries = selected_entries(input);
    let summary = input.context_summary_text.trim();
    if input.context_mode == ContextMode::Off || (entries.is_empty() && summary.is_empty()) {
        return ContextualPrompt {
            prompt: current_prompt.to_string(),
            message_count: 0,
            context_chars: 0,
        };
    }

    let context = format_context_block(summary, &entries);
    let prompt = format!(
        "You are continuing a multi-turn terminal chat conversation.\n\
         Use the conversation context to resolve follow-ups, pronouns, and references.\n\
         The current user request below is the highest priority.\n\n\
         Conversation context:\n{}\n\n\
         ---\n\
         Current user request:\n{}",
        context, current_prompt
    );

    ContextualPrompt {
        prompt,
        message_count: entries.len() + usize::from(!summary.is_empty()),
        context_chars: context.chars().count(),
    }
}

pub(super) fn context_summary(input: &InputState) -> String {
    if input.context_mode == ContextMode::Off {
        return "off".into();
    }

    let entries = selected_entries(input);
    let summary_note = if input.context_summary_text.trim().is_empty() {
        "none".into()
    } else {
        format!(
            "{} chars, through message {}",
            input.context_summary_text.chars().count(),
            input.context_summary_upto
        )
    };
    format!(
        "{} ({} recent message(s), summary: {}, last inject: {} item(s) / {} chars)",
        input.context_mode.as_str(),
        entries.len(),
        summary_note,
        input.last_context_messages,
        input.last_context_chars
    )
}

pub(super) fn handle_context_command(input: &mut InputState, args: &[&str]) -> String {
    match args.first().copied().unwrap_or("status") {
        "status" | "show" | "info" => {
            refresh_context_summary(input);
            format_context_status(input)
        }
        "on" | "compact" => {
            input.context_mode = ContextMode::Compact;
            refresh_context_summary(input);
            format_context_status(input)
        }
        "full" => {
            input.context_mode = ContextMode::Full;
            refresh_context_summary(input);
            format_context_status(input)
        }
        "off" | "none" | "disable" | "disabled" => {
            input.context_mode = ContextMode::Off;
            format_context_status(input)
        }
        "clear" | "reset" => {
            input.context_cutoff = input.transcript.len();
            input.context_summary_text.clear();
            input.context_summary_upto = input.context_cutoff;
            input.last_result_output = None;
            input.last_context_messages = 0;
            input.last_context_chars = 0;
            "Context memory cleared for future prompts. Visible transcript was kept.".into()
        }
        "help" | "?" => context_help(),
        other => format!("Unknown context option: {}\n\n{}", other, context_help()),
    }
}

pub(super) fn format_context_status(input: &InputState) -> String {
    let entries = selected_entries(input);
    let summary = input.context_summary_text.trim();
    let preview = if entries.is_empty() {
        if summary.is_empty() {
            "  (no remembered messages yet)".into()
        } else {
            format!("  summary: {}", truncate_chars(summary, 180))
        }
    } else {
        entries
            .iter()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|entry| {
                format!(
                    "  {}: {}",
                    entry.role,
                    truncate_chars(&entry.content.replace('\n', " "), 120)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "Context memory:\n  mode: {}\n  rolling summary: {}\n  recent messages: {}\n  last injection: {} item(s), {} chars\n\nCommands:\n  /context compact\n  /context full\n  /context off\n  /context clear\n\nPreview:\n{}",
        input.context_mode.as_str(),
        if summary.is_empty() {
            "none".into()
        } else {
            format!("{} chars through message {}", summary.chars().count(), input.context_summary_upto)
        },
        entries.len(),
        input.last_context_messages,
        input.last_context_chars,
        preview
    )
}

fn context_help() -> String {
    "Context commands:\n  /context              Show memory status\n  /context compact      Inject recent context (default)\n  /context full         Inject more history\n  /context off          Stop injecting history\n  /context clear        Forget prior history for future prompts".into()
}

pub(super) fn refresh_context_summary(input: &mut InputState) {
    if input.context_mode == ContextMode::Off {
        return;
    }

    let (message_limit, _, per_message_limit) = context_limits(input.context_mode);
    let trigger = summary_trigger(input.context_mode);
    let start = input.context_cutoff.min(input.transcript.len());
    let summary_start = input
        .context_summary_upto
        .max(start)
        .min(input.transcript.len());
    let available = input.transcript.len().saturating_sub(summary_start);
    if available <= message_limit + trigger {
        return;
    }

    let absorb_end = input.transcript.len().saturating_sub(message_limit);
    if absorb_end <= summary_start {
        return;
    }

    let mut pieces = Vec::new();
    if !input.context_summary_text.trim().is_empty() {
        pieces.push(input.context_summary_text.trim().to_string());
    }
    pieces.extend(
        input.transcript[summary_start..absorb_end]
            .iter()
            .filter_map(|msg| context_entry(msg, per_message_limit))
            .map(|entry| format!("{}: {}", entry.role, entry.content.replace('\n', " "))),
    );

    input.context_summary_text = truncate_summary(&pieces.join("\n"), input.context_mode);
    input.context_summary_upto = absorb_end;
}

fn selected_entries(input: &InputState) -> Vec<ContextEntry> {
    if input.context_mode == ContextMode::Off {
        return vec![];
    }

    let (message_limit, total_limit, per_message_limit) = context_limits(input.context_mode);
    let start = input
        .context_summary_upto
        .max(input.context_cutoff)
        .min(input.transcript.len());
    let mut entries: Vec<ContextEntry> = input.transcript[start..]
        .iter()
        .filter_map(|msg| context_entry(msg, per_message_limit))
        .collect();
    append_last_result_context(input, &mut entries, per_message_limit);

    if entries.len() > message_limit {
        entries = entries.split_off(entries.len() - message_limit);
    }

    while entries.len() > 1 && format_entries(&entries).chars().count() > total_limit {
        entries.remove(0);
    }

    entries
}

fn append_last_result_context(
    input: &InputState,
    entries: &mut Vec<ContextEntry>,
    per_message_limit: usize,
) {
    if input.context_cutoff >= input.transcript.len() {
        return;
    }
    let Some(result) = input.last_result_output.as_deref().map(str::trim) else {
        return;
    };
    if result.is_empty() {
        return;
    }
    if entries
        .iter()
        .any(|entry| entry.role == "assistant" && entry.content.contains(result))
    {
        return;
    }
    entries.push(ContextEntry {
        role: "assistant",
        content: truncate_chars(&format!("Last run result:\n{}", result), per_message_limit),
    });
}

fn summary_trigger(mode: ContextMode) -> usize {
    match mode {
        ContextMode::Off => usize::MAX,
        ContextMode::Compact => COMPACT_SUMMARY_TRIGGER_MESSAGES,
        ContextMode::Full => FULL_SUMMARY_TRIGGER_MESSAGES,
    }
}

fn truncate_summary(summary: &str, mode: ContextMode) -> String {
    let max = match mode {
        ContextMode::Off => 0,
        ContextMode::Compact => COMPACT_SUMMARY_TARGET_CHARS,
        ContextMode::Full => FULL_SUMMARY_TARGET_CHARS,
    };
    truncate_chars(summary.trim(), max)
}

fn context_limits(mode: ContextMode) -> (usize, usize, usize) {
    match mode {
        ContextMode::Off => (0, 0, 0),
        ContextMode::Compact => (
            COMPACT_MESSAGE_LIMIT,
            COMPACT_TOTAL_CHARS,
            COMPACT_MESSAGE_CHARS,
        ),
        ContextMode::Full => (FULL_MESSAGE_LIMIT, FULL_TOTAL_CHARS, FULL_MESSAGE_CHARS),
    }
}

fn context_entry(msg: &ChatMsg, per_message_limit: usize) -> Option<ContextEntry> {
    match msg.role.as_str() {
        "user" if msg.status == "complete" || msg.status == "queued_batch" => {
            cleaned_entry("user", &msg.content, per_message_limit)
        }
        "assistant"
            if msg.status == "complete" || msg.status == "warning" || msg.status == "error" =>
        {
            let content = assistant_context_content(&msg.content);
            cleaned_entry("assistant", &content, per_message_limit)
        }
        _ => None,
    }
}

fn cleaned_entry(role: &'static str, content: &str, limit: usize) -> Option<ContextEntry> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed == "thinking..." {
        return None;
    }

    Some(ContextEntry {
        role,
        content: truncate_chars(trimmed, limit),
    })
}

fn assistant_context_content(content: &str) -> String {
    if !content.contains("Final result:") {
        let trimmed = content.trim();
        if is_result_capture_notice(trimmed) {
            return String::new();
        }
        return content.trim().to_string();
    }

    if let Some((_, result)) = content.split_once("Final result:") {
        let plain = result.trim();
        if !plain.is_empty() && !plain.contains('│') {
            return plain.to_string();
        }
    }

    let mut lines = Vec::new();
    for line in content.lines() {
        let Some(line) = line.strip_prefix("│ ") else {
            continue;
        };
        let line = line.strip_suffix(" │").unwrap_or(line).trim_end();
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }

    if lines.is_empty() {
        content.trim().to_string()
    } else {
        lines.join("\n")
    }
}

fn is_result_capture_notice(content: &str) -> bool {
    is_completion_notice_prefix(content)
        && (content.contains("Result captured.") || content.contains("No final text was emitted"))
}

fn is_completion_notice_prefix(content: &str) -> bool {
    [
        "✓ done",
        "✓ answer complete",
        "✓ worker complete",
        "⚠ completed with review issues",
        "✗ worker failed",
        "✗ no worker result completed",
    ]
    .iter()
    .any(|prefix| content.starts_with(prefix))
}

fn format_entries(entries: &[ContextEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("{}:\n{}", entry.role, entry.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_context_block(summary: &str, entries: &[ContextEntry]) -> String {
    let recent = format_entries(entries);
    match (summary.trim().is_empty(), recent.is_empty()) {
        (true, true) => String::new(),
        (true, false) => recent,
        (false, true) => format!("Rolling summary:\n{}", summary.trim()),
        (false, false) => format!(
            "Rolling summary:\n{}\n\nRecent messages:\n{}",
            summary.trim(),
            recent
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, status: &str, content: &str) -> ChatMsg {
        ChatMsg {
            role: role.into(),
            content: content.into(),
            ts: std::time::SystemTime::now(),
            status: status.into(),
        }
    }

    #[test]
    fn contextual_prompt_includes_recent_complete_messages() {
        let mut input = InputState::default();
        input
            .transcript
            .push(msg("user", "complete", "build the tui"));
        input.transcript.push(msg(
            "assistant",
            "complete",
            "✓ done\n\nFinal result:\ndone",
        ));

        let built = build_contextual_prompt(&mut input, "make it remember");

        assert!(built.prompt.contains("Conversation context:"));
        assert!(built.prompt.contains("user:\nbuild the tui"));
        assert!(built.prompt.contains("assistant:\ndone"));
        assert!(built
            .prompt
            .contains("Current user request:\nmake it remember"));
        assert_eq!(built.message_count, 2);
    }

    #[test]
    fn contextual_prompt_includes_visible_last_result_for_followups() {
        let mut input = InputState {
            last_result_output: Some("implemented queue fixes".into()),
            ..InputState::default()
        };
        input.transcript.push(msg("user", "complete", "fix queue"));
        input
            .transcript
            .push(msg("assistant", "complete", "implemented queue fixes"));

        let built = build_contextual_prompt(&mut input, "continue polishing");

        assert!(built.prompt.contains("user:\nfix queue"));
        assert!(built.prompt.contains("assistant:\nimplemented queue fixes"));
        assert!(built.prompt.contains("implemented queue fixes"));
        assert!(!built.prompt.contains("Result captured. Use /copy"));
    }

    #[test]
    fn contextual_prompt_includes_review_unavailable_worker_result_without_notice() {
        let mut input = InputState {
            last_result_output: Some("implemented orchestration flow".into()),
            ..InputState::default()
        };
        input
            .transcript
            .push(msg("user", "complete", "优化编排流程"));
        input.transcript.push(msg(
            "assistant",
            "warning",
            "implemented orchestration flow",
        ));

        let built = build_contextual_prompt(&mut input, "继续");

        assert!(built.prompt.contains("user:\n优化编排流程"));
        assert!(built
            .prompt
            .contains("assistant:\nimplemented orchestration flow"));
        assert!(built.prompt.contains("implemented orchestration flow"));
        assert!(!built.prompt.contains("review unavailable"));
        assert!(!built.prompt.contains("Result captured. Use /copy"));
    }

    #[test]
    fn contextual_prompt_excludes_empty_worker_completion_notice() {
        let mut input = InputState::default();
        input.transcript.push(msg("user", "complete", "运行一下"));
        input.transcript.push(msg(
            "assistant",
            "complete",
            "✓ worker complete\n\nNo final text was emitted by the worker.\nDetails: /process 200",
        ));

        let built = build_contextual_prompt(&mut input, "继续");

        assert!(built.prompt.contains("user:\n运行一下"));
        assert!(!built.prompt.contains("No final text was emitted"));
        assert!(!built.prompt.contains("worker complete"));
    }

    #[test]
    fn context_clear_excludes_hidden_last_result() {
        let mut input = InputState {
            last_result_output: Some("implemented queue fixes".into()),
            ..InputState::default()
        };
        input.transcript.push(msg("user", "complete", "fix queue"));
        input
            .transcript
            .push(msg("assistant", "complete", "implemented queue fixes"));
        input.context_cutoff = input.transcript.len();

        let built = build_contextual_prompt(&mut input, "continue polishing");

        assert!(!built.prompt.contains("implemented queue fixes"));
        assert_eq!(built.message_count, 0);
    }

    #[test]
    fn contextual_prompt_includes_started_queue_batches() {
        let mut input = InputState::default();
        input.transcript.push(msg(
            "user",
            "queued_batch",
            "1. fix queue\n\n2. update docs",
        ));

        let built = build_contextual_prompt(&mut input, "continue");

        assert!(built.prompt.contains("user:\n1. fix queue"));
        assert!(built.prompt.contains("2. update docs"));
        assert_eq!(built.message_count, 1);
    }

    #[test]
    fn context_clear_keeps_transcript_but_stops_injection() {
        let mut input = InputState::default();
        input.transcript.push(msg("user", "complete", "old"));

        let clear = handle_context_command(&mut input, &["clear"]);
        let built = build_contextual_prompt(&mut input, "new");

        assert!(clear.contains("Visible transcript was kept"));
        assert_eq!(input.transcript.len(), 1);
        assert_eq!(built.prompt, "new");
        assert_eq!(built.message_count, 0);
    }

    #[test]
    fn long_context_rolls_older_messages_into_summary() {
        let mut input = InputState::default();
        for idx in 0..24 {
            input
                .transcript
                .push(msg("user", "complete", &format!("request {idx}")));
            input
                .transcript
                .push(msg("assistant", "complete", &format!("answer {idx}")));
        }

        let built = build_contextual_prompt(&mut input, "continue");

        assert!(built.prompt.contains("Rolling summary:"));
        assert!(built.prompt.contains("Recent messages:"));
        assert!(input.context_summary_upto > 0);
        assert!(!input.context_summary_text.is_empty());
        assert!(built.prompt.contains("request 0"));
        assert!(built.prompt.contains("answer 23"));
    }
}
