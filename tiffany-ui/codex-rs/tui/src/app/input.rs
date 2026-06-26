//! Keyboard input, external editor, and status-line dispatch for the TUI app.
//!
//! This module owns global key bindings that sit above ChatWidget, including transcript overlay
//! entry, Ctrl-L clear, external editor launch, and agent navigation shortcuts.

use super::*;
use crate::app_backtrack::SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE;
use std::path::Path;
use std::process::Stdio;

#[cfg(windows)]
fn default_shell_program() -> &'static str {
    "cmd.exe"
}

#[cfg(windows)]
fn default_shell_flag() -> &'static str {
    "/C"
}

#[cfg(not(windows))]
fn default_shell_program() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

#[cfg(not(windows))]
fn default_shell_flag() -> &'static str {
    "-lc"
}

async fn native_cli_return_snapshot(worktree: Option<&str>) -> crate::tiffany_orchestrator::TiffanyNativeCliReturn {
    let Some(worktree) = worktree.map(str::trim).filter(|value| !value.is_empty()) else {
        return crate::tiffany_orchestrator::TiffanyNativeCliReturn::default();
    };
    let cwd = Path::new(worktree);
    crate::tiffany_orchestrator::TiffanyNativeCliReturn {
        status: git_output(cwd, &["status", "--short"]).await,
        diff_stat: git_output(cwd, &["diff", "--stat"]).await,
        diff_patch: git_output(cwd, &["diff"]).await,
        staged_diff_stat: git_output(cwd, &["diff", "--cached", "--stat"]).await,
        staged_diff_patch: git_output(cwd, &["diff", "--cached"]).await,
        transcript_event_count: 0,
        transcript_preview: None,
    }
}

async fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

