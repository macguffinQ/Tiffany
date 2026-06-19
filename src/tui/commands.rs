use super::context::{context_summary, handle_context_command};
use super::process::{
    complete_live_trace, export_process_capture, format_process_capture, format_process_summary,
    live_trace_status, live_trace_summary, process_filter_summary, refresh_live_trace,
};
use super::run_state::format_final_result;
use super::state::{ChatMsg, InputState, TuiRuntimeConfig};
use super::util::{
    copy_to_clipboard, humanize_jsonish, is_low_value_execution_output, truncate_chars,
};
use crate::config::{Config, RoleConfig};
use crate::core::session_store::SessionStore;
use crate::core::types::Session;
use crate::runtime::{self, AGENT_RUNTIME_ROUTES};
use crate::usage::{compute_for_window, UsageWindow};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

pub(super) enum SlashAction {
    None,
    RunPrompt(String),
    RunQueuedNow,
    Exit,
}

#[cfg(test)]
pub(super) fn handle_slash_command(
    input_line: &str,
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
) -> SlashAction {
    handle_slash_command_with_runtime(
        input_line,
        store,
        config,
        &TuiRuntimeConfig::default(),
        input,
    )
}

pub(super) fn handle_slash_command_with_runtime(
    input_line: &str,
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    runtime: &TuiRuntimeConfig,
    input: &mut InputState,
) -> SlashAction {
    let raw = input_line.trim_start_matches('/').trim();
    let mut parts = raw.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();

    match cmd {
        "" | "help" | "h" | "?" | "commands" => {
            push_system(input, help_text());
        }
        "clear" | "c" | "new" => {
            let active_run = input.run_rx.is_some() || input.run_handle.is_some();
            input.transcript.clear();
            input.trace_message_index = None;
            input.context_cutoff = 0;
            input.context_summary_text.clear();
            input.context_summary_upto = 0;
            input.last_result_output = None;
            input.last_context_messages = 0;
            input.last_context_chars = 0;
            if active_run {
                refresh_live_trace(input, false);
            }
        }
        "compact" => {
            let keep = parse_count(args.first().copied(), 20, 1, 200);
            let before = input.transcript.len();
            if before > keep {
                let dropped = before - keep;
                let kept = input.transcript.split_off(before - keep);
                input.transcript = kept;
                input.context_cutoff = input
                    .context_cutoff
                    .saturating_sub(dropped)
                    .min(input.transcript.len());
                input.context_summary_upto = input
                    .context_summary_upto
                    .saturating_sub(dropped)
                    .min(input.transcript.len());
                if input.context_summary_upto == 0 {
                    input.context_summary_text.clear();
                }
            }
            input.trace_message_index =
                input.transcript.iter().rposition(|msg| msg.role == "trace");
            push_system(
                input,
                format!(
                    "Compacted transcript: kept {} of {} message(s).",
                    input.transcript.len(),
                    before
                ),
            );
        }
        "o" | "outline" | "fold" => {
            input.history_folded = !input.history_folded;
            let msg = if input.history_folded {
                "Process view folded. Future agent output is compact; use /o to expand details."
            } else {
                "Process view expanded. Future agent output shows more gray detail; use /o to fold."
            };
            push_system(input, msg.into());
        }
        "copy" | "save" => {
            if args.first().copied() == Some("result") {
                push_system(input, copy_last_result(input));
                return SlashAction::None;
            }
            let target = if cmd == "save" {
                "file"
            } else {
                args.first().copied().unwrap_or("file")
            };
            push_system(input, export_transcript(&input.transcript, target));
        }
        "result" | "final" => {
            push_system(input, format_last_result(input, args.first().copied()));
        }
        "status" | "st" | "s" if args.is_empty() => {
            push_system(input, format_tui_status(store, config, input));
        }
        "doctor" | "diagnose" | "checkup" => {
            push_system(input, format_doctor_command(runtime));
        }
        "model" | "models" => {
            push_system(input, format_model_command(config, &args));
        }
        "roles" | "role" => {
            let msg = handle_roles_command(config, runtime, input, &args);
            push_system(input, msg);
        }
        "workflow" | "flow-status" | "pipeline" => {
            push_system(input, format_workflow_status(config, input));
        }
        "agent" | "agents" => {
            let msg = handle_agent_command(config, input, &args);
            push_system(input, msg);
        }
        "usage" | "u" => {
            let window = args.first().copied().unwrap_or("today");
            push_system(input, format_usage(store, window));
        }
        "sessions" | "history" => {
            let limit = parse_count(args.first().copied(), 10, 1, 50);
            push_system(input, format_sessions(store, limit));
        }
        "import-cc" | "import-claude" => {
            let project = args.first().copied().map(PathBuf::from);
            handle_import_cc(store, project.as_deref(), input);
        }
        "resume" => {
            push_system(
                input,
                "Use /resume last in the terminal chat to restore the last terminal chat conversation."
                    .into(),
            );
        }
        "editor" | "edit" | "e" => {
            push_system(
                input,
                "Use /editor in the terminal chat to compose a longer prompt in $VISUAL or $EDITOR."
                    .into(),
            );
        }
        "session" | "show" | "s" => {
            if args.first().copied() == Some("tree") {
                push_system(
                    input,
                    format_session_tree_command(store, args.get(1).copied()),
                );
            } else {
                let selector = args.first().copied();
                push_system(input, format_session_detail(store, selector));
            }
        }
        "tree" | "session-tree" => {
            let selector = args.first().copied();
            push_system(input, format_session_tree_command(store, selector));
        }
        "log" | "tail" => {
            let (selector, lines) = parse_log_args(&args);
            push_system(input, format_log_tail(store, selector, lines));
        }
        "trace" => {
            let msg = handle_trace_command(input, &args);
            push_system(input, msg);
        }
        "context" | "ctx" => {
            let msg = handle_context_command(input, &args);
            push_system(input, msg);
        }
        "process" => {
            let msg = match args.first().copied() {
                Some("filter") => {
                    let filter = args.get(1..).unwrap_or(&[]).join(" ");
                    if filter.trim().is_empty() {
                        input.process_filter = None;
                        "Process filter cleared.".into()
                    } else {
                        input.process_filter = Some(filter.trim().to_string());
                        format!("Process filter set: {}", process_filter_summary(input))
                    }
                }
                Some("clear-filter") | Some("all") => {
                    input.process_filter = None;
                    "Process filter cleared.".into()
                }
                Some("save") | Some("file") => export_process_capture(input, "file"),
                Some("clipboard") | Some("cb") => export_process_capture(input, "clipboard"),
                Some("summary") | None => format_process_summary(input),
                Some("full") => format_process_capture(input, 500),
                other => {
                    let limit = parse_count(other, 80, 1, 500);
                    format_process_capture(input, limit)
                }
            };
            push_system(input, msg);
        }
        "diff" | "changes" => {
            push_system(input, format_diff_command(&args));
        }
        "tests" | "test" => {
            let msg = handle_tests_command(input, &args);
            push_system(input, msg);
        }
        "handoff" | "continue" => {
            let msg = handle_handoff_command(store, config, input, &args);
            push_system(input, msg);
        }
        "graph" | "flow" => {
            push_system(input, format_graph_command(input, &args));
        }
        "acp" => {
            push_system(input, format_acp_command(config, input, &args));
        }
        "checkpoint" | "cp" => {
            let msg = handle_checkpoint_command(input, &args);
            push_system(input, msg);
        }
        "rollback" | "revert" => {
            let msg = handle_rollback_command(input, &args);
            push_system(input, msg);
        }
        "export" => {
            let what = args.first().copied().unwrap_or("transcript");
            let target = args.get(1).copied().unwrap_or("file");
            let msg = match what {
                "process" | "trace" => export_process_capture(input, target),
                "transcript" | "chat" => export_transcript(&input.transcript, target),
                _ => "Usage: /export process|transcript [file|clipboard]".into(),
            };
            push_system(input, msg);
        }
        "grep" | "search" => {
            let pattern = args.join(" ");
            if pattern.is_empty() {
                push_system(input, "Usage: /grep <text>".into());
            } else {
                push_system(input, format_grep(store, &pattern, 20));
            }
        }
        "queue" => {
            let msg = handle_queue_command(input, &args);
            push_system(input, msg);
            if matches!(
                args.first().copied(),
                Some("resume" | "unpause" | "run" | "start" | "go")
            ) && input.run_rx.is_none()
                && input.run_handle.is_none()
                && !input.queue_paused
                && !input.queued_prompts.is_empty()
            {
                return SlashAction::RunQueuedNow;
            }
        }
        "retry" | "again" => {
            if input.run_rx.is_some() || input.run_handle.is_some() {
                push_system(
                    input,
                    "A task is already running; retry after it finishes.".into(),
                );
            } else if let Some(prompt) = input.last_prompt.clone() {
                push_system(input, "Retrying previous prompt.".into());
                return SlashAction::RunPrompt(prompt);
            } else {
                push_system(input, "No previous prompt to retry.".into());
            }
        }
        "cancel" | "stop" => {
            if input.run_rx.is_none() && input.run_handle.is_none() {
                push_system(input, "No active task to cancel.".into());
            } else {
                use std::sync::atomic::Ordering;
                if let Some(flag) = &input.cancel_flag {
                    flag.store(true, Ordering::Relaxed);
                }
                if let Some(handle) = input.run_handle.take() {
                    handle.abort();
                }
                input.run_rx = None;
                input.cancel_flag = None;
                input.current_stage = "Cancelled".into();
                input.current_stage_detail = "cancelled by /cancel".into();
                super::process::record_run_event(input, "Cancelled by user");
                if let Some(last) = input
                    .transcript
                    .iter_mut()
                    .rev()
                    .find(|msg| msg.role == "assistant" && msg.status == "thinking")
                {
                    last.status = "complete".into();
                    last.content = "○ cancelled".into();
                }
                refresh_live_trace(input, true);
                push_system(input, "Cancelled the active task.".into());
            }
        }
        "pwd" | "cwd" => {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("could not read cwd: {}", e));
            push_system(input, format!("cwd:\n  {}", cwd));
        }
        "logdir" => {
            push_system(
                input,
                format!("session log dir:\n  {}", store.log_dir().display()),
            );
        }
        "quit" | "exit" | "q" => return SlashAction::Exit,
        _ => {
            push_system(
                input,
                format!(
                    "Unknown command: /{}\nType /help for available commands.",
                    cmd
                ),
            );
        }
    }

    SlashAction::None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SlashArgCandidate {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: String,
}

#[cfg(test)]
pub(super) fn complete_slash_argument(
    store: &SessionStore,
    config: &Config,
    input: &mut InputState,
) -> bool {
    complete_slash_argument_at(store, config, input, 0)
}

pub(super) fn complete_slash_argument_at(
    store: &SessionStore,
    config: &Config,
    input: &mut InputState,
    selected: usize,
) -> bool {
    if let Some(prefix) = slash_command_prefix(&input.buffer, input.cursor) {
        let candidates = slash_command_candidates(&prefix);
        if candidates.is_empty() {
            return false;
        }
        let selected = selected.min(candidates.len().saturating_sub(1));
        apply_slash_command_completion(input, &candidates[selected].value);
        return true;
    }

    let Some(ctx) = slash_argument_context(&input.buffer, input.cursor) else {
        return false;
    };
    let candidates = argument_candidates(store, config, input, &ctx);
    if candidates.is_empty() {
        return false;
    }
    let selected = selected.min(candidates.len().saturating_sub(1));
    apply_argument_completion(input, &ctx, &candidates[selected].value);
    true
}

pub(super) fn slash_argument_candidates(
    store: &SessionStore,
    config: &Config,
    input: &InputState,
) -> Vec<SlashArgCandidate> {
    if let Some(prefix) = slash_command_prefix(&input.buffer, input.cursor) {
        return slash_command_candidates(&prefix);
    }

    let Some(ctx) = slash_argument_context(&input.buffer, input.cursor) else {
        return vec![];
    };
    argument_candidates(store, config, input, &ctx)
}

struct SlashArgContext {
    cmd: String,
    args: Vec<String>,
    current_index: usize,
    current_prefix: String,
}

fn slash_argument_context(buffer: &str, cursor: usize) -> Option<SlashArgContext> {
    if cursor != buffer.len() || !buffer.starts_with('/') {
        return None;
    }

    let has_arg_boundary = buffer.ends_with(char::is_whitespace);
    let trimmed = buffer.trim_end();
    let mut parts = trimmed.split_whitespace();
    let cmd = parts.next()?.trim_start_matches('/').to_string();
    if cmd.is_empty() {
        return None;
    }

    let args: Vec<String> = parts.map(str::to_string).collect();
    if args.is_empty() && !has_arg_boundary {
        return None;
    }

    let (current_index, current_prefix) = if has_arg_boundary {
        (args.len(), String::new())
    } else {
        (args.len().saturating_sub(1), args.last()?.clone())
    };

    Some(SlashArgContext {
        cmd,
        args,
        current_index,
        current_prefix,
    })
}

fn slash_command_prefix(buffer: &str, cursor: usize) -> Option<String> {
    if cursor != buffer.len() || !buffer.starts_with('/') {
        return None;
    }

    let raw = &buffer[1..];
    if raw.chars().any(char::is_whitespace) {
        return None;
    }
    Some(raw.to_string())
}

fn slash_command_candidates(prefix: &str) -> Vec<SlashArgCandidate> {
    slash_command_catalog()
        .iter()
        .filter(|command| candidate_matches(command.name, prefix))
        .map(|command| SlashArgCandidate {
            value: command.name.to_string(),
            label: format!("/{}", command.name),
            description: command.description.to_string(),
        })
        .collect()
}

struct SlashCommandDef {
    name: &'static str,
    description: &'static str,
}

fn slash_command_catalog() -> &'static [SlashCommandDef] {
    &[
        SlashCommandDef {
            name: "help",
            description: "show commands",
        },
        SlashCommandDef {
            name: "status",
            description: "show current run/config status",
        },
        SlashCommandDef {
            name: "doctor",
            description: "diagnose config and runtime setup",
        },
        SlashCommandDef {
            name: "roles",
            description: "show/select/save role routing",
        },
        SlashCommandDef {
            name: "workflow",
            description: "show active role pipeline",
        },
        SlashCommandDef {
            name: "agent",
            description: "route future worker tasks",
        },
        SlashCommandDef {
            name: "trace",
            description: "control live trace",
        },
        SlashCommandDef {
            name: "context",
            description: "control remembered context",
        },
        SlashCommandDef {
            name: "process",
            description: "show captured run process",
        },
        SlashCommandDef {
            name: "queue",
            description: "manage queued messages",
        },
        SlashCommandDef {
            name: "result",
            description: "show final result",
        },
        SlashCommandDef {
            name: "model",
            description: "show model assignments",
        },
        SlashCommandDef {
            name: "usage",
            description: "show token/cost usage",
        },
        SlashCommandDef {
            name: "sessions",
            description: "list recent sessions",
        },
        SlashCommandDef {
            name: "import-cc",
            description: "import Claude Code sessions",
        },
        SlashCommandDef {
            name: "resume",
            description: "restore last terminal chat",
        },
        SlashCommandDef {
            name: "session",
            description: "show one session summary",
        },
        SlashCommandDef {
            name: "tree",
            description: "show session parent/child tree",
        },
        SlashCommandDef {
            name: "log",
            description: "show session log tail",
        },
        SlashCommandDef {
            name: "editor",
            description: "compose a longer prompt",
        },
        SlashCommandDef {
            name: "diff",
            description: "show current git changes",
        },
        SlashCommandDef {
            name: "tests",
            description: "show or run test helpers",
        },
        SlashCommandDef {
            name: "handoff",
            description: "save or open handoff package",
        },
        SlashCommandDef {
            name: "continue",
            description: "open Claude/Codex CLI inline",
        },
        SlashCommandDef {
            name: "graph",
            description: "summarize conversation as graph",
        },
        SlashCommandDef {
            name: "acp",
            description: "show ACP setup hints",
        },
        SlashCommandDef {
            name: "checkpoint",
            description: "save current git patch",
        },
        SlashCommandDef {
            name: "rollback",
            description: "reverse-apply checkpoint",
        },
        SlashCommandDef {
            name: "export",
            description: "export process or transcript",
        },
        SlashCommandDef {
            name: "grep",
            description: "search session logs",
        },
        SlashCommandDef {
            name: "retry",
            description: "rerun the last task prompt",
        },
        SlashCommandDef {
            name: "cancel",
            description: "abort active task",
        },
        SlashCommandDef {
            name: "clear",
            description: "clear chat transcript",
        },
        SlashCommandDef {
            name: "compact",
            description: "keep only recent messages",
        },
        SlashCommandDef {
            name: "o",
            description: "fold or expand process detail",
        },
        SlashCommandDef {
            name: "copy",
            description: "copy result or transcript",
        },
        SlashCommandDef {
            name: "save",
            description: "save transcript to file",
        },
        SlashCommandDef {
            name: "pwd",
            description: "show current directory",
        },
        SlashCommandDef {
            name: "logdir",
            description: "show session log directory",
        },
        SlashCommandDef {
            name: "quit",
            description: "exit terminal chat",
        },
    ]
}

