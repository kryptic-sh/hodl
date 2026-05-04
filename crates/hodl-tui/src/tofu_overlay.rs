//! TOFU fingerprint mismatch remediation overlay.
//!
//! Drawn centred on top of whichever screen is active when a
//! `TofuMismatch` is detected.  The user chooses one of three actions:
//!
//! - **Trust new** — replace the stored pin with the presented fingerprint and
//!   retry the operation.
//! - **Stay pinned** — keep the stored pin and abort the current operation.
//! - **Abort** — close the overlay without changing the pin store (default).
//!
//! Default-selected button is `Abort` so that pressing Enter without reading
//! the prompt does NOT silently trust an unknown cert.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

// ── Public types ───────────────────────────────────────────────────────────

/// The choice the user made in the TOFU overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TofuChoice {
    TrustNew,
    StayPinned,
    Abort,
}

/// Action emitted by `TofuOverlay::handle_key`.
#[derive(Debug, PartialEq, Eq)]
pub enum TofuAction {
    /// User confirmed "Trust new" — caller should update the pin store.
    TrustNew,
    /// User confirmed "Stay pinned" — caller should abort the operation.
    StayPinned,
    /// User aborted — caller should close the overlay with no pin change.
    Abort,
    /// Selection changed but no confirmation yet; caller should redraw.
    Nav,
    /// Key absorbed but nothing meaningful happened.
    None,
}

/// Snapshot of the mismatch data, passed into the overlay and used by `app.rs`
/// to populate `App::tofu_overlay` from `AccountState::tofu_mismatch`.
#[derive(Debug, Clone)]
pub struct TofuMismatchInfo {
    pub host: String,
    pub pinned: String,
    pub presented: String,
}

// ── Overlay widget ─────────────────────────────────────────────────────────

pub struct TofuOverlay {
    host: String,
    pinned: String,
    presented: String,
    /// Currently highlighted button. Default is `Abort` (fail-closed).
    selected: TofuChoice,
}

impl TofuOverlay {
    pub fn new(host: String, pinned: String, presented: String) -> Self {
        Self {
            host,
            pinned,
            presented,
            selected: TofuChoice::Abort,
        }
    }

    /// The host whose fingerprint changed.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The fingerprint the server is currently presenting.
    pub fn presented(&self) -> &str {
        &self.presented
    }

    /// Draw the overlay centred on `area`. ~70 cols × 14 rows, red border.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let w = 72u16.min(area.width.saturating_sub(2));
        let h = 14u16.min(area.height.saturating_sub(2));
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let overlay = Rect::new(x, y, w, h);

