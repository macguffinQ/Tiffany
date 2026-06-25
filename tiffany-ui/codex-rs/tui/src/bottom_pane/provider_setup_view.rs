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
use super::textarea::TextArea;
use super::textarea::TextAreaState;

const LABEL_WIDTH: u16 = 13;
const FIELD_START_ROW: u16 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProviderSetupDraft {
    pub(crate) provider: String,
    pub(crate) kind: String,
    pub(crate) env: String,
    pub(crate) key: String,
    pub(crate) endpoint: String,
}

pub(crate) type ProviderSetupSubmitted =
    Box<dyn Fn(ProviderSetupDraft) -> Result<(), String> + Send + Sync>;

pub(crate) struct ProviderSetupView {
    fields: Vec<ProviderSetupField>,
    active: usize,
    dropdown: Option<ProviderSetupDropdown>,
    on_submit: ProviderSetupSubmitted,
    completion: Option<ViewCompletion>,
    error: Option<String>,
}

struct ProviderSetupField {
    kind: ProviderSetupFieldKind,
    label: &'static str,
    placeholder: &'static str,
    textarea: TextArea,
    state: RefCell<TextAreaState>,
}

struct ProviderSetupDropdown {
    selected: usize,
}

#[derive(Clone, Copy)]
struct ProviderSetupChoice {
    label: &'static str,
    value: &'static str,
    hint: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderSetupFieldKind {
    Provider,
    Kind,
    Env,
    Key,
    Endpoint,
}

impl ProviderSetupView {
    pub(crate) fn new(draft: ProviderSetupDraft, on_submit: ProviderSetupSubmitted) -> Self {
        Self {
            fields: vec![
                ProviderSetupField::new(
                    ProviderSetupFieldKind::Provider,
                    "Provider",
                    "openai",
                    draft.provider,
                ),
                ProviderSetupField::new(ProviderSetupFieldKind::Kind, "Type", "openai", draft.kind),
                ProviderSetupField::new(
                    ProviderSetupFieldKind::Env,
                    "Env",
                    "OPENAI_API_KEY",
                    draft.env,
                ),
                ProviderSetupField::new(ProviderSetupFieldKind::Key, "Key", "sk-...", draft.key),
                ProviderSetupField::new(
                    ProviderSetupFieldKind::Endpoint,
                    "Endpoint",
                    "https://api.openai.com/v1",
                    draft.endpoint,
                ),
            ],
            active: 0,
            dropdown: None,
            on_submit,
            completion: None,
            error: None,
        }
    }