fn argument_candidates(
    store: &SessionStore,
    config: &Config,
    input: &InputState,
    ctx: &SlashArgContext,
) -> Vec<SlashArgCandidate> {
    match ctx.cmd.as_str() {
        "session" | "show" | "s" if ctx.current_index == 0 => {
            let mut candidates = choice_candidates(ctx, 0, &[("tree", "show parent/child tree")]);
            candidates.extend(session_candidates(store, ctx));
            candidates
        }
        "session" | "show" | "s"
            if ctx.current_index == 1 && ctx.args.first().map(String::as_str) == Some("tree") =>
        {
            session_candidates(store, ctx)
        }
        "tree" | "session-tree" if ctx.current_index == 0 => session_candidates(store, ctx),
        "log" | "tail" if ctx.current_index == 0 => session_candidates(store, ctx),
        "log" | "tail" if ctx.current_index == 1 => choice_candidates(
            ctx,
            1,
            &[
                ("20", "tail 20 lines"),
                ("40", "tail 40 lines"),
                ("80", "tail 80 lines"),
                ("120", "tail 120 lines"),
                ("200", "tail 200 lines"),
            ],
        ),
        "sessions" | "history" => choice_candidates(
            ctx,
            0,
            &[
                ("5", "show 5 sessions"),
                ("10", "show 10 sessions"),
                ("20", "show 20 sessions"),
                ("50", "show 50 sessions"),
            ],
        ),
        "doctor" | "diagnose" | "checkup" => choice_candidates(
            ctx,
            0,
            &[("run", "diagnose config, runtimes, keys, and tools")],
        ),
        "resume" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[("last", "restore the last terminal chat conversation")],
        ),
        "compact" => choice_candidates(
            ctx,
            0,
            &[
                ("10", "keep 10 messages"),
                ("20", "keep 20 messages"),
                ("50", "keep 50 messages"),
                ("100", "keep 100 messages"),
            ],
        ),
        "process" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("20", "show 20 process events"),
                ("80", "show 80 process events"),
                ("200", "show 200 process events"),
                ("summary", "show compact process summary"),
                ("filter", "filter process events"),
                ("full", "show full captured process"),
                ("save", "save process capture"),
                ("clipboard", "copy process capture"),
                ("clear-filter", "clear process filter"),
                ("all", "clear process filter"),
            ],
        ),
        "diff" | "changes" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("summary", "show changed files"),
                ("stat", "show diff stats"),
                ("full", "show current git diff"),
                ("cached", "show staged diff"),
            ],
        ),
        "tests" | "test" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("suggest", "suggest tests for this project"),
                ("last", "show last captured test hint"),
                ("cargo", "show cargo test command"),
                ("quick", "run focused orchestrator checks"),
                ("run", "start a background test command"),
                ("status", "show background test status"),
                ("tail", "show test log tail"),
            ],
        ),
        "tests" | "test"
            if ctx.current_index == 1 && ctx.args.first().map(String::as_str) == Some("run") =>
        {
            choice_candidates(
                ctx,
                1,
                &[
                    ("cargo", "run cargo test"),
                    ("tui", "run cargo test tui::"),
                    ("quick", "run focused orchestrator checks"),
                    ("npm", "run npm test"),
                    ("pytest", "run pytest"),
                ],
            )
        }
        "roles" | "role" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("show", "show role registry"),
                ("route", "show default workflow route"),
                ("use", "route future worker tasks to a role"),
                ("snippet", "print a config role snippet"),
                ("save", "write a role to config"),
                ("clear", "clear current role hint"),
            ],
        ),
        "roles" | "role"
            if ctx.current_index == 1
                && matches!(
                    ctx.args.first().map(String::as_str),
                    Some("snippet" | "save" | "add" | "set")
                ) =>
        {
            role_name_template_candidates(ctx)
        }
        "roles" | "role"
            if ctx.current_index == 2
                && matches!(
                    ctx.args.first().map(String::as_str),
                    Some("snippet" | "save" | "add" | "set")
                ) =>
        {
            any_model_candidates(config, ctx)
        }
        "roles" | "role"
            if ctx.current_index == 3
                && matches!(
                    ctx.args.first().map(String::as_str),
                    Some("snippet" | "save" | "add" | "set")
                ) =>
        {
            runtime_candidates(config, ctx)
        }
        "roles" | "role"
            if ctx.current_index == 1 && ctx.args.first().map(String::as_str) == Some("use") =>
        {
            role_candidates(config, ctx)
        }
        "workflow" | "flow-status" | "pipeline" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("status", "show active role pipeline"),
                ("route", "show configured role route"),
            ],
        ),
        "handoff" | "continue" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("claude", "create Claude handoff package"),
                ("codex", "create Codex handoff package"),
                ("open", "save and open target CLI"),
                ("copy", "copy last handoff package"),
                ("status", "show last handoff path"),
            ],
        ),
        "handoff" | "continue"
            if ctx.current_index == 1 && ctx.args.first().map(String::as_str) == Some("open") =>
        {
            choice_candidates(
                ctx,
                1,
                &[
                    ("claude", "open Claude with handoff"),
                    ("codex", "open Codex with handoff"),
                ],
            )
        }
        "graph" | "flow" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("flow", "show conversation flow graph"),
                ("compact", "show compact flow graph"),
                ("full", "show more message nodes"),
                ("mermaid", "show Mermaid flowchart"),
                ("save", "save Mermaid flowchart"),
                ("clipboard", "copy Mermaid flowchart"),
            ],
        ),
        "acp" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("status", "show ACP server command"),
                ("claude", "show Claude ACP setup hint"),
                ("codex", "show Codex ACP setup hint"),
                ("command", "show raw ACP stdio command"),
            ],
        ),
        "checkpoint" | "cp" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("save", "save current git patch checkpoint"),
                ("status", "show last checkpoint"),
            ],
        ),
        "rollback" | "revert" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("last", "reverse-apply last checkpoint patch"),
                ("check", "check whether rollback can apply"),
            ],
        ),
        "trace" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("on", "show live trace block"),
                ("off", "stop live trace updates"),
                ("full", "show more live trace events"),
                ("compact", "show fewer live trace events"),
                ("status", "show live trace status"),
                ("process", "show captured process"),
            ],
        ),
        "context" | "ctx" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("status", "show context memory status"),
                ("compact", "inject recent context"),
                ("full", "inject more history"),
                ("off", "stop injecting context"),
                ("clear", "forget prior context"),
            ],
        ),
        "queue" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("show", "show queued messages"),
                ("clear", "remove every queued message"),
                ("remove", "remove one queued message"),
                ("promote", "run one queued message next"),
                ("edit", "replace one queued message"),
                ("pause", "hold queued messages after current run"),
                ("resume", "allow queued messages to run"),
                ("run", "run queued batch now when idle"),
            ],
        ),
        "queue"
            if ctx.current_index == 1
                && matches!(
                    ctx.args.first().map(String::as_str),
                    Some("remove" | "rm" | "delete" | "promote" | "up" | "next" | "edit" | "set")
                ) =>
        {
            queue_index_candidates(input, ctx)
        }
        "trace"
            if ctx.current_index == 1
                && ctx.args.first().map(String::as_str) == Some("process") =>
        {
            choice_candidates(
                ctx,
                1,
                &[
                    ("20", "show 20 process events"),
                    ("80", "show 80 process events"),
                    ("200", "show 200 process events"),
                ],
            )
        }
        "export" if ctx.current_index == 0 => choice_candidates(
            ctx,
            0,
            &[
                ("process", "export process capture"),
                ("transcript", "export chat transcript"),
            ],
        ),
        "export" if ctx.current_index == 1 => choice_candidates(
            ctx,
            1,
            &[("file", "save to file"), ("clipboard", "copy to clipboard")],
        ),
        "result" | "final" => choice_candidates(ctx, 0, &[("text", "show selectable plain text")]),
        "copy" => choice_candidates(
            ctx,
            0,
            &[
                ("result", "copy last final result"),
                ("file", "save transcript to file"),
                ("clipboard", "copy transcript to clipboard"),
            ],
        ),
        "usage" | "u" => choice_candidates(
            ctx,
            0,
            &[
                ("today", "today's usage"),
                ("week", "last 7 days"),
                ("month", "this month"),
                ("all", "all recorded usage"),
            ],
        ),
        "model" | "models" if ctx.current_index == 0 => role_candidates(config, ctx),
        "model" | "models" if ctx.current_index == 1 => model_candidates(config, ctx),
        "agent" | "agents" if ctx.current_index == 0 => agent_candidates(config, ctx),
        _ => vec![],
    }
}

fn session_candidates(store: &SessionStore, ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    let Ok(sessions) = store.list(50) else {
        return vec![];
    };
    sessions
        .into_iter()
        .filter(|s| candidate_matches(&s.id.to_string(), &ctx.current_prefix))
        .take(8)
        .map(|s| SlashArgCandidate {
            value: s.id.to_string(),
            label: session_id8(&s),
            description: format!(
                "{} {} {}",
                format_session_state(&s),
                s.role.as_str(),
                s.agent
            ),
        })
        .collect()
}

fn choice_candidates(
    ctx: &SlashArgContext,
    arg_index: usize,
    choices: &[(&str, &str)],
) -> Vec<SlashArgCandidate> {
    if ctx.current_index != arg_index {
        return vec![];
    }
    choices
        .iter()
        .filter(|(value, _)| candidate_matches(value, &ctx.current_prefix))
        .map(|(value, description)| SlashArgCandidate {
            value: (*value).to_string(),
            label: (*value).to_string(),
            description: (*description).to_string(),
        })
        .collect()
}

fn role_candidates(config: &Config, ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    let mut roles: Vec<_> = config.roles.iter().collect();
    roles.sort_by(|a, b| a.0.cmp(b.0));
    roles
        .into_iter()
        .filter(|(role, _)| candidate_matches(role, &ctx.current_prefix))
        .map(|(role, role_cfg)| SlashArgCandidate {
            value: role.clone(),
            label: role.clone(),
            description: format!(
                "{} · runtime {}",
                model_label(config, &role_cfg.model),
                role_cfg.runtime
            ),
        })
        .collect()
}

fn model_candidates(config: &Config, ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    if ctx
        .args
        .first()
        .and_then(|role| config.roles.get(role))
        .is_none()
    {
        return vec![];
    }
    sorted_models(config)
        .into_iter()
        .filter(|m| candidate_matches(&m.id, &ctx.current_prefix))
        .map(|m| SlashArgCandidate {
            value: m.id.clone(),
            label: m.id.clone(),
            description: format!("{} · {}", m.provider, m.name),
        })
        .collect()
}

fn any_model_candidates(config: &Config, ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    sorted_models(config)
        .into_iter()
        .filter(|m| candidate_matches(&m.id, &ctx.current_prefix))
        .map(|m| SlashArgCandidate {
            value: m.id.clone(),
            label: m.id.clone(),
            description: format!("{} · {}", m.provider, m.name),
        })
        .collect()
}

fn role_name_template_candidates(ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    choice_candidates(
        ctx,
        1,
        &[
            ("commander", "planner-style orchestration role"),
            ("critic", "review the plan before execution"),
            ("executor", "worker-style execution role"),
            ("reviewer", "review worker output"),
        ],
    )
}

fn runtime_candidates(config: &Config, ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    let mut runtimes: Vec<_> = config.runtimes.keys().cloned().collect();
    runtimes.sort();
    if runtimes.is_empty() {
        runtimes = vec!["claude-code".into(), "codex".into()];
    }
    runtimes
        .into_iter()
        .filter(|runtime| candidate_matches(runtime, &ctx.current_prefix))
        .map(|runtime| SlashArgCandidate {
            value: runtime.clone(),
            label: runtime.clone(),
            description: "configured runtime".into(),
        })
        .collect()
}

fn agent_candidates(config: &Config, ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    let mut candidates = AGENT_RUNTIME_ROUTES
        .iter()
        .map(|route| SlashArgCandidate {
            value: route.label.into(),
            label: route.label.into(),
            description: route.description.into(),
        })
        .collect::<Vec<_>>();
    candidates.push(SlashArgCandidate {
        value: "clear".into(),
        label: "clear".into(),
        description: "return to automatic routing".into(),
    });
    candidates.extend(role_candidates(config, ctx));
    candidates
        .into_iter()
        .filter(|candidate| candidate_matches(&candidate.value, &ctx.current_prefix))
        .collect()
}

fn queue_index_candidates(input: &InputState, ctx: &SlashArgContext) -> Vec<SlashArgCandidate> {
    input
        .queued_prompts
        .iter()
        .enumerate()
        .map(|(idx, prompt)| {
            let value = (idx + 1).to_string();
            SlashArgCandidate {
                label: value.clone(),
                value,
                description: truncate_chars(prompt, 80),
            }
        })
        .filter(|candidate| candidate_matches(&candidate.value, &ctx.current_prefix))
        .take(8)
        .collect()
}

fn candidate_matches(value: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || value
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
}

fn apply_argument_completion(input: &mut InputState, ctx: &SlashArgContext, value: &str) {
    let mut parts = Vec::with_capacity(ctx.current_index + 2);
    parts.push(format!("/{}", ctx.cmd));
    parts.extend(
        ctx.args
            .iter()
            .take(ctx.current_index)
            .map(|arg| arg.to_string()),
    );
    parts.push(value.to_string());

    input.buffer = format!("{} ", parts.join(" "));
    input.cursor = input.buffer.len();
}

fn apply_slash_command_completion(input: &mut InputState, value: &str) {
    input.buffer = format!("/{} ", value);
    input.cursor = input.buffer.len();
}

pub(super) fn push_system(input: &mut InputState, content: String) {
    input.transcript.push(ChatMsg {
        role: "system".into(),
        content,
        ts: std::time::SystemTime::now(),
        status: "complete".into(),
    });
}

