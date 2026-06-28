use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::Widget;
use std::cell::RefCell;

use crate::render::renderable::Renderable;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::bottom_pane_view::ViewCompletion;
use super::popup_consts::standard_popup_hint_line;
use super::role_setup_view::RoleSetupChoice;
use super::role_setup_view::choice;
use super::role_setup_view::dropdown_window_start;
use super::textarea::TextArea;
use super::textarea::TextAreaState;

const LABEL_WIDTH: u16 = 14;
const FIELD_START_ROW: u16 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoleProfileSetupDraft {
    pub(crate) name: String,
    pub(crate) planner: String,
    pub(crate) critic: String,
    pub(crate) reviewer: String,
    pub(crate) worker_cc: String,
    pub(crate) worker_codex: String,
    pub(crate) worker_gemini: String,
}

pub(crate) type RoleProfileSetupSubmitted =
    Box<dyn Fn(RoleProfileSetupDraft) -> Result<(), String> + Send + Sync>;

pub(crate) struct RoleProfileSetupView {
    fields: Vec<RoleProfileField>,
    active: usize,
    dropdown: Option<RoleProfileDropdown>,
    on_submit: RoleProfileSetupSubmitted,
    completion: Option<ViewCompletion>,
    error: Option<String>,
}

struct RoleProfileField {
    kind: RoleProfileFieldKind,
    label: &'static str,
    placeholder: &'static str,
    textarea: TextArea,
    state: RefCell<TextAreaState>,
}

struct RoleProfileDropdown {
    selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoleProfileFieldKind {
    Name,
    Planner,
    Critic,
    Reviewer,
    WorkerCc,
    WorkerCodex,
    WorkerGemini,
}

impl RoleProfileSetupView {
    pub(crate) fn new(draft: RoleProfileSetupDraft, on_submit: RoleProfileSetupSubmitted) -> Self {
        Self {
            fields: vec![
                RoleProfileField::new(RoleProfileFieldKind::Name, "Profile", "default", draft.name),
                RoleProfileField::new(
                    RoleProfileFieldKind::Planner,
                    "Planner",
                    "minimax/MiniMax-M3@codex",
                    draft.planner,
                ),
                RoleProfileField::new(
                    RoleProfileFieldKind::Critic,
                    "Critic",
                    "minimax/MiniMax-M3@codex",
                    draft.critic,
                ),
                RoleProfileField::new(
                    RoleProfileFieldKind::Reviewer,
                    "Reviewer",
                    "minimax/MiniMax-M3@codex",
                    draft.reviewer,
                ),
                RoleProfileField::new(
                    RoleProfileFieldKind::WorkerCc,
                    "Worker CC",
                    "anthropic/claude-sonnet-4-6@claude-code",
                    draft.worker_cc,
                ),
                RoleProfileField::new(
                    RoleProfileFieldKind::WorkerCodex,
                    "Worker Codex",
                    "minimax/MiniMax-M3@codex",
                    draft.worker_codex,
                ),
                RoleProfileField::new(
                    RoleProfileFieldKind::WorkerGemini,
                    "Worker Gemini",
                    "google/gemini-1.5-pro@gemini",
                    draft.worker_gemini,
                ),
            ],
            active: 0,
            dropdown: None,
            on_submit,
            completion: None,
            error: None,
        }
    }

    fn active_field_mut(&mut self) -> &mut RoleProfileField {
        &mut self.fields[self.active]
    }

    fn focus_next(&mut self) {
        self.dropdown = None;
        self.active = (self.active + 1) % self.fields.len();
    }

    fn focus_prev(&mut self) {
        self.dropdown = None;
        self.active = if self.active == 0 {
            self.fields.len().saturating_sub(1)
        } else {
            self.active - 1
        };
    }

    fn submit(&mut self) {
        match (self.on_submit)(self.draft()) {
            Ok(()) => {
                self.completion = Some(ViewCompletion::Accepted);
                self.error = None;
            }
            Err(err) => {
                self.error = Some(err);
            }
        }
    }

    fn active_choices(&self) -> Vec<RoleSetupChoice> {
        self.fields
            .get(self.active)
            .map(|field| field.kind.choices())
            .unwrap_or_default()
    }

