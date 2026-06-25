//! Slash-command dispatch and local-recall handoff for `ChatWidget`.
//!
//! `ChatComposer` parses slash input and stages recognized command text for local
//! Up-arrow recall before returning an input result. This module owns the app-level
//! dispatch step and records the staged entry once the command has been handled, so
//! slash-command recall follows the same submitted-input rule as ordinary text.

use super::*;
use crate::app_event::ThreadGoalSetMode;
use crate::bottom_pane::ProviderSetupDraft;
use crate::bottom_pane::ProviderSetupView;
use crate::bottom_pane::RoleSetupDraft;
use crate::bottom_pane::RoleSetupView;
use crate::bottom_pane::prompt_args::parse_slash_name;
use crate::bottom_pane::slash_commands::BuiltinCommandFlags;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::bottom_pane::slash_commands::SlashCommandItem;
use crate::bottom_pane::slash_commands::find_slash_command;
use crate::bottom_pane::slash_commands::tiffany_orchestrator_unsupported_command_message;
use crate::goal_display::GOAL_USAGE;
use crate::goal_files::GoalDraft;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlashCommandDispatchSource {
    Live,
    Queued,
}

struct PreparedSlashCommandArgs {
    args: String,
    text_elements: Vec<TextElement>,
    pending_pastes: Vec<(String, String)>,
    local_images: Vec<LocalImageAttachment>,
    remote_image_urls: Vec<String>,
    mention_bindings: Vec<MentionBinding>,
    source: SlashCommandDispatchSource,
}

const SIDE_STARTING_CONTEXT_LABEL: &str = "Side starting...";
const SIDE_SLASH_COMMAND_UNAVAILABLE_HINT: &str =
    "Press Ctrl+C to return to the main thread first.";
const GOAL_USAGE_HINT: &str = "Example: /goal improve benchmark coverage";
const RAW_USAGE: &str = "Usage: /raw [on|off]";
const USAGE_CHATGPT_LOGIN_REQUIRED: &str = "Sign in with ChatGPT to use /usage.";
const PROVIDER_USAGE: &str = "Usage: /provider [setup|list|delete <provider>|key <provider> <key-or-$ENV>|endpoint <provider> <url>]";
const ROLE_USAGE: &str =
    "Usage: /role [<role>|register <role> --model <model-id> --runtime <runtime-id>]";
const ROLES_USAGE: &str =
    "Usage: /roles [list|show <role>|register <role> --model <model-id> --runtime <runtime-id>]";
const THREAD_USAGE: &str = "Usage: /thread [list|show <role>|clear <role>]";
const DOCTOR_USAGE: &str = "Usage: /doctor [run]";

impl ChatWidget {
    /// Dispatch a bare slash command and record its staged local-history entry.
    ///
    /// The composer stages history before returning `InputResult::Command`; this wrapper commits
    /// that staged entry after dispatch so slash-command recall follows the same "submitted input"
    /// rule as normal text.
    pub(super) fn handle_slash_command_dispatch(&mut self, cmd: SlashCommand) {
        self.dispatch_command(cmd);
        if cmd == SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
        self.bottom_pane.record_pending_slash_command_history();
    }

    pub(super) fn handle_service_tier_command_dispatch(&mut self, command: ServiceTierCommand) {
        if self.active_side_conversation {
            self.add_error_message(format!(
                "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
                command.name
            ));
            self.bottom_pane.drain_pending_submission_state();
            self.bottom_pane.record_pending_slash_command_history();
            return;
        }
        self.toggle_service_tier_from_ui(command);
        self.bottom_pane.record_pending_slash_command_history();
    }

    /// Dispatch an inline slash command and record its staged local-history entry.
    ///
    /// Inline command arguments may later be prepared through the normal submission pipeline, but
    /// local command recall still tracks the original command invocation. Treating this wrapper as
    /// the only input-result entry point avoids double-recording commands with inline args.
    pub(super) fn handle_slash_command_with_args_dispatch(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        self.dispatch_command_with_args(cmd, args, text_elements);
        self.bottom_pane.record_pending_slash_command_history();
    }

    fn apply_plan_slash_command(&mut self) -> bool {
        if !self.collaboration_modes_enabled() {
            self.add_info_message(
                "Collaboration modes are disabled.".to_string(),
                Some("Enable collaboration modes to use /plan.".to_string()),
            );
            return false;
        }
        if let Some(mask) = collaboration_modes::plan_mask(self.model_catalog.as_ref()) {
            self.set_collaboration_mask_from_user_action(mask);
            true
        } else {
            self.add_info_message(
                "Plan mode unavailable right now.".to_string(),
                /*hint*/ None,
            );
            false
        }
    }

    fn request_side_conversation(
        &mut self,
        parent_thread_id: ThreadId,
        user_message: Option<UserMessage>,
    ) {
        self.set_side_conversation_context_label(Some(SIDE_STARTING_CONTEXT_LABEL.to_string()));
        self.request_redraw();
        self.app_event_tx.send(AppEvent::StartSide {
            parent_thread_id,
            user_message,
        });
    }

    fn request_empty_side_conversation(&mut self, cmd: SlashCommand) {
        let Some(parent_thread_id) = self.thread_id else {
            let command = cmd.command();
            self.add_error_message(format!(
                "'/{command}' is unavailable before the session starts."
            ));
            return;
        };

        self.request_side_conversation(parent_thread_id, /*user_message*/ None);
    }

    fn emit_raw_output_mode_changed(&self, enabled: bool) {
        self.app_event_tx
            .send(AppEvent::RawOutputModeChanged { enabled });
    }