fn help_text() -> String {
    "Available commands:\n\
     \n\
     /help                         Show this help\n\
     /status                       Show current terminal chat/run status\n\
     /doctor                       Diagnose config, runtimes, API keys, and tools\n\
     /roles [show|route|use|snippet|save] Show/select/save role routing\n\
     /workflow                     Show active role pipeline\n\
     /model [role]                 Show model assignments\n\
     /agent [role|clear]           Route future worker tasks\n\
     /usage [today|week|month|all] Show token/cost usage\n\
     /sessions [n]                 List recent sessions with tree/log shortcuts\n\
     /import-cc [project]          Import Claude Code sessions into chat history\n\
     /resume last                  Restore the last terminal chat conversation\n\
     /session [id|prefix|last]     Show one session summary\n\
     /session tree [id|prefix|last] Show parent/child session tree\n\
     /tree [id|prefix|last]        Show parent/child session tree\n\
     /log [id|prefix|last] [n]     Show session log tail\n\
     /trace [on|off|full|compact] Control live trace in the chat\n\
     /context [compact|full|off|clear] Control remembered conversation context\n\
     /process [summary|full|n]     Show captured run process\n\
     /editor [draft]               Compose a longer prompt in $VISUAL or $EDITOR\n\
     /diff [summary|stat|full]      Show current git changes\n\
     /tests [suggest|quick|run|status] Show or run test helpers\n\
     /handoff claude|codex          Save context package for another CLI\n\
     /handoff open claude|codex     Save handoff and open that CLI inline\n\
     /continue claude|codex         Save handoff and open that CLI inline\n\
     /graph [compact|full|mermaid|save] Compress conversation into a flow graph\n\
     /acp [status|claude|codex]     Show ACP server/client setup hints\n\
     /checkpoint [save|status]      Save current tracked git diff\n\
     /rollback [last|check]         Reverse-apply last checkpoint patch\n\
     /process filter <text>        Filter captured process events\n\
     /process clear-filter         Clear process event filter\n\
     /process save                 Save captured process\n\
     /export process [target]      Export process or transcript\n\
     /grep <text>                  Search session logs\n\
     /queue [clear|remove n|promote n|pause|resume|edit n text] Manage queued messages\n\
     /retry                        Re-run the last task prompt\n\
     /cancel                       Abort the active task\n\
     /clear                        Clear the chat transcript\n\
     /compact [n]                  Keep only the last n messages\n\
     /o                            Fold or expand future process detail\n\
     /copy [file|clipboard]        Export transcript\n\
     /copy result                  Copy last final result\n\
     /result [text]                Show last final result\n\
     /save                         Save transcript to a file\n\
     /pwd                          Show current working directory\n\
     /logdir                       Show session log directory\n\
     /quit                         Exit terminal chat\n\
     \n\
     Copy/paste: handled by your terminal/OS; drag-select visible text and use system shortcuts\n\
     Cancel/exit: Ctrl+C keeps the terminal convention, especially on macOS\n\
     Scroll: use your terminal's native scrollback; the app does not capture mouse wheel input\n\
     Completion: type / to open the command menu; Up/Down selects, Enter confirms\n\
     Process: /o folds/expands future gray agent output; /process 200 replays captured events\n\
     Anything else: sent as a task to the orchestrator"
        .into()
}

fn parse_count(raw: Option<&str>, default: usize, min: usize, max: usize) -> usize {
    raw.and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn parse_log_args<'a>(args: &'a [&'a str]) -> (Option<&'a str>, usize) {
    match args {
        [] => (None, 40),
        [first] => {
            if first.parse::<usize>().is_ok() {
                (None, parse_count(Some(first), 40, 1, 200))
            } else {
                (Some(first), 40)
            }
        }
        [first, second, ..] => (Some(first), parse_count(Some(second), 40, 1, 200)),
    }
}

fn handle_trace_command(input: &mut InputState, args: &[&str]) -> String {
    match args.first().copied() {
        None | Some("status") => live_trace_status(input),
        Some("on" | "enable" | "enabled") => {
            input.trace_live_enabled = true;
            refresh_live_trace(input, false);
            format!("Live trace enabled: {}", live_trace_summary(input))
        }
        Some("off" | "disable" | "disabled") => {
            complete_live_trace(input);
            input.trace_live_enabled = false;
            format!("Live trace disabled. Captured process is still available with /process [n].")
        }
        Some("full" | "expand" | "expanded") => {
            input.trace_live_enabled = true;
            input.trace_expanded = true;
            refresh_live_trace(input, false);
            format!("Live trace expanded: {}", live_trace_summary(input))
        }
        Some("compact" | "collapse" | "collapsed") => {
            input.trace_live_enabled = true;
            input.trace_expanded = false;
            refresh_live_trace(input, false);
            format!("Live trace compacted: {}", live_trace_summary(input))
        }
        Some("process") => {
            let limit = parse_count(args.get(1).copied(), 80, 1, 500);
            format_process_capture(input, limit)
        }
        Some(other) => format!(
            "Unknown trace option: {}\nUsage: /trace [on|off|full|compact|status]",
            other
        ),
    }
}

fn format_model_command(config: &Config, args: &[&str]) -> String {
    match args {
        [] => format_model_overview(config),
        [role] => format_model_role_detail(config, role),
        [role, model, ..] => format_model_switch_note(config, role, model),
    }
}

fn format_model_overview(config: &Config) -> String {
    let mut roles: Vec<_> = config.roles.iter().collect();
    roles.sort_by(|a, b| a.0.cmp(b.0));

    let mut out = String::from("Model assignments:");
    if roles.is_empty() {
        out.push_str("\n  none configured");
    }
    for (role, cfg) in roles {
        let model = model_label(config, &cfg.model);
        out.push_str(&format!(
            "\n  {:<14} {:<18} runtime={} teams={}",
            role, model, cfg.runtime, cfg.agent_teams
        ));
    }

    out.push_str("\n\nAvailable models:");
    if config.models.is_empty() {
        out.push_str("\n  none configured");
    }
    for model in sorted_models(config) {
        out.push_str(&format!(
            "\n  {:<18} {:<12} {}",
            model.id, model.provider, model.name
        ));
    }
    out
}

fn format_model_role_detail(config: &Config, role: &str) -> String {
    let Some(role_cfg) = config.roles.get(role) else {
        return format!(
            "Unknown role: {}\nAvailable roles: {}",
            role,
            available_role_names(config)
        );
    };

    let current = model_label(config, &role_cfg.model);
    let mut out = format!(
        "Role {}\n  model: {}\n  runtime: {}\n  agent teams: {}\n\nAvailable models:",
        role, current, role_cfg.runtime, role_cfg.agent_teams
    );
    for model in sorted_models(config) {
        out.push_str(&format!(
            "\n  {:<18} {:<12} {}",
            model.id, model.provider, model.name
        ));
    }
    out.push_str(&format!(
        "\n\nRuntime model hot-switch is not wired yet. Persistently change with:\n  orchestrator config set {} <model-id>",
        role
    ));
    out
}

fn format_model_switch_note(config: &Config, role: &str, model: &str) -> String {
    if !config.roles.contains_key(role) {
        return format!(
            "Unknown role: {}\nAvailable roles: {}",
            role,
            available_role_names(config)
        );
    }
    let Some(model_cfg) = config
        .models
        .iter()
        .find(|m| m.id == model || m.name == model)
    else {
        return format!(
            "Unknown model: {}\nAvailable models: {}",
            model,
            available_model_names(config)
        );
    };

    format!(
        "Model hot-switch is not active for the running terminal chat yet.\nRequested: {} -> {} ({})\nPersistently change with:\n  orchestrator config set {} {}",
        role, model_cfg.id, model_cfg.name, role, model_cfg.id
    )
}

fn handle_agent_command(config: &Config, input: &mut InputState, args: &[&str]) -> String {
    match args.first().copied() {
        None => format_agent_overview(config, input),
        Some("clear" | "auto" | "default") => {
            input.agent_hint = None;
            "Agent hint cleared. Future tasks will use automatic routing.".into()
        }
        Some(route) if runtime::route_by_label(route).is_some() => {
            let route = runtime::route_by_label(route).expect("checked route");
            input.agent_hint = route
                .runtime
                .and_then(|runtime_id| {
                    runtime::default_worker_role_for_runtime(&config.roles, runtime_id)
                })
                .or_else(|| route.role_hint.map(str::to_string));
            format_agent_selection(config, route.label, input.agent_hint.as_deref())
        }
        Some(role) => {
            if !config.roles.contains_key(role) {
                return format!(
                    "Unknown agent role: {}\nAvailable roles: {}",
                    role,
                    available_role_names(config)
                );
            }
            input.agent_hint = Some(role.to_string());
            format_agent_selection(config, role, Some(role))
        }
    }
}

fn format_agent_overview(config: &Config, input: &InputState) -> String {
    let selected = input
        .agent_hint
        .as_deref()
        .filter(|role| config.roles.contains_key(*role))
        .map(str::to_string);
    let default_worker = runtime::default_worker_role(&config.roles);
    let mut out = format!(
        "Agent routing\n  selected: {}\n  default: {}\n  mode: future prompts use this worker route\n\nRoutes:",
        selected
            .as_deref()
            .map(|role| format!("{} ({})", runtime::agent_label(Some(role)), role))
            .unwrap_or_else(|| "auto".into()),
        default_worker.as_deref().unwrap_or("(missing worker)")
    );
    out.push_str("\n  /agent claude     Claude Code worker");
    out.push_str("\n  /agent codex      Codex CLI worker");
    out.push_str("\n  /agent auto       configured routing");

    let worker_roles = worker_role_names(config);
    out.push_str("\n\nWorker roles:");
    if worker_roles.is_empty() {
        out.push_str("\n  none configured");
    } else {
        for role in worker_roles {
            out.push('\n');
            out.push_str("  ");
            out.push_str(&format_role_detail_line(config, &role));
        }
    }
    out.push_str("\n\nActions: /agent <role>, /roles use <role>, /agent clear");
    out
}

fn format_agent_selection(config: &Config, label: &str, role: Option<&str>) -> String {
    let mut out = format!("Agent route\n  ✓ selected: {}", label);
    match role.and_then(|role| config.roles.get(role).map(|cfg| (role, cfg))) {
        Some((role, role_cfg)) => {
            out.push_str(&format!(
                "\n  role: {}\n  runtime: {}\n  model: {}\n  teams: {}",
                role,
                role_cfg.runtime,
                model_label(config, &role_cfg.model),
                on_off(role_cfg.agent_teams)
            ));
        }
        None => {
            out.push_str(
                "\n  role: auto\n  runtime: configured router\n  model: configured router",
            );
        }
    }
    out.push_str("\n\nNext: future prompts in this chat use this route. Use /workflow to inspect the full pipeline.");
    out
}

fn handle_roles_command(
    config: &Config,
    runtime: &TuiRuntimeConfig,
    input: &mut InputState,
    args: &[&str],
) -> String {
    match args.first().copied().unwrap_or("show") {
        "show" | "list" | "status" => format_roles_registry(config, input),
        "route" | "workflow" => format_role_workflow(config, input),
        "use" | "select" => {
            let Some(role) = args.get(1).copied() else {
                return format!(
                    "Usage: /roles use <role>\nAvailable roles: {}",
                    available_role_names(config)
                );
            };
            if !config.roles.contains_key(role) {
                return format!(
                    "Unknown role: {}\nAvailable roles: {}",
                    role,
                    available_role_names(config)
                );
            }
            input.agent_hint = Some(role.to_string());
            format!(
                "Role selected\n  ✓ worker: {}\n\n{}",
                role,
                format_role_detail_line(config, role)
            )
        }
        "clear" | "auto" | "default" => {
            input.agent_hint = None;
            "Role hint cleared. Future tasks will use automatic routing.".into()
        }
        "snippet" => format_role_config_snippet(config, &args),
        "save" | "add" | "set" => save_role_config(config, runtime, &args),
        other => format!("Unknown roles option: {}\nUsage: /roles [show|route|use <role>|snippet <role> <model> <runtime>|save <role> <model> <runtime>|clear]", other),
    }
}

fn format_role_config_snippet(config: &Config, args: &[&str]) -> String {
    let role = args.get(1).copied().unwrap_or("executor");
    let model = args.get(2).copied().unwrap_or_else(|| {
        if config.models.iter().any(|m| m.id == "gpt4o") {
            "gpt4o"
        } else {
            config
                .models
                .first()
                .map(|m| m.id.as_str())
                .unwrap_or("model-id")
        }
    });
    let runtime = args.get(3).copied().unwrap_or_else(|| {
        if config.runtimes.contains_key("claude-code") {
            "claude-code"
        } else if config.runtimes.contains_key("codex") {
            "codex"
        } else {
            "runtime-id"
        }
    });
    let teams = matches!(runtime, "claude-code" | "claude");
    format!(
        "Role config snippet:\n  roles:\n    {role}:\n      model: {model}\n      runtime: {runtime}\n      agent_teams: {teams}\n\nExisting roles can be selected now with /roles use <role>. New or changed role definitions are loaded when terminal chat starts, so add this to ~/.orchestrator/config.yaml and restart."
    )
}

fn save_role_config(config: &Config, runtime: &TuiRuntimeConfig, args: &[&str]) -> String {
    let Some(path) = runtime.config_path.as_deref() else {
        return "Config path is unavailable in this context. Use /roles snippet <role> <model> <runtime> and add it to ~/.orchestrator/config.yaml.".into();
    };
    let Some(role) = args.get(1).copied() else {
        return "Usage: /roles save <role> <model> <runtime>".into();
    };
    let Some(model) = args.get(2).copied() else {
        return "Usage: /roles save <role> <model> <runtime>".into();
    };
    let Some(runtime_id) = args.get(3).copied() else {
        return "Usage: /roles save <role> <model> <runtime>".into();
    };
    let teams = args
        .get(4)
        .map(|raw| matches!(*raw, "true" | "yes" | "1" | "teams"))
        .unwrap_or_else(|| matches!(runtime_id, "claude-code" | "claude"));

    if !config
        .models
        .iter()
        .any(|m| m.id == model || m.name == model)
    {
        return format!(
            "Unknown model: {}\nAvailable models: {}",
            model,
            available_model_names(config)
        );
    }
    if !config.runtimes.contains_key(runtime_id) {
        return format!(
            "Unknown runtime: {}\nAvailable runtimes: {}",
            runtime_id,
            available_runtime_names(config)
        );
    }

    let role_cfg = RoleConfig {
        model: model.to_string(),
        runtime: runtime_id.to_string(),
        agent_teams: teams,
    };
    match Config::write_role_to_config_file(path, role, &role_cfg) {
        Ok(()) => format!(
            "Saved role {} to {}.\n  model: {}\n  runtime: {}\n  agent teams: {}\n\nRestart terminal chat for planner/critic/reviewer changes to rebuild the execution pipeline. Worker roles can be selected in this session with /roles use {}.",
            role,
            crate::config::expand_home(path).display(),
            model_label(config, model),
            runtime_id,
            teams,
            role
        ),
        Err(e) => format!("Could not save role {}: {:#}", role, e),
    }
}

fn format_roles_registry(config: &Config, input: &InputState) -> String {
    let default_worker = runtime::default_worker_role(&config.roles);
    let mut out = format!(
        "Role registry\n  selected: {}\n  default worker: {}\n\nPipeline roles:",
        selected_route_label(input),
        default_worker.as_deref().unwrap_or("(missing worker)")
    );
    for role in ["planner", "critic", "reviewer"] {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&format_role_detail_line(config, role));
    }

    let workers = worker_role_names(config);
    out.push_str("\n\nWorker roles:");
    if workers.is_empty() {
        out.push_str("\n  none configured");
    } else {
        for role in workers {
            out.push('\n');
            out.push_str("  ");
            out.push_str(&format_role_detail_line(config, &role));
        }
    }
    out.push_str(
        "\n\nActions: /roles use <role>, /role <role>, /roles route, /roles save <role> <model> <runtime>",
    );
    out
}

