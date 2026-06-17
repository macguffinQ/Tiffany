// Codex-style inline terminal surface for tiffany-loop lightweight terminal runner.
//
// This wraps the Codex-derived custom terminal and insert-history helpers. It
// owns finalized history insertion, live bottom viewport rendering, ANSI style
// conversion, and resize-reflow cleanup so `terminal.rs` only coordinates app
// events.

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::io::{self, Write};

type CodexTerminal = super::codex_terminal::Terminal<CrosstermBackend<io::Stdout>>;

pub(super) struct InlineTerminalSurface {
    terminal: CodexTerminal,
}

impl InlineTerminalSurface {
    pub(super) fn new() -> io::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = super::codex_terminal::Terminal::with_options(backend)?;
        let size = terminal.size()?;
        let y = terminal
            .last_known_cursor_pos
            .y
            .min(size.height.saturating_sub(1));
        terminal.set_viewport_area(Rect::new(0, y, size.width, 1));
        Ok(Self { terminal })
    }

    pub(super) fn write_history_lines(&mut self, lines: Vec<String>) -> io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let lines = lines
            .into_iter()
            .map(|line| ansi_to_ratatui_line(&line))
            .collect::<Vec<_>>();
        super::codex_insert_history::insert_history_lines(&mut self.terminal, lines)?;
        self.terminal.invalidate_viewport();
        Write::flush(self.terminal.backend_mut())
    }

    pub(super) fn render_input_area(
        &mut self,
        lines: Vec<String>,
        cursor_x: usize,
    ) -> io::Result<()> {
        let size = self.terminal.size()?;
        let height = (lines.len() as u16).max(1).min(size.height.max(1));
        if self.update_inline_viewport_for_resize_reflow(height)? {
            self.terminal.invalidate_viewport();
        }
        let area = self.terminal.viewport_area;
        let width = area.width.max(1);

        let styled_lines = lines
            .iter()
            .map(|line| ansi_to_ratatui_line(line))
            .collect::<Vec<_>>();
        let cursor_y = area.y + height.saturating_sub(1);
        let cursor_x = (cursor_x as u16).min(width.saturating_sub(1));

        self.terminal.draw(|frame| {
            let buffer = frame.buffer_mut();
            for (idx, line) in styled_lines.iter().enumerate() {
                if idx as u16 >= height {
                    break;
                }
                buffer.set_line(area.x, area.y + idx as u16, line, width);
            }
            frame.set_cursor_position(Position {
                x: cursor_x,
                y: cursor_y,
            });
        })
    }

    pub(super) fn clear_input_area(&mut self) -> io::Result<()> {
        if self.terminal.viewport_area.is_empty() {
            return Ok(());
        }
        let size = self.terminal.size()?;
        let clear_position = viewport_clear_position(
            self.terminal.viewport_area,
            self.terminal.viewport_area,
            size,
        );
        self.terminal.clear_after_position(clear_position)?;
        self.terminal.invalidate_viewport();
        Write::flush(self.terminal.backend_mut())
    }

    fn update_inline_viewport_for_resize_reflow(&mut self, height: u16) -> io::Result<bool> {
        let size = self.terminal.size()?;
        let previous_area = self.terminal.viewport_area;
        let previous_screen_size = self.terminal.last_known_screen_size;
        let area = inline_viewport_area_after_resize(
            previous_area,
            self.terminal.last_known_cursor_pos,
            previous_screen_size,
            size,
            height,
        );

        if area != self.terminal.viewport_area {
            let clear_position = viewport_clear_position(previous_area, area, size);
            self.terminal.set_viewport_area(area);
            self.terminal.resize(size)?;
            self.terminal.clear_after_position(clear_position)?;
            return Ok(true);
        }

        if size != previous_screen_size {
            self.terminal.resize(size)?;
        }
        Ok(false)
    }
}

fn inline_viewport_area_after_resize(
    previous_area: Rect,
    last_cursor_pos: Position,
    previous_screen_size: Size,
    screen_size: Size,
    requested_height: u16,
) -> Rect {
    let height = requested_height.max(1).min(screen_size.height.max(1));
    let width = screen_size.width.max(1);
    let previous_bottom = if previous_area.is_empty() {
        last_cursor_pos.y.saturating_add(1)
    } else {
        previous_area.bottom()
    };
    let was_bottom_aligned =
        !previous_area.is_empty() && previous_area.bottom() == previous_screen_size.height;
    let bottom = if was_bottom_aligned {
        screen_size.height
    } else {
        previous_bottom.min(screen_size.height)
    };
    let y = bottom
        .saturating_sub(height)
        .min(screen_size.height.saturating_sub(height));
    Rect::new(0, y, width, height)
}

