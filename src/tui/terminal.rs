//! Claude-style terminal chat that uses normal scrollback.

use super::agent_handoff::{
    agent_continue_command, parse_agent_continue_target, AgentContinueTarget, ExternalAgentCommand,
};
#[cfg(test)]
use super::codex_composer::{
    accept_visible_slash_completion,
    delete_char_before_cursor as delete_char_before_terminal_cursor, flatten_paste_text,
    insert_char_at_cursor as insert_char_at_terminal_cursor,
    move_cursor_left as move_terminal_cursor_left, move_slash_completion_selection,
    navigate_history as navigate_terminal_history, slash_completion_page_size,
};
use super::codex_composer::{
    clamp_cursor as clamp_terminal_cursor, remember_input as remember_terminal_input,
};
use super::codex_keymap::TerminalKeyAction;
use super::commands::{handle_slash_command_with_runtime, save_handoff_package, SlashAction};
#[cfg(test)]
use super::commands::{slash_argument_candidates, SlashArgCandidate};
use super::persist::{last_tui_session_path, load_last_tui_session, save_tui_session};
use super::run_state::{handle_run_event, RunController};
use super::state::{ChatMsg, InputState, TuiRuntimeConfig};
use crate::config::Config;
use crate::core::session_store::SessionStore;
use crate::pipeline::orchestrator::{Orchestrator, RunProgress};
use anyhow::{anyhow, Context, Result};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
#[cfg(test)]
use ratatui::layout::{Position, Rect, Size};
#[cfg(test)]
use ratatui::style::{Color, Modifier};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::MissedTickBehavior;
use unicode_width::UnicodeWidthStr;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const TIFFANY: &str = "\x1b[38;2;129;216;208m";
const CYAN: &str = TIFFANY;
#[cfg(test)]
const GREEN: &str = TIFFANY;
const YELLOW: &str = "\x1b[33m";
#[cfg(test)]
const RED: &str = "\x1b[31m";

macro_rules! term_line {
    () => {{
        let _ = terminal_write_line("");
    }};
    ($($arg:tt)*) => {{
        let _ = terminal_write_line(&format!($($arg)*));
    }};
}

enum TerminalAction {
    Continue,
    RunPrompt(String),
    RunQueuedNow,
    OpenEditor(String),
    OpenAgent(AgentContinueTarget),
    ResumeLast,
    Exit,
}

#[derive(Default)]
struct TerminalRenderState {
    surface: Option<super::codex_inline_surface::InlineTerminalSurface>,
    input_area_rows: usize,
    spinner_started_at: Option<Instant>,
}

pub async fn run(
    store: Arc<SessionStore>,
    orch: Arc<Orchestrator>,
    config: Arc<Config>,
    config_path: &Path,
) -> Result<()> {
    run_inner(store, orch, config, config_path, None).await
}

pub async fn run_with_mode_notice(
    store: Arc<SessionStore>,
    orch: Arc<Orchestrator>,
    config: Arc<Config>,
    config_path: &Path,
    mode_notice: &'static str,
) -> Result<()> {
    run_inner(store, orch, config, config_path, Some(mode_notice)).await
}

async fn run_inner(
    store: Arc<SessionStore>,
    orch: Arc<Orchestrator>,
    config: Arc<Config>,
    config_path: &Path,
    mode_notice: Option<&'static str>,
) -> Result<()> {
    let _terminal = TerminalInputGuard::enter()?;
    let mut input = InputState {
        trace_live_enabled: false,
        ..InputState::default()
    };
    let runtime = TuiRuntimeConfig {
        config_path: Some(config_path.to_path_buf()),
    };
    let controller = RunController::new(orch.clone(), store.clone());
    let mut active_rx: Option<UnboundedReceiver<RunProgress>> = None;
    let mut active_assistant_idx: Option<usize> = None;
    let (draw_tx, draw_rx) = broadcast::channel(64);
    let frame_requester = super::codex_frame_requester::FrameRequester::new(draw_tx);
    let mut event_stream = super::codex_event_stream::TuiEventStream::new(draw_rx);
    let mut progress_tick = tokio::time::interval(Duration::from_millis(30));
    progress_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    print_intro(mode_notice);
    let mut render_state = TerminalRenderState::with_codex_surface()?;
    render_input_area(&store, &config, &input, false, &mut render_state)?;
    loop {
        tokio::select! {
            _ = progress_tick.tick() => {
                if drain_terminal_progress(
                    &controller,
                    &mut input,
                    &mut active_rx,
                    &mut active_assistant_idx,
                    &mut render_state,
                ) {
                    autosave_terminal_session(&store, &input, &mut render_state);
                    frame_requester.schedule_frame();
                }
            }
            event = event_stream.next() => {
                match event? {
                    super::codex_event_stream::TuiEvent::Draw => {
                        render_input_area(
                            &store,
                            &config,
                            &input,
                            active_rx.is_some(),
                            &mut render_state,
                        )?;
                    }
                    super::codex_event_stream::TuiEvent::Key(key) => {
                        let pause_events = terminal_key_may_release_stdin(&key);
                        if pause_events {
                            event_stream.pause_events();
                        }
                        let handle_result = handle_terminal_key(
                            key,
                            &store,
                            &controller,
                            &config,
                            &runtime,
                            &mut input,
                            &mut active_rx,
                            &mut active_assistant_idx,
                            &mut render_state,
                        );
                        if pause_events {
                            event_stream.resume_events();
                        }
                        let should_exit = handle_result?;
                        autosave_terminal_session(&store, &input, &mut render_state);
                        if should_exit {
                            clear_input_area(&mut render_state)?;
                            write_history_line(&mut render_state, String::new());
                            return Ok(());
                        }
                        render_input_area(
                            &store,
                            &config,
                            &input,
                            active_rx.is_some(),
                            &mut render_state,
                        )?;
                    }
                    super::codex_event_stream::TuiEvent::Paste(text) => {
                        super::codex_keymap::handle_paste_text(&mut input, &text);
                        render_input_area(
                            &store,
                            &config,
                            &input,
                            active_rx.is_some(),
                            &mut render_state,
                        )?;
                    }
                    super::codex_event_stream::TuiEvent::Resize => {
                        clear_input_area(&mut render_state)?;
                        render_input_area(
                            &store,
                            &config,
                            &input,
                            active_rx.is_some(),
                            &mut render_state,
                        )?;
                    }
                    super::codex_event_stream::TuiEvent::Closed => {
                        clear_input_area(&mut render_state)?;
                        write_history_line(&mut render_state, String::new());
                        return Ok(());
                    }
                }
            }
        };
    }
}

fn terminal_key_may_release_stdin(key: &KeyEvent) -> bool {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return false;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Enter) || matches!(key.code, KeyCode::Char('c' | 'C') if ctrl)
}

impl TerminalRenderState {
    fn with_codex_surface() -> Result<Self> {
        Ok(Self {
            surface: Some(super::codex_inline_surface::InlineTerminalSurface::new()?),
            ..Self::default()
        })
    }