fn format_role_detail_line(config: &Config, role: &str) -> String {
    let Some(role_cfg) = config.roles.get(role) else {
        return format!("{} {:<18} missing", role_icon(role), role);
    };
    format!(
        "{} {:<18} {:<12} {:<26} teams={}",
        role_icon(role),
        role,
        role_cfg.runtime,
        truncate_chars(&model_label(config, &role_cfg.model), 26),
        on_off(role_cfg.agent_teams)
    )
}

fn format_role_workflow(config: &Config, input: &InputState) -> String {
    let planner = role_or_missing(config, "planner");
    let critic = role_or_missing(config, "critic");
    let worker = input
        .agent_hint
        .as_deref()
        .filter(|role| config.roles.contains_key(*role))
        .map(str::to_string)
        .or_else(|| runtime::default_worker_role(&config.roles))
        .unwrap_or_else(|| "(missing worker)".into());
    let reviewer = role_or_missing(config, "reviewer");

    format!(
        "Role flow\n  1. plan      {}\n  2. critique  {}\n  3. execute   {}\n  4. review    {}\n  5. answer    final text\n\nCurrent worker: {}",
        planner,
        critic,
        worker,
        reviewer,
        selected_route_label(input)
    )
}

fn format_workflow_status(config: &Config, input: &InputState) -> String {
    let worker = input
        .agent_hint
        .as_deref()
        .filter(|role| config.roles.contains_key(*role))
        .map(str::to_string)
        .or_else(|| runtime::default_worker_role(&config.roles))
        .unwrap_or_else(|| "(missing worker)".into());

    let queue = if input.queued_prompts.is_empty() {
        "empty".to_string()
    } else {
        format!(
            "{} pending ({})",
            input.queued_prompts.len(),
            queue_state_label(input)
        )
    };
    let run = if input.run_rx.is_some() || input.run_handle.is_some() {
        if input.current_stage.is_empty() {
            "active".to_string()
        } else {
            input.current_stage.clone()
        }
    } else {
        "idle".to_string()
    };

    format!(
        "Workflow status\n  run: {}\n  queue: {}\n  context: {}\n\nPipeline\n  1. plan      {}\n  2. critique  {}\n  3. execute   {}\n  4. review    {}\n  5. answer    final text\n\nWorker\n  selected: {}\n  details: {}",
        run,
        queue,
        context_summary(input),
        workflow_role_line(config, "planner"),
        workflow_role_line(config, "critic"),
        workflow_role_line(config, &worker),
        workflow_role_line(config, "reviewer"),
        selected_route_label(input),
        format_role_detail_line(config, &worker)
    )
}

fn workflow_role_line(config: &Config, role: &str) -> String {
    let Some(role_cfg) = config.roles.get(role) else {
        return "(missing)".into();
    };
    format!(
        "{} · {} · {}",
        role,
        role_cfg.runtime,
        model_label(config, &role_cfg.model)
    )
}

fn role_or_missing<'a>(config: &'a Config, role: &'a str) -> &'a str {
    if config.roles.contains_key(role) {
        role
    } else {
        "(missing)"
    }
}

fn worker_role_names(config: &Config) -> Vec<String> {
    let mut roles = config
        .roles
        .iter()
        .filter(|(name, _)| is_worker_role_name(name))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    roles.sort();
    roles
}

fn is_worker_role_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.contains("worker") || lower.contains("executor")) && !lower.contains("reviewer")
}

fn role_icon(role: &str) -> &'static str {
    let lower = role.to_ascii_lowercase();
    if lower.contains("planner") || lower == "commander" {
        "◇"
    } else if lower.contains("critic") {
        "◆"
    } else if lower.contains("reviewer") {
        "✓"
    } else if lower.contains("worker") || lower.contains("executor") {
        "●"
    } else {
        "·"
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

fn selected_route_label(input: &InputState) -> String {
    input
        .agent_hint
        .as_deref()
        .map(|role| format!("{} ({})", runtime::agent_label(Some(role)), role))
        .unwrap_or_else(|| "auto".into())
}

fn format_usage(store: &SessionStore, raw_window: &str) -> String {
    let Some((label, window)) = parse_usage_window(raw_window) else {
        return "Usage: /usage today|week|month|all".into();
    };
    let usage = match compute_for_window(store, window) {
        Ok(usage) => usage,
        Err(e) => return format!("Could not compute usage: {:#}", e),
    };

    let mut out = format!(
        "Token usage ({})\n  total: {} in / {} out / ${:.4}",
        label, usage.total_tokens_in, usage.total_tokens_out, usage.total_cost_usd
    );
    let mut providers: Vec<_> = usage.by_provider.iter().collect();
    providers.sort_by(|a, b| {
        let a_total = a.1.tokens_in + a.1.tokens_out;
        let b_total = b.1.tokens_in + b.1.tokens_out;
        b_total.cmp(&a_total).then_with(|| a.0.cmp(b.0))
    });
    if !providers.is_empty() {
        out.push_str("\n\nBy provider:");
        for (name, p) in providers.into_iter().take(8) {
            out.push_str(&format!(
                "\n  {:<18} {} in / {} out / ${:.4} ({} sessions)",
                truncate_chars(name, 18),
                p.tokens_in,
                p.tokens_out,
                p.cost_usd,
                p.session_count
            ));
        }
    }
    if !usage.by_day.is_empty() {
        out.push_str("\n\nBy day:");
        for day in usage.by_day.iter().rev().take(7).rev() {
            out.push_str(&format!(
                "\n  {}  {} in / {} out / ${:.4}",
                day.date, day.tokens_in, day.tokens_out, day.cost_usd
            ));
        }
    }
    out
}

fn parse_usage_window(raw: &str) -> Option<(&'static str, UsageWindow)> {
    match raw {
        "today" | "day" => Some(("today", UsageWindow::Today)),
        "week" => Some(("week", UsageWindow::LastDays(7))),
        "month" => Some(("month", UsageWindow::ThisMonth)),
        "all" => Some(("all", UsageWindow::All)),
        _ => None,
    }
}

fn model_label(config: &Config, model_id: &str) -> String {
    config
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| format!("{} ({})", m.id, m.name))
        .unwrap_or_else(|| model_id.to_string())
}

fn sorted_models(config: &Config) -> Vec<&crate::config::ModelConfig> {
    let mut models: Vec<_> = config.models.iter().collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

fn available_role_names(config: &Config) -> String {
    let mut roles: Vec<_> = config.roles.keys().cloned().collect();
    roles.sort();
    if roles.is_empty() {
        "(none)".into()
    } else {
        roles.join(", ")
    }
}

fn available_model_names(config: &Config) -> String {
    let mut models: Vec<_> = config.models.iter().map(|m| m.id.clone()).collect();
    models.sort();
    if models.is_empty() {
        "(none)".into()
    } else {
        models.join(", ")
    }
}

fn available_runtime_names(config: &Config) -> String {
    let mut runtimes: Vec<_> = config.runtimes.keys().cloned().collect();
    runtimes.sort();
    if runtimes.is_empty() {
        "(none)".into()
    } else {
        runtimes.join(", ")
    }
}

fn format_tui_status(store: &SessionStore, config: &Config, input: &InputState) -> String {
    let sessions = store.list(1).unwrap_or_default();
    let latest = sessions
        .first()
        .map(|s| format!("{} {}", session_id8(s), format_session_state(s)))
        .unwrap_or_else(|| "none".into());
    let run = if input.run_rx.is_some() || input.run_handle.is_some() {
        if input.current_stage.is_empty() {
            "active".to_string()
        } else if input.current_stage_detail.is_empty() {
            input.current_stage.clone()
        } else {
            format!("{} ({})", input.current_stage, input.current_stage_detail)
        }
    } else {
        "idle".into()
    };

    let process_file = input
        .process_capture_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "none".into());

    format!(
        "Terminal chat status:\n  run: {}\n  messages: {}\n  process view: {}\n  context memory: {}\n  queued messages: {} ({})\n  agent hint: {}\n  configured roles: {}\n  configured models: {}\n  live trace: {}\n  captured process events: {}\n  process filter: {}\n  process file: {}\n  latest session: {}\n  log dir: {}",
        run,
        input.transcript.len(),
        if input.history_folded {
            "folded (/o expands future details)"
        } else {
            "expanded (/o folds future details)"
        },
        context_summary(input),
        input.queued_prompts.len(),
        queue_state_label(input),
        input.agent_hint.as_deref().unwrap_or("auto"),
        config.roles.len(),
        config.models.len(),
        live_trace_summary(input),
        input.run_events.len(),
        process_filter_summary(input),
        process_file,
        latest,
        store.log_dir().display()
    )
}

fn format_doctor_command(runtime: &TuiRuntimeConfig) -> String {
    let Some(config_path) = runtime.config_path.as_deref() else {
        return "Doctor cannot run because the terminal chat was started without a config path."
            .into();
    };
    crate::doctor::run(config_path).render_text()
}

fn format_queue(input: &InputState) -> String {
    if input.queued_prompts.is_empty() {
        return "Queue is empty.\n\nWhile a run is active, type a normal message to queue it."
            .into();
    }

    let mut out = format!(
        "Queued batch ({}) — {}:",
        input.queued_prompts.len(),
        queue_state_label(input)
    );
    for (idx, prompt) in input.queued_prompts.iter().enumerate() {
        out.push('\n');
        out.push_str(&format!("  ↳ {}. {}", idx + 1, truncate_chars(prompt, 180)));
    }
    if input.queue_paused {
        out.push_str("\n\nPaused: the batch will stay here until /queue resume or /queue run.");
    } else {
        out.push_str(
            "\n\nExecution: all queued messages are merged into one follow-up prompt and run together after the current task finishes.",
        );
    }
    out.push_str(
        "\nManage: /queue edit <n> <text>, /queue promote <n>, /queue remove <n>, /queue pause, /queue clear.",
    );
    out
}

fn handle_queue_command(input: &mut InputState, args: &[&str]) -> String {
    match args.first().copied() {
        None | Some("show" | "list" | "status") => format_queue(input),
        Some("clear") => clear_queue(input),
        Some("remove" | "rm" | "delete") => remove_queued_prompt(input, args.get(1).copied()),
        Some("promote" | "up" | "next") => promote_queued_prompt(input, args.get(1).copied()),
        Some("edit" | "set") => {
            edit_queued_prompt(input, args.get(1).copied(), args.get(2..).unwrap_or(&[]))
        }
        Some("pause" | "hold") => pause_queue(input),
        Some("resume" | "unpause") => resume_queue(input),
        Some("run" | "start" | "go") => run_queue_now(input),
        Some(other) => format!(
            "Unknown queue option: {}\nUsage: /queue [show|clear|remove <n>|promote <n>|edit <n> <text>|pause|resume|run]",
            other
        ),
    }
}

fn queue_state_label(input: &InputState) -> &'static str {
    if input.queue_paused {
        "paused"
    } else {
        "ready"
    }
}

fn clear_queue(input: &mut InputState) -> String {
    let count = input.queued_prompts.len();
    if count == 0 {
        return "Queue is already empty.".into();
    }

    input.queued_prompts.clear();
    format!("Cleared {} queued message(s).", count)
}

fn remove_queued_prompt(input: &mut InputState, raw_index: Option<&str>) -> String {
    let index = match parse_queue_index(raw_index, input.queued_prompts.len(), "remove") {
        Ok(index) => index,
        Err(msg) => return msg,
    };
    let prompt = input.queued_prompts.remove(index);
    format!(
        "Removed queued message {}: {}\n{} pending.",
        index + 1,
        truncate_chars(&prompt, 160),
        input.queued_prompts.len()
    )
}

fn promote_queued_prompt(input: &mut InputState, raw_index: Option<&str>) -> String {
    let index = match parse_queue_index(raw_index, input.queued_prompts.len(), "promote") {
        Ok(index) => index,
        Err(msg) => return msg,
    };
    if index == 0 {
        return format!(
            "Queued message 1 is already next: {}",
            truncate_chars(&input.queued_prompts[0], 160)
        );
    }

    let prompt = input.queued_prompts.remove(index);
    input.queued_prompts.insert(0, prompt.clone());
    format!(
        "Promoted queued message {} to next: {}\n{} pending.",
        index + 1,
        truncate_chars(&prompt, 160),
        input.queued_prompts.len()
    )
}

fn edit_queued_prompt(
    input: &mut InputState,
    raw_index: Option<&str>,
    raw_text: &[&str],
) -> String {
    let index = match parse_queue_index(raw_index, input.queued_prompts.len(), "edit") {
        Ok(index) => index,
        Err(msg) => return msg,
    };
    let text = raw_text.join(" ").trim().to_string();
    if text.is_empty() {
        return "Usage: /queue edit <n> <replacement text>".into();
    }

    input.queued_prompts[index] = text.clone();
    format!(
        "Edited queued message {}: {}",
        index + 1,
        truncate_chars(&text, 160)
    )
}

fn pause_queue(input: &mut InputState) -> String {
    input.queue_paused = true;
    if input.queued_prompts.is_empty() {
        "Queue paused. New follow-up messages will wait until /queue resume.".into()
    } else {
        format!(
            "Queue paused. {} queued message(s) will wait after the current run.",
            input.queued_prompts.len()
        )
    }
}

fn resume_queue(input: &mut InputState) -> String {
    input.queue_paused = false;
    if input.queued_prompts.is_empty() {
        "Queue resumed. No queued messages are pending.".into()
    } else {
        format!(
            "Queue resumed. {} queued message(s) ready to run.",
            input.queued_prompts.len()
        )
    }
}

fn run_queue_now(input: &mut InputState) -> String {
    input.queue_paused = false;
    if input.queued_prompts.is_empty() {
        "Queue is empty.".into()
    } else if input.run_rx.is_some() || input.run_handle.is_some() {
        format!(
            "Queue will run after the active task finishes ({} message(s)).",
            input.queued_prompts.len()
        )
    } else {
        format!(
            "Starting queued batch now ({} message(s)).",
            input.queued_prompts.len()
        )
    }
}

fn parse_queue_index(
    raw_index: Option<&str>,
    queue_len: usize,
    action: &str,
) -> std::result::Result<usize, String> {
    if queue_len == 0 {
        return Err("Queue is empty.".into());
    }

    let Some(raw_index) = raw_index else {
        return Err(format!("Usage: /queue {} <n>", action));
    };
    let Ok(index) = raw_index.parse::<usize>() else {
        return Err(format!(
            "Invalid queue index: {}\nUsage: /queue {} <n>",
            raw_index, action
        ));
    };
    if index == 0 || index > queue_len {
        return Err(format!(
            "Queue index out of range: {} (valid: 1..={})",
            index, queue_len
        ));
    }
    Ok(index - 1)
}

fn format_diff_command(args: &[&str]) -> String {
    match args.first().copied().unwrap_or("summary") {
        "summary" | "status" | "files" => format_git_diff_summary(),
        "stat" | "stats" => format_git_command(
            "Diff stat",
            &["diff", "--stat", "--color=never"],
            8_000,
            "No unstaged diff.",
        ),
        "full" | "patch" => format_git_command(
            "Git diff",
            &["diff", "--color=never"],
            20_000,
            "No unstaged diff.",
        ),
        "cached" | "staged" => format_git_command(
            "Staged diff",
            &["diff", "--cached", "--color=never"],
            20_000,
            "No staged diff.",
        ),
        other => format!(
            "Unknown diff option: {}\nUsage: /diff [summary|stat|full|cached]",
            other
        ),
    }
}