pub(super) fn viewport_clear_position(
    previous_area: Rect,
    next_area: Rect,
    screen_size: Size,
) -> Position {
    let raw = if previous_area.is_empty() {
        next_area.as_position()
    } else {
        Position::new(0, previous_area.y.min(next_area.y))
    };
    Position {
        x: raw.x.min(screen_size.width.saturating_sub(1)),
        y: raw.y.min(screen_size.height.saturating_sub(1)),
    }
}

pub(super) fn ansi_to_ratatui_line(input: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut buf = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            let mut code = String::new();
            for next in chars.by_ref() {
                if next == 'm' {
                    break;
                }
                code.push(next);
            }
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), style));
            }
            apply_sgr(&mut style, &code);
        } else {
            buf.push(ch);
        }
    }

    if !buf.is_empty() || spans.is_empty() {
        spans.push(Span::styled(buf, style));
    }
    Line::from(spans)
}

fn apply_sgr(style: &mut Style, code: &str) {
    let values = if code.is_empty() {
        vec![0]
    } else {
        code.split(';')
            .filter_map(|part| part.parse::<u16>().ok())
            .collect::<Vec<_>>()
    };
    let mut idx = 0;
    while idx < values.len() {
        match values[idx] {
            0 => *style = Style::default(),
            1 => style.add_modifier.insert(Modifier::BOLD),
            2 => style.add_modifier.insert(Modifier::DIM),
            22 => {
                style.add_modifier.remove(Modifier::BOLD);
                style.add_modifier.remove(Modifier::DIM);
            }
            31 => style.fg = Some(Color::Red),
            33 => style.fg = Some(Color::Yellow),
            35 => style.fg = Some(Color::Magenta),
            36 => style.fg = Some(Color::Cyan),
            39 => style.fg = None,
            38 if values.get(idx + 1) == Some(&2) && idx + 4 < values.len() => {
                style.fg = Some(Color::Rgb(
                    values[idx + 2] as u8,
                    values[idx + 3] as u8,
                    values[idx + 4] as u8,
                ));
                idx += 4;
            }
            _ => {}
        }
        idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_line_strips_escape_codes_and_preserves_style() {
        let line = ansi_to_ratatui_line("\x1b[38;2;129;216;208m\x1b[1mok\x1b[0m \x1b[2mdim\x1b[0m");

        assert_eq!(line.spans[0].content, "ok");
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(129, 216, 208)));
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[1].content, " ");
        assert_eq!(line.spans[2].content, "dim");
        assert!(line.spans[2].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn clear_position_clamps_after_resize_shrink() {
        let previous = Rect::new(0, 40, 120, 4);
        let next = Rect::new(0, 17, 80, 3);
        let screen = Size::new(80, 20);

        assert_eq!(
            viewport_clear_position(previous, next, screen),
            Position::new(0, 17)
        );
    }

    #[test]
    fn clear_position_handles_empty_previous_area() {
        let previous = Rect::ZERO;
        let next = Rect::new(0, 22, 100, 2);
        let screen = Size::new(80, 20);

        assert_eq!(
            viewport_clear_position(previous, next, screen),
            Position::new(0, 19)
        );
    }

    #[test]
    fn inline_viewport_starts_at_real_cursor_instead_of_forcing_bottom() {
        let area = inline_viewport_area_after_resize(
            Rect::ZERO,
            Position::new(0, 4),
            Size::new(80, 24),
            Size::new(80, 24),
            1,
        );

        assert_eq!(area, Rect::new(0, 4, 80, 1));
    }

    #[test]
    fn inline_viewport_keeps_bottom_alignment_after_resize_when_already_bottom_aligned() {
        let area = inline_viewport_area_after_resize(
            Rect::new(0, 21, 80, 3),
            Position::new(3, 23),
            Size::new(80, 24),
            Size::new(100, 30),
            3,
        );

        assert_eq!(area, Rect::new(0, 27, 100, 3));
    }

    #[test]
    fn inline_viewport_preserves_non_bottom_anchor_on_width_resize() {
        let area = inline_viewport_area_after_resize(
            Rect::new(0, 4, 80, 2),
            Position::new(3, 5),
            Size::new(80, 24),
            Size::new(100, 24),
            2,
        );

        assert_eq!(area, Rect::new(0, 4, 100, 2));
    }
}