    fn active_field_mut(&mut self) -> &mut ProviderSetupField {
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

    fn active_choices(&self) -> Vec<ProviderSetupChoice> {
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
        self.dropdown = Some(ProviderSetupDropdown { selected });
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
        self.apply_active_choice(choice.value);
    }

    fn apply_active_choice(&mut self, value: &str) {
        let field_kind = self.fields[self.active].kind;
        self.set_field_value(field_kind, value);
        if field_kind == ProviderSetupFieldKind::Provider {
            self.apply_provider_defaults(value);
        }
    }

    fn active_is_selection_only(&self) -> bool {
        self.fields
            .get(self.active)
            .map(|field| field.kind.is_selection_only())
            .unwrap_or(false)
    }

    fn set_field_value(&mut self, kind: ProviderSetupFieldKind, value: &str) {
        if let Some(field) = self.fields.iter_mut().find(|field| field.kind == kind) {
            field.set_value(value);
        }
    }

    fn field_value(&self, kind: ProviderSetupFieldKind) -> String {
        self.fields
            .iter()
            .find(|field| field.kind == kind)
            .map(|field| field.textarea.text().trim().to_string())
            .unwrap_or_default()
    }

    fn apply_provider_defaults(&mut self, provider: &str) {
        self.set_field_value(
            ProviderSetupFieldKind::Kind,
            default_kind_for_provider(provider),
        );
        self.set_field_value(
            ProviderSetupFieldKind::Env,
            default_env_for_provider(provider),
        );
        self.set_field_value(
            ProviderSetupFieldKind::Endpoint,
            default_endpoint_for_provider(provider),
        );
        if self.field_value(ProviderSetupFieldKind::Key) == "<unchanged>" {
            self.set_field_value(ProviderSetupFieldKind::Key, "");
        }
    }

    pub(crate) fn draft(&self) -> ProviderSetupDraft {
        let mut draft = ProviderSetupDraft::default();
        for field in &self.fields {
            let value = field.textarea.text().trim().to_string();
            match field.kind {
                ProviderSetupFieldKind::Provider => draft.provider = value,
                ProviderSetupFieldKind::Kind => draft.kind = value,
                ProviderSetupFieldKind::Env => draft.env = value,
                ProviderSetupFieldKind::Key => draft.key = value,
                ProviderSetupFieldKind::Endpoint => draft.endpoint = value,
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
            provider_preset_line(&draft),
            provider_auth_line(&draft),
            provider_write_preview_line(&draft),
        ]
    }
}

impl ProviderSetupField {
    fn new(
        kind: ProviderSetupFieldKind,
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

impl BottomPaneView for ProviderSetupView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        if self.dropdown.is_some() {
            match key_event {
                KeyEvent {
                    code: KeyCode::Esc, ..
                } => {
                    self.dropdown = None;
                }
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
                        self.dropdown = Some(ProviderSetupDropdown { selected: idx });
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
                if self.active_is_selection_only() {
                    if matches!(
                        other.code,
                        KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
                    ) {
                        self.open_dropdown();
                    }
                } else {
                    self.active_field_mut().textarea.input(other);
                }
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
        if self.active_is_selection_only() {
            let value = pasted.trim();
            if let Some(choice) = self.active_choices().into_iter().find(|choice| {
                choice.value.eq_ignore_ascii_case(value) || choice.label.eq_ignore_ascii_case(value)
            }) {
                self.apply_active_choice(choice.value);
            } else {
                self.error = Some("choose from options".to_string());
            }
            return true;
        }
        let text = pasted.replace(['\r', '\n'], " ");
        self.active_field_mut().textarea.insert_str(text.trim());
        true
    }
}

impl Renderable for ProviderSetupView {
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

        let title_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        Paragraph::new(Line::from(vec![
            gutter(),
            Span::styled(
                "Configure provider",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]))
        .render(title_area, buf);

        let hint_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: 1,
        };
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
            " options".dim(),
        ]))
        .render(hint_area, buf);

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
            let label_area = Rect {
                x: area.x,
                y: area.y.saturating_add(row),
                width: LABEL_WIDTH.min(area.width),
                height: 1,
            };
            Paragraph::new(Line::from(vec![
                gutter(),
                Span::styled(format!("{:<9}{suffix}", field.label), label_style),
            ]))
            .render(label_area, buf);

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
                    "Options".cyan(),
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
                    Span::styled(choice.label, style),
                    if choice.hint.is_empty() {
                        Span::raw("")
                    } else {
                        Span::styled(
                            format!("  {}", choice.hint),
                            Style::default().fg(Color::DarkGray),
                        )
                    },
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

fn gutter() -> Span<'static> {
    "▌ ".cyan()
}

impl ProviderSetupFieldKind {
    fn is_selection_only(self) -> bool {
        matches!(
            self,
            ProviderSetupFieldKind::Provider | ProviderSetupFieldKind::Kind
        )
    }