fn format_git_diff_summary() -> String {
    let status = match run_git(&["status", "--short", "--branch"]) {
        Ok(output) => output,
        Err(e) => return e,
    };
    let stat = run_git(&["diff", "--stat", "--color=never"])
        .ok()
        .filter(|s| !s.trim().is_empty());
    let staged = run_git(&["diff", "--cached", "--stat", "--color=never"])
        .ok()
        .filter(|s| !s.trim().is_empty());

    let mut out = String::from("Git changes:");
    if status.trim().is_empty() {
        out.push_str("\n  working tree clean");
    } else {
        out.push('\n');
        out.push_str(&indent_block(
            &truncate_chars(status.trim_end(), 8_000),
            "  ",
        ));
    }
    if let Some(stat) = stat {
        out.push_str("\n\nUnstaged stat:\n");
        out.push_str(&indent_block(&truncate_chars(stat.trim_end(), 6_000), "  "));
    }
    if let Some(staged) = staged {
        out.push_str("\n\nStaged stat:\n");
        out.push_str(&indent_block(
            &truncate_chars(staged.trim_end(), 6_000),
            "  ",
        ));
    }
    out.push_str("\n\nMore: /diff full, /diff cached");
    out
}

fn format_git_command(title: &str, args: &[&str], max_chars: usize, empty: &str) -> String {
    let output = match run_git(args) {
        Ok(output) => output,
        Err(e) => return e,
    };
    if output.trim().is_empty() {
        return empty.into();
    }

    let truncated = truncate_chars(output.trim_end(), max_chars);
    let mut out = format!("{}:", title);
    out.push('\n');
    out.push_str(&indent_block(&truncated, "  "));
    if output.chars().count() > max_chars {
        out.push_str("\n\nOutput truncated. Use git directly for the full patch.");
    }
    out
}

fn run_git(args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("Could not run git: {}", e))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("git {:?} exited with status {}", args, output.status)
        } else {
            stderr
        };
        Err(format!("Git command failed: {}", message))
    }
}

fn handle_tests_command(input: &mut InputState, args: &[&str]) -> String {
    match args.first().copied().unwrap_or("suggest") {
        "suggest" | "plan" => format_test_suggestions(),
        "cargo" | "rust" => {
            "Suggested Rust test commands:\n  cargo test tui::\n  cargo test acp::tests::acp_protocol_flow_initializes_creates_session_and_runs_prompt\n  cargo test runtime\n  cargo test\n\nRun the focused suite in the background with: /tests run quick"
                .into()
        }
        "quick" => start_test_run(input, &["quick"]),
        "run" | "start" => start_test_run(input, args.get(1..).unwrap_or(&[])),
        "last" | "status" => format_test_status(input),
        "tail" | "log" => {
            let lines = parse_count(args.get(1).copied(), 60, 1, 300);
            format_test_log_tail(input, lines)
        }
        other => format!(
            "Unknown tests option: {}\nUsage: /tests [suggest|cargo|run|status|tail]",
            other
        ),
    }
}

fn format_test_suggestions() -> String {
    let mut out = String::from("Test suggestions:");
    let cargo = std::path::Path::new("Cargo.toml").exists();
    let package_json = std::path::Path::new("package.json").exists();
    let pyproject = std::path::Path::new("pyproject.toml").exists();

    if cargo {
        out.push_str("\n  cargo test tui::");
        out.push_str("\n  cargo test acp::tests::acp_protocol_flow_initializes_creates_session_and_runs_prompt");
        out.push_str("\n  cargo test runtime");
        out.push_str("\n  cargo test");
    }
    if package_json {
        out.push_str("\n  npm test");
    }
    if pyproject {
        out.push_str("\n  pytest");
    }
    if !cargo && !package_json && !pyproject {
        out.push_str("\n  No known test manifest found in the current directory.");
    }

    out.push_str("\n\nRun in the background with: /tests run cargo");
    out
}

fn start_test_run(input: &mut InputState, args: &[&str]) -> String {
    if let Some(child) = input.test_child.as_mut() {
        match child.try_wait() {
            Ok(None) => {
                return "A test run is already active. Use /tests status or /tests tail.".into()
            }
            Ok(Some(status)) => {
                input.last_test_status = Some(format!("finished before restart: {}", status));
                input.test_child = None;
            }
            Err(e) => {
                input.last_test_status = Some(format!("status check failed: {}", e));
                input.test_child = None;
            }
        }
    }

    let Some(spec) = test_command_spec(args) else {
        return "Usage: /tests run cargo|tui|quick|npm|pytest|<custom command>".into();
    };
    let log_path = match create_timestamped_file_path(".orchestrator/tests", "test", "log") {
        Ok(path) => path,
        Err(e) => return format!("Could not create test log path: {}", e),
    };
    let stdout = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(file) => file,
        Err(e) => return format!("Could not open test log {}: {}", log_path.display(), e),
    };
    let stderr = match stdout.try_clone() {
        Ok(file) => file,
        Err(e) => return format!("Could not clone test log handle: {}", e),
    };

    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    command.stdout(Stdio::from(stdout));
    command.stderr(Stdio::from(stderr));
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return format!("Could not start test command `{}`: {}", spec.display(), e),
    };

    input.test_child = Some(child);
    input.test_log_path = Some(log_path.clone());
    input.last_test_status = Some(format!("running: {}", spec.display()));
    format!(
        "Started test run:\n  command: {}\n  log: {}\n\nUse /tests status or /tests tail.",
        spec.display(),
        log_path.display()
    )
}

struct TestCommandSpec {
    program: String,
    args: Vec<String>,
}

impl TestCommandSpec {
    fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

fn test_command_spec(args: &[&str]) -> Option<TestCommandSpec> {
    match args {
        [] => Some(TestCommandSpec {
            program: "cargo".into(),
            args: vec!["test".into()],
        }),
        ["cargo", rest @ ..] => Some(TestCommandSpec {
            program: "cargo".into(),
            args: if rest.is_empty() {
                vec!["test".into()]
            } else {
                rest.iter().map(|s| (*s).to_string()).collect()
            },
        }),
        ["tui", ..] => Some(TestCommandSpec {
            program: "cargo".into(),
            args: vec!["test".into(), "tui::".into()],
        }),
        ["quick", ..] => Some(TestCommandSpec {
            program: default_shell_program().into(),
            args: vec![
                default_shell_flag().into(),
                "cargo test tui:: && cargo test acp::tests::acp_protocol_flow_initializes_creates_session_and_runs_prompt && cargo test runtime".into(),
            ],
        }),
        ["npm", rest @ ..] => Some(TestCommandSpec {
            program: "npm".into(),
            args: if rest.is_empty() {
                vec!["test".into()]
            } else {
                rest.iter().map(|s| (*s).to_string()).collect()
            },
        }),
        ["pytest", rest @ ..] => Some(TestCommandSpec {
            program: "pytest".into(),
            args: rest.iter().map(|s| (*s).to_string()).collect(),
        }),
        [program, rest @ ..] => Some(TestCommandSpec {
            program: (*program).to_string(),
            args: rest.iter().map(|s| (*s).to_string()).collect(),
        }),
    }
}

fn format_test_status(input: &mut InputState) -> String {
    let Some(child) = input.test_child.as_mut() else {
        let status = input
            .last_test_status
            .as_deref()
            .unwrap_or("no test run started");
        return format!(
            "Test status: {}\n  log: {}",
            status,
            optional_path_label(input.test_log_path.as_ref())
        );
    };

    match child.try_wait() {
        Ok(None) => format!(
            "Test status: running\n  log: {}",
            optional_path_label(input.test_log_path.as_ref())
        ),
        Ok(Some(status)) => {
            input.last_test_status = Some(format!("finished: {}", status));
            input.test_child = None;
            format!(
                "Test status: finished ({})\n  log: {}",
                status,
                optional_path_label(input.test_log_path.as_ref())
            )
        }
        Err(e) => {
            input.last_test_status = Some(format!("status check failed: {}", e));
            input.test_child = None;
            format!(
                "Test status check failed: {}\n  log: {}",
                e,
                optional_path_label(input.test_log_path.as_ref())
            )
        }
    }
}

fn format_test_log_tail(input: &InputState, lines: usize) -> String {
    let Some(path) = input.test_log_path.as_ref() else {
        return "No test log yet. Start one with /tests run cargo.".into();
    };
    format_file_tail(path, lines, "Test log")
}

fn indent_block(text: &str, prefix: &str) -> String {
    if text.is_empty() {
        return prefix.to_string();
    }
    text.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn handle_handoff_command(
    store: &SessionStore,
    config: &Config,
    input: &mut InputState,
    args: &[&str],
) -> String {
    match args.first().copied().unwrap_or("status") {
        "claude" | "codex" | "auto" => save_handoff_package(store, config, input, args[0]),
        "copy" | "clipboard" => copy_last_handoff(input),
        "status" | "last" => format!(
            "Last handoff package:\n  {}",
            optional_path_label(input.last_handoff_path.as_ref())
        ),
        other => format!(
            "Unknown handoff target: {}\nUsage: /handoff claude|codex|copy|status",
            other
        ),
    }
}

pub(super) fn save_handoff_package(
    store: &SessionStore,
    config: &Config,
    input: &mut InputState,
    target: &str,
) -> String {
    let path = match create_timestamped_file_path(".orchestrator/handoff", target, "md") {
        Ok(path) => path,
        Err(e) => return format!("Could not create handoff path: {}", e),
    };
    let body = format_handoff_package(store, config, input, target);
    if let Err(e) = fs::write(&path, body) {
        return format!("Could not write handoff package {}: {}", path.display(), e);
    }
    input.last_handoff_path = Some(path.clone());
    format!(
        "Handoff package saved for {}:\n  {}\n\nUse it from the target CLI, or /handoff copy to copy the package.",
        target,
        path.display()
    )
}

fn format_handoff_package(
    store: &SessionStore,
    config: &Config,
    input: &InputState,
    target: &str,
) -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(unknown)".into());
    let mut out = format!(
        "# Orchestrator Handoff: {}\n\nGenerated for: {}\nWorking directory: {}\n\n",
        target, target, cwd
    );
    out.push_str("## Current State\n\n");
    out.push_str(&format!(
        "- Selected worker route: {}\n- Context memory: {}\n- Last result: {}\n- Process log: {}\n- Session log dir: {}\n\n",
        runtime::agent_label(input.agent_hint.as_deref()),
        context_summary(input),
        input
            .last_result_output
            .as_deref()
            .map(|s| truncate_chars(s, 220))
            .unwrap_or_else(|| "none".into()),
        optional_path_label(input.process_capture_path.as_ref()),
        store.log_dir().display()
    ));

    out.push_str("## Role Flow\n\n");
    out.push_str("```text\n");
    out.push_str(&format_role_workflow(config, input));
    out.push_str("\n```\n\n");

    out.push_str("## Conversation Flow\n\n");
    out.push_str("```text\n");
    out.push_str(&conversation_graph(input, 18));
    out.push_str("\n```\n\n");

    out.push_str("## Recent Transcript\n\n");
    let recent = input.transcript.len().saturating_sub(16);
    for msg in &input.transcript[recent..] {
        out.push_str(&format!(
            "### {} ({})\n\n{}\n\n",
            handoff_role_label(msg),
            msg.status,
            truncate_chars(&msg.content, 2_000)
        ));
    }

    if let Some(result) = input.last_result_output.as_deref() {
        out.push_str("## Final Result\n\n");
        out.push_str(result.trim());
        out.push_str("\n\n");
    }

    if let Ok(diff) = run_git(&["diff", "--stat", "--color=never"]) {
        if !diff.trim().is_empty() {
            out.push_str("## Current Git Diff Stat\n\n```text\n");
            out.push_str(diff.trim_end());
            out.push_str("\n```\n\n");
        }
    }

    out.push_str("## Suggested Continuation Prompt\n\n");
    out.push_str("Continue from this orchestrator handoff. Preserve the current goal, inspect the referenced files/logs as needed, and avoid repeating completed work.\n");
    out
}

fn handoff_role_label(msg: &ChatMsg) -> &'static str {
    match msg.role.as_str() {
        "user" => "User",
        "assistant" => "Orchestrator",
        "trace" => "Process Trace",
        "system" => "System",
        _ => "Message",
    }
}

fn copy_last_handoff(input: &InputState) -> String {
    let Some(path) = input.last_handoff_path.as_ref() else {
        return "No handoff package yet. Use /handoff claude or /handoff codex first.".into();
    };
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(e) => return format!("Could not read handoff package {}: {}", path.display(), e),
    };
    match copy_to_clipboard(&body) {
        Ok(()) => format!(
            "Copied handoff package to clipboard ({} bytes).",
            body.len()
        ),
        Err(e) => format!("Handoff clipboard copy failed: {}", e),
    }
}

fn format_graph_command(input: &InputState, args: &[&str]) -> String {
    match args.first().copied().unwrap_or("flow") {
        "compact" | "flow" | "summary" => conversation_graph(input, 14),
        "full" | "all" => conversation_graph(input, 32),
        "mermaid" | "diagram" => conversation_mermaid_graph(input, 24),
        "save" | "file" => save_conversation_mermaid_graph(input),
        "clipboard" | "copy" => copy_conversation_mermaid_graph(input),
        other => {
            return format!(
                "Unknown graph option: {}\nUsage: /graph [compact|full|mermaid|save|clipboard]",
                other
            );
        }
    }
}

fn conversation_graph(input: &InputState, limit: usize) -> String {
    let mut out = String::from("Conversation flow:");
    if input.transcript.is_empty() {
        out.push_str("\n  (no messages yet)");
        return out;
    }

    let start = input.transcript.len().saturating_sub(limit);
    if start > 0 {
        out.push_str(&format!("\n  ... {} earlier message(s)", start));
    }
    for (idx, msg) in input.transcript[start..].iter().enumerate() {
        let number = start + idx + 1;
        let label = graph_label(msg);
        let summary = graph_message_summary(msg);
        out.push_str(&format!("\n  {}. {} -> {}", number, label, summary));
    }
    if !input.queued_prompts.is_empty() {
        out.push_str(&format!(
            "\n  queue -> {} pending message(s) ({})",
            input.queued_prompts.len(),
            queue_state_label(input)
        ));
    }
    out
}

fn conversation_mermaid_graph(input: &InputState, limit: usize) -> String {
    let mut out = String::from("Conversation Mermaid flowchart:\n```mermaid\nflowchart TD\n");
    if input.transcript.is_empty() {
        out.push_str("  empty[\"No messages yet\"]\n");
        out.push_str("```\n");
        return out;
    }

    out.push_str(&conversation_mermaid_body(input, limit));
    out.push_str("```\n");
    out
}

fn conversation_mermaid_body(input: &InputState, limit: usize) -> String {
    let mut out = String::new();
    let start = input.transcript.len().saturating_sub(limit);
    if start > 0 {
        out.push_str(&format!(
            "  earlier[\"{} earlier message(s)\"]:::system\n",
            start
        ));
    }

    let mut previous = if start > 0 {
        Some("earlier".to_string())
    } else {
        None
    };
    for (idx, msg) in input.transcript[start..].iter().enumerate() {
        let number = start + idx + 1;
        let node = format!("m{}", number);
        let label = format!(
            "{}: {}",
            graph_label(msg),
            truncate_chars(&graph_message_summary(msg), 72)
        );
        out.push_str(&format!(
            "  {}[\"{}\"]:::{}\n",
            node,
            escape_mermaid_label(&label),
            mermaid_class(msg)
        ));
        if let Some(prev) = previous {
            out.push_str(&format!("  {} --> {}\n", prev, node));
        }
        previous = Some(node);
    }

    if !input.queued_prompts.is_empty() {
        out.push_str(&format!(
            "  queue[\"queue: {} pending ({})\"]:::queue\n",
            input.queued_prompts.len(),
            escape_mermaid_label(&queue_state_label(input))
        ));
        if let Some(prev) = previous {
            out.push_str(&format!("  {} --> queue\n", prev));
        }
    }
    out.push_str("  classDef user fill:#e6fffb,stroke:#81d8d0,color:#16302f\n");
    out.push_str("  classDef assistant fill:#f6fffe,stroke:#81d8d0,color:#16302f\n");
    out.push_str("  classDef system fill:#f5f5f5,stroke:#b8b8b8,color:#444\n");
    out.push_str("  classDef trace fill:#fffbe6,stroke:#d6b656,color:#3f3200\n");
    out.push_str("  classDef queue fill:#fff7e6,stroke:#d6a656,color:#3f2800\n");
    out
}

