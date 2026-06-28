use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct NativeHistoryOptions {
    pub turn_limit: usize,
    pub event_limit_per_turn: usize,
    pub full_event_content: bool,
    pub filter: Option<NativeHistoryFilter>,
}

#[derive(Clone, Debug)]
pub enum NativeHistoryCommand {
    Approvals {
        turn_limit: usize,
    },
    Show(NativeHistoryOptions),
    Export {
        out: Option<PathBuf>,
        filter: Option<NativeHistoryFilter>,
    },
    Graph {
        mermaid: bool,
        out: Option<PathBuf>,
        export: bool,
    },
    Compact {
        turn_limit: usize,
        filter: Option<NativeHistoryFilter>,
    },
    Sync,
    Status,
    Search {
        pattern: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeHistoryFilter {
    Role(String),
    Thread(String),
    NativeSession(String),
    Kind(String),
}

#[derive(Clone, Copy, Debug)]
pub struct NativeHistoryEventView<'a> {
    pub role: &'a str,
    pub agent: Option<&'a str>,
    pub worker_role: Option<&'a str>,
    pub worker_thread_id: Option<&'a str>,
    pub native_session_id: Option<&'a str>,
    pub kind: Option<&'a str>,
}

impl NativeHistoryFilter {
    pub fn display(&self) -> String {
        match self {
            Self::Role(role) => format!("role {role}"),
            Self::Thread(thread) => format!("thread {thread}"),
            Self::NativeSession(native) => format!("native {native}"),
            Self::Kind(kind) => format!("kind {kind}"),
        }
    }

    pub fn matches_event(&self, event: NativeHistoryEventView<'_>) -> bool {
        match self {
            Self::Role(role) => {
                event.worker_role == Some(role.as_str())
                    || event.agent == Some(role.as_str())
                    || event.role == role
            }
            Self::Thread(thread) => event
                .worker_thread_id
                .is_some_and(|id| id == thread || id.starts_with(thread)),
            Self::NativeSession(native) => event
                .native_session_id
                .is_some_and(|id| id == native || id.starts_with(native)),
            Self::Kind(kind) => event
                .kind
                .is_some_and(|event_kind| native_history_kind_matches(event_kind, kind)),
        }
    }
}

pub fn native_history_kind_matches(event_kind: &str, filter: &str) -> bool {
    event_kind.eq_ignore_ascii_case(filter) || event_kind.starts_with(filter) || {
        let event_kind = tiffany_event_format::normalize_event_kind(event_kind);
        let filter = tiffany_event_format::normalize_event_kind(filter);
        let event_visible = event_kind
            .as_deref()
            .and_then(tiffany_event_format::visible_agent_output_kind_for_event_kind);
        let filter_visible = filter
            .as_deref()
            .and_then(tiffany_event_format::visible_agent_output_kind_for_event_kind);
        matches!(
            (
                event_kind.as_deref(),
                filter.as_deref(),
                event_visible,
                filter_visible
            ),
            (Some(event_kind), Some(filter), _, _)
                if event_kind.eq_ignore_ascii_case(filter) || event_kind.starts_with(filter)
        ) || matches!(
            (event_visible, filter_visible),
            (Some(event_visible), Some(filter_visible)) if event_visible == filter_visible
        )
    }
}

impl NativeHistoryCommand {
    pub fn parse(args: &str) -> Result<Self, String> {
        let parts = args.split_whitespace().collect::<Vec<_>>();
        match parts.as_slice() {
            ["approvals"] | ["approval"] => Ok(Self::Approvals { turn_limit: 20 }),
            ["approvals" | "approval", "--turns" | "-n", value] => {
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "approvals --turns needs a number".to_string())?;
                if parsed == 0 {
                    return Err("approvals --turns must be greater than zero".to_string());
                }
                Ok(Self::Approvals {
                    turn_limit: parsed.min(50),
                })
            }
            ["approvals" | "approval", ..] => Err("Usage: /approvals [--turns <n>]".to_string()),
            [] | ["last"] | ["summary"] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 5,
                event_limit_per_turn: 6,
                full_event_content: false,
                filter: None,
            })),
            ["full"] | ["all"] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 10,
                event_limit_per_turn: usize::MAX,
                full_event_content: true,
                filter: None,
            })),
            [count] if count.parse::<usize>().is_ok() => {
                let count = count.parse::<usize>().unwrap_or_default();
                if count == 0 {
                    return Err("history count must be greater than zero".to_string());
                }
                Ok(Self::Show(NativeHistoryOptions {
                    turn_limit: count.min(50),
                    event_limit_per_turn: 10,
                    full_event_content: false,
                    filter: None,
                }))
            }
            ["role", role] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 20,
                event_limit_per_turn: usize::MAX,
                full_event_content: true,
                filter: Some(NativeHistoryFilter::Role(role.to_string())),
            })),
            ["role", ..] => Err("history role needs <role>".to_string()),
            ["thread", thread] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 20,
                event_limit_per_turn: usize::MAX,
                full_event_content: true,
                filter: Some(NativeHistoryFilter::Thread(thread.to_string())),
            })),
            ["thread", ..] => Err("history thread needs <id>".to_string()),
            ["native" | "session", native] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 20,
                event_limit_per_turn: usize::MAX,
                full_event_content: true,
                filter: Some(NativeHistoryFilter::NativeSession(native.to_string())),
            })),
            ["native" | "session", ..] => Err("history native needs <session-id>".to_string()),
            ["kind", kind] => Ok(Self::Show(NativeHistoryOptions {
                turn_limit: 20,
                event_limit_per_turn: usize::MAX,
                full_event_content: true,
                filter: Some(NativeHistoryFilter::Kind(kind.to_string())),
            })),
            ["kind", ..] => Err("history kind needs <event-kind>".to_string()),
            ["export", rest @ ..] => parse_native_history_export_command(rest),
            ["compact" | "story" | "summary", rest @ ..] => {
                parse_native_history_compact_command(rest)
            }
            ["sync" | "import" | "import-native"] => Ok(Self::Sync),
            ["sync" | "import" | "import-native", ..] => {
                Err("history sync does not accept extra arguments".to_string())
            }
            ["status" | "db" | "sources"] => Ok(Self::Status),
            ["status" | "db" | "sources", ..] => {
                Err("history status does not accept extra arguments".to_string())
            }
            ["graph" | "flow"] => Ok(Self::Graph {
                mermaid: false,
                out: None,
                export: false,
            }),
            ["mermaid" | "diagram"] => Ok(Self::Graph {
                mermaid: true,
                out: None,
                export: false,
            }),
            ["export-graph", rest @ ..] | ["save-graph", rest @ ..] => {
                parse_native_history_graph_command(rest)
            }
            ["search" | "grep" | "find", pattern @ ..] => {
                let pattern = pattern.join(" ").trim().to_string();
                if pattern.is_empty() {
                    return Err("history search needs text".to_string());
                }
                Ok(Self::Search { pattern })
            }
            [unknown, ..] => Err(format!("unknown /history option '{unknown}'")),
        }
    }
}