    fn choices(self) -> Vec<ProviderSetupChoice> {
        match self {
            ProviderSetupFieldKind::Provider => vec![
                choice("custom", "custom", "OpenAI-compatible · custom endpoint"),
                choice("minimax", "minimax", "OpenAI-compatible · MINIMAX_API_KEY"),
                choice("anthropic", "anthropic", "hosted · ANTHROPIC_API_KEY"),
                choice("google", "google", "hosted · GOOGLE_API_KEY"),
                choice("ollama", "ollama", "local · no api key"),
                choice(
                    "deepseek",
                    "deepseek",
                    "OpenAI-compatible · DEEPSEEK_API_KEY",
                ),
                choice(
                    "openrouter",
                    "openrouter",
                    "OpenAI-compatible · OPENROUTER_API_KEY",
                ),
                choice(
                    "moonshot",
                    "moonshot",
                    "OpenAI-compatible · MOONSHOT_API_KEY",
                ),
                choice("mistral", "mistral", "OpenAI-compatible · MISTRAL_API_KEY"),
                choice("openai", "openai", "hosted · OPENAI_API_KEY"),
            ],
            ProviderSetupFieldKind::Kind => vec![
                choice("openai-compatible", "openai", "Chat Completions style"),
                choice("anthropic", "anthropic", "Claude API style"),
                choice("google", "google", "Gemini API style"),
                choice("ollama", "ollama", "local Ollama"),
            ],
            ProviderSetupFieldKind::Env => vec![
                choice("OPENAI_API_KEY", "OPENAI_API_KEY", "env ref"),
                choice("ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY", "env ref"),
                choice("GOOGLE_API_KEY", "GOOGLE_API_KEY", "env ref"),
                choice("MINIMAX_API_KEY", "MINIMAX_API_KEY", "env ref"),
                choice("DEEPSEEK_API_KEY", "DEEPSEEK_API_KEY", "env ref"),
                choice("OPENROUTER_API_KEY", "OPENROUTER_API_KEY", "env ref"),
                choice("MOONSHOT_API_KEY", "MOONSHOT_API_KEY", "env ref"),
                choice("MISTRAL_API_KEY", "MISTRAL_API_KEY", "env ref"),
                choice("CUSTOM_API_KEY", "CUSTOM_API_KEY", "env ref"),
                choice("none", "", "local or no credential"),
            ],
            ProviderSetupFieldKind::Endpoint => vec![
                choice("OpenAI default", "https://api.openai.com/v1", ""),
                choice("Ollama local", "http://localhost:11434", ""),
                choice("MiniMax", "https://api.minimaxi.com/v1", ""),
                choice("DeepSeek", "https://api.deepseek.com/v1", ""),
                choice("OpenRouter", "https://openrouter.ai/api/v1", ""),
                choice("Moonshot", "https://api.moonshot.ai/v1", ""),
                choice("Mistral", "https://api.mistral.ai/v1", ""),
                choice("none", "", "provider default"),
            ],
            ProviderSetupFieldKind::Key => Vec::new(),
        }
    }
}

fn default_kind_for_provider(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => "anthropic",
        "google" | "gemini" => "google",
        "ollama" => "ollama",
        _ => "openai",
    }
}

fn default_env_for_provider(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => "ANTHROPIC_API_KEY",
        "google" | "gemini" => "GOOGLE_API_KEY",
        "minimax" => "MINIMAX_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "moonshot" => "MOONSHOT_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "custom" => "CUSTOM_API_KEY",
        "ollama" => "",
        _ => "CUSTOM_API_KEY",
    }
}

fn default_endpoint_for_provider(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "ollama" => "http://localhost:11434",
        "minimax" => "https://api.minimaxi.com/v1",
        "deepseek" => "https://api.deepseek.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "moonshot" => "https://api.moonshot.ai/v1",
        "mistral" => "https://api.mistral.ai/v1",
        _ => "",
    }
}

fn choice(label: &'static str, value: &'static str, hint: &'static str) -> ProviderSetupChoice {
    ProviderSetupChoice { label, value, hint }
}

fn dropdown_window_start(selected: usize, len: usize, visible: usize) -> usize {
    if len <= visible {
        0
    } else if selected >= visible {
        selected + 1 - visible
    } else {
        0
    }
}