fn save_conversation_mermaid_graph(input: &InputState) -> String {
    let path = match create_timestamped_file_path(".orchestrator/graphs", "conversation", "mmd") {
        Ok(path) => path,
        Err(e) => return format!("Could not create graph path: {}", e),
    };
    let body = format!("flowchart TD\n{}", conversation_mermaid_body(input, 48));
    if let Err(e) = fs::write(&path, body) {
        return format!("Could not write graph {}: {}", path.display(), e);
    }
    format!("Conversation graph saved:\n  {}", path.display())
}

fn copy_conversation_mermaid_graph(input: &InputState) -> String {
    let body = format!("flowchart TD\n{}", conversation_mermaid_body(input, 48));
    match copy_to_clipboard(&body) {
        Ok(()) => format!("Copied Mermaid graph to clipboard ({} bytes).", body.len()),
        Err(e) => format!("Mermaid graph clipboard copy failed: {}", e),
    }
}

fn mermaid_class(msg: &ChatMsg) -> &'static str {
    match msg.role.as_str() {
        "user" => "user",
        "assistant" => "assistant",
        "trace" => "trace",
        _ => "system",
    }
}

fn escape_mermaid_label(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('[', "(")
        .replace(']', ")")
        .replace('{', "(")
        .replace('}', ")")
        .replace('\n', " ")
}

fn format_acp_command(config: &Config, input: &InputState, args: &[&str]) -> String {
    let target = args.first().copied().unwrap_or("status");
    let agent = runtime::agent_label(input.agent_hint.as_deref());
    let command = format!("orchestrator acp --agent {}", agent);
    match target {
        "status" | "show" => format!(
            "ACP server\n  protocol: Agent Client Protocol over stdio\n  command: {}\n  selected worker route: {}\n  configured roles: {}\n  session store: ~/.orchestrator/sessions/acp\n\nUse /acp claude or /acp codex for client setup hints.",
            command,
            agent,
            config.roles.len()
        ),
        "command" | "cmd" => command,
        "claude" | "claude-code" => format!(
            "Claude ACP setup hint\n  server command: {}\n  transport: stdio\n\nAdd this server command in the Claude client surface that supports ACP, then start a session from that client. Inside orchestrator TUI, /handoff open claude remains the lightweight inline handoff path.",
            command
        ),
        "codex" => format!(
            "Codex ACP setup hint\n  server command: {}\n  transport: stdio\n\nUse this command from a Codex client surface that supports ACP. For local inline continuation without ACP, use /continue codex or /handoff open codex.",
            command
        ),
        other => format!("Unknown ACP option: {}\nUsage: /acp [status|command|claude|codex]", other),
    }
}

fn graph_label(msg: &ChatMsg) -> &'static str {
    match msg.role.as_str() {
        "user" => "user request",
        "assistant" if msg.status == "thinking" => "orchestrator running",
        "assistant" if msg.status == "error" => "error",
        "assistant" if msg.status == "warning" => "review warning",
        "assistant" => "orchestrator result",
        "trace" => "process trace",
        "system" => "system command",
        _ => "message",
    }
}

fn graph_message_summary(msg: &ChatMsg) -> String {
    let one_line = msg
        .content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('╭') && !line.starts_with('╰'))
        .unwrap_or("");
    truncate_chars(one_line, 96)
}

fn handle_checkpoint_command(input: &mut InputState, args: &[&str]) -> String {
    match args.first().copied().unwrap_or("save") {
        "save" | "create" => save_checkpoint(input),
        "status" | "last" => format!(
            "Last checkpoint:\n  {}",
            optional_path_label(input.last_checkpoint_path.as_ref())
        ),
        other => format!(
            "Unknown checkpoint option: {}\nUsage: /checkpoint [save|status]",
            other
        ),
    }
}

fn save_checkpoint(input: &mut InputState) -> String {
    let patch = match run_git(&["diff", "HEAD", "--binary", "--color=never"]) {
        Ok(diff) => diff,
        Err(e) => return e,
    };
    if patch.trim().is_empty() {
        return "No tracked git changes to checkpoint.".into();
    }

    let path =
        match create_timestamped_file_path(".orchestrator/checkpoints", "checkpoint", "patch") {
            Ok(path) => path,
            Err(e) => return format!("Could not create checkpoint path: {}", e),
        };
    if let Err(e) = fs::write(&path, patch) {
        return format!("Could not write checkpoint {}: {}", path.display(), e);
    }
    input.last_checkpoint_path = Some(path.clone());
    format!(
        "Checkpoint saved:\n  {}\n\nRollback tracked changes with: /rollback last",
        path.display()
    )
}

fn handle_rollback_command(input: &mut InputState, args: &[&str]) -> String {
    match args.first().copied().unwrap_or("last") {
        "last" | "apply" => rollback_checkpoint(input, false),
        "check" | "dry-run" => rollback_checkpoint(input, true),
        other => format!(
            "Unknown rollback option: {}\nUsage: /rollback [last|check]",
            other
        ),
    }
}

fn rollback_checkpoint(input: &InputState, check_only: bool) -> String {
    let Some(path) = input.last_checkpoint_path.as_ref() else {
        return "No checkpoint saved in this terminal session. Use /checkpoint first.".into();
    };
    if !path.exists() {
        return format!("Checkpoint patch does not exist: {}", path.display());
    }

    let patch_path = path.to_string_lossy().to_string();
    let mut args = vec!["apply", "-R"];
    if check_only {
        args.push("--check");
    }
    args.push(&patch_path);
    let output = Command::new("git").args(&args).output();
    match output {
        Ok(output) if output.status.success() => {
            if check_only {
                format!("Rollback check passed for:\n  {}", path.display())
            } else {
                format!(
                    "Rolled back tracked changes from checkpoint:\n  {}",
                    path.display()
                )
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!(
                "Rollback failed for:\n  {}\n\n{}",
                path.display(),
                truncate_chars(stderr.trim(), 1_200)
            )
        }
        Err(e) => format!("Could not run git apply: {}", e),
    }
}

fn create_timestamped_file_path(
    relative_dir: &str,
    prefix: &str,
    extension: &str,
) -> std::io::Result<PathBuf> {
    let home = home::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home dir"))?;
    let dir = home.join(relative_dir);
    fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let pid = std::process::id();
    for attempt in 0..100 {
        let path = dir.join(format!(
            "{}-{}-{}-{}.{}",
            prefix, ts, pid, attempt, extension
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate unique timestamped file path",
    ))
}

fn default_shell_program() -> &'static str {
    if cfg!(windows) {
        "cmd"
    } else {
        "sh"
    }
}

fn default_shell_flag() -> &'static str {
    if cfg!(windows) {
        "/C"
    } else {
        "-c"
    }
}

fn optional_path_label(path: Option<&PathBuf>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "none".into())
}

fn format_file_tail(path: &Path, lines: usize, title: &str) -> String {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) => {
            return format!(
                "Could not open {} {}: {}",
                title.to_lowercase(),
                path.display(),
                e
            )
        }
    };
    let mut body = String::new();
    if let Err(e) = file
        .seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_string(&mut body))
    {
        return format!(
            "Could not read {} {}: {}",
            title.to_lowercase(),
            path.display(),
            e
        );
    }
    let raw_lines: Vec<_> = body.lines().collect();
    let start = raw_lines.len().saturating_sub(lines);
    let mut out = format!(
        "{} tail (last {} of {} line(s)):\n  {}",
        title,
        raw_lines.len().saturating_sub(start),
        raw_lines.len(),
        path.display()
    );
    for line in &raw_lines[start..] {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&truncate_chars(line, 220));
    }
    out
}

pub(super) fn format_sessions(store: &SessionStore, limit: usize) -> String {
    match store.list(10_000) {
        Ok(sessions) => {
            let shown = sessions.iter().take(limit).cloned().collect::<Vec<_>>();
            crate::session_display::format_session_list(&shown, &sessions)
        }
        Err(e) => format!("Could not list sessions: {:#}", e),
    }
}

const IMPORT_CHAT_EVENT_LIMIT: usize = 80;
const IMPORT_CHAT_TEXT_MAX_CHARS: usize = 8_000;

fn handle_import_cc(store: &SessionStore, project: Option<&Path>, input: &mut InputState) {
    let cwd = match project {
        Some(path) => path.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(path) => path,
            Err(err) => {
                push_system(
                    input,
                    format!("Could not resolve current project directory: {err}"),
                );
                return;
            }
        },
    };

    let sessions = crate::cc_session_import::load_cc_sessions(&cwd);
    match crate::cc_session_import::import_cc_sessions(store, &cwd) {
        Ok(report) => {
            let selected = select_imported_cc_session(&sessions, &report.session_ids);
            let restored = selected
                .map(build_imported_cc_chat_messages)
                .unwrap_or_else(|| (Vec::new(), false));
            let restored_count = restored.0.len();
            let restored_truncated = restored.1;
            let mut out = format_import_cc_report(&report, selected, restored_count);
            if restored_truncated {
                out.push_str(&format!(
                    "\n  restored chat truncated at {} readable event(s); use /log for full history",
                    IMPORT_CHAT_EVENT_LIMIT
                ));
            }
            push_system(input, out);
            input.transcript.extend(restored.0);
        }
        Err(err) => push_system(
            input,
            format!("Could not import Claude Code sessions: {err:#}"),
        ),
    }
}

fn format_import_cc_report(
    report: &crate::cc_session_import::CCImportReport,
    restored: Option<&crate::cc_session_import::CCSession>,
    restored_count: usize,
) -> String {
    let mut out = format!(
        "Claude Code import\n  project: {}\n  discovered: {}\n  imported: {}\n  backfilled: {}\n  skipped: {}",
        report.project.display(),
        report.discovered,
        report.imported,
        report.backfilled,
        report.skipped
    );
    if report.session_ids.is_empty() {
        out.push_str("\n  no new session logs imported");
    } else {
        out.push_str("\n\nImported session ids:");
        for id in report.session_ids.iter().take(12) {
            let short = id.to_string().chars().take(8).collect::<String>();
            out.push_str(&format!("\n  {}  /log {} 120", short, short));
        }
        if report.session_ids.len() > 12 {
            out.push_str(&format!(
                "\n  ... {} more; use /sessions 50",
                report.session_ids.len() - 12
            ));
        }
    }

    if let Some(session) = restored {
        let short = crate::cc_session_import::cc_session_uuid(&session.id)
            .to_string()
            .chars()
            .take(8)
            .collect::<String>();
        out.push_str(&format!(
            "\n\nRestored into chat:\n  {} message(s) from {}  /log {} 120",
            restored_count, short, short
        ));
        if let Some(summary) = session.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            out.push_str(&format!("\n  summary: {}", truncate_chars(summary, 160)));
        }
    } else {
        out.push_str("\n\nRestored into chat:\n  no Claude Code session found for this project");
    }

    out
}

fn select_imported_cc_session<'a>(
    sessions: &'a [crate::cc_session_import::CCSession],
    imported_ids: &[uuid::Uuid],
) -> Option<&'a crate::cc_session_import::CCSession> {
    sessions
        .iter()
        .find(|session| {
            let id = crate::cc_session_import::cc_session_uuid(&session.id);
            imported_ids.iter().any(|imported| imported == &id)
        })
        .or_else(|| sessions.first())
}

fn build_imported_cc_chat_messages(
    session: &crate::cc_session_import::CCSession,
) -> (Vec<ChatMsg>, bool) {
    let mut messages = Vec::new();
    let mut scanned = 0usize;
    for event in &session.events {
        if scanned >= IMPORT_CHAT_EVENT_LIMIT {
            return (messages, true);
        }
        scanned += 1;
        if let Some(msg) = imported_cc_event_to_chat_msg(event) {
            messages.push(msg);
        }
    }
    (messages, false)
}

fn imported_cc_event_to_chat_msg(event: &crate::cc_session_import::CCEvent) -> Option<ChatMsg> {
    let role = match event.kind.as_str() {
        "user" => "user",
        "assistant" => "assistant",
        "tool_use" | "tool_result" | "result" => "tool",
        _ => "tool",
    };
    let content = match event.kind.as_str() {
        "tool_use" | "tool_result" | "result" => {
            crate::agent_events::sanitize_text(event.content.trim(), IMPORT_CHAT_TEXT_MAX_CHARS)
        }
        _ => humanize_jsonish(&event.content, IMPORT_CHAT_TEXT_MAX_CHARS),
    };
    let content = content.trim();
    if content.is_empty() || is_low_value_execution_output(content) {
        return None;
    }

    Some(ChatMsg {
        role: role.into(),
        content: content.to_string(),
        ts: system_time_from_chrono(event.ts),
        status: "complete".into(),
    })
}

fn system_time_from_chrono(ts: chrono::DateTime<chrono::Utc>) -> std::time::SystemTime {
    let millis = ts.timestamp_millis();
    if millis <= 0 {
        UNIX_EPOCH
    } else {
        UNIX_EPOCH + Duration::from_millis(millis as u64)
    }
}

fn format_session_detail(store: &SessionStore, selector: Option<&str>) -> String {
    match resolve_session(store, selector) {
        Ok(s) => {
            let files = if s.files_touched.is_empty() {
                "none".to_string()
            } else {
                truncate_chars(&s.files_touched.join(", "), 240)
            };
            let parents = if s.parent_session_ids.is_empty() {
                "none".to_string()
            } else {
                s.parent_session_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!(
                "Session {}\n  task: {}\n  parents: {}\n  state: {}\n  agent: {}\n  role: {}\n  model: {}\n  started: {}\n  ended: {}\n  tokens: in={} out={} total={}\n  cost: ${:.4}\n  files: {}\n  log: {}",
                s.id,
                s.task_id,
                parents,
                format_session_state(&s),
                s.agent,
                s.role.as_str(),
                if s.model.is_empty() { "(none)" } else { &s.model },
                s.started_at.to_rfc3339(),
                s.ended_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "(running)".into()),
                s.token_in,
                s.token_out,
                s.token_in + s.token_out,
                s.cost_usd,
                files,
                store.log_path(s.id).display()
            )
        }
        Err(e) => e,
    }
}

fn format_session_tree_command(store: &SessionStore, selector: Option<&str>) -> String {
    let session = match resolve_session(store, selector) {
        Ok(session) => session,
        Err(err) => return err,
    };
    match store.list(10_000) {
        Ok(sessions) => {
            crate::session_display::format_session_tree(&session, &sessions, store.log_dir())
        }
        Err(err) => format!("Could not list sessions for tree: {err:#}"),
    }
}

fn format_log_tail(store: &SessionStore, selector: Option<&str>, lines: usize) -> String {
    let session = match resolve_session(store, selector) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = store.log_path(session.id);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => return format!("Could not read log {}: {}", path.display(), e),
    };
    let raw_lines: Vec<&str> = content.lines().collect();
    if raw_lines.is_empty() {
        return format!("Log is empty for session {}.", session_id8(&session));
    }

    let start = raw_lines.len().saturating_sub(lines);
    let mut out = format!(
        "Log tail for {} (last {} of {} line(s)):",
        session_id8(&session),
        raw_lines.len() - start,
        raw_lines.len()
    );
    for raw in &raw_lines[start..] {
        out.push('\n');
        out.push_str(&indent_block(&format_event_line(raw), "  "));
    }
    out
}

