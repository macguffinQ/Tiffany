use std::io::Write;
use std::process::{Command, Stdio};

use crate::agent_events;

pub(super) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars().take(max.saturating_sub(1)) {
        out.push(c);
    }
    out.push('…');
    out
}

pub(super) fn humanize_jsonish(s: &str, max: usize) -> String {
    agent_events::humanize_jsonish(s, max)
}

pub(super) fn is_low_value_execution_output(text: &str) -> bool {
    agent_events::is_low_value_output(text)
}

pub(super) fn summarize_execution_output(raw: &str, max: usize) -> Option<String> {
    let display = humanize_jsonish(raw, max);
    (!is_low_value_execution_output(&display)).then_some(display)
}

pub(super) fn normalize_execution_output_summary(display: &str) -> String {
    agent_events::normalize_output_summary(display)
}

pub(super) fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return format!("{duration_ms}ms");
    }
    let seconds = duration_ms / 1_000;
    let millis = duration_ms % 1_000;
    if seconds < 60 {
        if millis == 0 {
            return format!("{seconds}s");
        }
        return format!("{seconds}.{}s", millis / 100);
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}m{seconds:02}s")
}

pub(super) fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("pbcopy")
    } else if cfg!(target_os = "windows") {
        Command::new("clip.exe")
    } else if Command::new("xclip")
        .arg("--version")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        Command::new("xclip")
    } else {
        Command::new("wl-copy")
    };

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "clipboard command failed: {:?}",
            status.code()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_claude_stream_json_without_showing_raw_json() {
        let raw = r#"claude assistant: {"type":"assistant","message":{"content":[{"type":"text","text":"Working on it"}]}}"#;

        let display = humanize_jsonish(raw, 200);

        assert_eq!(display, "claude assistant: Working on it");
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_json_code_fence() {
        let raw = "```json\n{\"approved\":false,\"issues\":[\"missing test\"]}\n```";

        let display = humanize_jsonish(raw, 200);

        assert_eq!(display, "needs changes: 1 issue(s)\n  - missing test");
        assert!(!display.contains('{'));
    }

    #[test]
    fn humanizes_embedded_json() {
        let raw = "final payload {\"result\":\"done\"}";

        let display = humanize_jsonish(raw, 200);

        assert_eq!(display, "final payload done");
    }

    #[test]
    fn formats_worker_durations_for_progress() {
        assert_eq!(format_duration_ms(25), "25ms");
        assert_eq!(format_duration_ms(1_250), "1.2s");
        assert_eq!(format_duration_ms(62_000), "1m02s");
    }
}