    fn dispatch_tiffany_roles_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(
                format!(
                    "'/roles' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{ROLES_USAGE}"
                ),
            );
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorRolesCommand { args: args.into() });
    }

    fn dispatch_tiffany_role_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/role' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{ROLE_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorRolesCommand { args: args.into() });
    }

    fn dispatch_tiffany_provider_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/provider' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{PROVIDER_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorProviderCommand { args: args.into() });
    }

    fn dispatch_tiffany_doctor_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/doctor' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{DOCTOR_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorDoctorCommand { args: args.into() });
    }

    fn dispatch_tiffany_thread_command(&mut self, args: impl Into<String>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/thread' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{THREAD_USAGE}"
            ));
            return;
        }
        self.app_event_tx
            .send(AppEvent::TiffanyOrchestratorThreadCommand { args: args.into() });
    }

    fn open_tiffany_provider_setup_prompt(&mut self, provider: Option<&str>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/provider' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{PROVIDER_USAGE}"
            ));
            return;
        }

        let provider = provider.unwrap_or("openai");
        let draft =
            provider_setup_initial_draft(provider, self.tiffany_orchestrator_config.as_ref());
        let tx = self.app_event_tx.clone();
        let view = ProviderSetupView::new(
            draft,
            Box::new(
                move |draft: ProviderSetupDraft| match provider_setup_draft_args(&draft) {
                    Ok(args) => {
                        tx.send(AppEvent::TiffanyOrchestratorProviderCommand { args });
                        Ok(())
                    }
                    Err(err) => Err(err),
                },
            ),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    fn open_tiffany_role_setup_prompt(&mut self, role: Option<&str>) {
        if !self.tiffany_orchestrator_shell {
            self.add_error_message(format!(
                "'/role' is available in tiffany-loop orchestrator mode. Start it with `tiffany-loop` or `./scripts/tiffany-dev`.\n{ROLE_USAGE}"
            ));
            return;
        }

        let draft = role_setup_initial_draft(role);
        let tx = self.app_event_tx.clone();
        let view = RoleSetupView::new(
            draft,
            Box::new(
                move |draft: RoleSetupDraft| match role_setup_draft_args(&draft) {
                    Ok(args) => {
                        tx.send(AppEvent::TiffanyOrchestratorRolesCommand { args });
                        Ok(())
                    }
                    Err(err) => Err(err),
                },
            ),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    pub(super) fn dispatch_command(&mut self, cmd: SlashCommand) {
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.bottom_pane.drain_pending_submission_state();
            self.request_redraw();
            return;
        }

        match cmd {
            SlashCommand::Feedback => {
                if !self.config.feedback_enabled {
                    let params = crate::bottom_pane::feedback_disabled_params();
                    self.bottom_pane.show_selection_view(params);
                    self.request_redraw();
                    return;
                }
                // Step 1: pick a category (UI built in feedback_view)
                let params =
                    crate::bottom_pane::feedback_selection_params(self.app_event_tx.clone());
                self.bottom_pane.show_selection_view(params);
                self.request_redraw();
            }
            SlashCommand::New => {
                self.app_event_tx.send(AppEvent::NewSession);
            }
            SlashCommand::Archive => {
                self.bottom_pane.show_selection_view(SelectionViewParams {
                    title: Some("Archive this session?".to_string()),
                    subtitle: Some(
                        "Are you sure? This will archive the current session and exit tiffany-loop"
                            .to_string(),
                    ),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: vec![
                        SelectionItem {
                            name: "No, don't archive".to_string(),
                            description: Some("Return to the current session".to_string()),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, archive and exit".to_string(),
                            description: Some("Archive this session now".to_string()),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::ArchiveCurrentThread);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Delete => {
                self.bottom_pane.show_selection_view(SelectionViewParams {
                    title: Some("Delete this session?".to_string()),
                    subtitle: Some(
                        "Cannot be undone. Subagent threads will also be deleted.".to_string(),
                    ),
                    footer_hint: Some(standard_popup_hint_line()),
                    items: vec![
                        SelectionItem {
                            name: "No, keep this session".to_string(),
                            description: Some("Return to the current session".to_string()),
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                        SelectionItem {
                            name: "Yes, delete and exit".to_string(),
                            description: Some("Permanently delete this session now".to_string()),
                            actions: vec![Box::new(|tx| {
                                tx.send(AppEvent::DeleteCurrentThread);
                            })],
                            dismiss_on_select: true,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                self.request_redraw();
            }
            SlashCommand::Clear => {
                self.app_event_tx.send(AppEvent::ClearUi);
            }
            SlashCommand::Resume => {
                self.app_event_tx.send(AppEvent::OpenResumePicker);
            }
            SlashCommand::Fork => {
                self.app_event_tx.send(AppEvent::ForkCurrentSession);
            }
            SlashCommand::App => {
                let Some(thread_id) = self.thread_id else {
                    self.add_error_message(
                        "Session is still starting; try /app again in a moment.".to_string(),
                    );
                    return;
                };
                self.app_event_tx
                    .send(AppEvent::OpenDesktopThread { thread_id });
            }
            SlashCommand::Init => {
                const INIT_PROMPT: &str = include_str!("../../prompt_for_init_command.md");
                self.submit_user_message(INIT_PROMPT.to_string().into());
            }
            SlashCommand::Compact => {
                self.clear_token_usage();
                if !self.bottom_pane.is_task_running() {
                    self.bottom_pane.set_task_running(/*running*/ true);
                }
                self.app_event_tx.compact();
            }
            SlashCommand::Review => {
                self.open_review_popup();
            }
            SlashCommand::Rename => {
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                self.show_rename_prompt();
            }
            SlashCommand::Model => {
                self.open_model_popup();
            }
            SlashCommand::Personality => {
                self.open_personality_popup();
            }
            SlashCommand::Plan => {
                self.apply_plan_slash_command();
            }
            SlashCommand::Goal => {
                if !self.config.features.enabled(Feature::Goals) {
                    return;
                }
                if let Some(thread_id) = self.thread_id {
                    self.app_event_tx
                        .send(AppEvent::OpenThreadGoalMenu { thread_id });
                    self.append_message_history_entry("/goal".to_string());
                } else {
                    self.add_info_message(
                        GOAL_USAGE.to_string(),
                        Some(GOAL_USAGE_HINT.to_string()),
                    );
                }
            }
            SlashCommand::Roles => {
                self.dispatch_tiffany_roles_command("list".to_string());
            }
            SlashCommand::Thread => {
                self.dispatch_tiffany_thread_command("list".to_string());
            }
            SlashCommand::Role => {
                self.open_tiffany_role_setup_prompt(None);
            }
            SlashCommand::Provider => {
                self.open_tiffany_provider_setup_prompt(None);
            }
            SlashCommand::Doctor => {
                self.dispatch_tiffany_doctor_command("");
            }
            SlashCommand::Side | SlashCommand::Btw => {
                self.request_empty_side_conversation(cmd);
            }
            SlashCommand::Agent | SlashCommand::MultiAgents => {
                self.app_event_tx.send(AppEvent::OpenAgentPicker);
            }
            SlashCommand::Permissions => {
                self.open_permissions_popup();
            }
            SlashCommand::Vim => {
                self.toggle_vim_mode_and_notify();
            }
            SlashCommand::Keymap => {
                self.open_keymap_picker();
            }
            SlashCommand::ElevateSandbox => {
                #[cfg(target_os = "windows")]
                {
                    let windows_sandbox_level =
                        crate::windows_sandbox::level_from_config(&self.config);
                    let windows_degraded_sandbox_enabled =
                        matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken);
                    if !windows_degraded_sandbox_enabled {
                        // This command should not be visible/recognized outside degraded mode,
                        // but guard anyway in case something dispatches it directly.
                        return;
                    }

                    let Some(preset) = builtin_approval_presets()
                        .into_iter()
                        .find(|preset| preset.id == "auto")
                    else {
                        // Avoid panicking in interactive UI; treat this as a recoverable
                        // internal error.
                        self.add_error_message(
                            "Internal error: missing the 'auto' approval preset.".to_string(),
                        );
                        return;
                    };

                    if let Err(err) = self
                        .config
                        .permissions
                        .approval_policy
                        .can_set(&preset.approval)
                    {
                        self.add_error_message(err.to_string());
                        return;
                    }

                    self.session_telemetry.counter(
                        "codex.windows_sandbox.setup_elevated_sandbox_command",
                        /*inc*/ 1,
                        &[],
                    );
                    self.app_event_tx
                        .send(AppEvent::BeginWindowsSandboxElevatedSetup {
                            preset,
                            profile_selection: None,
                        });
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = &self.session_telemetry;
                    // Not supported; on non-Windows this command should never be reachable.
                }
            }
            SlashCommand::SandboxReadRoot => {
                self.add_error_message(
                    "Usage: /sandbox-add-read-dir <absolute-directory-path>".to_string(),
                );
            }
            SlashCommand::Experimental => {
                self.open_experimental_popup();
            }
            SlashCommand::AutoReview => {
                self.open_auto_review_denials_popup();
            }
            SlashCommand::Memories => {
                self.open_memories_popup();
            }
            SlashCommand::Quit | SlashCommand::Exit => {
                self.request_quit_without_confirmation();
            }
            SlashCommand::Logout => {
                self.add_error_message(
                    "tiffany-loop does not use account login. Configure providers with /provider or `orchestrator config provider`."
                        .to_string(),
                );
            }
            SlashCommand::Copy => {
                self.copy_last_agent_markdown();
            }
            SlashCommand::Raw => {
                let enabled = self.toggle_raw_output_mode_and_notify();
                self.emit_raw_output_mode_changed(enabled);
            }
            SlashCommand::Diff => {
                self.add_diff_in_progress();
                let tx = self.app_event_tx.clone();
                let runner = self.workspace_command_runner.clone();
                let cwd = self
                    .current_cwd
                    .clone()
                    .unwrap_or_else(|| self.config.cwd.to_path_buf());
                tokio::spawn(async move {
                    let text = match runner {
                        Some(runner) => match get_git_diff(runner.as_ref(), &cwd).await {
                            Ok((is_git_repo, diff_text)) => {
                                if is_git_repo {
                                    diff_text
                                } else {
                                    "`/diff` — _not inside a git repository_".to_string()
                                }
                            }
                            Err(e) => format!("Failed to compute diff: {e}"),
                        },
                        None => "Failed to compute diff: workspace command runner unavailable"
                            .to_string(),
                    };
                    tx.send(AppEvent::DiffResult(text));
                });
            }
            SlashCommand::Mention => {
                self.insert_str("@");
            }
            SlashCommand::Skills => {
                self.open_skills_menu();
            }
            SlashCommand::Import => {
                self.app_event_tx
                    .send(AppEvent::OpenExternalAgentConfigMigration);
            }
            SlashCommand::Hooks => {
                self.add_hooks_output();
            }
            SlashCommand::Status => {
                if self.should_prefetch_rate_limits() {
                    let request_id = self.next_status_refresh_request_id;
                    self.next_status_refresh_request_id =
                        self.next_status_refresh_request_id.wrapping_add(1);
                    self.add_status_output(/*refreshing_rate_limits*/ true, Some(request_id));
                    self.app_event_tx.send(AppEvent::RefreshRateLimits {
                        origin: RateLimitRefreshOrigin::StatusCommand { request_id },
                    });
                } else {
                    self.add_status_output(
                        /*refreshing_rate_limits*/ false, /*request_id*/ None,
                    );
                }
            }
            SlashCommand::Usage => {
                if self.ensure_token_activity_command_available() {
                    self.add_token_activity_output(tokens::TokenActivityView::Daily);
                }
            }
            SlashCommand::Ide => {
                self.handle_ide_command();
            }
            SlashCommand::DebugConfig => {
                self.add_debug_config_output();
            }
            SlashCommand::Title => {
                self.open_terminal_title_setup();
            }
            SlashCommand::Statusline => {
                self.open_status_line_setup();
            }
            SlashCommand::Theme => {
                self.open_theme_picker();
            }
            SlashCommand::Pets => {
                self.open_pets_picker();
            }
            SlashCommand::Ps => {
                self.add_ps_output();
            }
            SlashCommand::Stop => {
                self.clean_background_terminals();
            }
            SlashCommand::MemoryDrop => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::MemoryUpdate => {
                self.add_app_server_stub_message("Memory maintenance");
            }
            SlashCommand::Mcp => {
                self.add_mcp_output(McpServerStatusDetail::ToolsAndAuthOnly);
            }
            SlashCommand::Apps => {
                self.add_connectors_output();
            }
            SlashCommand::Plugins => {
                self.add_plugins_output();
            }
            SlashCommand::Rollout => {
                if let Some(path) = self.rollout_path() {
                    self.add_info_message(
                        format!("Current rollout path: {}", path.display()),
                        /*hint*/ None,
                    );
                } else {
                    self.add_info_message(
                        "Rollout path is not available yet.".to_string(),
                        /*hint*/ None,
                    );
                }
            }
            SlashCommand::TestApproval => {
                use std::collections::HashMap;

                use crate::approval_events::ApplyPatchApprovalRequestEvent;
                use crate::diff_model::FileChange;

                self.on_apply_patch_approval_request(
                    "1".to_string(),
                    ApplyPatchApprovalRequestEvent {
                        call_id: "1".to_string(),
                        turn_id: "turn-1".to_string(),
                        changes: HashMap::from([
                            (
                                PathBuf::from("/tmp/test.txt"),
                                FileChange::Add {
                                    content: "test".to_string(),
                                },
                            ),
                            (
                                PathBuf::from("/tmp/test2.txt"),
                                FileChange::Update {
                                    unified_diff: "+test\n-test2".to_string(),
                                    move_path: None,
                                },
                            ),
                        ]),
                        reason: None,
                        grant_root: Some(PathBuf::from("/tmp")),
                    },
                );
            }
        }
    }

    /// Run an inline slash command.
    ///
    /// Branches that prepare arguments should pass `record_history: false` to the composer because
    /// the staged slash-command entry is the recall record; using the normal submission-history
    /// path as well would make a single command appear twice during Up-arrow navigation.
    pub(super) fn dispatch_command_with_args(
        &mut self,
        cmd: SlashCommand,
        args: String,
        text_elements: Vec<TextElement>,
    ) {
        if !self.ensure_slash_command_allowed_in_side_conversation(cmd) {
            return;
        }
        if !self.ensure_side_command_allowed_outside_review(cmd) {
            return;
        }
        if !cmd.supports_inline_args() {
            self.dispatch_command(cmd);
            return;
        }
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.request_redraw();
            return;
        }

        let trimmed = args.trim();
        if trimmed.is_empty() {
            self.dispatch_command(cmd);
            return;
        }

        if cmd == SlashCommand::Goal {
            self.dispatch_prepared_command_with_args(
                cmd,
                PreparedSlashCommandArgs {
                    args,
                    text_elements,
                    pending_pastes: self.bottom_pane.composer_pending_pastes(),
                    local_images: self.bottom_pane.composer_local_images(),
                    remote_image_urls: self.bottom_pane.remote_image_urls(),
                    mention_bindings: Vec::new(),
                    source: SlashCommandDispatchSource::Live,
                },
            );
            return;
        }

        let Some((prepared_args, prepared_elements)) =
            self.prepare_live_inline_args(args, text_elements)
        else {
            return;
        };
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: prepared_args,
                text_elements: prepared_elements,
                pending_pastes: Vec::new(),
                local_images: Vec::new(),
                remote_image_urls: Vec::new(),
                mention_bindings: Vec::new(),
                source: SlashCommandDispatchSource::Live,
            },
        );
    }

    fn prepare_live_inline_args(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
    ) -> Option<(String, Vec<TextElement>)> {
        if self.bottom_pane.composer_text().is_empty() {
            Some((args, text_elements))
        } else {
            self.bottom_pane
                .prepare_inline_args_submission(/*record_history*/ false)
        }
    }

    fn clear_live_goal_submission(&mut self) {
        self.bottom_pane
            .set_composer_text(String::new(), Vec::new(), Vec::new());
        self.bottom_pane.set_composer_pending_pastes(Vec::new());
        self.bottom_pane.drain_pending_submission_state();
    }

    fn prepared_inline_user_message(
        &mut self,
        args: String,
        text_elements: Vec<TextElement>,
        mut local_images: Vec<LocalImageAttachment>,
        mut remote_image_urls: Vec<String>,
        mut mention_bindings: Vec<MentionBinding>,
        source: SlashCommandDispatchSource,
    ) -> UserMessage {
        if source == SlashCommandDispatchSource::Live {
            local_images = self
                .bottom_pane
                .take_recent_submission_images_with_placeholders();
            remote_image_urls = self.take_remote_image_urls();
            mention_bindings = self.bottom_pane.take_recent_submission_mention_bindings();
        }
        UserMessage {
            text: args,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        }
    }

    fn dispatch_prepared_command_with_args(
        &mut self,
        cmd: SlashCommand,
        prepared: PreparedSlashCommandArgs,
    ) {
        let PreparedSlashCommandArgs {
            args,
            text_elements,
            pending_pastes,
            local_images,
            remote_image_urls,
            mention_bindings,
            source,
        } = prepared;
        let trimmed = args.trim();
        match cmd {
            SlashCommand::Usage => {
                if self.ensure_token_activity_command_available() {
                    match tokens::TokenActivityView::parse(trimmed) {
                        Some(view) => self.add_token_activity_output(view),
                        None => self.add_error_message(
                            "Usage: /usage [daily|weekly|cumulative]".to_string(),
                        ),
                    }
                }
            }
            SlashCommand::Ide => {
                self.handle_ide_command_args(trimmed);
            }
            SlashCommand::Mcp => match trimmed.to_ascii_lowercase().as_str() {
                "verbose" => self.add_mcp_output(McpServerStatusDetail::Full),
                _ => self.add_error_message("Usage: /mcp [verbose]".to_string()),
            },
            SlashCommand::Keymap => match trimmed.to_ascii_lowercase().as_str() {
                "" => self.open_keymap_picker(),
                "debug" => {
                    match crate::keymap::RuntimeKeymap::from_config(&self.config.tui_keymap) {
                        Ok(runtime_keymap) => self.open_keymap_debug(&runtime_keymap),
                        Err(err) => {
                            self.add_error_message(format!(
                                "Invalid `tui.keymap` configuration: {err}"
                            ));
                        }
                    }
                }
                _ => self.add_error_message("Usage: /keymap [debug]".to_string()),
            },
            SlashCommand::Raw => match trimmed.to_ascii_lowercase().as_str() {
                "on" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ true);
                    self.emit_raw_output_mode_changed(/*enabled*/ true);
                }
                "off" => {
                    self.set_raw_output_mode_and_notify(/*enabled*/ false);
                    self.emit_raw_output_mode_changed(/*enabled*/ false);
                }
                _ => self.add_error_message(RAW_USAGE.to_string()),
            },
            SlashCommand::Rename if !trimmed.is_empty() => {
                if !self.ensure_thread_rename_allowed() {
                    return;
                }
                self.session_telemetry
                    .counter("codex.thread.rename", /*inc*/ 1, &[]);
                let Some(name) = normalize_thread_name(&args) else {
                    self.add_error_message("Thread name cannot be empty.".to_string());
                    return;
                };
                self.app_event_tx.set_thread_name(name);
            }
            SlashCommand::Plan if !trimmed.is_empty() => {
                if !self.apply_plan_slash_command() {
                    return;
                }
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                if self.is_session_configured() {
                    self.reasoning_buffer.clear();
                    self.full_reasoning_buffer.clear();
                    self.set_status_header(String::from("Working"));
                    self.submit_user_message(user_message);
                } else {
                    self.queue_user_message(user_message);
                }
            }
            SlashCommand::Goal if !trimmed.is_empty() => {
                if !self.config.features.enabled(Feature::Goals) {
                    if source == SlashCommandDispatchSource::Live {
                        self.clear_live_goal_submission();
                    }
                    return;
                }
                enum GoalControlCommand {
                    Clear,
                    SetStatus(AppThreadGoalStatus),
                }
                let control_command = match trimmed.to_ascii_lowercase().as_str() {
                    "clear" => Some(GoalControlCommand::Clear),
                    "edit" => {
                        self.app_event_tx.send(AppEvent::OpenThreadGoalEditor {
                            thread_id: self.thread_id,
                        });
                        if source == SlashCommandDispatchSource::Live {
                            self.clear_live_goal_submission();
                        }
                        return;
                    }
                    "pause" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Paused)),
                    "resume" => Some(GoalControlCommand::SetStatus(AppThreadGoalStatus::Active)),
                    _ => None,
                };
                if let Some(command) = control_command {
                    let Some(thread_id) = self.thread_id else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some(
                                "The session must start before you can change a goal.".to_string(),
                            ),
                        );
                        if source == SlashCommandDispatchSource::Live {
                            self.clear_live_goal_submission();
                        }
                        return;
                    };
                    match command {
                        GoalControlCommand::Clear => {
                            self.app_event_tx
                                .send(AppEvent::ClearThreadGoal { thread_id });
                        }
                        GoalControlCommand::SetStatus(status) => {
                            self.app_event_tx
                                .send(AppEvent::SetThreadGoalStatus { thread_id, status });
                        }
                    }
                    self.append_message_history_entry(format!("/goal {trimmed}"));
                    if source == SlashCommandDispatchSource::Live {
                        self.clear_live_goal_submission();
                    }
                    return;
                }
                let draft = GoalDraft {
                    objective: args,
                    text_elements,
                    pending_pastes,
                    local_images,
                    remote_image_urls,
                };
                let Some(thread_id) = self.thread_id else {
                    if source == SlashCommandDispatchSource::Live {
                        const GOAL_PREFIX: &str = "/goal ";
                        let text_elements = draft
                            .text_elements
                            .into_iter()
                            .map(|element| {
                                element.map_range(|range| ByteRange {
                                    start: range.start + GOAL_PREFIX.len(),
                                    end: range.end + GOAL_PREFIX.len(),
                                })
                            })
                            .collect();
                        self.queue_user_message_with_options(
                            UserMessage {
                                text: format!("{GOAL_PREFIX}{}", draft.objective),
                                local_images: draft.local_images,
                                remote_image_urls: draft.remote_image_urls,
                                text_elements,
                                mention_bindings: Vec::new(),
                            },
                            QueuedInputAction::ParseSlash,
                            draft.pending_pastes,
                        );
                        self.clear_live_goal_submission();
                    } else {
                        self.add_info_message(
                            GOAL_USAGE.to_string(),
                            Some("The session must start before you can set a goal.".to_string()),
                        );
                    }
                    return;
                };
                let history_objective = draft.objective.clone();
                self.app_event_tx.send(AppEvent::SetThreadGoalDraft {
                    thread_id,
                    draft,
                    mode: ThreadGoalSetMode::ConfirmIfExists,
                });
                self.append_message_history_entry(format!("/goal {history_objective}"));
                if source == SlashCommandDispatchSource::Live {
                    self.clear_live_goal_submission();
                }
            }
            SlashCommand::Roles if !trimmed.is_empty() => {
                self.dispatch_tiffany_roles_command(args);
            }
            SlashCommand::Role if !trimmed.is_empty() => {
                if let Some(role) = role_setup_prompt_role(trimmed) {
                    self.open_tiffany_role_setup_prompt(role);
                } else {
                    self.dispatch_tiffany_role_command(args);
                }
            }
            SlashCommand::Provider if !trimmed.is_empty() => {
                if let Some(provider) = provider_setup_prompt_provider(trimmed) {
                    self.open_tiffany_provider_setup_prompt(provider);
                } else {
                    self.dispatch_tiffany_provider_command(args);
                }
            }
            SlashCommand::Doctor if !trimmed.is_empty() => {
                self.dispatch_tiffany_doctor_command(args);
            }
            SlashCommand::Thread if !trimmed.is_empty() => {
                self.dispatch_tiffany_thread_command(args);
            }
            SlashCommand::Side | SlashCommand::Btw if !trimmed.is_empty() => {
                let Some(parent_thread_id) = self.thread_id else {
                    let command = cmd.command();
                    self.add_error_message(format!(
                        "'/{command}' is unavailable before the session starts."
                    ));
                    return;
                };
                let user_message = self.prepared_inline_user_message(
                    args,
                    text_elements,
                    local_images,
                    remote_image_urls,
                    mention_bindings,
                    source,
                );
                self.request_side_conversation(parent_thread_id, Some(user_message));
            }
            SlashCommand::Review if !trimmed.is_empty() => {
                self.submit_op(AppCommand::review(ReviewTarget::Custom {
                    instructions: args,
                }));
            }
            SlashCommand::Resume if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::ResumeSessionByIdOrName(args));
            }
            SlashCommand::SandboxReadRoot if !trimmed.is_empty() => {
                self.app_event_tx
                    .send(AppEvent::BeginWindowsSandboxGrantReadRoot { path: args });
            }
            SlashCommand::Pets
                if matches!(
                    args.trim().to_ascii_lowercase().as_str(),
                    "disable" | "disabled" | "hide" | "hidden" | "off" | "none"
                ) =>
            {
                self.app_event_tx.send(AppEvent::PetDisabled);
            }
            SlashCommand::Pets if !trimmed.is_empty() => {
                self.select_pet_by_id(args);
            }
            _ => self.dispatch_command(cmd),
        }
        if source == SlashCommandDispatchSource::Live && cmd != SlashCommand::Goal {
            self.bottom_pane.drain_pending_submission_state();
        }
    }

    pub(super) fn submit_queued_slash_prompt(
        &mut self,
        queued_message: QueuedUserMessage,
    ) -> QueueDrain {
        let QueuedUserMessage {
            user_message,
            pending_pastes,
            ..
        } = queued_message;
        let UserMessage {
            text,
            local_images,
            remote_image_urls,
            text_elements,
            mention_bindings,
        } = user_message;
        let Some((name, rest, rest_offset)) = parse_slash_name(&text) else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        if name.contains('/') {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }

        let service_tier_commands = self.current_model_service_tier_commands();
        let Some(command) =
            find_slash_command(name, self.builtin_command_flags(), &service_tier_commands)
        else {
            let message = if self.tiffany_orchestrator_shell {
                tiffany_orchestrator_unsupported_command_message(name)
            } else {
                None
            }
            .unwrap_or_else(|| {
                format!(
                    r#"Unrecognized command '/{name}'. Type "/" for a list of supported commands."#
                )
            });
            self.add_info_message(message, /*hint*/ None);
            return QueueDrain::Continue;
        };

        if rest.is_empty() {
            return match command {
                SlashCommandItem::Builtin(cmd) => {
                    self.dispatch_command(cmd);
                    self.queued_command_drain_result(cmd)
                }
                SlashCommandItem::ServiceTier(command) => {
                    self.handle_service_tier_command_dispatch(command);
                    QueueDrain::Continue
                }
            };
        }

        if !command.supports_inline_args() {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        }
        let SlashCommandItem::Builtin(cmd) = command else {
            self.submit_user_message(UserMessage {
                text,
                local_images,
                remote_image_urls,
                text_elements,
                mention_bindings,
            });
            return QueueDrain::Stop;
        };

        let trimmed_start = rest.trim_start();
        let leading_trimmed = rest.len().saturating_sub(trimmed_start.len());
        let trimmed_rest = trimmed_start.trim_end();
        let args_elements = Self::slash_command_args_elements(
            trimmed_rest,
            rest_offset + leading_trimmed,
            &text_elements,
        );
        self.dispatch_prepared_command_with_args(
            cmd,
            PreparedSlashCommandArgs {
                args: trimmed_rest.to_string(),
                text_elements: args_elements,
                pending_pastes,
                local_images,
                remote_image_urls,
                mention_bindings,
                source: SlashCommandDispatchSource::Queued,
            },
        );
        self.queued_command_drain_result(cmd)
    }

    fn builtin_command_flags(&self) -> BuiltinCommandFlags {
        #[cfg(target_os = "windows")]
        let allow_elevate_sandbox = {
            let windows_sandbox_level = crate::windows_sandbox::level_from_config(&self.config);
            matches!(windows_sandbox_level, WindowsSandboxLevel::RestrictedToken)
        };
        #[cfg(not(target_os = "windows"))]
        let allow_elevate_sandbox = false;

        BuiltinCommandFlags {
            collaboration_modes_enabled: self.collaboration_modes_enabled(),
            connectors_enabled: self.connectors_enabled(),
            plugins_command_enabled: self.config.features.enabled(Feature::Plugins),
            token_activity_command_enabled: self.has_codex_backend_auth,
            goal_command_enabled: self.config.features.enabled(Feature::Goals),
            service_tier_commands_enabled: self.fast_mode_enabled(),
            personality_command_enabled: self.config.features.enabled(Feature::Personality),
            allow_elevate_sandbox,
            side_conversation_active: self.active_side_conversation,
            tiffany_orchestrator_shell: self.tiffany_orchestrator_shell,
        }
    }

    fn ensure_token_activity_command_available(&mut self) -> bool {
        if self.has_codex_backend_auth {
            return true;
        }
        self.add_error_message(USAGE_CHATGPT_LOGIN_REQUIRED.to_string());
        false
    }

    fn queued_command_drain_result(&self, cmd: SlashCommand) -> QueueDrain {
        if self.is_user_turn_pending_or_running() || !self.bottom_pane.no_modal_or_popup_active() {
            return QueueDrain::Stop;
        }
        match cmd {
            SlashCommand::Ide
            | SlashCommand::Status
            | SlashCommand::Usage
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Rollout
            | SlashCommand::Copy
            | SlashCommand::Raw
            | SlashCommand::Vim
            | SlashCommand::Diff
            | SlashCommand::App
            | SlashCommand::Rename
            | SlashCommand::Provider
            | SlashCommand::Role
            | SlashCommand::Roles
            | SlashCommand::Thread
            | SlashCommand::Doctor
            | SlashCommand::TestApproval => QueueDrain::Continue,
            SlashCommand::Feedback
            | SlashCommand::New
            | SlashCommand::Archive
            | SlashCommand::Delete
            | SlashCommand::Clear
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Review
            | SlashCommand::Model
            | SlashCommand::Personality
            | SlashCommand::Plan
            | SlashCommand::Goal
            | SlashCommand::Side
            | SlashCommand::Btw
            | SlashCommand::Keymap
            | SlashCommand::Agent
            | SlashCommand::MultiAgents
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::AutoReview
            | SlashCommand::Memories
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Logout
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Import
            | SlashCommand::Hooks
            | SlashCommand::Title
            | SlashCommand::Statusline
            | SlashCommand::Theme
            | SlashCommand::Pets => QueueDrain::Stop,
        }
    }

    fn slash_command_args_elements(
        rest: &str,
        rest_offset: usize,
        text_elements: &[TextElement],
    ) -> Vec<TextElement> {
        if rest.is_empty() || text_elements.is_empty() {
            return Vec::new();
        }
        text_elements
            .iter()
            .filter_map(|elem| {
                if elem.byte_range.end <= rest_offset {
                    return None;
                }
                let start = elem.byte_range.start.saturating_sub(rest_offset);
                let mut end = elem.byte_range.end.saturating_sub(rest_offset);
                if start >= rest.len() {
                    return None;
                }
                end = end.min(rest.len());
                (start < end).then_some(elem.map_range(|_| ByteRange { start, end }))
            })
            .collect()
    }

    fn ensure_slash_command_allowed_in_side_conversation(&mut self, cmd: SlashCommand) -> bool {
        if !self.active_side_conversation || cmd.available_in_side_conversation() {
            return true;
        }
        self.add_error_message(format!(
            "'/{}' is unavailable in side conversations. {SIDE_SLASH_COMMAND_UNAVAILABLE_HINT}",
            cmd.command()
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }

    fn ensure_side_command_allowed_outside_review(&mut self, cmd: SlashCommand) -> bool {
        if !matches!(cmd, SlashCommand::Side | SlashCommand::Btw) || !self.review.is_review_mode {
            return true;
        }

        let command = cmd.command();
        self.add_error_message(format!(
            "'/{command}' is unavailable while code review is running."
        ));
        self.bottom_pane.drain_pending_submission_state();
        false
    }
}

fn provider_setup_prompt_provider(args: &str) -> Option<Option<&str>> {
    let mut parts = args.split_whitespace();
    let first = parts.next()?;
    if first.eq_ignore_ascii_case("setup")
        || first.eq_ignore_ascii_case("input")
        || first.eq_ignore_ascii_case("edit")
        || first.eq_ignore_ascii_case("modify")
    {
        let provider = parts.next();
        if parts.next().is_some() {
            return None;
        }
        return Some(provider);
    }

    if matches!(
        first.to_ascii_lowercase().as_str(),
        "list"
            | "show"
            | "status"
            | "key"
            | "set-key"
            | "env"
            | "set-env"
            | "endpoint"
            | "base-url"
            | "url"
            | "set-endpoint"
    ) || parts.next().is_some()
    {
        return None;
    }

    Some(Some(first))
}

fn role_setup_prompt_role(args: &str) -> Option<Option<&str>> {
    let mut parts = args.split_whitespace();
    let first = parts.next()?;
    if first.eq_ignore_ascii_case("setup")
        || first.eq_ignore_ascii_case("input")
        || first.eq_ignore_ascii_case("edit")
        || first.eq_ignore_ascii_case("modify")
    {
        let role = parts.next();
        if parts.next().is_some() {
            return None;
        }
        return Some(role);
    }

    if matches!(
        first.to_ascii_lowercase().as_str(),
        "list" | "show" | "route" | "use" | "snippet" | "save" | "set" | "add" | "register"
    ) || parts.next().is_some()
    {
        return None;
    }

    Some(Some(first))
}

fn provider_setup_initial_draft(
    provider: &str,
    config: Option<&crate::tiffany_orchestrator::TiffanyOrchestratorConfig>,
) -> ProviderSetupDraft {
    let provider = provider.trim();
    let provider = if provider.is_empty() {
        "openai"
    } else {
        provider
    };
    let existing = config.and_then(|config| read_provider_config(config, provider));
    let existing_key = existing
        .as_ref()
        .and_then(|provider| provider.api_key.as_deref());
    let env = existing_key
        .and_then(env_var_reference)
        .or_else(|| provider_default_env(provider).map(str::to_string))
        .unwrap_or_default();
    let key = match existing_key {
        Some(value) if !value.trim().is_empty() && env_var_reference(value).is_none() => {
            "<unchanged>".to_string()
        }
        _ => String::new(),
    };

    let kind = existing
        .as_ref()
        .and_then(|provider| provider.kind.clone())
        .unwrap_or_else(|| provider_default_kind(provider).to_string());

    ProviderSetupDraft {
        provider: provider.to_string(),
        kind,
        env,
        key,
        endpoint: existing
            .as_ref()
            .and_then(|provider| provider.base_url.clone())
            .or_else(|| provider_default_endpoint(provider).map(str::to_string))
            .unwrap_or_default(),
    }
}

fn role_setup_initial_draft(role: Option<&str>) -> RoleSetupDraft {
    let role = role
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("worker-codex");
    let defaults = role_default_setup(role).unwrap_or(WORKER_CODEX_ROLE_DEFAULTS);
    RoleSetupDraft {
        role: role.to_string(),
        provider: defaults.provider.to_string(),
        model_id: defaults.model_id.to_string(),
        model_name: defaults.model_name.to_string(),
        runtime: defaults.runtime.to_string(),
        teams: defaults.teams.to_string(),
    }
}

#[derive(Debug, Default)]
struct RawProviderConfig {
    kind: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
}

fn read_provider_config(
    config: &crate::tiffany_orchestrator::TiffanyOrchestratorConfig,
    provider: &str,
) -> Option<RawProviderConfig> {
    let path = orchestrator_config_path(config)?;
    let raw = std::fs::read_to_string(path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).ok()?;
    let providers = yaml.get("providers")?.as_mapping()?;
    let provider_value = serde_yaml::Value::String(provider.to_string());
    let mapping = providers.get(&provider_value)?.as_mapping()?;
    Some(RawProviderConfig {
        kind: yaml_mapping_string(mapping, "type"),
        api_key: yaml_mapping_string(mapping, "api_key"),
        base_url: yaml_mapping_string(mapping, "base_url"),
    })
}

fn orchestrator_config_path(
    config: &crate::tiffany_orchestrator::TiffanyOrchestratorConfig,
) -> Option<std::path::PathBuf> {
    match config
        .config_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    {
        Some(path) => Some(expand_home_path(path)),
        None => dirs::home_dir().map(|home| home.join(".orchestrator/config.yaml")),
    }
}

fn expand_home_path(path: &str) -> std::path::PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(path)
}