fn parse_native_history_compact_command(parts: &[&str]) -> Result<NativeHistoryCommand, String> {
    let mut turn_limit = 12;
    let mut filter = None;
    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "--turns" | "-n" => {
                let Some(value) = parts.get(i + 1) else {
                    return Err("history compact --turns needs a number".to_string());
                };
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "history compact --turns needs a number".to_string())?;
                if parsed == 0 {
                    return Err("history compact --turns must be greater than zero".to_string());
                }
                turn_limit = parsed.min(50);
                i += 2;
            }
            "role" => {
                let Some(role) = parts.get(i + 1) else {
                    return Err("history compact role needs <role>".to_string());
                };
                filter = Some(NativeHistoryFilter::Role((*role).to_string()));
                i += 2;
            }
            "thread" => {
                let Some(thread) = parts.get(i + 1) else {
                    return Err("history compact thread needs <id>".to_string());
                };
                filter = Some(NativeHistoryFilter::Thread((*thread).to_string()));
                i += 2;
            }
            "native" | "session" => {
                let Some(native) = parts.get(i + 1) else {
                    return Err("history compact native needs <session-id>".to_string());
                };
                filter = Some(NativeHistoryFilter::NativeSession((*native).to_string()));
                i += 2;
            }
            "kind" => {
                let Some(kind) = parts.get(i + 1) else {
                    return Err("history compact kind needs <event-kind>".to_string());
                };
                filter = Some(NativeHistoryFilter::Kind((*kind).to_string()));
                i += 2;
            }
            value => return Err(format!("unknown /history compact option '{value}'")),
        }
    }
    Ok(NativeHistoryCommand::Compact { turn_limit, filter })
}

fn parse_native_history_export_command(parts: &[&str]) -> Result<NativeHistoryCommand, String> {
    let mut out = None;
    let mut filter = None;
    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "--out" | "-o" => {
                let Some(path) = parts.get(i + 1) else {
                    return Err("history export --out needs a file path".to_string());
                };
                out = Some(PathBuf::from(path));
                i += 2;
            }
            "role" => {
                let Some(role) = parts.get(i + 1) else {
                    return Err("history export role needs <role>".to_string());
                };
                filter = Some(NativeHistoryFilter::Role((*role).to_string()));
                i += 2;
            }
            "thread" => {
                let Some(thread) = parts.get(i + 1) else {
                    return Err("history export thread needs <id>".to_string());
                };
                filter = Some(NativeHistoryFilter::Thread((*thread).to_string()));
                i += 2;
            }
            "native" | "session" => {
                let Some(native) = parts.get(i + 1) else {
                    return Err("history export native needs <session-id>".to_string());
                };
                filter = Some(NativeHistoryFilter::NativeSession((*native).to_string()));
                i += 2;
            }
            "kind" => {
                let Some(kind) = parts.get(i + 1) else {
                    return Err("history export kind needs <event-kind>".to_string());
                };
                filter = Some(NativeHistoryFilter::Kind((*kind).to_string()));
                i += 2;
            }
            value => {
                return Err(format!("unknown /history export option '{value}'"));
            }
        }
    }
    Ok(NativeHistoryCommand::Export { out, filter })
}