fn provider_preset_line(draft: &ProviderSetupDraft) -> Line<'static> {
    let provider = draft.provider.trim();
    Line::from(vec![
        gutter(),
        "Preset ".dim(),
        provider_display_name(provider).cyan(),
        "  ".dim(),
        provider_hint(provider).dim(),
    ])
}

fn provider_auth_line(draft: &ProviderSetupDraft) -> Line<'static> {
    let env = draft.env.trim().trim_start_matches('$');
    let key = draft.key.trim();
    if is_keep_key_marker(key) {
        return Line::from(vec![gutter(), "Auth   ".dim(), "existing key kept".green()]);
    }
    if !key.is_empty() {
        return Line::from(vec![
            gutter(),
            "Auth   ".dim(),
            "literal key".fg(Color::Yellow),
            "  stored in config; env ref is safer".dim(),
        ]);
    }
    if !env.is_empty() {
        let status = if std::env::var(env).unwrap_or_default().is_empty() {
            "unset"
        } else {
            "set"
        };
        let status_style = if status == "set" {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Yellow)
        };
        return Line::from(vec![
            gutter(),
            "Auth   ".dim(),
            format!("${{{env}}}").cyan(),
            "  ".dim(),
            Span::styled(status, status_style),
        ]);
    }
    if draft.provider.trim().eq_ignore_ascii_case("ollama") {
        return Line::from(vec![gutter(), "Auth   ".dim(), "not required".green()]);
    }
    Line::from(vec![
        gutter(),
        "Auth   ".dim(),
        "missing".fg(Color::Yellow),
        "  choose Env or enter Key".dim(),
    ])
}

fn provider_write_preview_line(draft: &ProviderSetupDraft) -> Line<'static> {
    Line::from(vec![
        gutter(),
        "Writes ".dim(),
        provider_setup_preview(draft).fg(Color::Gray),
    ])
}

fn provider_setup_preview(draft: &ProviderSetupDraft) -> String {
    let provider = draft.provider.trim();
    let kind = draft.kind.trim();
    let env = draft.env.trim().trim_start_matches('$');
    let key = draft.key.trim();
    let endpoint = draft.endpoint.trim();
    let mut parts = vec!["config provider setup".to_string()];
    if provider.is_empty() {
        parts.push("<provider>".to_string());
    } else {
        parts.push(provider.to_string());
    }
    if !kind.is_empty() {
        parts.push("--type".to_string());
        parts.push(kind.to_string());
    }
    if !env.is_empty() {
        parts.push("--env".to_string());
        parts.push(env.to_string());
    }
    if !key.is_empty() && !is_keep_key_marker(key) {
        parts.push("--key".to_string());
        parts.push("<redacted>".to_string());
    }
    if !endpoint.is_empty() {
        parts.push("--endpoint".to_string());
        parts.push(endpoint.to_string());
    }
    parts.join(" ")
}

fn provider_display_name(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => "Anthropic",
        "google" | "gemini" => "Google",
        "ollama" => "Ollama",
        "minimax" => "MiniMax",
        "deepseek" => "DeepSeek",
        "openrouter" => "OpenRouter",
        "moonshot" => "Moonshot",
        "mistral" => "Mistral",
        "custom" => "Custom",
        "openai" | "" => "OpenAI",
        _ => "Custom",
    }
}

fn provider_hint(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "anthropic" => "Claude API provider",
        "google" | "gemini" => "Gemini API provider",
        "ollama" => "Local model runtime, no API key",
        "minimax" => "OpenAI-compatible hosted endpoint",
        "deepseek" => "OpenAI-compatible hosted endpoint",
        "openrouter" => "OpenAI-compatible model router",
        "moonshot" => "OpenAI-compatible Kimi endpoint",
        "mistral" => "OpenAI-compatible hosted endpoint",
        "custom" => "Bring your own OpenAI-compatible endpoint",
        _ => "Hosted OpenAI-compatible provider",
    }
}

fn is_keep_key_marker(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "<unchanged>" | "unchanged" | "keep")
}