fn yaml_mapping_string(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string)
}

fn env_var_reference(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(env) = value
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
    {
        return (!env.trim().is_empty()).then(|| env.trim().to_string());
    }
    value
        .strip_prefix('$')
        .filter(|env| !env.trim().is_empty())
        .map(|env| env.trim().to_string())
}

fn provider_default_kind(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => "anthropic",
        "google" | "gemini" => "google",
        "ollama" => "ollama",
        _ => "openai",
    }
}

fn provider_default_env(provider: &str) -> Option<&'static str> {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "minimax" => Some("MINIMAX_API_KEY"),
        "deepseek" => Some("DEEPSEEK_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "moonshot" => Some("MOONSHOT_API_KEY"),
        "openai" => Some("OPENAI_API_KEY"),
        "mistral" => Some("MISTRAL_API_KEY"),
        "cohere" => Some("COHERE_API_KEY"),
        "custom" => Some("CUSTOM_API_KEY"),
        "google" | "gemini" => Some("GOOGLE_API_KEY"),
        "ollama" => None,
        _ => Some("OPENAI_API_KEY"),
    }
}

fn provider_default_endpoint(provider: &str) -> Option<&'static str> {
    match provider.to_ascii_lowercase().as_str() {
        "ollama" => Some("http://localhost:11434"),
        "minimax" => Some("https://api.minimaxi.com/v1"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "moonshot" => Some("https://api.moonshot.ai/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        _ => None,
    }
}

pub(super) fn provider_setup_draft_args(draft: &ProviderSetupDraft) -> Result<String, String> {
    let provider = draft.provider.trim();
    let kind = draft.kind.trim();
    let env = draft.env.trim().trim_start_matches('$');
    let key = draft.key.trim();
    let endpoint = draft.endpoint.trim();

    if provider.is_empty() {
        return Err("provider is required".to_string());
    }
    if !env.is_empty() && !key.is_empty() {
        return Err("fill either env or key, not both".to_string());
    }

    let mut args = vec!["setup".to_string(), provider.to_string()];
    if !kind.is_empty() {
        args.push("--type".to_string());
        args.push(kind.to_string());
    }
    if !env.is_empty() {
        args.push("--env".to_string());
        args.push(env.to_string());
    }
    if !key.is_empty() && !is_keep_key_marker(key) {
        args.push("--key".to_string());
        args.push(key.to_string());
    }
    if !endpoint.is_empty() {
        args.push("--endpoint".to_string());
        args.push(endpoint.to_string());
    }

    Ok(args.join(" "))
}

#[derive(Clone, Copy)]
struct RoleSetupDefaults {
    provider: &'static str,
    model_id: &'static str,
    model_name: &'static str,
    runtime: &'static str,
    teams: &'static str,
}

const WORKER_CODEX_ROLE_DEFAULTS: RoleSetupDefaults = RoleSetupDefaults {
    provider: "minimax",
    model_id: "minimax-m3-codex",
    model_name: "MiniMax-M3",
    runtime: "codex",
    teams: "no",
};

fn role_default_setup(role: &str) -> Option<RoleSetupDefaults> {
    match role.to_ascii_lowercase().as_str() {
        "worker-codex" => Some(WORKER_CODEX_ROLE_DEFAULTS),
        "worker-cc" => Some(RoleSetupDefaults {
            provider: "anthropic",
            model_id: "sonnet",
            model_name: "claude-sonnet-4-6",
            runtime: "claude-code",
            teams: "yes",
        }),
        "planner" => Some(RoleSetupDefaults {
            provider: "openai",
            model_id: "gpt4o",
            model_name: "gpt-4o",
            runtime: "codex",
            teams: "no",
        }),
        "critic" => Some(RoleSetupDefaults {
            provider: "openai",
            model_id: "gpt4o",
            model_name: "gpt-4o",
            runtime: "codex",
            teams: "no",
        }),
        "reviewer" => Some(RoleSetupDefaults {
            provider: "openai",
            model_id: "gpt4o-mini",
            model_name: "gpt-4o-mini",
            runtime: "codex",
            teams: "no",
        }),
        _ => None,
    }
}

pub(super) fn role_setup_draft_args(draft: &RoleSetupDraft) -> Result<String, String> {
    let role = draft.role.trim();
    let provider = draft.provider.trim();
    let model_id = draft.model_id.trim();
    let model_name = draft.model_name.trim();
    let runtime = draft.runtime.trim();
    let teams = draft.teams.trim().to_ascii_lowercase();

    if role.is_empty() {
        return Err("role is required".to_string());
    }
    if model_id.is_empty() {
        return Err("model is required".to_string());
    }
    if runtime.is_empty() {
        return Err("runtime is required".to_string());
    }

    let mut args = vec![
        "register".to_string(),
        role.to_string(),
        "--model".to_string(),
        model_id.to_string(),
        "--runtime".to_string(),
        runtime.to_string(),
    ];
    if !provider.is_empty() {
        args.push("--provider".to_string());
        args.push(provider.to_string());
    }
    if !model_name.is_empty() {
        args.push("--model-name".to_string());
        args.push(model_name.to_string());
    }
    match teams.as_str() {
        "yes" | "true" | "on" | "agent-teams" => args.push("--agent-teams".to_string()),
        "no" | "false" | "off" | "no-agent-teams" => args.push("--no-agent-teams".to_string()),
        "auto" | "" => {}
        _ => return Err("teams must be auto, yes, or no".to_string()),
    }

    Ok(args.join(" "))
}

fn is_keep_key_marker(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "<unchanged>" | "unchanged" | "keep")
}