fn parse_native_history_graph_command(parts: &[&str]) -> Result<NativeHistoryCommand, String> {
    let mut out = None;
    let mut mermaid = true;
    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "--out" | "-o" => {
                let Some(path) = parts.get(i + 1) else {
                    return Err("history export-graph --out needs a file path".to_string());
                };
                out = Some(PathBuf::from(path));
                i += 2;
            }
            "--text" | "text" => {
                mermaid = false;
                i += 1;
            }
            "--mermaid" | "mermaid" => {
                mermaid = true;
                i += 1;
            }
            value => return Err(format!("unknown /history export-graph option '{value}'")),
        }
    }
    Ok(NativeHistoryCommand::Graph {
        mermaid,
        out,
        export: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sync_and_status_without_extra_args() {
        assert!(matches!(
            NativeHistoryCommand::parse("sync").expect("history sync"),
            NativeHistoryCommand::Sync
        ));
        assert!(NativeHistoryCommand::parse("sync now").is_err());
        assert!(matches!(
            NativeHistoryCommand::parse("status").expect("history status"),
            NativeHistoryCommand::Status
        ));
        assert!(NativeHistoryCommand::parse("status now").is_err());
    }

    #[test]
    fn parses_show_modes_and_filters() {
        assert!(matches!(
            NativeHistoryCommand::parse("").expect("default history"),
            NativeHistoryCommand::Show(NativeHistoryOptions {
                turn_limit: 5,
                event_limit_per_turn: 6,
                full_event_content: false,
                filter: None,
            })
        ));
        assert!(matches!(
            NativeHistoryCommand::parse("full").expect("full history"),
            NativeHistoryCommand::Show(NativeHistoryOptions {
                turn_limit: 10,
                event_limit_per_turn: usize::MAX,
                full_event_content: true,
                filter: None,
            })
        ));
        assert!(matches!(
            NativeHistoryCommand::parse("role worker-cc").expect("role filter"),
            NativeHistoryCommand::Show(NativeHistoryOptions {
                filter: Some(NativeHistoryFilter::Role(role)),
                ..
            }) if role == "worker-cc"
        ));
        assert!(matches!(
            NativeHistoryCommand::parse("kind permission").expect("kind filter"),
            NativeHistoryCommand::Show(NativeHistoryOptions {
                filter: Some(NativeHistoryFilter::Kind(kind)),
                ..
            }) if kind == "permission"
        ));
    }

    #[test]
    fn parses_compact_export_graph_and_search() {
        assert!(matches!(
            NativeHistoryCommand::parse("compact --turns 7 kind diff").expect("compact"),
            NativeHistoryCommand::Compact {
                turn_limit: 7,
                filter: Some(NativeHistoryFilter::Kind(kind)),
            } if kind == "diff"
        ));
        assert!(matches!(
            NativeHistoryCommand::parse("export --out /tmp/history.md role worker-cc")
                .expect("export"),
            NativeHistoryCommand::Export {
                out: Some(path),
                filter: Some(NativeHistoryFilter::Role(role)),
            } if path == PathBuf::from("/tmp/history.md") && role == "worker-cc"
        ));
        assert!(matches!(
            NativeHistoryCommand::parse("export-graph --out /tmp/history.mmd").expect("graph"),
            NativeHistoryCommand::Graph {
                mermaid: true,
                out: Some(path),
                export: true,
            } if path == PathBuf::from("/tmp/history.mmd")
        ));
        assert!(matches!(
            NativeHistoryCommand::parse("search login failed").expect("search"),
            NativeHistoryCommand::Search { pattern } if pattern == "login failed"
        ));
    }

    #[test]
    fn filter_matches_event_views_and_kind_aliases() {
        let event = NativeHistoryEventView {
            role: "worker",
            agent: Some("claude-code"),
            worker_role: Some("worker-cc"),
            worker_thread_id: Some("abcdef12-0000"),
            native_session_id: Some("native-cc"),
            kind: Some("approval"),
        };

        assert!(NativeHistoryFilter::Role("worker-cc".into()).matches_event(event));
        assert!(NativeHistoryFilter::Thread("abcdef12".into()).matches_event(event));
        assert!(NativeHistoryFilter::NativeSession("native".into()).matches_event(event));
        assert!(NativeHistoryFilter::Kind("permission".into()).matches_event(event));
        assert!(!NativeHistoryFilter::Role("worker-codex".into()).matches_event(event));
    }
}