    fn write_history_lines(&mut self, lines: Vec<String>) -> io::Result<()> {
        if let Some(surface) = self.surface.as_mut() {
            surface.write_history_lines(lines)
        } else {
            for line in lines {
                terminal_write_line(&line)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
fn viewport_clear_position(previous_area: Rect, next_area: Rect, screen_size: Size) -> Position {
    super::codex_inline_surface::viewport_clear_position(previous_area, next_area, screen_size)
}

struct TerminalInputGuard;

impl TerminalInputGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("enabling terminal raw mode")?;
        execute!(io::stdout(), EnableBracketedPaste).context("enabling bracketed paste")?;
        Ok(Self)
    }
}

impl Drop for TerminalInputGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = disable_raw_mode();
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_terminal_key(
    key: KeyEvent,
    store: &Arc<SessionStore>,
    controller: &RunController,
    config: &Arc<Config>,
    runtime: &TuiRuntimeConfig,
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
    render_state: &mut TerminalRenderState,
) -> Result<bool> {
    match super::codex_keymap::handle_composer_key(key, store, config, input) {
        TerminalKeyAction::None => {}
        TerminalKeyAction::CancelOrExit => {
            if active_rx.is_some() || input.run_rx.is_some() || input.run_handle.is_some() {
                clear_input_area(render_state)?;
                let before = input.transcript.len();
                let action =
                    handle_slash_command_with_runtime("/cancel", store, config, runtime, input);
                write_messages_from(render_state, input, before);
                return Ok(matches!(action, SlashAction::Exit));
            }
            return Ok(true);
        }
        TerminalKeyAction::ExitRequested => return Ok(true),
        TerminalKeyAction::Submit(line) => {
            clear_input_area(render_state)?;
            let active = terminal_run_is_active(input, active_rx);
            if should_echo_terminal_submission(&line, active) {
                write_submitted_line(render_state, active, &line);
            }
            let should_exit = handle_terminal_line(
                line,
                store,
                controller,
                config,
                runtime,
                input,
                active_rx,
                active_assistant_idx,
                render_state,
            )?;
            if should_exit {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn drain_terminal_progress(
    controller: &RunController,
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
    render_state: &mut TerminalRenderState,
) -> bool {
    let mut events = Vec::new();
    let mut disconnected = false;
    if let Some(rx) = active_rx.as_mut() {
        loop {
            match rx.try_recv() {
                Ok(progress) => events.push(progress),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
    }

    if events.is_empty() && !disconnected {
        return false;
    }

    let _ = clear_input_area(render_state);
    let mut terminal_event_seen = false;
    for progress in events {
        if handle_terminal_progress(
            progress,
            controller,
            input,
            active_rx,
            active_assistant_idx,
            render_state,
        ) {
            terminal_event_seen = true;
        }
    }
    if should_synthesize_disconnected_failure(
        disconnected,
        active_rx.is_some(),
        terminal_event_seen,
    ) {
        handle_terminal_progress(
            RunProgress::Failed("run ended without a final event".into()),
            controller,
            input,
            active_rx,
            active_assistant_idx,
            render_state,
        );
        terminal_event_seen = true;
    }
    terminal_event_seen || active_rx.is_some()
}

fn should_synthesize_disconnected_failure(
    disconnected: bool,
    active: bool,
    terminal_event_seen: bool,
) -> bool {
    disconnected && active && !terminal_event_seen
}

fn render_input_area(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &InputState,
    active: bool,
    state: &mut TerminalRenderState,
) -> io::Result<()> {
    let width = terminal_width();
    let completion_lines =
        super::codex_composer::terminal_slash_completion_lines(store, config, input, width);

    update_spinner_state(active, state);
    let status_lines = terminal_status_lines(input, active, state, width);

    let hint_lines = terminal_hint_lines(input, active, width);

    let queue_lines = terminal_queue_lines(input, width);

    let prompt = terminal_prompt(input, active);
    let prompt_width = terminal_prompt_display_width(active);
    let available = width.saturating_sub(prompt_width).max(1);
    let cursor = clamp_terminal_cursor(&input.buffer, input.cursor);
    let rendered_buffer = terminal_buffer_view(&input.buffer, cursor, available);
    let rendered_cursor = cursor.min(rendered_buffer.len());
    let rendered_cursor = clamp_terminal_cursor(&rendered_buffer, rendered_cursor);
    let after_width = UnicodeWidthStr::width(&rendered_buffer[rendered_cursor..]);
    let prompt_line = format!("{prompt}{rendered_buffer}");
    let cursor_x = UnicodeWidthStr::width(prompt_line.as_str()).saturating_sub(after_width);
    let mut lines = Vec::new();
    lines.extend(completion_lines.iter().cloned());
    lines.extend(status_lines.iter().cloned());
    lines.extend(hint_lines.iter().cloned());
    lines.extend(queue_lines.iter().cloned());
    lines.push(prompt_line.clone());

    if let Some(surface) = state.surface.as_mut() {
        surface.render_input_area(lines, cursor_x)?;
        state.input_area_rows =
            completion_lines.len() + status_lines.len() + hint_lines.len() + queue_lines.len() + 1;
        return Ok(());
    }

    clear_input_area(state)?;
    for line in &completion_lines {
        terminal_write_line(line)?;
    }
    for line in &status_lines {
        terminal_write_line(line)?;
    }
    for line in &hint_lines {
        terminal_write_line(line)?;
    }
    for line in &queue_lines {
        terminal_write_line(line)?;
    }

    print!("\r\x1b[2K{prompt_line}");
    if after_width > 0 {
        print!("\x1b[{after_width}D");
    }
    io::stdout().flush()?;
    state.input_area_rows =
        completion_lines.len() + status_lines.len() + hint_lines.len() + queue_lines.len() + 1;
    Ok(())
}

fn clear_input_area(state: &mut TerminalRenderState) -> io::Result<()> {
    if let Some(surface) = state.surface.as_mut() {
        surface.clear_input_area()?;
        state.input_area_rows = 0;
        return Ok(());
    }

    let rows = state.input_area_rows;
    if rows == 0 {
        print!("\r\x1b[2K");
        io::stdout().flush()?;
        return Ok(());
    }

    for row in 0..rows {
        print!("\r\x1b[2K");
        if row + 1 < rows {
            print!("\x1b[1A");
        }
    }
    state.input_area_rows = 0;
    io::stdout().flush()
}

fn terminal_line_text(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 2);
    out.push_str(line);
    out.push_str("\r\n");
    out
}

fn terminal_write_line(line: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(terminal_line_text(line).as_bytes())?;
    stdout.flush()
}

#[cfg(test)]
fn ansi_to_ratatui_line(input: &str) -> ratatui::text::Line<'static> {
    super::codex_inline_surface::ansi_to_ratatui_line(input)
}

fn write_submitted_line(state: &mut TerminalRenderState, active: bool, line: &str) {
    write_history_line(state, format!("{}{}", terminal_prompt_plain(active), line));
}

fn terminal_run_is_active(
    input: &InputState,
    active_rx: &Option<UnboundedReceiver<RunProgress>>,
) -> bool {
    active_rx.is_some() || input.run_rx.is_some() || input.run_handle.is_some()
}

fn should_echo_terminal_submission(line: &str, active: bool) -> bool {
    !active || line.trim_start().starts_with('/')
}

fn terminal_prompt(_input: &InputState, active: bool) -> String {
    terminal_prompt_plain(active).to_string()
}

fn terminal_prompt_display_width(active: bool) -> usize {
    if active {
        "queued> ".len()
    } else {
        "> ".len()
    }
}

fn terminal_buffer_view(buffer: &str, cursor: usize, max_width: usize) -> String {
    super::codex_bottom_pane::buffer_view(buffer, cursor, max_width)
}

fn terminal_width() -> usize {
    terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(100)
        .max(24)
}

fn update_spinner_state(active: bool, state: &mut TerminalRenderState) {
    if active {
        if state.spinner_started_at.is_none() {
            state.spinner_started_at = Some(Instant::now());
        }
    } else {
        state.spinner_started_at = None;
    }
}

fn terminal_status_lines(
    input: &InputState,
    active: bool,
    state: &TerminalRenderState,
    terminal_width: usize,
) -> Vec<String> {
    super::codex_bottom_pane::status_lines(input, active, state.spinner_started_at, terminal_width)
}

fn terminal_hint_lines(input: &InputState, active: bool, terminal_width: usize) -> Vec<String> {
    super::codex_bottom_pane::hint_lines(input, active, terminal_width)
}

fn terminal_queue_lines(input: &InputState, terminal_width: usize) -> Vec<String> {
    super::codex_bottom_pane::queue_lines(input, terminal_width)
}

fn terminal_prompt_plain(active: bool) -> &'static str {
    super::codex_bottom_pane::prompt_plain(active)
}

#[cfg(test)]
fn slash_completion_lines(candidates: &[SlashArgCandidate]) -> Vec<String> {
    super::codex_composer::slash_completion_lines(candidates, terminal_width())
}

#[cfg(test)]
fn slash_completion_lines_with_selection(
    candidates: &[SlashArgCandidate],
    selected: Option<usize>,
) -> Vec<String> {
    super::codex_composer::slash_completion_lines_with_selection(
        candidates,
        selected,
        terminal_width(),
    )
}

#[cfg(test)]
fn slash_completion_window_start(total: usize, selected: usize, limit: usize) -> usize {
    super::codex_composer::slash_completion_window_start(total, selected, limit)
}

#[cfg(test)]
fn terminal_slash_completion_lines(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &InputState,
) -> Vec<String> {
    super::codex_composer::terminal_slash_completion_lines(store, config, input, terminal_width())
}

#[cfg(test)]
fn set_terminal_buffer(input: &mut InputState, text: String) {
    super::codex_composer::set_buffer(input, text);
}

#[cfg(test)]
fn complete_terminal_slash_argument(
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    input: &mut InputState,
    _render_state: &mut TerminalRenderState,
) -> Result<bool> {
    Ok(super::codex_composer::complete_selected_slash_completion(
        store, config, input,
    ))
}

fn print_intro(mode_notice: Option<&str>) {
    term_line!("{CYAN}{BOLD}tiffany-loop{RESET} {DIM}upstream-derived terminal UI{RESET}");
    if let Some(mode_notice) = mode_notice {
        term_line!("{YELLOW}⚠{RESET} {DIM}{mode_notice}{RESET}");
    }
    term_line!(
        "{DIM}tiffany-loop-style scrollback renderer: native selection/copy/paste, IME-friendly input. /help for commands.{RESET}"
    );
    term_line!(
        "{DIM}Process detail: /o toggles folded/expanded gray agent output. Results: /copy result or /result.{RESET}"
    );
    term_line!();
}

fn handle_terminal_command(
    line: &str,
    store: &Arc<SessionStore>,
    config: &Arc<Config>,
    runtime: &TuiRuntimeConfig,
    input: &mut InputState,
    render_state: &mut TerminalRenderState,
) -> TerminalAction {
    let command = line
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("");

    match command {
        "" | "help" | "h" | "?" | "commands" => {
            write_system(render_state, terminal_help());
            return TerminalAction::Continue;
        }
        "clear" | "c" | "new" => {
            input.transcript.clear();
            input.trace_message_index = None;
            input.context_cutoff = 0;
            input.context_summary_text.clear();
            input.context_summary_upto = 0;
            input.last_context_messages = 0;
            input.last_context_chars = 0;
            write_system(
                render_state,
                "Transcript memory cleared. Terminal scrollback is controlled by your terminal."
                    .into(),
            );
            return TerminalAction::Continue;
        }
        "mouse" => {
            write_system(
                render_state,
                "Mouse capture has been removed from terminal chat mode. Selection, copy, paste, and scrolling are handled by your terminal/OS."
                    .into(),
            );
            return TerminalAction::Continue;
        }
        "resume" => {
            let arg = line
                .trim_start_matches('/')
                .split_whitespace()
                .nth(1)
                .unwrap_or("last");
            if arg == "last" {
                return TerminalAction::ResumeLast;
            }
            write_system(render_state, "Usage: /resume last".into());
            return TerminalAction::Continue;
        }
        "editor" | "edit" | "e" => {
            let initial = line
                .trim_start_matches('/')
                .split_once(char::is_whitespace)
                .map(|(_, rest)| rest.trim().to_string())
                .unwrap_or_default();
            return TerminalAction::OpenEditor(initial);
        }
        "continue" => {
            let args = line
                .trim_start_matches('/')
                .split_whitespace()
                .skip(1)
                .collect::<Vec<_>>();
            if let Some(target) = parse_agent_continue_target(&args) {
                return TerminalAction::OpenAgent(target);
            }
        }
        "handoff" => {
            let args = line
                .trim_start_matches('/')
                .split_whitespace()
                .skip(1)
                .collect::<Vec<_>>();
            if matches!(args.first().copied(), Some("open")) {
                if let Some(target) = parse_agent_continue_target(&args) {
                    return TerminalAction::OpenAgent(target);
                }
            }
        }
        _ => {}
    }

    let before = input.transcript.len();
    let action = handle_slash_command_with_runtime(line, store, config, runtime, input);
    write_messages_from(render_state, input, before);

    match action {
        SlashAction::None => TerminalAction::Continue,
        SlashAction::RunPrompt(prompt) => TerminalAction::RunPrompt(prompt),
        SlashAction::RunQueuedNow => TerminalAction::RunQueuedNow,
        SlashAction::Exit => TerminalAction::Exit,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_terminal_line(
    line: String,
    store: &Arc<SessionStore>,
    controller: &RunController,
    config: &Arc<Config>,
    runtime: &TuiRuntimeConfig,
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
    render_state: &mut TerminalRenderState,
) -> Result<bool> {
    let prompt = line.trim();
    if prompt.is_empty() {
        return Ok(false);
    }

    let was_active = terminal_run_is_active(input, active_rx);

    if prompt.starts_with('/') {
        remember_terminal_input(input, prompt);
        match handle_terminal_command(prompt, store, config, runtime, input, render_state) {
            TerminalAction::Continue => {
                if was_active && input.run_rx.is_none() && input.run_handle.is_none() {
                    finish_terminal_run(
                        input,
                        active_rx,
                        active_assistant_idx,
                        controller,
                        render_state,
                    );
                }
                return Ok(false);
            }
            TerminalAction::RunPrompt(prompt) => {
                start_or_queue_terminal_prompt(
                    prompt,
                    controller,
                    input,
                    active_rx,
                    active_assistant_idx,
                    render_state,
                );
                return Ok(false);
            }
            TerminalAction::RunQueuedNow => {
                start_terminal_queued_batch(
                    controller,
                    input,
                    active_rx,
                    active_assistant_idx,
                    render_state,
                );
                return Ok(false);
            }
            TerminalAction::OpenEditor(initial) => {
                match open_prompt_editor(&initial, render_state) {
                    Ok(Some(prompt)) => {
                        start_or_queue_terminal_prompt(
                            prompt,
                            controller,
                            input,
                            active_rx,
                            active_assistant_idx,
                            render_state,
                        );
                    }
                    Ok(None) => {
                        write_system(render_state, "Editor closed with no prompt to send.".into());
                    }
                    Err(e) => {
                        write_system(render_state, format!("Editor failed: {:#}", e));
                    }
                }
                return Ok(false);
            }
            TerminalAction::OpenAgent(target) => {
                if was_active {
                    write_system(
                        render_state,
                        "Cannot open another CLI while a task is running; use /cancel first."
                            .into(),
                    );
                    return Ok(false);
                }
                match open_agent_continue(target, store, config, input, render_state) {
                    Ok(message) => write_system(render_state, message),
                    Err(e) => write_system(
                        render_state,
                        format!("Could not open {}: {:#}", target.as_str(), e),
                    ),
                }
                return Ok(false);
            }
            TerminalAction::ResumeLast => {
                if was_active {
                    write_system(
                        render_state,
                        "Cannot resume while a task is running; use /cancel first.".into(),
                    );
                    return Ok(false);
                }
                resume_last_terminal_session(store, input, render_state);
                return Ok(false);
            }
            TerminalAction::Exit => return Ok(true),
        }
    }

    remember_terminal_input(input, prompt);
    start_or_queue_terminal_prompt(
        prompt.to_string(),
        controller,
        input,
        active_rx,
        active_assistant_idx,
        render_state,
    );
    Ok(false)
}

fn open_prompt_editor(
    initial: &str,
    render_state: &mut TerminalRenderState,
) -> Result<Option<String>> {
    let editor = editor_command()
        .ok_or_else(|| anyhow!("set $VISUAL or $EDITOR to use /editor, for example EDITOR=vim"))?;
    let mut file = tempfile::Builder::new()
        .prefix("orchestrator-prompt-")
        .suffix(".md")
        .tempfile()
        .context("creating temporary prompt file")?;
    let template = editor_template(initial);
    file.write_all(template.as_bytes())
        .context("writing prompt template")?;
    file.flush().context("flushing prompt template")?;
    let path = file.path().to_path_buf();

    write_history_line(
        render_state,
        format!(
            "{DIM}opening editor: {} {}{RESET}",
            editor.display(),
            path.display()
        ),
    );
    let status = {
        let _terminal = TerminalModeSuspension::suspend()?;
        Command::new(&editor.program)
            .args(&editor.args)
            .arg(&path)
            .status()
            .with_context(|| format!("running editor `{}`", editor.display()))?
    };
    if !status.success() {
        return Err(anyhow!("editor exited with status {status}"));
    }

    let edited = std::fs::read_to_string(&path)
        .with_context(|| format!("reading edited prompt {}", path.display()))?;
    Ok(clean_editor_prompt(&edited))
}

struct TerminalModeSuspension;

impl TerminalModeSuspension {
    fn suspend() -> Result<Self> {
        execute!(io::stdout(), DisableBracketedPaste).context("disabling bracketed paste")?;
        disable_raw_mode().context("disabling terminal raw mode")?;
        Ok(Self)
    }
}

impl Drop for TerminalModeSuspension {
    fn drop(&mut self) {
        let _ = enable_raw_mode();
        let _ = execute!(io::stdout(), EnableBracketedPaste);
    }
}

struct EditorCommand {
    program: String,
    args: Vec<String>,
}

impl EditorCommand {
    fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

fn editor_command() -> Option<EditorCommand> {
    std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .and_then(|value| parse_editor_command(&value))
}

fn parse_editor_command(raw: &str) -> Option<EditorCommand> {
    let mut parts = raw.split_whitespace();
    let program = parts.next()?.to_string();
    if program.is_empty() {
        return None;
    }
    Some(EditorCommand {
        program,
        args: parts.map(str::to_string).collect(),
    })
}

fn editor_template(initial: &str) -> String {
    let mut out = String::new();
    if !initial.trim().is_empty() {
        out.push_str(initial.trim());
        out.push_str("\n\n");
    }
    out.push_str("# Write your prompt above. Lines starting with # are ignored.\n");
    out.push_str("# Save and close the editor to send; leave blank to cancel.\n");
    out
}

fn clean_editor_prompt(edited: &str) -> Option<String> {
    let cleaned = edited
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn open_agent_continue(
    target: AgentContinueTarget,
    store: &SessionStore,
    config: &Config,
    input: &mut InputState,
    render_state: &mut TerminalRenderState,
) -> Result<String> {
    let saved = save_handoff_package(store, config, input, target.as_str());
    let Some(path) = input.last_handoff_path.clone() else {
        return Err(anyhow!("handoff package was not created: {}", saved));
    };
    let command = agent_continue_command(target, config, &path);

    clear_for_external_cli(target, &command, &path, render_state)?;
    let status = {
        let _terminal = TerminalModeSuspension::suspend()?;
        Command::new(&command.program)
            .args(&command.args)
            .status()
            .with_context(|| format!("running `{}`", command.display()))?
    };
    write_history_line(render_state, String::new());

    if status.success() {
        Ok(format!(
            "Returned from {}.\n  handoff: {}\n  command: {}",
            target.as_str(),
            path.display(),
            command.display()
        ))
    } else {
        Ok(format!(
            "{} exited with status {}.\n  handoff: {}\n  command: {}",
            target.as_str(),
            status,
            path.display(),
            command.display()
        ))
    }
}

fn clear_for_external_cli(
    target: AgentContinueTarget,
    command: &ExternalAgentCommand,
    path: &Path,
    render_state: &mut TerminalRenderState,
) -> Result<()> {
    write_history_lines(
        render_state,
        vec![
            format!(
                "{DIM}opening {} with handoff: {}{RESET}",
                target.as_str(),
                path.display()
            ),
            format!("{DIM}command: {}{RESET}", command.display()),
            String::new(),
        ],
    );
    io::stdout().flush().context("flushing terminal output")
}

fn start_or_queue_terminal_prompt(
    prompt: String,
    controller: &RunController,
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
    render_state: &mut TerminalRenderState,
) {
    let was_active = terminal_run_is_active(input, active_rx);
    let user_idx = input.transcript.len();
    controller.start_user_prompt(prompt, input);

    if was_active {
        write_queue_receipt(render_state, input);
        return;
    }

    write_started_run_messages(render_state, input, user_idx);
    activate_terminal_run(input, active_rx, active_assistant_idx);
}

fn start_terminal_queued_batch(
    controller: &RunController,
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
    render_state: &mut TerminalRenderState,
) {
    if terminal_run_is_active(input, active_rx) || input.queued_prompts.is_empty() {
        return;
    }

    let queued_count = input.queued_prompts.len();
    let before = input.transcript.len();
    controller.start_next_queued(input);
    if input.run_rx.is_none() && input.run_handle.is_none() {
        return;
    }
    activate_terminal_run(input, active_rx, active_assistant_idx);
    write_queue_batch_start(render_state, queued_count);
    write_started_run_messages(render_state, input, before);
}

fn activate_terminal_run(
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
) {
    if active_rx.is_none() {
        *active_rx = input.run_rx.take();
    }
    if active_rx.is_some() {
        *active_assistant_idx = input
            .transcript
            .iter()
            .rposition(|msg| msg.role == "assistant" && msg.status == "thinking")
            .or_else(|| {
                input
                    .transcript
                    .iter()
                    .rposition(|msg| msg.role == "assistant")
            });
    }
}

fn handle_terminal_progress(
    event: RunProgress,
    controller: &RunController,
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
    render_state: &mut TerminalRenderState,
) -> bool {
    write_progress(render_state, &event, input);
    let terminal_event = handle_run_event(event, input);
    if terminal_event {
        finish_terminal_run(
            input,
            active_rx,
            active_assistant_idx,
            controller,
            render_state,
        );
    }
    terminal_event
}

fn finish_terminal_run(
    input: &mut InputState,
    active_rx: &mut Option<UnboundedReceiver<RunProgress>>,
    active_assistant_idx: &mut Option<usize>,
    controller: &RunController,
    render_state: &mut TerminalRenderState,
) {
    *active_rx = None;
    let _ = active_assistant_idx.take();
    if let Some(msg) = input
        .transcript
        .iter()
        .rev()
        .find(|msg| msg.role == "assistant")
    {
        write_message(render_state, msg);
    }
    if input.queued_prompts.is_empty() || input.queue_paused {
        write_history_line(render_state, String::new());
        return;
    }

    let queued_count = input.queued_prompts.len();
    let before = input.transcript.len();
    controller.start_next_queued(input);
    if input.run_rx.is_none() && input.run_handle.is_none() {
        write_history_line(render_state, String::new());
        return;
    }
    activate_terminal_run(input, active_rx, active_assistant_idx);
    write_queue_batch_start(render_state, queued_count);
    write_started_run_messages(render_state, input, before);
}

fn resume_last_terminal_session(
    store: &SessionStore,
    input: &mut InputState,
    render_state: &mut TerminalRenderState,
) {
    match load_last_tui_session(store) {
        Ok(restored) => {
            *input = restored;
            write_system(
                render_state,
                format!(
                    "Resumed last terminal chat session from {}.",
                    last_tui_session_path(store)
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|_| "last session file".into())
                ),
            );
            write_messages_from(render_state, input, 0);
        }
        Err(e) => {
            write_system(
                render_state,
                format!("Could not resume last terminal chat session: {}", e),
            );
        }
    }
}

fn autosave_terminal_session(
    store: &SessionStore,
    input: &InputState,
    render_state: &mut TerminalRenderState,
) {
    if let Err(e) = save_tui_session(store, input) {
        write_history_line(render_state, format!("{DIM}autosave failed: {e}{RESET}"));
    }
}

fn write_progress(state: &mut TerminalRenderState, event: &RunProgress, input: &mut InputState) {
    let lines = super::codex_process_cell::progress_history_lines(event, input);
    if !lines.is_empty() {
        write_history_lines(state, lines);
    }
}

#[cfg(test)]
fn progress_line(
    event: &RunProgress,
    review_issue_count: usize,
    input: &InputState,
) -> Option<(&'static str, &'static str, String)> {
    super::codex_process_cell::progress_line(event, review_issue_count, input)
}

#[cfg(test)]
fn remember_visible_progress(input: &mut InputState, event: &RunProgress, line: &str) {
    super::codex_process_cell::remember_visible_progress(input, event, line);
}

fn write_messages_from(state: &mut TerminalRenderState, input: &InputState, start: usize) {
    if start >= input.transcript.len() {
        return;
    }
    for msg in &input.transcript[start..] {
        write_message(state, msg);
    }
}

fn write_started_run_messages(state: &mut TerminalRenderState, input: &InputState, start: usize) {
    if start >= input.transcript.len() {
        return;
    }
    for msg in &input.transcript[start..] {
        if msg.role == "assistant" && msg.status == "thinking" {
            continue;
        }
        write_message(state, msg);
    }
}

fn write_queue_receipt(state: &mut TerminalRenderState, input: &InputState) {
    let Some(last) = input.queued_prompts.last() else {
        return;
    };
    write_history_lines(
        state,
        super::codex_history_cell::queue_receipt_lines(
            input.queued_prompts.len(),
            input.queue_paused,
            last,
        ),
    );
}

fn write_queue_batch_start(state: &mut TerminalRenderState, count: usize) {
    write_history_lines(
        state,
        super::codex_history_cell::queue_batch_start_lines(count),
    );
}

fn write_message(state: &mut TerminalRenderState, msg: &ChatMsg) {
    write_history_lines(state, super::codex_history_cell::message_lines(msg));
}

fn write_system(state: &mut TerminalRenderState, content: String) {
    let mut lines = super::codex_history_cell::system_lines(&content);
    lines.push(String::new());
    write_history_lines(state, lines);
}

#[cfg(test)]
fn assistant_content_history_lines(content: &str) -> Vec<String> {
    super::codex_history_cell::assistant_content_lines(content)
}

fn write_history_line(state: &mut TerminalRenderState, line: String) {
    write_history_lines(state, vec![line]);
}

fn write_history_lines(state: &mut TerminalRenderState, lines: Vec<String>) {
    let _ = state.write_history_lines(lines);
}

#[cfg(test)]
fn status_icon(status: &str) -> (&'static str, &'static str) {
    super::codex_history_cell::status_icon(status)
}

fn terminal_help() -> String {
    "Terminal chat commands:\n\
     /help                         Show this help\n\
     /status                       Show current run/config status\n\
     /doctor                       Diagnose config, runtimes, API keys, and tools\n\
     /roles [show|route|use|snippet|save] Show/select/save role routing\n\
     /workflow                     Show selected flow and role wiring\n\
     /agent [role|clear]           Route future worker tasks\n\
     /model [role]                 Show model assignments\n\
     /usage [today|week|month|all] Show token/cost usage\n\
     /sessions [n]                 List recent sessions with tree/log shortcuts\n\
     /import-cc [project]          Import Claude Code sessions into chat history\n\
     /resume last                  Restore the last terminal chat conversation\n\
     /session [id|prefix|last]     Show one session summary\n\
     /session tree [id|prefix|last] Show parent/child session tree\n\
     /session flow [id|prefix|last] [n] Show orchestration event waterfall\n\
     /tree [id|prefix|last]        Show parent/child session tree\n\
     /run-flow [id|prefix|last] [n] Show orchestration event waterfall\n\
     /log [id|prefix|last] [n]     Show session log tail\n\
     /trace [on|off|full|compact] Configure captured trace verbosity\n\
     /context [compact|full|off|clear] Control remembered conversation context\n\
     /process [summary|full|n]     Show captured run process\n\
     /diff [summary|stat|full]     Show current git changes\n\
     /tests [suggest|quick|run|status] Show or run test helpers\n\
     /handoff claude|codex         Save context package for another CLI\n\
     /handoff open claude|codex    Save handoff and open that CLI inline\n\
     /continue claude|codex        Save handoff and open that CLI inline\n\
     /graph [compact|full|mermaid|save] Compress conversation into a flow graph\n\
     /acp [status|claude|codex]    Show ACP server/client setup hints\n\
     /checkpoint [save|status]     Save current tracked git diff\n\
     /rollback [last|check]        Reverse-apply last checkpoint patch\n\
     /editor [draft]               Compose a longer prompt in $VISUAL or $EDITOR\n\
     /result [text]                Show last final result\n\
     /copy [file|clipboard]        Export transcript\n\
     /copy result                  Copy last final result\n\
     /save                         Save transcript to a file\n\
     /queue [clear|remove n|promote n|edit n text|pause|resume|run] Manage queued messages\n\
     /retry                        Re-run the last task prompt\n\
     /clear                        Clear transcript memory\n\
     /quit                         Exit\n\
     \n\
     Multi-turn: while a run is active, type a normal message to queue it. Queued messages merge into the next batch and run together.\n\
     History: Up/Down recalls previous prompts; edit before pressing Enter.\n\
     Completion: type / to open the command menu; Up/Down selects, Enter confirms.\n\
     Process: gray agent output scrolls with the chat; /o folds/expands future details.\n\
     Full logs: use /process 200 or /trace full for captured worker output.\n\
     Copy/paste/scrolling: handled by your terminal/OS."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_help_points_to_system_scrollback() {
        let help = terminal_help();

        assert!(help.contains("Copy/paste/scrolling"));
        assert!(help.contains("Queued messages merge into the next batch"));
        assert!(help.contains("/o folds/expands future details"));
        assert!(help.contains("/result [text]"));
        assert!(help.contains("/resume last"));
        assert!(help.contains("/editor"));
        assert!(help.contains("Up/Down recalls previous prompts"));
        assert!(!help.contains("--fullscreen"));
    }

    #[test]
    fn terminal_line_text_uses_raw_mode_safe_newline() {
        assert_eq!(terminal_line_text("ok"), "ok\r\n");
        assert_eq!(terminal_line_text(""), "\r\n");
    }

    #[test]
    fn ansi_to_ratatui_line_strips_escape_codes_and_preserves_style() {
        let line = ansi_to_ratatui_line(&format!("{CYAN}{BOLD}ok{RESET} {DIM}dim{RESET}"));

        assert_eq!(line.spans[0].content, "ok");
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(129, 216, 208)));
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[1].content, " ");
        assert_eq!(line.spans[2].content, "dim");
        assert!(line.spans[2].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn viewport_clear_position_clamps_after_resize_shrink() {
        let previous = Rect::new(0, 40, 120, 4);
        let next = Rect::new(0, 17, 80, 3);
        let screen = Size::new(80, 20);

        assert_eq!(
            viewport_clear_position(previous, next, screen),
            Position::new(0, 17)
        );
    }

    #[test]
    fn viewport_clear_position_handles_empty_previous_area() {
        let previous = Rect::ZERO;
        let next = Rect::new(0, 22, 100, 2);
        let screen = Size::new(80, 20);

        assert_eq!(
            viewport_clear_position(previous, next, screen),
            Position::new(0, 19)
        );
    }

    #[test]
    fn assistant_history_lines_keep_plain_final_result_selectable() {
        let lines = assistant_content_history_lines("✓ done\n\nFinal result:\nhello");

        assert!(lines.iter().any(|line| line.contains("Final result:")));
        assert!(!lines.iter().any(|line| line.contains("╭─ Final result:")));
        assert!(lines.iter().any(|line| line.ends_with("hello")));
    }

    #[test]
    fn terminal_prompt_keeps_queue_summary_at_bottom() {
        let mut input = InputState {
            queued_prompts: vec![
                "first queued message".into(),
                "second queued message".into(),
            ],
            ..InputState::default()
        };

        let prompt = terminal_prompt(&input, true);

        assert_eq!(prompt, terminal_prompt_plain(true));

        input.queued_prompts.clear();
        assert_eq!(terminal_prompt(&input, false), terminal_prompt_plain(false));
    }

    #[test]
    fn terminal_buffer_view_never_exceeds_available_width() {
        let buffer = "abcdefghijklmnopqrstuvwxyz";
        let rendered = terminal_buffer_view(buffer, buffer.len(), 12);

        assert!(UnicodeWidthStr::width(rendered.as_str()) <= 12);
        assert!(rendered.contains('…'));
        assert!(rendered.ends_with('z'));
    }

    #[test]
    fn terminal_buffer_view_preserves_cjk_boundaries() {
        let buffer = "输入一段很长的中文内容用于测试窗口缩放";
        let rendered = terminal_buffer_view(buffer, buffer.len(), 14);

        assert!(UnicodeWidthStr::width(rendered.as_str()) <= 14);
        assert!(rendered.is_char_boundary(rendered.len()));
        assert!(rendered.ends_with('放'));
    }

    #[test]
    fn terminal_status_line_shows_codex_style_run_hud() {
        let input = InputState {
            current_stage: "Worker: claude-code (abc12345)".into(),
            current_stage_detail: "running tests".into(),
            agent_hint: Some("worker-codex".into()),
            run_route: Some("single-worker".into()),
            queued_prompts: vec!["follow up".into(), "then document".into()],
            history_folded: true,
            last_context_messages: 3,
            last_context_chars: 420,
            ..InputState::default()
        };
        let render_state = TerminalRenderState {
            spinner_started_at: Some(Instant::now()),
            input_area_rows: 0,
            ..TerminalRenderState::default()
        };

        let lines = terminal_status_lines(&input, true, &render_state, 220);
        let joined = lines.join("\n");

        assert_eq!(lines.len(), 1);
        assert!(joined.contains("Worker: claude-code"));
        assert!(joined.contains("running tests"));
        assert!(joined.contains("flow single"));
        assert!(joined.contains("worker codex"));
        assert!(joined.contains("ctx compact 3 msg/420 chars"));
        assert!(joined.contains("queue 2 next"));
        assert!(joined.contains("detail folded /o"));
    }

    #[test]
    fn terminal_status_line_shows_ready_context_without_active_run() {
        let input = InputState {
            current_stage: "Done".into(),
            agent_hint: Some("worker-cc".into()),
            run_route: Some("full-pipeline".into()),
            context_mode: super::super::state::ContextMode::Full,
            ..InputState::default()
        };

        let lines = terminal_status_lines(&input, false, &TerminalRenderState::default(), 120);
        let joined = lines.join("\n");

        assert!(joined.contains("ready"));
        assert!(joined.contains("last Done"));
        assert!(joined.contains("last flow full"));
        assert!(joined.contains("worker claude"));
        assert!(joined.contains("ctx full"));
        assert!(joined.contains("/ for commands"));
    }

    #[test]
    fn terminal_status_line_respects_terminal_width() {
        let input = InputState {
            current_stage: "Executing a very long stage name that should not overflow".into(),
            current_stage_detail: "with a very long detail string".into(),
            agent_hint: Some("worker-codex".into()),
            queued_prompts: vec!["follow up".into()],
            ..InputState::default()
        };
        let render_state = TerminalRenderState {
            spinner_started_at: Some(Instant::now()),
            input_area_rows: 0,
            ..TerminalRenderState::default()
        };

        let lines = terminal_status_lines(&input, true, &render_state, 36);

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains('…'));
    }

    #[test]
    fn terminal_hint_line_shows_contextual_actions() {
        let idle = InputState::default();
        let idle_lines = terminal_hint_lines(&idle, false, 120);
        assert_eq!(idle_lines.len(), 1);
        assert!(idle_lines[0].contains("/doctor"));
        assert!(idle_lines[0].contains("history"));

        let active = InputState::default();
        let active_lines = terminal_hint_lines(&active, true, 120);
        assert!(active_lines[0].contains("/process 200"));
        assert!(active_lines[0].contains("/cancel"));

        let queued = InputState {
            queued_prompts: vec!["next".into()],
            ..InputState::default()
        };
        let queued_lines = terminal_hint_lines(&queued, false, 120);
        assert!(queued_lines[0].contains("queue ready"));
        assert!(queued_lines[0].contains("/queue run"));
    }

    #[test]
    fn terminal_hint_line_hides_behind_slash_menu_and_truncates() {
        let mut input = InputState {
            buffer: "/".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();

        assert!(terminal_hint_lines(&input, false, 24).is_empty());

        input.slash_completion_dismissed = true;
        let lines = terminal_hint_lines(&input, false, 24);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains('…'));
    }

    #[test]
    fn terminal_queue_lines_show_pending_batch_preview() {
        let input = InputState {
            queued_prompts: vec![
                "first queued message".into(),
                "second queued message".into(),
                "third queued message".into(),
                "fourth queued message".into(),
            ],
            ..InputState::default()
        };

        let lines = terminal_queue_lines(&input, 80);

        assert!(lines[0].contains("4 item(s)"));
        assert!(lines[0].contains("runs together"));
        assert!(lines
            .iter()
            .any(|line| line.contains("first queued message")));
        assert!(lines
            .iter()
            .any(|line| line.contains("second queued message")));
        assert!(lines
            .iter()
            .any(|line| line.contains("third queued message")));
        assert!(lines
            .iter()
            .any(|line| line.contains("fourth queued message")));
        assert!(!lines.iter().any(|line| line.contains("more")));
    }

    #[test]
    fn terminal_queue_lines_collapse_large_pending_batch() {
        let input = InputState {
            queued_prompts: vec![
                "one".into(),
                "two".into(),
                "three".into(),
                "four".into(),
                "five".into(),
                "six".into(),
            ],
            ..InputState::default()
        };

        let lines = terminal_queue_lines(&input, 80);
        let joined = lines.join("\n");

        assert!(joined.contains("↳ 1."));
        assert!(joined.contains("↳ 4."));
        assert!(!joined.contains("↳ 5."));
        assert!(joined.contains("+ 2 more queued"));
        assert!(joined.contains("/queue show"));
    }

    #[test]
    fn terminal_queue_lines_show_paused_state() {
        let input = InputState {
            queued_prompts: vec!["waiting followup".into()],
            queue_paused: true,
            ..InputState::default()
        };

        let lines = terminal_queue_lines(&input, 80);

        assert!(lines[0].contains("paused"));
        assert!(lines.iter().any(|line| line.contains("waiting followup")));
    }

    #[test]
    fn terminal_queue_lines_wrap_long_and_cjk_messages() {
        let input = InputState {
            queued_prompts: vec!["这是一个很长的排队消息，需要在底部队列区域换行完整展示".into()],
            ..InputState::default()
        };

        let lines = terminal_queue_lines(&input, 32);
        let joined = lines.join("");

        assert!(lines.len() > 2);
        assert!(lines.iter().any(|line| line.contains("↳ 1.")));
        for ch in ['完', '整', '展', '示'] {
            assert!(joined.contains(ch));
        }
    }

    #[test]
    fn status_icon_maps_terminal_states() {
        assert_eq!(status_icon("complete"), ("✓", GREEN));
        assert_eq!(status_icon("error"), ("✗", RED));
        assert_eq!(status_icon("thinking"), ("●", YELLOW));
        assert_eq!(status_icon("warning"), ("⚠", YELLOW));
        assert_eq!(status_icon("queued"), ("↳", CYAN));
        assert_eq!(status_icon("removed"), ("○", DIM));
    }

    #[test]
    fn progress_line_hides_low_value_worker_output() {
        let task_id = uuid::Uuid::new_v4();

        assert!(progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content: "claude system".into(),
            },
            0,
            &InputState::default()
        )
        .is_none());
        assert!(progress_line(
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: "claude assistant: thinking".into(),
            },
            0,
            &InputState::default()
        )
        .is_none());

        assert!(progress_line(
            &RunProgress::Done { task_count: 2 },
            0,
            &InputState::default(),
        )
        .is_none());
    }

    #[test]
    fn progress_line_shows_useful_execution_summaries_once() {
        let task_id = uuid::Uuid::nil();
        let mut input = InputState {
            history_folded: true,
            ..InputState::default()
        };
        let first = RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: "claude assistant: useful summary".into(),
        };

        let line = progress_line(&first, 0, &input).expect("visible worker output");
        assert_eq!(line.0, "↳");
        assert!(line.2.contains("claude-code"));
        assert!(line.2.contains("useful summary"));

        remember_visible_progress(&mut input, &first, &line.2);
        let duplicate = RunProgress::WorkerOutput {
            task_id,
            agent: "claude-code".into(),
            role: "worker-cc".into(),
            content: "claude result: useful summary".into(),
        };
        assert!(progress_line(&duplicate, 0, &input).is_none());

        assert!(progress_line(
            &RunProgress::RoleOutput {
                role: "planner".into(),
                content: r#"{"sub_tasks":[{"prompt":"do one thing"}]}"#.into(),
            },
            0,
            &input,
        )
        .is_none());

        let role_line = progress_line(
            &RunProgress::RoleOutput {
                role: "diagnostic".into(),
                content: r#"{"approved":false,"issues":["missing test"]}"#.into(),
            },
            0,
            &input,
        )
        .expect("visible diagnostic output");
        assert_eq!(role_line.0, "·");
        assert!(role_line.2.contains("role"));
        assert!(role_line.2.contains("needs changes"));
        assert!(!role_line.2.contains('{'));
    }

    #[test]
    fn progress_line_marks_final_worker_output_without_duplication() {
        let task_id = uuid::Uuid::nil();

        let line = progress_line(
            &RunProgress::WorkerOutput {
                task_id,
                agent: "claude-code".into(),
                role: "worker-cc".into(),
                content:
                    r#"claude-code result: {"result":"Implemented it in several paragraphs."}"#
                        .into(),
            },
            0,
            &InputState::default(),
        )
        .expect("final capture marker");
        assert!(line.2.contains("final response captured"));
        assert!(line.2.contains("/result"));
        assert!(!line.2.contains("Implemented it in several paragraphs."));
        assert!(!line.2.contains('{'));
    }

    #[test]
    fn progress_line_marks_done_with_review_issues_as_warning() {
        assert!(progress_line(
            &RunProgress::Done { task_count: 1 },
            4,
            &InputState::default(),
        )
        .is_none());
    }

    #[test]
    fn active_plain_prompt_is_not_echoed_into_scrollback() {
        assert!(!should_echo_terminal_submission("你好", true));
        assert!(should_echo_terminal_submission("你好", false));
        assert!(should_echo_terminal_submission("/queue", true));
    }

    #[test]
    fn disconnected_channel_after_terminal_event_is_not_reported_as_failure() {
        assert!(!should_synthesize_disconnected_failure(true, true, true));
        assert!(should_synthesize_disconnected_failure(true, true, false));
        assert!(!should_synthesize_disconnected_failure(true, false, false));
        assert!(!should_synthesize_disconnected_failure(false, true, false));
    }

    #[test]
    fn editor_template_and_cleanup_ignore_comments() {
        let template = editor_template("draft task");
        assert!(template.starts_with("draft task"));
        assert!(template.contains("Lines starting with # are ignored"));

        let cleaned =
            clean_editor_prompt("implement this\n# comment\n\n  # another comment\nwith details\n")
                .expect("clean prompt");

        assert_eq!(cleaned, "implement this\n\nwith details");
        assert!(clean_editor_prompt("# only comments\n\n").is_none());
    }

    #[test]
    fn parses_editor_command_with_args() {
        let cmd = parse_editor_command("code --wait").expect("editor command");
        assert_eq!(cmd.program, "code");
        assert_eq!(cmd.args, vec!["--wait"]);
        assert_eq!(cmd.display(), "code --wait");

        assert!(parse_editor_command("   ").is_none());
    }

    #[test]
    fn terminal_history_navigates_and_restores_draft() {
        let mut input = InputState::default();
        remember_terminal_input(&mut input, "first");
        remember_terminal_input(&mut input, "second");
        set_terminal_buffer(&mut input, "draft".into());

        assert!(navigate_terminal_history(&mut input, -1));
        assert_eq!(input.buffer, "second");
        assert!(navigate_terminal_history(&mut input, -1));
        assert_eq!(input.buffer, "first");
        assert!(navigate_terminal_history(&mut input, 1));
        assert_eq!(input.buffer, "second");
        assert!(navigate_terminal_history(&mut input, 1));
        assert_eq!(input.buffer, "draft");
        assert_eq!(input.history_index, None);
    }

    #[test]
    fn terminal_tab_completes_single_slash_argument() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(Config::default());
        let mut input = InputState {
            buffer: "/usage w".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();
        let mut render_state = TerminalRenderState::default();

        let completed =
            complete_terminal_slash_argument(&store, &cfg, &mut input, &mut render_state)
                .expect("tab completion");

        assert!(completed);
        assert_eq!(input.buffer, "/usage week ");
        assert_eq!(input.cursor, input.buffer.len());
    }

    #[test]
    fn terminal_tab_formats_multiple_slash_candidates_without_raw_json() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap();
        let cfg = Config::default();
        let mut input = InputState {
            buffer: "/process ".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();

        let candidates = slash_argument_candidates(&store, &cfg, &input);
        let lines = slash_completion_lines(&candidates);
        let joined = lines.join("\n");

        assert!(joined.contains("commands"));
        assert!(joined.contains("filter process events"));
        assert!(joined.contains("copy process capture"));
        assert!(!joined.contains('{'));

        input.buffer = "/".into();
        input.cursor = input.buffer.len();
        let command_candidates = slash_argument_candidates(&store, &cfg, &input);
        let command_lines = slash_completion_lines(&command_candidates);
        let command_joined = command_lines.join("\n");

        assert!(command_candidates
            .iter()
            .any(|candidate| candidate.label == "/queue"));
        assert!(command_joined.contains("/roles"));
        assert!(command_joined.contains("/trace"));
        assert!(!command_joined.contains('{'));
    }

    #[test]
    fn slash_menu_opens_automatically_for_command_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(Config::default());
        let mut input = InputState {
            buffer: "/".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();

        let lines = terminal_slash_completion_lines(&store, &cfg, &input);
        let joined = lines.join("\n");

        assert!(joined.contains("commands"));
        assert!(joined.contains("›"));
        assert!(joined.contains("/roles"));
        assert!(joined.contains("/trace"));

        let queue_idx = slash_argument_candidates(&store, &cfg, &input)
            .iter()
            .position(|candidate| candidate.label == "/queue")
            .expect("queue command candidate");
        input.slash_completion_index = queue_idx;
        let queue_lines = terminal_slash_completion_lines(&store, &cfg, &input);
        assert!(queue_lines.join("\n").contains("/queue"));

        input.slash_completion_dismissed = true;
        assert!(terminal_slash_completion_lines(&store, &cfg, &input).is_empty());
    }

    #[test]
    fn slash_menu_selection_moves_and_enter_accepts_partial_command() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(Config::default());
        let mut input = InputState {
            buffer: "/ro".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();

        assert!(accept_visible_slash_completion(
            &store, &cfg, &mut input, false
        ));
        assert_eq!(input.buffer, "/roles ");

        input.buffer = "/".into();
        input.cursor = input.buffer.len();
        assert!(move_slash_completion_selection(&store, &cfg, &mut input, 1));
        assert_eq!(input.slash_completion_index, 1);
        assert!(move_slash_completion_selection(
            &store, &cfg, &mut input, -1
        ));
        assert_eq!(input.slash_completion_index, 0);
    }

    #[test]
    fn slash_menu_scrolls_to_keep_selection_visible() {
        let candidates = (1..=14)
            .map(|idx| SlashArgCandidate {
                value: format!("cmd{idx}"),
                label: format!("/cmd{idx}"),
                description: format!("command {idx}"),
            })
            .collect::<Vec<_>>();

        let lines = slash_completion_lines_with_selection(&candidates, Some(11));
        let joined = lines.join("\n");

        assert!(joined.contains("commands 12/14"));
        assert!(joined.contains("↑ 4 more"));
        assert!(joined.contains("/cmd12"));
        assert!(joined.contains("›"));
        assert!(!joined.contains("/cmd1 "));
    }

    #[test]
    fn slash_menu_page_keys_jump_by_visible_window() {
        let candidates = (1..=14)
            .map(|idx| SlashArgCandidate {
                value: format!("cmd{idx}"),
                label: format!("/cmd{idx}"),
                description: String::new(),
            })
            .collect::<Vec<_>>();

        let page_size = slash_completion_page_size() as usize;

        assert_eq!(
            slash_completion_window_start(candidates.len(), 0, page_size),
            0
        );
        assert_eq!(
            slash_completion_window_start(candidates.len(), 11, page_size),
            4
        );
        assert_eq!(
            slash_completion_window_start(candidates.len(), 13, page_size),
            6
        );
    }

    #[test]
    fn slash_menu_enter_does_not_steal_exact_command() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SessionStore::open(&tmp.path().join("logs"), &tmp.path().join("db.sqlite")).unwrap(),
        );
        let cfg = Arc::new(Config::default());
        let mut input = InputState {
            buffer: "/roles".into(),
            ..InputState::default()
        };
        input.cursor = input.buffer.len();

        assert!(!accept_visible_slash_completion(
            &store, &cfg, &mut input, false
        ));
        assert_eq!(input.buffer, "/roles");
    }