fn format_grep(store: &SessionStore, pattern: &str, limit: usize) -> String {
    match store.grep(pattern) {
        Ok(hits) if hits.is_empty() => format!("No session log hits for {:?}.", pattern),
        Ok(hits) => {
            let total = hits.len();
            let mut out = format!("Search hits for {:?} (showing up to {}):", pattern, limit);
            for (session, event) in hits.into_iter().take(limit) {
                out.push('\n');
                out.push_str(&format!(
                    "  {} {} {}",
                    session_id8(&session),
                    event.kind,
                    humanize_jsonish(&event.payload.to_string(), 180)
                ));
            }
            if total > limit {
                out.push_str(&format!("\n  ... {} more hit(s)", total - limit));
            }
            out
        }
        Err(e) => format!("Search failed: {:#}", e),
    }
}

fn resolve_session(
    store: &SessionStore,
    selector: Option<&str>,
) -> std::result::Result<Session, String> {
    let selector = selector.unwrap_or("last");
    if selector == "last" || selector == "." {
        return store
            .list(1)
            .map_err(|e| format!("Could not list sessions: {:#}", e))?
            .into_iter()
            .next()
            .ok_or_else(|| "No sessions yet.".to_string());
    }

    if let Ok(id) = uuid::Uuid::parse_str(selector) {
        return store
            .get_many(&[id])
            .map_err(|e| format!("Could not read session {}: {:#}", selector, e))?
            .into_iter()
            .next()
            .ok_or_else(|| format!("Session not found: {}", selector));
    }

    let matches: Vec<Session> = store
        .list(500)
        .map_err(|e| format!("Could not list sessions: {:#}", e))?
        .into_iter()
        .filter(|s| s.id.to_string().starts_with(selector))
        .collect();

    match matches.len() {
        0 => Err(format!("Session not found: {}", selector)),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => Err(format!(
            "Ambiguous session prefix: {} ({} matches)",
            selector,
            matches.len()
        )),
    }
}

fn format_session_state(s: &Session) -> &'static str {
    if s.ended_at.is_some() {
        "done"
    } else {
        "running"
    }
}

fn session_id8(s: &Session) -> String {
    s.id.to_string().chars().take(8).collect()
}

fn format_event_line(raw: &str) -> String {
    crate::session_display::format_session_event_line(raw)
}

fn format_transcript_for_copy(transcript: &[ChatMsg]) -> String {
    let mut out = String::new();
    for msg in transcript {
        let ts = msg
            .ts
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| {
                let secs = d.as_secs() % 86400;
                let h = (secs / 3600) % 24;
                let m = (secs / 60) % 60;
                let s = secs % 60;
                format!("{:02}:{:02}:{:02}", h, m, s)
            })
            .unwrap_or_else(|_| "??:??:??".to_string());
        let role = match msg.role.as_str() {
            "user" => "you",
            "assistant" => "orchestrator",
            _ => "sys",
        };
        out.push_str(&format!("[{}] {}: {}\n\n", ts, role, msg.content));
    }
    out
}

fn save_transcript_to_file(transcript: &[ChatMsg]) -> std::io::Result<std::path::PathBuf> {
    let home = home::home_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home dir"))?;
    let dir = home.join(".orchestrator").join("transcripts");
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("transcript-{}.md", ts));
    std::fs::write(&path, format_transcript_for_copy(transcript))?;
    Ok(path)
}

fn export_transcript(transcript: &[ChatMsg], target: &str) -> String {
    if transcript.is_empty() {
        return "Transcript is empty.".into();
    }

    let formatted = format_transcript_for_copy(transcript);
    match target {
        "clipboard" | "cb" => match copy_to_clipboard(&formatted) {
            Ok(()) => format!(
                "Copied {} message(s) to clipboard ({} bytes).",
                transcript.len(),
                formatted.len()
            ),
            Err(e) => format!("Clipboard copy failed: {}", e),
        },
        "file" | "disk" | "md" => match save_transcript_to_file(transcript) {
            Ok(p) => format!(
                "Saved {} message(s) to:\n  {}",
                transcript.len(),
                p.display()
            ),
            Err(e) => format!("File save failed: {}", e),
        },
        other => format!(
            "Unknown transcript export target: {}\nUse `file` or `clipboard`.",
            other
        ),
    }
}

fn format_last_result(input: &InputState, mode: Option<&str>) -> String {
    let Some(result) = input.last_result_output.as_deref() else {
        return "No final result captured yet.".into();
    };
    match mode {
        Some("box") => format_final_result(Some(result)),
        Some("text" | "plain" | "raw") | None => {
            format!("Final result (plain text):\n\n{}", result)
        }
        Some(other) => format!(
            "Unknown result mode: {}\nUse /result for plain text.",
            other
        ),
    }
}

