use serde::Deserialize;
use serde::Serialize;

use crate::visible_output::VisibleOutputEvent;
use crate::visible_output::visible_output_kind_for_event;

pub const NATIVE_CHAT_STORE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NativeChatStore {
    #[serde(default = "native_chat_store_schema_version")]
    pub version: u32,
    #[serde(default)]
    pub conversations: Vec<NativeChatConversation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NativeChatConversation {
    pub id: String,
    pub cwd: String,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    #[serde(default)]
    pub turns: Vec<NativeChatTurn>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NativeChatTurn {
    pub user_prompt: String,
    pub result: String,
    pub captured_at_unix: u64,
    #[serde(default)]
    pub events: Vec<NativeChatEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NativeChatEvent {
    pub role: String,
    pub status: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub content: Option<String>,
    pub agent: Option<String>,
    pub worker_role: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub task_id: Option<String>,
    #[serde(default)]
    pub worker_thread_id: Option<String>,
    pub native_session_id: Option<String>,
}

pub fn native_chat_store_schema_version() -> u32 {
    NATIVE_CHAT_STORE_SCHEMA_VERSION
}

pub fn normalize_native_chat_store(
    store: NativeChatStore,
    visible_content_max_chars: usize,
) -> NativeChatStore {
    NativeChatStore {
        version: NATIVE_CHAT_STORE_SCHEMA_VERSION,
        conversations: store
            .conversations
            .into_iter()
            .filter_map(|conversation| {
                normalize_native_conversation(conversation, visible_content_max_chars)
            })
            .collect(),
    }
}

pub fn normalize_native_conversation(
    conversation: NativeChatConversation,
    visible_content_max_chars: usize,
) -> Option<NativeChatConversation> {
    let cwd = conversation.cwd.trim().to_string();
    if cwd.is_empty() {
        return None;
    }

    let turns = conversation
        .turns
        .into_iter()
        .filter_map(|turn| normalize_native_turn(turn, visible_content_max_chars))
        .collect::<Vec<_>>();
    if turns.is_empty() {
        return None;
    }

    let id = if conversation.id.trim().is_empty() {
        native_conversation_id(&cwd)
    } else {
        conversation.id.trim().to_string()
    };
    let first_turn_ts = turns
        .first()
        .map(|turn| turn.captured_at_unix)
        .unwrap_or_default();
    let last_turn_ts = turns
        .last()
        .map(|turn| turn.captured_at_unix)
        .unwrap_or(first_turn_ts);
    let created_at_unix = if conversation.created_at_unix == 0 {
        first_turn_ts
    } else {
        conversation.created_at_unix
    };
    let updated_at_unix = conversation.updated_at_unix.max(last_turn_ts);

    Some(NativeChatConversation {
        id,
        cwd,
        created_at_unix,
        updated_at_unix,
        turns,
    })
}

pub fn normalize_native_turn(
    turn: NativeChatTurn,
    visible_content_max_chars: usize,
) -> Option<NativeChatTurn> {
    let user_prompt = turn.user_prompt.trim().to_string();
    let result = turn.result.trim().to_string();
    if user_prompt.is_empty() || result.is_empty() {
        return None;
    }
    Some(NativeChatTurn {
        user_prompt,
        result,
        captured_at_unix: turn.captured_at_unix,
        events: turn
            .events
            .into_iter()
            .filter_map(|event| normalize_native_event(event, visible_content_max_chars))
            .collect(),
    })
}

pub fn normalize_native_event(
    event: NativeChatEvent,
    visible_content_max_chars: usize,
) -> Option<NativeChatEvent> {
    let role = event.role.trim().to_string();
    let status = event.status.trim().to_string();
    let title = event.title.trim().to_string();
    if role.is_empty() || status.is_empty() || title.is_empty() {
        return None;
    }
    let kind = event.kind.and_then(trimmed_string).or_else(|| {
        inferred_native_event_kind(&title, event.content.as_deref(), visible_content_max_chars)
    });
    Some(NativeChatEvent {
        role,
        status,
        title,
        kind,
        content: event.content.and_then(trimmed_string),
        agent: event.agent.and_then(trimmed_string),
        worker_role: event.worker_role.and_then(trimmed_string),
        model: event.model.and_then(trimmed_string),
        provider: event.provider.and_then(trimmed_string),
        task_id: event.task_id.and_then(trimmed_string),
        worker_thread_id: event.worker_thread_id.and_then(trimmed_string),
        native_session_id: event.native_session_id.and_then(trimmed_string),
    })
}

pub fn native_conversation_id(cwd_key: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in cwd_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("tiffany-native-{hash:016x}")
}

fn inferred_native_event_kind(
    title: &str,
    content: Option<&str>,
    visible_content_max_chars: usize,
) -> Option<String> {
    let title = title.to_ascii_lowercase();
    for (needle, kind) in [
        ("worker final", "final"),
        ("worker answer", "answer"),
        ("worker question", "question"),
        ("worker approval", "approval"),
        ("worker tool call", "tool_call"),
        ("worker tool result", "tool_result"),
        ("worker diff", "diff"),
        ("worker patch", "patch"),
        ("worker file update", "file_update"),
        ("worker stderr", "stderr"),
        ("worker alert", "alert"),
        ("worker session recovery", "session_recovery"),
    ] {
        if title.contains(needle) {
            return Some(kind.to_string());
        }
    }
    content
        .and_then(|raw| {
            visible_output_kind_for_event(
                &VisibleOutputEvent {
                    role: "worker",
                    status: "output",
                    agent: None,
                    worker_role: None,
                    task_id: None,
                    event_kind: None,
                    recovery: None,
                    content: Some(raw),
                },
                raw,
                visible_content_max_chars,
            )
        })
        .map(native_event_kind_label)
        .map(str::to_string)
}

fn native_event_kind_label(kind: tiffany_event_format::VisibleAgentOutputKind) -> &'static str {
    match kind {
        tiffany_event_format::VisibleAgentOutputKind::Final => "final",
        tiffany_event_format::VisibleAgentOutputKind::Answer => "answer",
        tiffany_event_format::VisibleAgentOutputKind::Question => "question",
        tiffany_event_format::VisibleAgentOutputKind::Approval => "approval",
        tiffany_event_format::VisibleAgentOutputKind::ToolCall => "tool_call",
        tiffany_event_format::VisibleAgentOutputKind::ToolResult => "tool_result",
        tiffany_event_format::VisibleAgentOutputKind::Diff => "diff",
        tiffany_event_format::VisibleAgentOutputKind::Patch => "patch",
        tiffany_event_format::VisibleAgentOutputKind::FileUpdate => "file_update",
        tiffany_event_format::VisibleAgentOutputKind::Stderr => "stderr",
        tiffany_event_format::VisibleAgentOutputKind::Actionable => "alert",
        tiffany_event_format::VisibleAgentOutputKind::Normal => "output",
    }
}

fn trimmed_string(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_native_chat_store_and_infers_event_kind() {
        let store = NativeChatStore {
            version: 0,
            conversations: vec![
                NativeChatConversation {
                    id: " ".to_string(),
                    cwd: " /tmp/project ".to_string(),
                    created_at_unix: 0,
                    updated_at_unix: 1,
                    turns: vec![NativeChatTurn {
                        user_prompt: "  hello ".to_string(),
                        result: " done ".to_string(),
                        captured_at_unix: 10,
                        events: vec![
                            NativeChatEvent {
                                role: " worker ".to_string(),
                                status: " output ".to_string(),
                                title: "worker tool result · worker-cc".to_string(),
                                kind: None,
                                content: Some(" tool output ".to_string()),
                                agent: Some(" claude-code ".to_string()),
                                worker_role: Some(" worker-cc ".to_string()),
                                model: None,
                                provider: None,
                                task_id: None,
                                worker_thread_id: Some(" thread-1 ".to_string()),
                                native_session_id: Some(" native-1 ".to_string()),
                            },
                            NativeChatEvent {
                                role: " ".to_string(),
                                status: "output".to_string(),
                                title: "bad".to_string(),
                                kind: None,
                                content: None,
                                agent: None,
                                worker_role: None,
                                model: None,
                                provider: None,
                                task_id: None,
                                worker_thread_id: None,
                                native_session_id: None,
                            },
                        ],
                    }],
                },
                NativeChatConversation {
                    id: "empty".to_string(),
                    cwd: "/tmp/empty".to_string(),
                    created_at_unix: 0,
                    updated_at_unix: 0,
                    turns: Vec::new(),
                },
            ],
        };

        let store = normalize_native_chat_store(store, 240_000);

        assert_eq!(store.version, NATIVE_CHAT_STORE_SCHEMA_VERSION);
        assert_eq!(store.conversations.len(), 1);
        let conversation = &store.conversations[0];
        assert_eq!(conversation.id, native_conversation_id("/tmp/project"));
        assert_eq!(conversation.cwd, "/tmp/project");
        assert_eq!(conversation.created_at_unix, 10);
        assert_eq!(conversation.updated_at_unix, 10);
        assert_eq!(conversation.turns[0].user_prompt, "hello");
        assert_eq!(conversation.turns[0].result, "done");
        assert_eq!(conversation.turns[0].events.len(), 1);
        let event = &conversation.turns[0].events[0];
        assert_eq!(event.role, "worker");
        assert_eq!(event.status, "output");
        assert_eq!(event.kind.as_deref(), Some("tool_result"));
        assert_eq!(event.content.as_deref(), Some("tool output"));
        assert_eq!(event.worker_role.as_deref(), Some("worker-cc"));
        assert_eq!(event.worker_thread_id.as_deref(), Some("thread-1"));
        assert_eq!(event.native_session_id.as_deref(), Some("native-1"));
    }
}