    #[test]
    fn continue_target_parses_open_aliases() {
        assert_eq!(
            parse_agent_continue_target(&["claude"]),
            Some(AgentContinueTarget::Claude)
        );
        assert_eq!(
            parse_agent_continue_target(&["open", "codex"]),
            Some(AgentContinueTarget::Codex)
        );
        assert_eq!(parse_agent_continue_target(&["status"]), None);
    }

    #[test]
    fn agent_continue_command_uses_inline_terminal_modes() {
        let mut cfg = Config::default();
        cfg.runtimes.insert(
            "claude-code".into(),
            crate::config::RuntimeConfig {
                kind: "subprocess".into(),
                binary: Some("claude-custom".into()),
                supports_mcp: true,
                supports_agent_teams: true,
            },
        );
        cfg.runtimes.insert(
            "codex".into(),
            crate::config::RuntimeConfig {
                kind: "subprocess".into(),
                binary: Some("codex-custom".into()),
                supports_mcp: false,
                supports_agent_teams: false,
            },
        );
        let handoff = std::path::Path::new("/tmp/orchestrator-handoff.md");

        let claude = agent_continue_command(AgentContinueTarget::Claude, &cfg, handoff);
        assert_eq!(claude.program, "claude-custom");
        assert!(claude.args.contains(&"--add-dir".into()));
        assert!(claude.display().contains("/tmp/orchestrator-handoff.md"));

        let codex = agent_continue_command(AgentContinueTarget::Codex, &cfg, handoff);
        assert_eq!(codex.program, "codex-custom");
        assert!(codex.args.contains(&"--no-alt-screen".into()));
        assert!(codex.display().contains("/tmp/orchestrator-handoff.md"));
    }

    #[test]
    fn terminal_cursor_editing_preserves_utf8_boundaries() {
        let mut input = InputState::default();
        set_terminal_buffer(&mut input, "a你b".into());

        move_terminal_cursor_left(&mut input);
        delete_char_before_terminal_cursor(&mut input);

        assert_eq!(input.buffer, "ab");
        assert_eq!(input.cursor, 1);

        insert_char_at_terminal_cursor(&mut input, '好');
        assert_eq!(input.buffer, "a好b");
        assert_eq!(input.cursor, "a好".len());
    }

    #[test]
    fn terminal_paste_flattens_multiline_text_for_single_line_editor() {
        assert_eq!(flatten_paste_text("a\r\nb\nc"), "a b c");
    }
}
