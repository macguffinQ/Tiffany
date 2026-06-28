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
use std::borrow::Cow;
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
pub(crate) struct RoleSetupDraft {
    pub(crate) role: String,
    pub(crate) provider: String,
    pub(crate) model_id: String,
    pub(crate) model_name: String,
    pub(crate) runtime: String,
    pub(crate) teams: String,
}

pub(crate) type RoleSetupSubmitted =
    Box<dyn Fn(RoleSetupDraft) -> Result<(), String> + Send + Sync>;

pub(crate) struct RoleSetupView {
    fields: Vec<RoleSetupField>,
    active: usize,
    dropdown: Option<RoleSetupDropdown>,
    catalog: RoleSetupCatalog,
    on_submit: RoleSetupSubmitted,
    completion: Option<ViewCompletion>,
    error: Option<String>,
}

struct RoleSetupField {
    kind: RoleSetupFieldKind,
    label: &'static str,
    placeholder: &'static str,
    textarea: TextArea,
    state: RefCell<TextAreaState>,
}

struct RoleSetupDropdown {
    selected: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoleSetupCatalog {
    pub(crate) roles: Vec<RoleSetupChoice>,
    pub(crate) providers: Vec<RoleSetupChoice>,
    pub(crate) runtimes: Vec<RoleSetupChoice>,
    pub(crate) models: Vec<RoleSetupModelChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoleSetupChoice {
    pub(crate) label: Cow<'static, str>,
    pub(crate) value: Cow<'static, str>,
    pub(crate) hint: Cow<'static, str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoleSetupModelChoice {
    pub(crate) provider: String,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) hint: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoleSetupFieldKind {
    Role,
    Provider,
    ModelName,
    Runtime,
    Teams,
}

impl RoleSetupView {
    pub(crate) fn with_catalog(
        draft: RoleSetupDraft,
        catalog: RoleSetupCatalog,
        on_submit: RoleSetupSubmitted,
    ) -> Self {
        let RoleSetupDraft {
            role,
            provider,
            model_id,
            model_name,
            runtime,
            teams,
        } = draft;
        let api_model = if model_name.trim().is_empty() {
            catalog
                .model_name_for_model_id(&provider, &model_id)
                .or_else(|| model_name_for_model_id(&provider, &model_id))
                .unwrap_or(model_id)
        } else {
            model_name
        };
        Self {
            fields: vec![
                RoleSetupField::new(RoleSetupFieldKind::Role, "Role", "worker-codex", role),
                RoleSetupField::new(
                    RoleSetupFieldKind::Provider,
                    "Provider",
                    "minimax",
                    provider,
                ),
                RoleSetupField::new(
                    RoleSetupFieldKind::ModelName,
                    "API Model",
                    "MiniMax-M3",
                    api_model,
                ),
                RoleSetupField::new(RoleSetupFieldKind::Runtime, "Runtime", "codex", runtime),
                RoleSetupField::new(RoleSetupFieldKind::Teams, "Teams", "auto", teams),
            ],
            active: 0,
            dropdown: None,
            catalog,
            on_submit,
            completion: None,
            error: None,
        }
    }

    fn active_field_mut(&mut self) -> &mut RoleSetupField {
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
            .map(|field| self.choices_for_field(field.kind))
            .unwrap_or_default()
    }

    fn choices_for_field(&self, kind: RoleSetupFieldKind) -> Vec<RoleSetupChoice> {
        match kind {
            RoleSetupFieldKind::Role => merge_choices(self.catalog.roles.clone(), kind.choices()),
            RoleSetupFieldKind::Provider => {
                merge_choices(self.catalog.providers.clone(), kind.choices())
            }
            RoleSetupFieldKind::ModelName => merge_choices(
                self.catalog.model_name_choices_for_provider(
                    &self.field_value(RoleSetupFieldKind::Provider),
                ),
                model_name_choices_for_provider(&self.field_value(RoleSetupFieldKind::Provider)),
            ),
            RoleSetupFieldKind::Runtime => {
                merge_choices(self.catalog.runtimes.clone(), kind.choices())
            }
            RoleSetupFieldKind::Teams => kind.choices(),
        }
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
        self.dropdown = Some(RoleSetupDropdown { selected });
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
        let value = choice.value.to_string();
        self.apply_active_choice(&value);
    }

    fn apply_active_choice(&mut self, value: &str) {
        let field_kind = self.fields[self.active].kind;
        self.set_field_value(field_kind, value);
        match field_kind {
            RoleSetupFieldKind::Role => self.apply_role_defaults(value),
            RoleSetupFieldKind::Provider => self.apply_provider_defaults(value),
            RoleSetupFieldKind::Runtime => self.apply_runtime_defaults(value),
            _ => {}
        }
    }

    fn active_is_selection_only(&self) -> bool {
        let Some(field) = self.fields.get(self.active) else {
            return false;
        };
        match field.kind {
            RoleSetupFieldKind::ModelName => {
                !is_free_model_provider(&self.field_value(RoleSetupFieldKind::Provider))
            }
            _ => field.kind.is_selection_only(),
        }
    }

    fn set_field_value(&mut self, kind: RoleSetupFieldKind, value: &str) {
        if let Some(field) = self.fields.iter_mut().find(|field| field.kind == kind) {
            field.set_value(value);
        }
    }

    fn field_value(&self, kind: RoleSetupFieldKind) -> String {
        self.fields
            .iter()
            .find(|field| field.kind == kind)
            .map(|field| field.textarea.text().trim().to_string())
            .unwrap_or_default()
    }

    fn apply_role_defaults(&mut self, role: &str) {
        if let Some(defaults) = default_role_setup(role) {
            self.set_field_value(RoleSetupFieldKind::Provider, defaults.provider);
            self.set_field_value(RoleSetupFieldKind::ModelName, defaults.model_name);
            self.set_field_value(RoleSetupFieldKind::Runtime, defaults.runtime);
            self.set_field_value(RoleSetupFieldKind::Teams, defaults.teams);
        }
    }

    fn apply_provider_defaults(&mut self, provider: &str) {
        if let Some(defaults) = default_provider_setup(provider) {
            self.set_field_value(RoleSetupFieldKind::ModelName, defaults.model_name);
            self.set_field_value(RoleSetupFieldKind::Runtime, defaults.runtime);
        }
    }

    fn apply_runtime_defaults(&mut self, runtime: &str) {
        if self.field_value(RoleSetupFieldKind::Provider).is_empty() {
            self.set_field_value(
                RoleSetupFieldKind::Provider,
                default_provider_for_runtime(runtime),
            );
        }
    }

    pub(crate) fn draft(&self) -> RoleSetupDraft {
        let mut draft = RoleSetupDraft::default();
        for field in &self.fields {
            let value = field.textarea.text().trim().to_string();
            match field.kind {
                RoleSetupFieldKind::Role => draft.role = value,
                RoleSetupFieldKind::Provider => draft.provider = value,
                RoleSetupFieldKind::ModelName => draft.model_name = value,
                RoleSetupFieldKind::Runtime => draft.runtime = value,
                RoleSetupFieldKind::Teams => draft.teams = value,
            }
        }
        if draft.provider.trim().is_empty() {
            draft.model_id = draft.model_name.trim().to_string();
            draft.model_name.clear();
        } else {
            draft.model_id = self
                .catalog
                .model_id_for_selection(&draft.provider, &draft.model_name)
                .unwrap_or_else(|| {
                    model_id_for_selection(&draft.provider, &draft.runtime, &draft.model_name)
                });
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
            role_binding_line(&draft),
            role_runtime_line(&draft),
            role_write_preview_line(&draft),
        ]
    }
}

impl RoleSetupField {
    fn new(
        kind: RoleSetupFieldKind,
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

impl BottomPaneView for RoleSetupView {
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
                        self.dropdown = Some(RoleSetupDropdown { selected: idx });
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
                self.apply_active_choice(&choice.value);
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

impl Renderable for RoleSetupView {
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
                "Register role",
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
            " options".dim(),
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
            let suffix = if !self.choices_for_field(field.kind).is_empty() {
                " ▾"
            } else {
                ""
            };
            Paragraph::new(Line::from(vec![
                gutter(),
                Span::styled(format!("{:<9}{suffix}", field.label), label_style),
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
                    Span::styled(choice.label.clone(), style),
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

impl RoleSetupCatalog {
    fn model_name_choices_for_provider(&self, provider: &str) -> Vec<RoleSetupChoice> {
        self.models
            .iter()
            .filter(|model| model.provider.eq_ignore_ascii_case(provider))
            .map(|model| {
                RoleSetupChoice::new(model.name.clone(), model.name.clone(), model.hint.clone())
            })
            .collect()
    }

    fn model_name_for_model_id(&self, provider: &str, model_id: &str) -> Option<String> {
        self.models
            .iter()
            .find(|model| {
                model.provider.eq_ignore_ascii_case(provider)
                    && model.id.eq_ignore_ascii_case(model_id)
            })
            .map(|model| model.name.clone())
    }

    fn model_id_for_selection(&self, provider: &str, model_name: &str) -> Option<String> {
        self.models
            .iter()
            .find(|model| {
                model.provider.eq_ignore_ascii_case(provider)
                    && (model.name.eq_ignore_ascii_case(model_name)
                        || model.id.eq_ignore_ascii_case(model_name))
            })
            .map(|model| model.id.clone())
    }
}

impl RoleSetupChoice {
    pub(crate) fn new(
        label: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
        hint: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            hint: hint.into(),
        }
    }
}

impl RoleSetupModelChoice {
    pub(crate) fn new(
        provider: impl Into<String>,
        id: impl Into<String>,
        name: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            id: id.into(),
            name: name.into(),
            hint: hint.into(),
        }
    }
}

impl RoleSetupFieldKind {
    fn is_selection_only(self) -> bool {
        matches!(
            self,
            RoleSetupFieldKind::Role
                | RoleSetupFieldKind::Provider
                | RoleSetupFieldKind::ModelName
                | RoleSetupFieldKind::Runtime
                | RoleSetupFieldKind::Teams
        )
    }

    fn choices(self) -> Vec<RoleSetupChoice> {
        match self {
            RoleSetupFieldKind::Role => vec![
                choice("worker-codex", "worker-codex", "Codex worker"),
                choice("worker-cc", "worker-cc", "Claude worker"),
                choice("worker-gemini", "worker-gemini", "Gemini worker"),
                choice("planner", "planner", "plans tasks"),
                choice("critic", "critic", "checks plans"),
                choice("reviewer", "reviewer", "reviews outputs"),
            ],
            RoleSetupFieldKind::Provider => vec![
                choice("minimax", "minimax", "OpenAI-compatible"),
                choice("openai", "openai", "OpenAI"),
                choice("anthropic", "anthropic", "Claude API"),
                choice("google", "google", "Gemini API"),
                choice("ollama", "ollama", "local"),
                choice("deepseek", "deepseek", "OpenAI-compatible"),
                choice("openrouter", "openrouter", "OpenAI-compatible"),
                choice("moonshot", "moonshot", "OpenAI-compatible"),
                choice("mistral", "mistral", "OpenAI-compatible"),
                choice("custom", "custom", "custom endpoint"),
                choice("none", "", "manual existing model"),
            ],
            RoleSetupFieldKind::ModelName => model_name_choices_for_provider(""),
            RoleSetupFieldKind::Runtime => vec![
                choice("codex", "codex", "Codex CLI"),
                choice("claude-code", "claude-code", "Claude Code CLI"),
                choice("gemini", "gemini", "Gemini CLI"),
                choice("direct", "direct", "direct SDK when supported"),
            ],
            RoleSetupFieldKind::Teams => vec![
                choice("auto", "auto", "runtime default"),
                choice("yes", "yes", "force agent teams"),
                choice("no", "no", "disable agent teams"),
            ],
        }
    }
}

#[derive(Clone, Copy)]
struct ModelOption {
    id: &'static str,
    name: &'static str,
    hint: &'static str,
}

fn model_name_choices_for_provider(provider: &str) -> Vec<RoleSetupChoice> {
    model_options_for_provider(provider)
        .into_iter()
        .map(|model| choice(model.name, model.name, model.hint))
        .collect()
}

fn model_name_for_model_id(provider: &str, model_id: &str) -> Option<String> {
    model_options_for_provider(provider)
        .into_iter()
        .find(|model| model.id.eq_ignore_ascii_case(model_id))
        .map(|model| model.name.to_string())
}

fn model_id_for_selection(provider: &str, runtime: &str, model_name: &str) -> String {
    let provider = provider.trim();
    let runtime = runtime.trim().to_ascii_lowercase();
    let model_name = model_name.trim();
    if provider.eq_ignore_ascii_case("minimax") && model_name.eq_ignore_ascii_case("MiniMax-M3") {
        return if matches!(runtime.as_str(), "claude" | "claude-code") {
            "minimax-m3-claude".to_string()
        } else {
            "minimax-m3-codex".to_string()
        };
    }
    if let Some(model) = model_options_for_provider(provider)
        .into_iter()
        .find(|model| {
            model.name.eq_ignore_ascii_case(model_name) || model.id.eq_ignore_ascii_case(model_name)
        })
    {
        return model.id.to_string();
    }
    slug_model_id(model_name)
}

fn slug_model_id(model_name: &str) -> String {
    let slug = model_name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "custom-model".to_string()
    } else {
        slug
    }
}

fn model_options_for_provider(provider: &str) -> Vec<ModelOption> {
    match provider.to_ascii_lowercase().as_str() {
        "minimax" => vec![
            model("minimax-m3-codex", "MiniMax-M3", "MiniMax via tiffany-loop"),
            model("minimax-m3-claude", "MiniMax-M3", "MiniMax via Claude"),
        ],
        "anthropic" => vec![
            model("opus", "claude-opus-4-6", "Claude smartest"),
            model("sonnet", "claude-sonnet-4-6", "Claude balanced"),
            model("haiku", "claude-haiku-4-5", "Claude low cost"),
        ],
        "openai" => vec![
            model("gpt4o", "gpt-4o", "OpenAI flagship"),
            model("gpt4o-mini", "gpt-4o-mini", "OpenAI low cost"),
        ],
        "google" | "gemini" => vec![model("gemini-pro", "gemini-1.5-pro", "Gemini")],
        "deepseek" => vec![model("deepseek-chat", "deepseek-chat", "DeepSeek")],
        "openrouter" => vec![
            model("openrouter-auto", "openrouter/auto", "OpenRouter auto"),
            model("openrouter-gpt4o", "openai/gpt-4o", "OpenAI via router"),
        ],
        "moonshot" => vec![model("kimi-k2", "kimi-k2", "Moonshot/Kimi")],
        "mistral" => vec![model(
            "mistral-large",
            "mistral-large-latest",
            "Mistral Large",
        )],
        "ollama" => vec![model("llama3", "llama3", "Ollama local")],
        "custom" => vec![model("custom-model", "custom-model", "custom endpoint")],
        "" => vec![
            model("minimax-m3-codex", "MiniMax-M3", "MiniMax via tiffany-loop"),
            model("gpt4o", "gpt-4o", "OpenAI"),
            model("sonnet", "claude-sonnet-4-6", "Claude"),
            model("deepseek-chat", "deepseek-chat", "DeepSeek"),
            model("llama3", "llama3", "Ollama"),
        ],
        _ => vec![model("custom-model", "custom-model", "custom provider")],
    }
}

fn is_free_model_provider(provider: &str) -> bool {
    matches!(provider.to_ascii_lowercase().as_str(), "" | "custom")
}

fn model(id: &'static str, name: &'static str, hint: &'static str) -> ModelOption {
    ModelOption { id, name, hint }
}

#[derive(Clone, Copy)]
struct RoleDefaults {
    provider: &'static str,
    model_name: &'static str,
    runtime: &'static str,
    teams: &'static str,
}

fn default_role_setup(role: &str) -> Option<RoleDefaults> {
    match role.to_ascii_lowercase().as_str() {
        "worker-codex" => Some(RoleDefaults {
            provider: "minimax",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        "worker-cc" => Some(RoleDefaults {
            provider: "anthropic",
            model_name: "claude-sonnet-4-6",
            runtime: "claude-code",
            teams: "yes",
        }),
        "worker-gemini" => Some(RoleDefaults {
            provider: "google",
            model_name: "gemini-1.5-pro",
            runtime: "gemini",
            teams: "no",
        }),
        "planner" => Some(RoleDefaults {
            provider: "minimax",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        "critic" => Some(RoleDefaults {
            provider: "minimax",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        "reviewer" => Some(RoleDefaults {
            provider: "minimax",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        _ => None,
    }
}

fn default_provider_setup(provider: &str) -> Option<RoleDefaults> {
    match provider.to_ascii_lowercase().as_str() {
        "minimax" => Some(RoleDefaults {
            provider: "minimax",
            model_name: "MiniMax-M3",
            runtime: "codex",
            teams: "no",
        }),
        "anthropic" => Some(RoleDefaults {
            provider: "anthropic",
            model_name: "claude-sonnet-4-6",
            runtime: "claude-code",
            teams: "auto",
        }),
        "openai" => Some(RoleDefaults {
            provider: "openai",
            model_name: "gpt-4o",
            runtime: "codex",
            teams: "no",
        }),
        "google" | "gemini" => Some(RoleDefaults {
            provider: "google",
            model_name: "gemini-1.5-pro",
            runtime: "gemini",
            teams: "no",
        }),
        "deepseek" => Some(RoleDefaults {
            provider: "deepseek",
            model_name: "deepseek-chat",
            runtime: "codex",
            teams: "no",
        }),
        "openrouter" => Some(RoleDefaults {
            provider: "openrouter",
            model_name: "openrouter/auto",
            runtime: "codex",
            teams: "no",
        }),
        "moonshot" => Some(RoleDefaults {
            provider: "moonshot",
            model_name: "kimi-k2",
            runtime: "codex",
            teams: "no",
        }),
        "mistral" => Some(RoleDefaults {
            provider: "mistral",
            model_name: "mistral-large-latest",
            runtime: "codex",
            teams: "no",
        }),
        "ollama" => Some(RoleDefaults {
            provider: "ollama",
            model_name: "llama3",
            runtime: "codex",
            teams: "no",
        }),
        "custom" => Some(RoleDefaults {
            provider: "custom",
            model_name: "custom-model",
            runtime: "codex",
            teams: "no",
        }),
        _ => None,
    }
}

fn default_provider_for_runtime(runtime: &str) -> &'static str {
    match runtime.to_ascii_lowercase().as_str() {
        "claude-code" => "anthropic",
        "codex" => "openai",
        _ => "",
    }
}

pub(super) fn choice(
    label: &'static str,
    value: &'static str,
    hint: &'static str,
) -> RoleSetupChoice {
    RoleSetupChoice::new(label, value, hint)
}

fn merge_choices(
    mut preferred: Vec<RoleSetupChoice>,
    fallback: Vec<RoleSetupChoice>,
) -> Vec<RoleSetupChoice> {
    for choice in fallback {
        let duplicate = preferred.iter().any(|existing| {
            existing.value.eq_ignore_ascii_case(&choice.value)
                || (!existing.label.is_empty()
                    && !choice.label.is_empty()
                    && existing.label.eq_ignore_ascii_case(&choice.label))
        });
        if !duplicate {
            preferred.push(choice);
        }
    }
    preferred
}

pub(super) fn dropdown_window_start(selected: usize, len: usize, visible: usize) -> usize {
    if len <= visible {
        0
    } else if selected >= visible {
        selected + 1 - visible
    } else {
        0
    }
}

fn role_binding_line(draft: &RoleSetupDraft) -> Line<'static> {
    let role = draft.role.trim();
    let model = draft.model_name.trim();
    if role.is_empty() {
        return Line::from(vec![
            gutter(),
            "Role   ".dim(),
            "missing".fg(Color::Yellow),
            "  choose a role".dim(),
        ]);
    }
    Line::from(vec![
        gutter(),
        "Role   ".dim(),
        role.to_string().cyan(),
        "  ".dim(),
        model.to_string().fg(Color::Gray),
    ])
}

fn role_runtime_line(draft: &RoleSetupDraft) -> Line<'static> {
    Line::from(vec![
        gutter(),
        "Route  ".dim(),
        draft.runtime.trim().to_string().cyan(),
        "  provider ".dim(),
        draft.provider.trim().to_string().fg(Color::Gray),
        "  teams ".dim(),
        draft.teams.trim().to_string().fg(Color::Gray),
    ])
}

fn role_write_preview_line(draft: &RoleSetupDraft) -> Line<'static> {
    Line::from(vec![
        gutter(),
        "Writes ".dim(),
        role_setup_preview(draft).fg(Color::Gray),
    ])
}

fn role_setup_preview(draft: &RoleSetupDraft) -> String {
    let role = if draft.role.trim().is_empty() {
        "<role>"
    } else {
        draft.role.trim()
    };
    let model = if draft.model_name.trim().is_empty() {
        "<api-model>"
    } else {
        draft.model_name.trim()
    };
    let provider = if draft.provider.trim().is_empty() {
        "existing"
    } else {
        draft.provider.trim()
    };
    let runtime = if draft.runtime.trim().is_empty() {
        "runtime"
    } else {
        draft.runtime.trim()
    };
    format!("roles register {role} -> {provider}/{model} @ {runtime} (model id auto)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_catalog_filters_models_by_provider_and_preserves_model_id() {
        let catalog = RoleSetupCatalog {
            providers: vec![RoleSetupChoice::new("acme", "acme", "configured")],
            models: vec![
                RoleSetupModelChoice::new("acme", "fast", "fast-v2", "id fast"),
                RoleSetupModelChoice::new("other", "slow", "slow-v1", "id slow"),
            ],
            ..RoleSetupCatalog::default()
        };
        let view = RoleSetupView::with_catalog(
            RoleSetupDraft {
                role: "worker-codex".to_string(),
                provider: "acme".to_string(),
                model_id: "fast".to_string(),
                model_name: String::new(),
                runtime: "codex".to_string(),
                teams: "no".to_string(),
            },
            catalog,
            Box::new(|_| Ok(())),
        );

        let choices = view.choices_for_field(RoleSetupFieldKind::ModelName);

        assert_eq!(view.draft().model_name, "fast-v2");
        assert_eq!(view.draft().model_id, "fast");
        assert_eq!(choices[0].label.as_ref(), "fast-v2");
        assert!(
            choices
                .iter()
                .all(|choice| choice.label.as_ref() != "slow-v1")
        );
    }
}