fn copy_last_result(input: &InputState) -> String {
    let Some(result) = input.last_result_output.as_deref() else {
        return "No final result captured yet.".into();
    };
    match copy_to_clipboard(result) {
        Ok(()) => format!("Copied final result to clipboard ({} bytes).", result.len()),
        Err(e) => format!("Final result clipboard copy failed: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Role, Session};
    use crate::tui::state::ContextMode;
    use std::collections::HashMap;

    fn test_config() -> Config {
        let mut cfg = Config::default();
        cfg.runtimes.insert(
            "claude-code".into(),
            crate::config::RuntimeConfig {
                kind: "subprocess".into(),
                binary: Some("claude".into()),
                supports_mcp: true,
                supports_agent_teams: true,
            },
        );
        cfg.runtimes.insert(
            "codex".into(),
            crate::config::RuntimeConfig {
                kind: "subprocess".into(),
                binary: Some("codex".into()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        cfg.models = vec![
            crate::config::ModelConfig {
                id: "sonnet".into(),
                provider: "anthropic".into(),
                name: "claude-sonnet-4-6".into(),
            },
            crate::config::ModelConfig {
                id: "gpt4o".into(),
                provider: "openai".into(),
                name: "gpt-4o".into(),
            },
        ];
        cfg.roles = HashMap::from([
            (
                "planner".into(),
                crate::config::RoleConfig {
                    model: "sonnet".into(),
                    runtime: "claude-code".into(),
                    agent_teams: false,
                },
            ),
            (
                "worker-cc".into(),
                crate::config::RoleConfig {
                    model: "sonnet".into(),
                    runtime: "claude-code".into(),
                    agent_teams: true,
                },
            ),
            (
                "worker-cc-minimax".into(),
                crate::config::RoleConfig {
                    model: "sonnet".into(),
                    runtime: "claude-code".into(),
                    agent_teams: true,
                },
            ),
            (
                "worker-codex".into(),
                crate::config::RoleConfig {
                    model: "gpt4o".into(),
                    runtime: "codex".into(),
                    agent_teams: false,
                },
            ),
        ]);
        cfg
    }

    #[test]
    fn detects_slash_argument_completion_context() {
        let ctx = slash_argument_context("/session ", "/session ".len()).unwrap();
        assert_eq!(ctx.cmd, "session");
        assert_eq!(ctx.current_index, 0);
        assert_eq!(ctx.current_prefix, "");

        let ctx = slash_argument_context("/log abc", "/log abc".len()).unwrap();
        assert_eq!(ctx.cmd, "log");
        assert_eq!(ctx.current_index, 0);
        assert_eq!(ctx.current_prefix, "abc");

        let ctx = slash_argument_context("/export process c", "/export process c".len()).unwrap();
        assert_eq!(ctx.cmd, "export");
        assert_eq!(ctx.current_index, 1);
        assert_eq!(ctx.current_prefix, "c");

        assert!(slash_argument_context("/session", "/session".len()).is_none());
        assert!(slash_argument_context("hello", "hello".len()).is_none());
    }

    #[test]
    fn completes_session_argument_from_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let session = Session::new(uuid::Uuid::new_v4(), "agent", Role::Worker);
        let prefix = session.id.to_string()[..8].to_string();
        store.finalize(&session).unwrap();

        let mut input = InputState {
            buffer: format!("/session {}", prefix),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();

        let cfg = test_config();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, format!("/session {} ", session.id));
        assert_eq!(input.cursor, input.buffer.len());
    }

    #[test]
    fn completes_session_tree_argument_from_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let session = Session::new(uuid::Uuid::new_v4(), "orchestrator", Role::Orchestrator);
        let prefix = session.id.to_string()[..8].to_string();
        store.finalize(&session).unwrap();

        let mut input = InputState {
            buffer: format!("/session tree {}", prefix),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();

        let cfg = test_config();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, format!("/session tree {} ", session.id));
    }

    #[test]
    fn session_tree_command_shows_child_workers() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let mut root = Session::new(uuid::Uuid::new_v4(), "orchestrator", Role::Orchestrator);
        root.ended_at = Some(chrono::Utc::now());
        let mut worker = Session::new(uuid::Uuid::new_v4(), "claude-code", Role::Worker);
        worker.parent_session_ids.push(root.id);
        worker.ended_at = Some(chrono::Utc::now());
        store.finalize(&root).unwrap();
        store.finalize(&worker).unwrap();

        let tree = format_session_tree_command(&store, Some(&root.id.to_string()[..8]));

        assert!(tree.contains("Session tree"));
        assert!(tree.contains("Children (1):"));
        assert!(tree.contains("claude-code"));
        assert!(tree.contains("/log"));
    }

    #[test]
    fn sessions_command_shows_roles_relations_and_shortcuts() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let mut root = Session::new(uuid::Uuid::new_v4(), "orchestrator", Role::Orchestrator);
        root.ended_at = Some(chrono::Utc::now());
        let mut worker = Session::new(uuid::Uuid::new_v4(), "worker-codex", Role::Worker);
        worker.parent_session_ids.push(root.id);
        worker.ended_at = Some(chrono::Utc::now());
        store.finalize(&root).unwrap();
        store.finalize(&worker).unwrap();

        let list = format_sessions(&store, 10);

        assert!(list.contains("Recent sessions"));
        assert!(list.contains("orchestrator"));
        assert!(list.contains("worker"));
        assert!(list.contains("children=1"));
        assert!(list.contains("parent="));
        assert!(list.contains("/tree"));
        assert!(list.contains("/log"));
    }

    #[test]
    fn completes_static_slash_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let cfg = test_config();

        let mut input = InputState {
            buffer: "/ro".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/roles ");

        input.buffer = "/".into();
        input.cursor = input.buffer.len();
        let candidates = slash_argument_candidates(&store, &cfg, &input);
        assert!(candidates
            .iter()
            .any(|candidate| candidate.label == "/roles"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.label == "/doctor"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.label == "/queue"));
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.label.contains('{')));

        let mut input = InputState {
            buffer: "/export p".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/export process ");

        input.buffer = "/export process c".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/export process clipboard ");

        input.buffer = "/process f".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/process filter ");

        input.buffer = "/trace f".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/trace full ");

        input.buffer = "/resume l".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/resume last ");

        input.buffer = "/context c".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/context compact ");

        input.buffer = "/queue r".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/queue remove ");

        input.queued_prompts = vec!["first".into(), "second".into()];
        input.buffer = "/queue remove 2".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/queue remove 2 ");

        input.buffer = "/queue e".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/queue edit ");

        input.buffer = "/diff st".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/diff stat ");

        input.buffer = "/tests c".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/tests cargo ");

        input.buffer = "/doctor ".into();
        input.cursor = input.buffer.len();
        let candidates = slash_argument_candidates(&store, &cfg, &input);
        assert!(candidates.iter().any(|candidate| candidate.label == "run"));
    }

    #[test]
    fn doctor_command_reports_environment_in_terminal_chat() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let config_path = tmp.path().join("config.yaml");
        let worktree_base = tmp.path().join("worktrees");
        let session_log_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&worktree_base).unwrap();
        std::fs::create_dir_all(&session_log_dir).unwrap();
        std::fs::write(
            &config_path,
            format!(
                r#"
providers:
  local:
    type: ollama
    api_key:
    base_url: http://localhost:11434
runtimes:
  direct:
    type: direct
    binary:
models:
  - id: local
    provider: local
    name: local-model
roles:
  planner:
    model: local
    runtime: direct
  critic:
    model: local
    runtime: direct
  worker-cc:
    model: local
    runtime: direct
  worker-codex:
    model: local
    runtime: direct
  reviewer:
    model: local
    runtime: direct
behavior:
  worktree_base: {}
  db_path: {}
  session_log_dir: {}
"#,
                worktree_base.display(),
                tmp.path().join("state.db").display(),
                session_log_dir.display()
            ),
        )
        .unwrap();
        let cfg = Arc::new(test_config());
        let runtime = TuiRuntimeConfig {
            config_path: Some(config_path),
        };
        let mut input = InputState::default();

        handle_slash_command_with_runtime("/doctor", &store, &cfg, &runtime, &mut input);

        let msg = input.transcript.last().expect("doctor response");
        assert!(msg.content.contains("orchestrator doctor"));
        assert!(msg.content.contains("config parsed"));
        assert!(msg.content.contains("Runtimes:"));
        assert!(msg.content.contains("Roles:"));
    }

    #[test]
    fn completes_config_backed_slash_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let cfg = test_config();

        let mut input = InputState {
            buffer: "/agent work".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/agent worker-cc ");

        input.buffer = "/model planner g".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/model planner gpt4o ");

        input.buffer = "/usage w".into();
        input.cursor = input.buffer.len();
        assert!(complete_slash_argument(&store, &cfg, &mut input));
        assert_eq!(input.buffer, "/usage week ");
    }

    #[test]
    fn process_filter_command_sets_and_clears_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.run_events = vec![
            "10:00:00  Planning: decomposing task".into(),
            "10:00:01  Worker started: agent-a".into(),
        ];

        handle_slash_command("/process filter worker", &store, &cfg, &mut input);
        assert_eq!(input.process_filter.as_deref(), Some("worker"));
        assert!(input
            .transcript
            .last()
            .unwrap()
            .content
            .contains("1 of 2 event(s)"));

        handle_slash_command("/process clear-filter", &store, &cfg, &mut input);
        assert_eq!(input.process_filter, None);
    }

    #[test]
    fn process_command_defaults_to_summary_and_supports_full() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.run_events = vec![
            "10:00:00  Planning: decomposing task".into(),
            "10:00:01  Worker output: abcdef12 verbose progress".into(),
            "10:00:02  Worker done: abcdef12".into(),
        ];

        handle_slash_command("/process", &store, &cfg, &mut input);
        let summary = input.transcript.last().expect("process summary");
        assert!(summary.content.contains("Process summary"));
        assert!(!summary.content.contains("verbose progress"));

        handle_slash_command("/process full", &store, &cfg, &mut input);
        let full = input.transcript.last().expect("process full");
        assert!(full.content.contains("Captured run process"));
        assert!(full.content.contains("verbose progress"));
    }

    #[test]
    fn result_command_replays_last_final_result_as_plain_text() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState {
            last_result_output: Some("Implemented the feature.".into()),
            ..InputState::default()
        };

        handle_slash_command("/result", &store, &cfg, &mut input);

        let msg = input.transcript.last().expect("result response");
        assert!(msg.content.contains("Final result (plain text):"));
        assert!(msg.content.contains("Implemented the feature."));
        assert!(!msg.content.contains("╭─ Final result:"));
    }

    #[test]
    fn result_box_command_is_legacy_plain_text_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState {
            last_result_output: Some("Implemented the feature.".into()),
            ..InputState::default()
        };

        handle_slash_command("/result box", &store, &cfg, &mut input);

        let msg = input.transcript.last().expect("result response");
        assert!(msg.content.contains("Final result:"));
        assert!(msg.content.contains("Implemented the feature."));
        assert!(!msg.content.contains("╭─ Final result:"));
    }

    #[test]
    fn result_text_command_replays_selectable_plain_text() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState {
            last_result_output: Some("Implemented the feature.".into()),
            ..InputState::default()
        };

        handle_slash_command("/result text", &store, &cfg, &mut input);

        let msg = input.transcript.last().expect("result response");
        assert!(msg.content.contains("Final result (plain text):"));
        assert!(msg.content.contains("Implemented the feature."));
        assert!(!msg.content.contains("╭─ Final result:"));
    }

    #[test]
    fn result_command_reports_missing_result() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();

        handle_slash_command("/result", &store, &cfg, &mut input);

        let msg = input.transcript.last().expect("result response");
        assert!(msg.content.contains("No final result captured yet"));
    }

    #[test]
    fn diff_and_tests_commands_return_readable_text() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();

        handle_slash_command("/tests cargo", &store, &cfg, &mut input);
        let tests = input.transcript.last().expect("tests response");
        assert!(tests.content.contains("cargo test"));
        assert!(!tests.content.contains('{'));

        handle_slash_command("/diff unknown", &store, &cfg, &mut input);
        let diff = input.transcript.last().expect("diff response");
        assert!(diff.content.contains("Usage: /diff"));
    }

    #[test]
    fn roles_command_shows_registry_and_sets_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();

        handle_slash_command("/roles", &store, &cfg, &mut input);
        let registry = input.transcript.last().expect("roles response");
        assert!(registry.content.contains("Role registry"));
        assert!(registry.content.contains("worker-cc"));

        handle_slash_command("/roles use worker-cc", &store, &cfg, &mut input);
        assert_eq!(input.agent_hint.as_deref(), Some("worker-cc"));
    }

    #[test]
    fn roles_snippet_prints_config_backed_role_definition() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();

        handle_slash_command(
            "/roles snippet executor sonnet claude-code",
            &store,
            &cfg,
            &mut input,
        );

        let snippet = input.transcript.last().expect("roles snippet");
        assert!(snippet.content.contains("executor:"));
        assert!(snippet.content.contains("model: sonnet"));
        assert!(snippet.content.contains("runtime: claude-code"));
        assert!(snippet.content.contains("restart"));
    }

    #[test]
    fn roles_save_writes_config_role_without_env_expansion() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let config_path = tmp.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  openai:\n    type: openai\n    api_key: ${OPENAI_API_KEY}\nruntimes:\n  claude-code:\n    type: subprocess\n    binary: claude\n  codex:\n    type: subprocess\n    binary: codex\nmodels:\n  - id: sonnet\n    provider: anthropic\n    name: claude-sonnet-4-6\nroles: {}\nbehavior: {}\n",
        )
        .unwrap();
        let runtime = TuiRuntimeConfig {
            config_path: Some(config_path.clone()),
        };
        let mut input = InputState::default();

        handle_slash_command_with_runtime(
            "/roles save executor sonnet claude-code",
            &store,
            &cfg,
            &runtime,
            &mut input,
        );

        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(body.contains("executor"));
        assert!(body.contains("model: sonnet"));
        assert!(body.contains("runtime: claude-code"));
        assert!(body.contains("${OPENAI_API_KEY}"));
        assert!(input
            .transcript
            .last()
            .unwrap()
            .content
            .contains("Saved role executor"));
    }

    #[test]
    fn workflow_command_shows_active_role_pipeline() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState {
            agent_hint: Some("worker-cc".into()),
            queued_prompts: vec!["follow up".into()],
            ..InputState::default()
        };

        handle_slash_command("/workflow", &store, &cfg, &mut input);

        let workflow = input.transcript.last().expect("workflow response");
        assert!(workflow.content.contains("Workflow status"));
        assert!(workflow.content.contains("Pipeline"));
        assert!(workflow.content.contains("execute"));
        assert!(workflow.content.contains("worker-cc"));
        assert!(workflow.content.contains("1 pending"));
    }

    #[test]
    fn graph_command_summarizes_conversation_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "build feature".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "✓ done\n\nFinal result: built".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/graph", &store, &cfg, &mut input);

        let graph = input.transcript.last().expect("graph response");
        assert!(graph.content.contains("Conversation flow"));
        assert!(graph.content.contains("user request"));
        assert!(graph.content.contains("orchestrator result"));
    }

    #[test]
    fn graph_command_can_render_mermaid_flowchart() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState {
            queued_prompts: vec!["next step".into()],
            ..InputState::default()
        };
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "build feature".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });
        input.transcript.push(ChatMsg {
            role: "assistant".into(),
            content: "✓ done\n\nFinal result: built".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/graph mermaid", &store, &cfg, &mut input);

        let graph = input.transcript.last().expect("mermaid graph response");
        assert!(graph.content.contains("```mermaid"));
        assert!(graph.content.contains("flowchart TD"));
        assert!(graph.content.contains("queue: 1 pending"));
        assert!(!graph.content.contains('{'));
    }

    #[test]
    fn acp_command_shows_stdio_server_command() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState {
            agent_hint: Some("worker-cc".into()),
            ..InputState::default()
        };

        handle_slash_command("/acp status", &store, &cfg, &mut input);

        let acp = input.transcript.last().expect("acp response");
        assert!(acp.content.contains("Agent Client Protocol"));
        assert!(acp.content.contains("orchestrator acp --agent claude"));
        assert!(acp.content.contains("stdio"));
    }

    #[test]
    fn handoff_command_saves_markdown_package() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState {
            last_result_output: Some("done".into()),
            ..InputState::default()
        };
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "finish task".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/handoff claude", &store, &cfg, &mut input);

        let path = input.last_handoff_path.as_ref().expect("handoff path");
        let body = std::fs::read_to_string(path).expect("handoff body");
        assert!(body.contains("Orchestrator Handoff"));
        assert!(body.contains("finish task"));
        assert!(body.contains("done"));
    }

    #[test]
    fn test_command_spec_maps_presets() {
        let cargo = test_command_spec(&["cargo"]).expect("cargo spec");
        assert_eq!(cargo.display(), "cargo test");

        let tui = test_command_spec(&["tui"]).expect("tui spec");
        assert_eq!(tui.display(), "cargo test tui::");

        let quick = test_command_spec(&["quick"]).expect("quick spec");
        assert!(quick.display().contains("cargo test tui::"));
        assert!(quick.display().contains("cargo test runtime"));

        let custom = test_command_spec(&["echo", "ok"]).expect("custom spec");
        assert_eq!(custom.display(), "echo ok");
    }

    #[test]
    fn checkpoint_reports_no_git_changes_without_crashing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();

        handle_slash_command("/checkpoint status", &store, &cfg, &mut input);

        let status = input.transcript.last().expect("checkpoint response");
        assert!(status.content.contains("Last checkpoint"));
    }

    #[test]
    fn log_event_line_humanizes_json_payload() {
        let raw = serde_json::json!({
            "session_id": uuid::Uuid::nil(),
            "task_id": uuid::Uuid::nil(),
            "ts": "2026-06-10T12:00:00Z",
            "type": "assistant",
            "payload": {
                "message": {
                    "content": [
                        {"type": "text", "text": "Readable output"}
                    ]
                }
            }
        })
        .to_string();

        let line = format_event_line(&raw);

        assert!(line.contains("assistant Readable output"));
        assert!(!line.contains('{'));
    }

    #[test]
    fn log_event_line_expands_imported_claude_content() {
        let raw = serde_json::json!({
            "session_id": uuid::Uuid::nil(),
            "task_id": uuid::Uuid::nil(),
            "ts": "2026-06-10T12:00:00Z",
            "type": "tool_result",
            "payload": {
                "source": "claude-code",
                "content": "toolu_1: first line\nsecond line",
                "raw": {"type": "tool_result"}
            }
        })
        .to_string();

        let line = format_event_line(&raw);

        assert!(line.contains("12:00:00 tool result toolu_1: first line"));
        assert!(line.contains("\n    second line"));
        assert!(!line.contains("\"raw\""));
    }

    #[test]
    fn imported_claude_events_restore_as_chat_messages() {
        let ts = chrono::Utc::now();
        let session = crate::cc_session_import::CCSession {
            id: uuid::Uuid::new_v4().to_string(),
            path: std::path::PathBuf::from("session.jsonl"),
            events: vec![
                crate::cc_session_import::CCEvent {
                    ts,
                    kind: "user".into(),
                    content: "hello".into(),
                    tool_name: None,
                    raw: None,
                },
                crate::cc_session_import::CCEvent {
                    ts,
                    kind: "tool_use".into(),
                    content: "Bash({\"command\":\"cargo test\"})".into(),
                    tool_name: Some("Bash".into()),
                    raw: None,
                },
                crate::cc_session_import::CCEvent {
                    ts,
                    kind: "assistant".into(),
                    content: "done".into(),
                    tool_name: None,
                    raw: None,
                },
            ],
            started_at: Some(ts),
            ended_at: Some(ts),
            summary: Some("test restore".into()),
            cwd: None,
            git_branch: None,
            model: None,
            size_bytes: 0,
        };

        let (messages, truncated) = build_imported_cc_chat_messages(&session);

        assert!(!truncated);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "tool");
        assert_eq!(messages[2].role, "assistant");
        assert!(messages[1].content.contains("cargo test"));
    }

    #[test]
    fn outline_command_toggles_process_detail_without_deleting_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "line one\nline two".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/o", &store, &cfg, &mut input);
        assert!(input.history_folded);
        assert_eq!(input.transcript[0].content, "line one\nline two");
        assert!(input
            .transcript
            .last()
            .expect("fold response")
            .content
            .contains("Process view folded"));

        handle_slash_command("/o", &store, &cfg, &mut input);
        assert!(!input.history_folded);
        assert_eq!(input.transcript[0].content, "line one\nline two");
        assert!(input
            .transcript
            .last()
            .expect("expand response")
            .content
            .contains("Process view expanded"));
    }

    #[test]
    fn queue_command_lists_pending_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.queued_prompts = vec![
            "first queued message".into(),
            "second queued message".into(),
        ];

        handle_slash_command("/queue", &store, &cfg, &mut input);

        let msg = input.transcript.last().expect("queue response");
        assert!(msg.content.contains("Queued batch (2)"));
        assert!(msg.content.contains("ready"));
        assert!(msg.content.contains("↳ 1. first queued message"));
        assert!(msg.content.contains("↳ 2. second queued message"));
        assert!(msg.content.contains("run together"));
    }

    #[test]
    fn queue_command_removes_one_pending_message() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.queued_prompts = vec![
            "first queued message".into(),
            "second queued message".into(),
        ];
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "already ran".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/queue remove 1", &store, &cfg, &mut input);

        assert_eq!(input.queued_prompts, vec!["second queued message"]);
        assert_eq!(input.transcript[0].status, "complete");
        assert!(input
            .transcript
            .last()
            .expect("queue response")
            .content
            .contains("Removed queued message 1"));
    }

    #[test]
    fn queue_command_promotes_pending_message() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.queued_prompts = vec!["first".into(), "second".into(), "third".into()];

        handle_slash_command("/queue promote 3", &store, &cfg, &mut input);

        assert_eq!(input.queued_prompts, vec!["third", "first", "second"]);
        assert!(input
            .transcript
            .last()
            .expect("queue response")
            .content
            .contains("Promoted queued message 3 to next"));
    }

    #[test]
    fn queue_command_pauses_resumes_and_runs_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.queued_prompts = vec!["first".into(), "second".into()];

        let action = handle_slash_command("/queue pause", &store, &cfg, &mut input);
        assert!(matches!(action, SlashAction::None));
        assert!(input.queue_paused);

        let action = handle_slash_command("/queue resume", &store, &cfg, &mut input);
        assert!(matches!(action, SlashAction::RunQueuedNow));
        assert!(!input.queue_paused);

        input.queue_paused = true;
        let action = handle_slash_command("/queue run", &store, &cfg, &mut input);
        assert!(matches!(action, SlashAction::RunQueuedNow));
        assert!(!input.queue_paused);
    }

    #[test]
    fn queue_command_edits_pending_message_without_touching_history() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.queued_prompts = vec!["first".into(), "second".into()];
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "already ran".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/queue edit 2 changed followup", &store, &cfg, &mut input);

        assert_eq!(input.queued_prompts, vec!["first", "changed followup"]);
        assert_eq!(input.transcript[0].content, "already ran");
        assert!(input
            .transcript
            .last()
            .expect("queue response")
            .content
            .contains("Edited queued message 2"));
    }

    #[test]
    fn queue_command_clear_keeps_history_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.queued_prompts = vec!["first".into(), "second".into()];
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "already ran".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/queue clear", &store, &cfg, &mut input);

        assert!(input.queued_prompts.is_empty());
        assert_eq!(input.transcript[0].status, "complete");
        assert!(input
            .transcript
            .last()
            .expect("queue response")
            .content
            .contains("Cleared 2 queued message(s)"));
    }

    #[test]
    fn context_command_controls_memory_mode_and_cutoff() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.transcript.push(ChatMsg {
            role: "user".into(),
            content: "old context".into(),
            ts: std::time::SystemTime::now(),
            status: "complete".into(),
        });

        handle_slash_command("/context off", &store, &cfg, &mut input);
        assert_eq!(input.context_mode, ContextMode::Off);
        assert!(input
            .transcript
            .last()
            .unwrap()
            .content
            .contains("mode: off"));

        handle_slash_command("/context full", &store, &cfg, &mut input);
        assert_eq!(input.context_mode, ContextMode::Full);

        handle_slash_command("/context clear", &store, &cfg, &mut input);
        assert_eq!(input.context_cutoff, input.transcript.len() - 1);
        assert!(input
            .transcript
            .last()
            .unwrap()
            .content
            .contains("Visible transcript was kept"));
    }

    #[test]
    fn trace_command_controls_live_trace_block() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();
        input.run_events = vec![
            "10:00:00  Planning: decomposing task".into(),
            "10:00:01  Worker started: agent-a".into(),
        ];

        handle_slash_command("/trace full", &store, &cfg, &mut input);

        assert!(input.trace_live_enabled);
        assert!(input.trace_expanded);
        let trace = input
            .transcript
            .iter()
            .find(|msg| msg.role == "trace")
            .expect("trace block");
        assert!(trace.content.contains("now: ● worker - started agent-a"));

        handle_slash_command("/trace off", &store, &cfg, &mut input);

        assert!(!input.trace_live_enabled);
        let trace = input
            .transcript
            .iter()
            .find(|msg| msg.role == "trace")
            .expect("trace block");
        assert_eq!(trace.status, "complete");
    }

    #[test]
    fn agent_command_sets_route_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(test_config());
        let mut input = InputState::default();

        handle_slash_command("/agent claude", &store, &cfg, &mut input);
        assert_eq!(input.agent_hint.as_deref(), Some("worker-cc"));

        handle_slash_command("/agent worker-cc", &store, &cfg, &mut input);
        assert_eq!(input.agent_hint.as_deref(), Some("worker-cc"));

        handle_slash_command("/agent worker-cc-minimax", &store, &cfg, &mut input);
        assert_eq!(input.agent_hint.as_deref(), Some("worker-cc-minimax"));

        handle_slash_command("/agent clear", &store, &cfg, &mut input);
        assert_eq!(input.agent_hint, None);
    }

    #[test]
    fn agent_claude_route_uses_configured_claude_worker_without_worker_cc() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let mut cfg = test_config();
        cfg.roles.remove("worker-cc");
        let cfg = Arc::new(cfg);
        let mut input = InputState::default();

        handle_slash_command("/agent claude", &store, &cfg, &mut input);
        assert_eq!(input.agent_hint.as_deref(), Some("worker-cc-minimax"));
    }

    #[test]
    fn model_command_is_read_only_about_hot_switching() {
        let cfg = test_config();
        let msg = format_model_command(&cfg, &["planner", "gpt4o"]);

        assert!(msg.contains("hot-switch is not active"));
        assert!(msg.contains("orchestrator config set planner gpt4o"));
    }
}
