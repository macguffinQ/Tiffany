use super::state::{ChatMsg, ContextMode, InputState};
use crate::core::session_store::SessionStore;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LAST_SESSION_FILE: &str = "last.json";

#[derive(Serialize, Deserialize)]
struct PersistedTuiSession {
    version: u32,
    saved_at_unix: u64,
    transcript: Vec<PersistedChatMsg>,
    queued_prompts: Vec<String>,
    #[serde(default)]
    queue_paused: bool,
    history_folded: bool,
    trace_live_enabled: bool,
    trace_expanded: bool,
    agent_hint: Option<String>,
    context_mode: PersistedContextMode,
    context_cutoff: usize,
    #[serde(default)]
    context_summary_text: String,
    #[serde(default)]
    context_summary_upto: usize,
    last_context_messages: usize,
    last_context_chars: usize,
    last_prompt: Option<String>,
    #[serde(default)]
    last_result_output: Option<String>,
    #[serde(default)]
    last_handoff_path: Option<PathBuf>,
    #[serde(default)]
    last_checkpoint_path: Option<PathBuf>,
    #[serde(default)]
    test_log_path: Option<PathBuf>,
    #[serde(default)]
    last_test_status: Option<String>,
    input_history: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct PersistedChatMsg {
    role: String,
    content: String,
    ts_unix: u64,
    status: String,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedContextMode {
    Off,
    Compact,
    Full,
}

pub(super) fn save_tui_session(store: &SessionStore, input: &InputState) -> Result<PathBuf> {
    let path = last_session_path(store)?;
    let snapshot = PersistedTuiSession::from_input(input);
    let body = serde_json::to_string_pretty(&snapshot)?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub(super) fn load_last_tui_session(store: &SessionStore) -> Result<InputState> {
    let path = last_session_path(store)?;
    let body =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let snapshot: PersistedTuiSession =
        serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    Ok(snapshot.into_input())
}

pub(super) fn last_tui_session_path(store: &SessionStore) -> Result<PathBuf> {
    last_session_path(store)
}

fn last_session_path(store: &SessionStore) -> Result<PathBuf> {
    let dir = store.tui_state_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join(LAST_SESSION_FILE))
}

impl PersistedTuiSession {
    fn from_input(input: &InputState) -> Self {
        Self {
            version: 1,
            saved_at_unix: system_time_to_unix(SystemTime::now()),
            transcript: input
                .transcript
                .iter()
                .map(PersistedChatMsg::from_chat_msg)
                .collect(),
            queued_prompts: input.queued_prompts.clone(),
            queue_paused: input.queue_paused,
            history_folded: input.history_folded,
            trace_live_enabled: input.trace_live_enabled,
            trace_expanded: input.trace_expanded,
            agent_hint: input.agent_hint.clone(),
            context_mode: PersistedContextMode::from(input.context_mode),
            context_cutoff: input.context_cutoff.min(input.transcript.len()),
            context_summary_text: input.context_summary_text.clone(),
            context_summary_upto: input.context_summary_upto.min(input.transcript.len()),
            last_context_messages: input.last_context_messages,
            last_context_chars: input.last_context_chars,
            last_prompt: input.last_prompt.clone(),
            last_result_output: input.last_result_output.clone(),
            last_handoff_path: input.last_handoff_path.clone(),
            last_checkpoint_path: input.last_checkpoint_path.clone(),
            test_log_path: input.test_log_path.clone(),
            last_test_status: input.last_test_status.clone(),
            input_history: input.input_history.clone(),
        }
    }

    fn into_input(self) -> InputState {
        let mut input = InputState {
            trace_live_enabled: self.trace_live_enabled,
            trace_expanded: self.trace_expanded,
            history_folded: self.history_folded,
            agent_hint: self.agent_hint,
            context_mode: self.context_mode.into(),
            context_cutoff: self.context_cutoff,
            context_summary_text: self.context_summary_text,
            context_summary_upto: self.context_summary_upto,
            last_context_messages: self.last_context_messages,
            last_context_chars: self.last_context_chars,
            queued_prompts: self.queued_prompts,
            queue_paused: self.queue_paused,
            last_prompt: self.last_prompt,
            last_result_output: self.last_result_output,
            last_handoff_path: self.last_handoff_path,
            last_checkpoint_path: self.last_checkpoint_path,
            test_log_path: self.test_log_path,
            last_test_status: self.last_test_status,
            input_history: self.input_history,
            ..InputState::default()
        };
        input.transcript = self
            .transcript
            .into_iter()
            .map(PersistedChatMsg::into_chat_msg)
            .collect();
        input.context_cutoff = input.context_cutoff.min(input.transcript.len());
        input.context_summary_upto = input
            .context_summary_upto
            .max(input.context_cutoff)
            .min(input.transcript.len());
        input.trace_message_index = input.transcript.iter().rposition(|msg| msg.role == "trace");
        input
    }
}

impl PersistedChatMsg {
    fn from_chat_msg(msg: &ChatMsg) -> Self {
        Self {
            role: msg.role.clone(),
            content: msg.content.clone(),
            ts_unix: system_time_to_unix(msg.ts),
            status: if msg.status == "thinking" {
                "complete".into()
            } else {
                msg.status.clone()
            },
        }
    }

    fn into_chat_msg(self) -> ChatMsg {
        ChatMsg {
            role: self.role,
            content: self.content,
            ts: UNIX_EPOCH + Duration::from_secs(self.ts_unix),
            status: self.status,
        }
    }
}

impl From<ContextMode> for PersistedContextMode {
    fn from(value: ContextMode) -> Self {
        match value {
            ContextMode::Off => Self::Off,
            ContextMode::Compact => Self::Compact,
            ContextMode::Full => Self::Full,
        }
    }
}

impl From<PersistedContextMode> for ContextMode {
    fn from(value: PersistedContextMode) -> Self {
        match value {
            PersistedContextMode::Off => Self::Off,
            PersistedContextMode::Compact => Self::Compact,
            PersistedContextMode::Full => Self::Full,
        }
    }
}

fn system_time_to_unix(ts: SystemTime) -> u64 {
    ts.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads_tui_session_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let mut input = InputState {
            context_mode: ContextMode::Full,
            context_cutoff: 1,
            context_summary_text: "earlier summary".into(),
            context_summary_upto: 1,
            last_prompt: Some("continue".into()),
            last_result_output: Some("done text".into()),
            last_handoff_path: Some(tmp.path().join("handoff.md")),
            last_checkpoint_path: Some(tmp.path().join("checkpoint.patch")),
            test_log_path: Some(tmp.path().join("test.log")),
            last_test_status: Some("finished: ok".into()),
            agent_hint: Some("worker-cc".into()),
            queue_paused: true,
            ..InputState::default()
        };
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "hello".into(),
            ts: SystemTime::now(),
            status: "complete".into(),
        });
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "thinking...".into(),
            ts: SystemTime::now(),
            status: "thinking".into(),
        });
        input.queued_prompts.push("next".into());

        let path = save_tui_session(&store, &input).unwrap();
        let loaded = load_last_tui_session(&store).unwrap();

        assert!(path.ends_with("last.json"));
        assert_eq!(loaded.transcript.len(), 2);
        assert_eq!(loaded.transcript[1].status, "complete");
        assert_eq!(loaded.context_mode, ContextMode::Full);
        assert_eq!(loaded.context_cutoff, 1);
        assert_eq!(loaded.context_summary_text, "earlier summary");
        assert_eq!(loaded.context_summary_upto, 1);
        assert_eq!(loaded.queued_prompts, vec!["next"]);
        assert!(loaded.queue_paused);
        assert_eq!(loaded.last_prompt.as_deref(), Some("continue"));
        assert_eq!(loaded.last_result_output.as_deref(), Some("done text"));
        assert_eq!(
            loaded.last_handoff_path.as_deref(),
            Some(tmp.path().join("handoff.md").as_path())
        );
        assert_eq!(
            loaded.last_checkpoint_path.as_deref(),
            Some(tmp.path().join("checkpoint.patch").as_path())
        );
        assert_eq!(
            loaded.test_log_path.as_deref(),
            Some(tmp.path().join("test.log").as_path())
        );
        assert_eq!(loaded.last_test_status.as_deref(), Some("finished: ok"));
        assert_eq!(loaded.agent_hint.as_deref(), Some("worker-cc"));
    }
}