        let block = Block::default()
            .title(format!(" TLS fingerprint changed for {} ", self.host))
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Black).fg(Color::Red));

        let inner = block.inner(overlay);
        f.render_widget(Clear, overlay);
        f.render_widget(block, overlay);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Layout: heading (1) + blank (1) + fingerprints (4) + blank (1) +
        // buttons (1) + blank (1) + footer (1) = 10 rows. If inner is shorter
        // some rows are simply clipped by ratatui.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // heading
                Constraint::Length(1), // blank
                Constraint::Length(4), // fingerprint side-by-side
                Constraint::Length(1), // blank
                Constraint::Length(1), // buttons
                Constraint::Length(1), // blank
                Constraint::Length(1), // footer
            ])
            .split(inner);

        // Heading (already in the border title, but render a short cue inside).
        let heading = Paragraph::new(Line::from(Span::styled(
            "Review the fingerprints below before trusting.",
            Style::default().fg(Color::Yellow),
        )))
        .alignment(Alignment::Center);
        f.render_widget(heading, chunks[0]);

        // Fingerprint columns: pinned (left) vs presented (right).
        let fp_halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[2]);

        let pinned_lines =
            fingerprint_lines("Pinned (since first connect):", &self.pinned, Color::Yellow);
        let presented_lines =
            fingerprint_lines("Server presented now:", &self.presented, Color::Red);

        f.render_widget(Paragraph::new(pinned_lines), fp_halves[0]);
        f.render_widget(Paragraph::new(presented_lines), fp_halves[1]);

        // Buttons row.
        let btn_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(34),
                Constraint::Percentage(33),
            ])
            .split(chunks[4]);

        let btn = |label: &str, choice: TofuChoice| {
            let selected = self.selected == choice;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Paragraph::new(Line::from(Span::styled(format!("[ {label} ]"), style)))
                .alignment(Alignment::Center)
        };

        f.render_widget(btn("Trust new", TofuChoice::TrustNew), btn_chunks[0]);
        f.render_widget(btn("Stay pinned", TofuChoice::StayPinned), btn_chunks[1]);
        f.render_widget(btn("Abort", TofuChoice::Abort), btn_chunks[2]);

        // Footer hint.
        let footer = Paragraph::new(Line::from(Span::styled(
            "h/l move  •  Enter confirm  •  t trust  •  s stay  •  a/Esc abort",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center);
        f.render_widget(footer, chunks[6]);
    }

    /// Route a keypress to the overlay. Returns what the caller should do next.
    pub fn handle_key(&mut self, k: KeyEvent) -> TofuAction {
        match k.code {
            // Cycle left.
            KeyCode::Char('h') | KeyCode::Left => {
                self.selected = cycle_left(self.selected);
                TofuAction::Nav
            }
            // Cycle right / Tab.
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => {
                self.selected = cycle_right(self.selected);
                TofuAction::Nav
            }
            // Confirm selection.
            KeyCode::Enter => match self.selected {
                TofuChoice::TrustNew => TofuAction::TrustNew,
                TofuChoice::StayPinned => TofuAction::StayPinned,
                TofuChoice::Abort => TofuAction::Abort,
            },
            // Escape / q / a → always Abort.
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('a') => TofuAction::Abort,
            // Shortcut: t → TrustNew.
            KeyCode::Char('t') => TofuAction::TrustNew,
            // Shortcut: s → StayPinned.
            KeyCode::Char('s') => TofuAction::StayPinned,
            _ => TofuAction::None,
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Build the lines for one fingerprint column. The hex string is wrapped at
/// 32 characters per line for readability.
fn fingerprint_lines(label: &str, fp: &str, color: Color) -> Vec<Line<'static>> {
    let label_line = Line::from(Span::styled(
        label.to_string(),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ));
    let chunks: Vec<&str> = fp
        .as_bytes()
        .chunks(32)
        .map(|c| std::str::from_utf8(c).unwrap_or("?"))
        .collect();
    let mut lines = vec![label_line];
    for chunk in chunks {
        lines.push(Line::from(Span::styled(
            chunk.to_string(),
            Style::default().fg(color),
        )));
    }
    lines
}

fn cycle_right(c: TofuChoice) -> TofuChoice {
    match c {
        TofuChoice::TrustNew => TofuChoice::StayPinned,
        TofuChoice::StayPinned => TofuChoice::Abort,
        TofuChoice::Abort => TofuChoice::TrustNew,
    }
}

fn cycle_left(c: TofuChoice) -> TofuChoice {
    match c {
        TofuChoice::TrustNew => TofuChoice::Abort,
        TofuChoice::StayPinned => TofuChoice::TrustNew,
        TofuChoice::Abort => TofuChoice::StayPinned,
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

    fn overlay() -> TofuOverlay {
        TofuOverlay::new(
            "electrum.example.com:50002".into(),
            "aabbccdd".repeat(8),
            "11223344".repeat(8),
        )
    }

    #[test]
    fn default_selection_is_abort() {
        let o = overlay();
        assert_eq!(o.selected, TofuChoice::Abort);
    }

    #[test]
    fn l_cycles_right_from_abort_to_trust_new() {
        let mut o = overlay();
        // Abort → TrustNew
        assert_eq!(o.handle_key(press(KeyCode::Char('l'))), TofuAction::Nav);
        assert_eq!(o.selected, TofuChoice::TrustNew);
    }

    #[test]
    fn h_cycles_left_from_abort_to_stay_pinned() {
        let mut o = overlay();
        // Abort → StayPinned
        assert_eq!(o.handle_key(press(KeyCode::Char('h'))), TofuAction::Nav);
        assert_eq!(o.selected, TofuChoice::StayPinned);
    }

    #[test]
    fn tab_cycles_right() {
        let mut o = overlay();
        // Abort → TrustNew
        assert_eq!(o.handle_key(press(KeyCode::Tab)), TofuAction::Nav);
        assert_eq!(o.selected, TofuChoice::TrustNew);
        // TrustNew → StayPinned
        assert_eq!(o.handle_key(press(KeyCode::Tab)), TofuAction::Nav);
        assert_eq!(o.selected, TofuChoice::StayPinned);
        // StayPinned → Abort
        assert_eq!(o.handle_key(press(KeyCode::Tab)), TofuAction::Nav);
        assert_eq!(o.selected, TofuChoice::Abort);
    }

    #[test]
    fn enter_on_trust_new_emits_trust_new() {
        let mut o = overlay();
        // Move to TrustNew.
        o.handle_key(press(KeyCode::Char('l')));
        assert_eq!(o.selected, TofuChoice::TrustNew);
        assert_eq!(o.handle_key(press(KeyCode::Enter)), TofuAction::TrustNew);
    }

    #[test]
    fn enter_on_default_abort_emits_abort() {
        let mut o = overlay();
        assert_eq!(o.selected, TofuChoice::Abort);
        assert_eq!(o.handle_key(press(KeyCode::Enter)), TofuAction::Abort);
    }

    #[test]
    fn esc_always_aborts_regardless_of_selection() {
        let mut o = overlay();
        o.handle_key(press(KeyCode::Char('l'))); // TrustNew
        assert_eq!(o.handle_key(press(KeyCode::Esc)), TofuAction::Abort);
    }

    #[test]
    fn shortcut_t_emits_trust_new() {
        let mut o = overlay();
        assert_eq!(
            o.handle_key(press(KeyCode::Char('t'))),
            TofuAction::TrustNew
        );
    }

    #[test]
    fn shortcut_s_emits_stay_pinned() {
        let mut o = overlay();
        assert_eq!(
            o.handle_key(press(KeyCode::Char('s'))),
            TofuAction::StayPinned
        );
    }

    #[test]
    fn shortcut_a_emits_abort() {
        let mut o = overlay();
        assert_eq!(o.handle_key(press(KeyCode::Char('a'))), TofuAction::Abort);
    }

    #[test]
    fn enter_on_stay_pinned_emits_stay_pinned() {
        let mut o = overlay();
        o.handle_key(press(KeyCode::Char('h'))); // Abort → StayPinned
        assert_eq!(o.selected, TofuChoice::StayPinned);
        assert_eq!(o.handle_key(press(KeyCode::Enter)), TofuAction::StayPinned);
    }
}
