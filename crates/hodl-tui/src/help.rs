//! Contextual help overlay — draws a scrollable two-column keybind reference
//! on top of whichever screen is active.
//!
//! Each screen exposes `help_lines() -> Vec<(String, String)>` (key, description)
//! which is captured at overlay-open time so help text never drifts from the
//! actual handlers.
//!
//! Trigger key: `?` on screens without text-input mode.
//! Screens that have a form-input mode (`Send`, `Onboarding`) use `F1` instead
//! so `?` can still be typed in form fields.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// What the caller should do after a key is routed to the overlay.
#[derive(Debug, PartialEq, Eq)]
pub enum HelpAction {
    /// Overlay should be closed.
    Close,
    /// Scroll happened; caller should redraw.
    Scroll,
    /// Key was absorbed but no state change the caller cares about.
    None,
}

pub struct HelpOverlay {
    lines: Vec<(String, String)>,
    scroll: u16,
    title: String,
    /// Body rows visible at last draw — used by `handle_key` to clamp scroll
    /// against the bottom edge instead of `lines.len() - 1`.
    last_visible_rows: u16,
}

impl HelpOverlay {
    pub fn new(title: impl Into<String>, lines: Vec<(String, String)>) -> Self {
        Self {
            lines,
            scroll: 0,
            title: title.into(),
            last_visible_rows: 0,
        }
    }

    fn max_scroll(&self) -> u16 {
        let visible = self.last_visible_rows.max(1) as usize;
        self.lines.len().saturating_sub(visible) as u16
    }

    /// Draw the overlay centred at ~60% width, up to ~70% height.
    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        let w = ((area.width as f32) * 0.60) as u16;
        let max_h = ((area.height as f32) * 0.70) as u16;
        // 2 border rows + content + 1 footer row.
        let content_rows = self.lines.len() as u16;
        let h = (2 + content_rows + 1).min(max_h).max(5);

        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let overlay = Rect::new(x, y, w, h);

        let block = Block::default()
            .title(format!(" Help — {} ", self.title))
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Black).fg(Color::Green));

        let inner = block.inner(overlay);
        f.render_widget(Clear, overlay);
        f.render_widget(block, overlay);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Split inner: hint footer (1 row), body (rest).
        let body_area = if inner.height > 1 {
            Rect::new(inner.x, inner.y, inner.width, inner.height - 1)
        } else {
            inner
        };
        let footer_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );

        // Key-label column width: 12 chars + 2 padding.
        let key_col = 14u16.min(inner.width / 3);
        let desc_col = inner.width.saturating_sub(key_col);

        let visible_rows = body_area.height as usize;
        self.last_visible_rows = body_area.height;
        // Clamp scroll if the overlay shrunk since the last keypress.
        self.scroll = self.scroll.min(self.max_scroll());
        let start = self.scroll as usize;
        let end = (start + visible_rows).min(self.lines.len());

        let left_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(key_col), Constraint::Length(desc_col)])
            .split(body_area);

        let visible = &self.lines[start..end];

        let keys: Vec<Line> = visible
            .iter()
            .map(|(k, _)| {
                Line::from(Span::styled(
                    format!("{:>width$}", k, width = key_col.saturating_sub(2) as usize),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect();

        let descs: Vec<Line> = visible
            .iter()
            .map(|(_, d)| Line::from(Span::styled(d.clone(), Style::default().fg(Color::White))))
            .collect();

        f.render_widget(Paragraph::new(keys), left_chunks[0]);
        f.render_widget(Paragraph::new(descs), left_chunks[1]);

        // Footer scroll hint.
        let can_scroll_up = self.scroll > 0;
        let can_scroll_down = end < self.lines.len();
        let hint = match (can_scroll_up, can_scroll_down) {
            (true, true) => "k/↑ up  j/↓ down  g top  G end  ? / Esc / q close",
            (true, false) => "k/↑ up  g top  ? / Esc / q close",
            (false, true) => "j/↓ down  G end  ? / Esc / q close",
            (false, false) => "? / Esc / q close",
        };
        let footer = Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center);
        f.render_widget(footer, footer_area);
    }

    /// Route a keypress to the overlay.
    pub fn handle_key(&mut self, k: KeyEvent) -> HelpAction {
        match k.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(1) => {
                HelpAction::Close
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll = (self.scroll + 1).min(self.max_scroll());
                HelpAction::Scroll
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                HelpAction::Scroll
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.scroll = 0;
                HelpAction::Scroll
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.scroll = self.max_scroll();
                HelpAction::Scroll
            }
            _ => HelpAction::None,
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn scroll_clamps_to_max_visible_window() {
        let lines: Vec<_> = (0..10)
            .map(|i| (i.to_string(), format!("line {i}")))
            .collect();
        let mut overlay = HelpOverlay::new("Test", lines);
        // Simulate a draw that fit 4 visible rows.
        overlay.last_visible_rows = 4;
        // 10 lines, 4 visible → max scroll = 6.
        for _ in 0..20 {
            overlay.handle_key(press(KeyCode::Char('j')));
        }
        assert_eq!(overlay.scroll, 6);
        // G also clamps to 6, not 9.
        overlay.scroll = 0;
        overlay.handle_key(press(KeyCode::Char('G')));
        assert_eq!(overlay.scroll, 6);
    }

    #[test]
    fn scroll_down_up_smoke() {
        let lines = vec![
            ("a".to_string(), "alpha".to_string()),
            ("b".to_string(), "beta".to_string()),
            ("c".to_string(), "gamma".to_string()),
        ];
        let mut overlay = HelpOverlay::new("Test", lines);

        assert_eq!(overlay.scroll, 0);

        // Down twice.
        overlay.handle_key(press(KeyCode::Char('j')));
        overlay.handle_key(press(KeyCode::Char('j')));
        assert_eq!(overlay.scroll, 2);

        // Up once.
        overlay.handle_key(press(KeyCode::Char('k')));
        assert_eq!(overlay.scroll, 1);
    }
}