    fn open_dropdown(&mut self) {
        let choices = self.active_choices();
        if choices.is_empty() {
            return;
        }
        let current = self
            .fields
            .get(self.active)
            .map(|field| field.textarea.text().trim().to_ascii_lowercase())
            .unwrap_or_default();
        let selected = choices
            .iter()
            .position(|choice| choice.value.eq_ignore_ascii_case(&current))
            .unwrap_or(0);
        self.dropdown = Some(RoleProfileDropdown { selected });
    }

    fn move_dropdown_selection(&mut self, delta: isize) {
        let len = self.active_choices().len();
        if len == 0 {
            self.dropdown = None;
            return;
        }
        if let Some(dropdown) = &mut self.dropdown {
            let len = len as isize;
            let selected = dropdown.selected as isize;
            dropdown.selected = (selected + delta).rem_euclid(len) as usize;
        }
    }

    fn apply_dropdown_selection(&mut self) {
        let choices = self.active_choices();
        let Some(dropdown) = self.dropdown.take() else {
            return;
        };
        let Some(choice) = choices.get(dropdown.selected) else {
            return;
        };
        self.active_field_mut().set_value(&choice.value);
    }

    pub(crate) fn draft(&self) -> RoleProfileSetupDraft {
        let mut draft = RoleProfileSetupDraft::default();
        for field in &self.fields {
            let value = field.textarea.text().trim().to_string();
            match field.kind {
                RoleProfileFieldKind::Name => draft.name = value,
                RoleProfileFieldKind::Planner => draft.planner = value,
                RoleProfileFieldKind::Critic => draft.critic = value,
                RoleProfileFieldKind::Reviewer => draft.reviewer = value,
                RoleProfileFieldKind::WorkerCc => draft.worker_cc = value,
                RoleProfileFieldKind::WorkerCodex => draft.worker_codex = value,
                RoleProfileFieldKind::WorkerGemini => draft.worker_gemini = value,
            }
        }
        draft
    }

    fn field_input_rect(area: Rect, row: u16) -> Rect {
        let x = area.x.saturating_add(LABEL_WIDTH);
        Rect {
            x,
            y: area.y.saturating_add(row),
            width: area.width.saturating_sub(LABEL_WIDTH),
            height: 1,
        }
    }

    fn dropdown_height(&self) -> u16 {
        if self.dropdown.is_some() {
            let rows = self.active_choices().len().min(6) as u16;
            1 + rows
        } else {
            0
        }
    }

    fn status_lines(&self) -> [Line<'static>; 3] {
        let draft = self.draft();
        [
            profile_summary_line(&draft),
            profile_workers_line(&draft),
            profile_write_preview_line(&draft),
        ]
    }
}

impl RoleProfileField {
    fn new(
        kind: RoleProfileFieldKind,
        label: &'static str,
        placeholder: &'static str,
        value: String,
    ) -> Self {
        let mut textarea = TextArea::new();
        if !value.is_empty() {
            textarea.set_text_clearing_elements(&value);
            textarea.set_cursor(value.len());
        }
        Self {
            kind,
            label,
            placeholder,
            textarea,
            state: RefCell::new(TextAreaState::default()),
        }
    }

    fn set_value(&mut self, value: &str) {
        self.textarea.set_text_clearing_elements(value);
        self.textarea.set_cursor(value.len());
    }
}

impl BottomPaneView for RoleProfileSetupView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        if self.dropdown.is_some() {
            match key_event {
                KeyEvent {
                    code: KeyCode::Esc, ..
                } => self.dropdown = None,
                KeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.apply_dropdown_selection(),
                KeyEvent {
                    code: KeyCode::Down,
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.move_dropdown_selection(1),
                KeyEvent {
                    code: KeyCode::Up,
                    modifiers: KeyModifiers::NONE,
                    ..
                } => self.move_dropdown_selection(-1),
                KeyEvent {
                    code: KeyCode::Char(ch),
                    modifiers: KeyModifiers::NONE,
                    ..
                } if ch.is_ascii_digit() && ch != '0' => {
                    let idx = ch.to_digit(10).unwrap_or_default() as usize - 1;
                    if idx < self.active_choices().len() {
                        self.dropdown = Some(RoleProfileDropdown { selected: idx });
                        self.apply_dropdown_selection();
                    }
                }
                _ => {}
            }
            return;
        }

