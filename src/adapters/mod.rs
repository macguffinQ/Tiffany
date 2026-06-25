//! Worker adapters: one per agent runtime.

use std::sync::{Arc, Mutex};

pub mod claude_code;
pub mod codex_cli;
pub mod direct_api;
pub mod gemini_cli;

#[derive(Clone, Default)]
pub(crate) struct StderrCapture {
    lines: Arc<Mutex<Vec<String>>>,
}

impl StderrCapture {
    pub(crate) fn push(&self, line: String) {
        let Ok(mut lines) = self.lines.lock() else {
            return;
        };
        if lines.len() >= 40 {
            lines.remove(0);
        }
        lines.push(line);
    }

    pub(crate) fn error_suffix(&self) -> String {
        let Ok(lines) = self.lines.lock() else {
            return String::new();
        };
        let mut recent = lines
            .iter()
            .rev()
            .filter_map(|line| {
                let line = line.trim();
                (!line.is_empty()).then_some(line.to_string())
            })
            .take(8)
            .collect::<Vec<_>>();
        recent.reverse();
        if recent.is_empty() {
            return String::new();
        }
        format!("; stderr: {}", truncate_stderr(&recent.join(" | "), 800))
    }
}

fn truncate_stderr(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return value.to_string();
        };
        out.push(ch);
    }
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}