impl App {
    pub(super) async fn launch_external_editor(&mut self, tui: &mut tui::Tui) {
        let editor_cmd = match external_editor::resolve_editor_command() {
            Ok(cmd) => cmd,
            Err(external_editor::EditorError::MissingEditor) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(
                    "Cannot open external editor: set $VISUAL or $EDITOR before starting tiffany-loop."
                        .to_string(),
                ));
                self.reset_external_editor_state(tui);
                return;
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
                self.reset_external_editor_state(tui);
                return;
            }
        };

        let seed = self.chat_widget.composer_text_with_pending();
        let editor_result = tui
            .with_restored(tui::RestoreMode::KeepRaw, || async {
                external_editor::run_editor(&seed, &editor_cmd).await
            })
            .await;
        self.reset_external_editor_state(tui);

        match editor_result {
            Ok(new_text) => {
                // Trim trailing whitespace
                let cleaned = new_text.trim_end().to_string();
                self.chat_widget.apply_external_edit(cleaned);
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
            }
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) async fn open_tiffany_native_cli(
        &mut self,
        tui: &mut tui::Tui,
        command: crate::tiffany_orchestrator::TiffanyNativeCliCommand,
    ) {
        let command_text = command.command.clone();
        let transcript_cursor =
            crate::tiffany_orchestrator::native_cli_transcript_cursor(&command);
        let status = tui
            .with_restored(tui::RestoreMode::Full, || async move {
                let mut shell = tokio::process::Command::new(default_shell_program());
                shell
                    .arg(default_shell_flag())
                    .arg(&command_text)
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .status()
                    .await
            })
            .await;

        match status {
            Ok(status) if status.success() => {
                let outcome = native_cli_return_snapshot(command.worktree.as_deref()).await;
                let transcript_events =
                    crate::tiffany_orchestrator::native_cli_transcript_events_since(
                        &command,
                        transcript_cursor.as_ref(),
                    );
                let transcript_preview =
                    crate::tiffany_orchestrator::native_cli_transcript_preview(
                        &transcript_events,
                    );
                let outcome = crate::tiffany_orchestrator::TiffanyNativeCliReturn {
                    transcript_event_count: transcript_events.len(),
                    transcript_preview,
                    ..outcome
                };
                self.chat_widget.add_plain_history_lines(
                    crate::tiffany_orchestrator::native_cli_return_lines(&command, &outcome),
                );
                if let Ok(Some(path)) = crate::tiffany_orchestrator::append_native_cli_return(
                    self.config.codex_home.as_path(),
                    self.config.cwd.as_path(),
                    &command,
                    &outcome,
                    transcript_events,
                ) && let Some(config) = self.tiffany_orchestrator.clone()
                {
                    crate::tiffany_orchestrator::spawn_native_history_import(config, path);
                }
            }
            Ok(status) => {
                self.chat_widget.add_to_history(history_cell::new_error_event(
                    format!("{} native CLI exited with {status}", command.role),
                ));
            }
            Err(err) => {
                self.chat_widget.add_to_history(history_cell::new_error_event(
                    format!("Failed to open {} native CLI: {err}", command.role),
                ));
            }
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn request_external_editor_launch(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Requested);
        self.chat_widget.set_footer_hint_override(Some(vec![(
            EXTERNAL_EDITOR_HINT.to_string(),
            String::new(),
        )]));
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn reset_external_editor_state(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Closed);
        self.chat_widget.set_footer_hint_override(/*items*/ None);
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn apply_raw_output_mode(
        &mut self,
        tui: &mut tui::Tui,
        enabled: bool,
        notify: bool,
    ) {
        if notify {
            self.chat_widget.set_raw_output_mode_and_notify(enabled);
        } else {
            self.chat_widget.set_raw_output_mode(enabled);
        }
        if let Err(err) = self.reflow_transcript_now(tui) {
            tracing::warn!(error = %err, "failed to reflow transcript after raw output mode toggle");
            self.chat_widget
                .add_error_message(format!("Failed to redraw transcript: {err}"));
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) async fn handle_key_event(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        key_event: KeyEvent,
    ) {
        // Some terminals, especially on macOS, encode Option+Left/Right as Option+b/f unless
        // enhanced keyboard reporting is available. We only treat those word-motion fallbacks as
        // agent-switch shortcuts when the composer is empty so we never steal the expected
        // editing behavior for moving across words inside a draft.
        let allow_agent_word_motion_fallback = !self.enhanced_keys_supported
            && self.chat_widget.composer_text_with_pending().is_empty();
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Alt+Left/Right are also natural word-motion keys in the composer. Keep agent
            // fast-switch available only once the draft is empty so editing behavior wins whenever
            // there is text on screen.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && previous_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Previous)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Mirror the previous-agent rule above: empty drafts may use these keys for thread
            // switching, but non-empty drafts keep them for expected word-wise cursor motion.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && next_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Next)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if side_return_shortcut_matches(key_event)
            && self.maybe_return_from_side(tui, app_server).await
        {
            return;
        }

        let app_keymap_shortcuts_available = self.app_keymap_shortcuts_available();

        if app_keymap_shortcuts_available && self.keymap.app.toggle_vim_mode.is_pressed(key_event) {
            self.chat_widget.toggle_vim_mode_and_notify();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.toggle_fast_mode.is_pressed(key_event)
            && self.chat_widget.can_toggle_fast_mode_from_keybinding()
        {
            self.chat_widget.toggle_fast_mode_from_ui();
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.toggle_raw_output.is_pressed(key_event)
        {
            let enabled = !self.chat_widget.raw_output_mode();
            self.apply_raw_output_mode(tui, enabled, /*notify*/ false);
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.open_transcript.is_pressed(key_event) {
            // Enter alternate screen and set viewport to full size.
            let _ = tui.enter_alt_screen();
            self.overlay = Some(Overlay::new_transcript(
                self.transcript_cells.clone(),
                self.keymap.pager.clone(),
            ));
            tui.frame_requester().schedule_frame();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.open_external_editor.is_pressed(key_event)
        {
            // Only launch the external editor if there is no overlay and the bottom pane is not in use.
            // Note that it can be launched while a task is running to enable editing while the previous turn is ongoing.
            if self.overlay.is_none()
                && self.chat_widget.can_launch_external_editor()
                && self.chat_widget.external_editor_state() == ExternalEditorState::Closed
            {
                self.request_external_editor_launch(tui);
            }
            return;
        }

        if matches!(key_event.code, KeyCode::Esc)
            && matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            // Esc primes/advances backtracking only in normal (not working) mode
            // with the composer focused and empty. In any other state, forward
            // Esc so the active UI (e.g. status indicator, modals, popups)
            // handles it.
            if self.should_handle_backtrack_esc(key_event) {
                self.handle_backtrack_esc_key(tui);
            } else if self.should_reject_side_backtrack_esc(key_event) {
                self.reject_side_backtrack_esc();
            } else {
                self.chat_widget.handle_key_event(key_event);
            }
            return;
        }

        match key_event {
            _ if app_keymap_shortcuts_available
                && self.keymap.app.clear_terminal.is_pressed(key_event) =>
            {
                if !self.chat_widget.can_run_ctrl_l_clear_now() {
                    return;
                }
                if let Err(err) = self.clear_terminal_ui(tui, /*redraw_header*/ false) {
                    tracing::warn!(error = %err, "failed to clear terminal UI");
                    self.chat_widget
                        .add_error_message(format!("Failed to clear terminal UI: {err}"));
                } else {
                    self.reset_app_ui_state_after_clear();
                    self.queue_clear_ui_header(tui);
                    tui.frame_requester().schedule_frame();
                }
            }
            // Enter confirms backtrack when primed + count > 0. Otherwise pass to widget.
            KeyEvent {
                code: KeyCode::Enter,
                kind: KeyEventKind::Press,
                ..
            } if self.backtrack.primed
                && self.backtrack.nth_user_message != usize::MAX
                && self.chat_widget.composer_is_empty() =>
            {
                if let Some(selection) = self.confirm_backtrack_from_main() {
                    self.apply_backtrack_selection(tui, selection);
                }
            }
            KeyEvent {
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                // Any non-Esc key press should cancel a primed backtrack.
                // This avoids stale "Esc-primed" state after the user starts typing
                // (even if they later backspace to empty).
                if key_event.code != KeyCode::Esc && self.backtrack.primed {
                    self.reset_backtrack_state();
                }
                self.chat_widget.handle_key_event(key_event);
            }
            _ => {
                self.chat_widget.handle_key_event(key_event);
            }
        };
    }

    pub(super) fn should_handle_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        !self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn should_reject_side_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn reject_side_backtrack_esc(&mut self) {
        self.reset_backtrack_state();
        self.chat_widget
            .add_error_message(SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE.to_string());
    }

    fn app_keymap_shortcuts_available(&self) -> bool {
        self.overlay.is_none() && self.chat_widget.no_modal_or_popup_active()
    }

    pub(super) fn refresh_status_line(&mut self) {
        self.chat_widget.refresh_status_line();
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::make_test_app;

    #[tokio::test]
    async fn app_keymap_shortcuts_are_disabled_while_keymap_view_is_active() {
        let mut app = make_test_app().await;
        assert!(app.app_keymap_shortcuts_available());

        let keymap = app.keymap.clone();
        app.chat_widget.open_keymap_debug(&keymap);

        assert!(!app.app_keymap_shortcuts_available());
    }
}