        match key_event {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.on_ctrl_c();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.submit(),
            KeyEvent {
                code: KeyCode::F(4),
                ..
            }
            | KeyEvent {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                ..
            } if !self.active_choices().is_empty() => self.open_dropdown(),
            KeyEvent {
                code: KeyCode::Down | KeyCode::Tab,
                modifiers,
                ..
            } if modifiers == KeyModifiers::NONE => self.focus_next(),
            KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::BackTab,
                ..
            } => self.focus_prev(),
            other => {
                self.error = None;
                self.active_field_mut().textarea.input(other);
            }
        }
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.completion = Some(ViewCompletion::Cancelled);
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.completion.is_some()
    }

    fn completion(&self) -> Option<ViewCompletion> {
        self.completion
    }

    fn handle_paste(&mut self, pasted: String) -> bool {
        if pasted.is_empty() {
            return false;
        }
        self.error = None;
        let text = pasted.replace(['\r', '\n'], " ");
        self.active_field_mut().textarea.insert_str(text.trim());
        true
    }
}

impl Renderable for RoleProfileSetupView {
    fn desired_height(&self, _width: u16) -> u16 {
        FIELD_START_ROW
            + self.fields.len() as u16
            + self.dropdown_height()
            + 1
            + u16::from(self.error.is_some())
            + 1
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        Paragraph::new(Line::from(vec![
            gutter(),
            Span::styled(
                "Role profile",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]))
        .render(
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 1,
            },
            buf,
        );

        Paragraph::new(Line::from(vec![
            gutter(),
            "Use ".dim(),
            "Up".cyan(),
            "/".dim(),
            "Down".cyan(),
            " fields, ".dim(),
            "Space".cyan(),
            "/".dim(),
            "F4".cyan(),
            " templates".dim(),
        ]))
        .render(
            Rect {
                x: area.x,
                y: area.y.saturating_add(1),
                width: area.width,
                height: 1,
            },
            buf,
        );

        let status_lines = self.status_lines();
        for (idx, line) in status_lines.into_iter().enumerate() {
            let y = area.y.saturating_add(2 + idx as u16);
            if y >= area.y.saturating_add(area.height) {
                return;
            }
            Paragraph::new(line).render(
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }

        let mut row = FIELD_START_ROW;
        for (idx, field) in self.fields.iter().enumerate() {
            if row >= area.height {
                return;
            }
            let is_active = idx == self.active;
            let label_style = if is_active {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let suffix = if !field.kind.choices().is_empty() {
                " ▾"
            } else {
                ""
            };
            Paragraph::new(Line::from(vec![
                gutter(),
                Span::styled(format!("{:<10}{suffix}", field.label), label_style),
            ]))
            .render(
                Rect {
                    x: area.x,
                    y: area.y.saturating_add(row),
                    width: LABEL_WIDTH.min(area.width),
                    height: 1,
                },
                buf,
            );

            let input_area = Self::field_input_rect(area, row);
            if input_area.width > 0 {
                Clear.render(input_area, buf);
                let mut state = field.state.borrow_mut();
                StatefulWidgetRef::render_ref(&(&field.textarea), input_area, buf, &mut state);
                if field.textarea.text().is_empty() {
                    Paragraph::new(Line::from(field.placeholder.dim())).render(input_area, buf);
                }
            }
            row += 1;
        }

        if let Some(dropdown) = &self.dropdown {
            let choices = self.active_choices();
            let start = dropdown_window_start(dropdown.selected, choices.len(), 6);
            if row < area.height {
                Paragraph::new(Line::from(vec![
                    gutter(),
                    "Templates".cyan(),
                    "  Enter selects, Esc closes".dim(),
                ]))
                .render(
                    Rect {
                        x: area.x,
                        y: area.y.saturating_add(row),
                        width: area.width,
                        height: 1,
                    },
                    buf,
                );
                row += 1;
            }
            for (idx, choice) in choices.iter().enumerate().skip(start).take(6) {
                if row >= area.height {
                    return;
                }
                let active = idx == dropdown.selected;
                let marker = if active { "›" } else { " " };
                let style = if active {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Paragraph::new(Line::from(vec![
                    gutter(),
                    Span::styled(format!("{marker} {} ", idx + 1), style),
                    Span::styled(choice.label.clone(), style),
                    Span::styled(
                        format!("  {}", choice.hint),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
                .render(
                    Rect {
                        x: area.x,
                        y: area.y.saturating_add(row),
                        width: area.width,
                        height: 1,
                    },
                    buf,
                );
                row += 1;
            }
        }

        if let Some(error) = &self.error
            && row < area.height
        {
            Paragraph::new(Line::from(vec![gutter(), error.clone().red()])).render(
                Rect {
                    x: area.x,
                    y: area.y.saturating_add(row),
                    width: area.width,
                    height: 1,
                },
                buf,
            );
            row += 1;
        }

        if row < area.height {
            Clear.render(
                Rect {
                    x: area.x,
                    y: area.y.saturating_add(row),
                    width: area.width,
                    height: 1,
                },
                buf,
            );
            row += 1;
        }
        if row < area.height {
            Paragraph::new(standard_popup_hint_line()).render(
                Rect {
                    x: area.x,
                    y: area.y.saturating_add(row),
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let row = FIELD_START_ROW.saturating_add(self.active as u16);
        let field = self.fields.get(self.active)?;
        let input_area = Self::field_input_rect(area, row);
        let state = *field.state.borrow();
        field.textarea.cursor_pos_with_state(input_area, state)
    }
}

impl RoleProfileFieldKind {
    fn choices(self) -> Vec<RoleSetupChoice> {
        match self {
            RoleProfileFieldKind::Name => Vec::new(),
            RoleProfileFieldKind::Planner
            | RoleProfileFieldKind::Critic
            | RoleProfileFieldKind::Reviewer => vec![
                choice("Skip", "skip", "leave unchanged"),
                choice(
                    "MiniMax Codex",
                    "minimax/MiniMax-M3@codex",
                    "default control role",
                ),
                choice("OpenAI Codex", "openai/gpt-4o@codex", "OpenAI-compatible"),
                choice(
                    "Claude Code",
                    "anthropic/claude-sonnet-4-6@claude-code",
                    "Claude runtime",
                ),
                choice("Ollama", "ollama/llama3@codex", "local"),
            ],
            RoleProfileFieldKind::WorkerCc => vec![
                choice("Skip", "skip", "leave unchanged"),
                choice(
                    "Claude Code",
                    "anthropic/claude-sonnet-4-6@claude-code",
                    "default Claude worker",
                ),
                choice(
                    "MiniMax Claude",
                    "minimax/MiniMax-M3@claude-code",
                    "Claude-compatible",
                ),
            ],
            RoleProfileFieldKind::WorkerCodex => vec![
                choice("Skip", "skip", "leave unchanged"),
                choice(
                    "MiniMax Codex",
                    "minimax/MiniMax-M3@codex",
                    "default Codex worker",
                ),
                choice("OpenAI Codex", "openai/gpt-4o@codex", "OpenAI-compatible"),
                choice("Ollama", "ollama/llama3@codex", "local"),
            ],
            RoleProfileFieldKind::WorkerGemini => vec![
                choice("Skip", "skip", "leave unchanged"),
                choice(
                    "Google Gemini",
                    "google/gemini-1.5-pro@gemini",
                    "default Gemini worker",
                ),
                choice("Gemini Pro", "gemini/gemini-1.5-pro@gemini", "Gemini CLI"),
            ],
        }
    }
}

fn gutter() -> Span<'static> {
    "▌ ".cyan()
}

fn profile_summary_line(draft: &RoleProfileSetupDraft) -> Line<'static> {
    let name = if draft.name.trim().is_empty() {
        "default"
    } else {
        draft.name.trim()
    };
    Line::from(vec![
        gutter(),
        "Profile ".dim(),
        name.to_string().cyan(),
        "  saves selected roles together".dim(),
    ])
}

fn profile_workers_line(draft: &RoleProfileSetupDraft) -> Line<'static> {
    Line::from(vec![
        gutter(),
        "Workers ".dim(),
        summarize_binding(&draft.worker_cc).cyan(),
        "  ".dim(),
        summarize_binding(&draft.worker_codex).fg(Color::Gray),
        "  ".dim(),
        summarize_binding(&draft.worker_gemini).fg(Color::Gray),
    ])
}

fn profile_write_preview_line(draft: &RoleProfileSetupDraft) -> Line<'static> {
    Line::from(vec![
        gutter(),
        "Writes  ".dim(),
        profile_setup_preview(draft).fg(Color::Gray),
    ])
}

fn summarize_binding(binding: &str) -> String {
    binding
        .split('@')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("skip")
        .to_string()
}

fn profile_setup_preview(draft: &RoleProfileSetupDraft) -> String {
    format!(
        "roles profile {} with {} binding(s)",
        profile_name_or_default(draft),
        profile_binding_count(draft)
    )
}

pub(crate) fn role_profile_setup_draft_args(
    draft: &RoleProfileSetupDraft,
) -> Result<String, String> {
    let name = profile_name_or_default(draft);
    let mut args = vec!["profile".to_string(), name];
    push_binding_arg(&mut args, "--planner", &draft.planner)?;
    push_binding_arg(&mut args, "--critic", &draft.critic)?;
    push_binding_arg(&mut args, "--reviewer", &draft.reviewer)?;
    push_binding_arg(&mut args, "--worker-cc", &draft.worker_cc)?;
    push_binding_arg(&mut args, "--worker-codex", &draft.worker_codex)?;
    push_binding_arg(&mut args, "--worker-gemini", &draft.worker_gemini)?;
    if args.len() == 2 {
        return Err("set at least one role binding".to_string());
    }
    Ok(args.join(" "))
}

fn profile_name_or_default(draft: &RoleProfileSetupDraft) -> String {
    let normalized = draft
        .name
        .trim()
        .chars()
        .map(|ch| if ch.is_ascii_whitespace() { '-' } else { ch })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if normalized.is_empty() {
        "default".to_string()
    } else {
        normalized
    }
}

fn push_binding_arg(args: &mut Vec<String>, flag: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("skip") || value == "-" {
        return Ok(());
    }
    validate_binding(value)?;
    args.push(flag.to_string());
    args.push(value.to_string());
    Ok(())
}

fn validate_binding(value: &str) -> Result<(), String> {
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{value} must not contain whitespace"));
    }
    let Some((model, runtime)) = value.rsplit_once('@') else {
        return Err(format!("{value} must be model@runtime"));
    };
    if model.trim().is_empty() || runtime.trim().is_empty() {
        return Err(format!("{value} must include model and runtime"));
    }
    Ok(())
}

fn profile_binding_count(draft: &RoleProfileSetupDraft) -> usize {
    [
        draft.planner.as_str(),
        draft.critic.as_str(),
        draft.reviewer.as_str(),
        draft.worker_cc.as_str(),
        draft.worker_codex.as_str(),
        draft.worker_gemini.as_str(),
    ]
    .into_iter()
    .filter(|value| {
        let value = value.trim();
        !value.is_empty() && !value.eq_ignore_ascii_case("skip") && value != "-"
    })
    .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_profile_setup_draft_args_skips_empty_bindings() {
        let args = role_profile_setup_draft_args(&RoleProfileSetupDraft {
            name: "dev profile".to_string(),
            planner: "minimax/MiniMax-M3@codex".to_string(),
            worker_cc: "anthropic/claude-sonnet-4-6@claude-code".to_string(),
            worker_gemini: "google/gemini-1.5-pro@gemini".to_string(),
            ..RoleProfileSetupDraft::default()
        })
        .unwrap();

        assert_eq!(
            args,
            "profile dev-profile --planner minimax/MiniMax-M3@codex --worker-cc anthropic/claude-sonnet-4-6@claude-code --worker-gemini google/gemini-1.5-pro@gemini"
        );
    }

    #[test]
    fn role_profile_setup_draft_args_rejects_invalid_binding() {
        let err = role_profile_setup_draft_args(&RoleProfileSetupDraft {
            name: "dev".to_string(),
            planner: "minimax/MiniMax-M3".to_string(),
            ..RoleProfileSetupDraft::default()
        })
        .unwrap_err();

        assert!(err.contains("must be model@runtime"));
    }

    #[test]
    fn role_profile_setup_draft_args_rejects_ambiguous_binding() {
        let err = role_profile_setup_draft_args(&RoleProfileSetupDraft {
            name: "dev".to_string(),
            planner: "mini max/MiniMax-M3@codex".to_string(),
            ..RoleProfileSetupDraft::default()
        })
        .unwrap_err();

        assert!(err.contains("must not contain whitespace"));
    }
}
