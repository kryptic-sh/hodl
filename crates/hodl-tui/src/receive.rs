//! Receive screen — QR code + address display + clipboard yank.
//!
//! QR rendering uses two-row half-block characters (`▀` / `▄` / ` ` / `█`):
//! each terminal character encodes two vertically-stacked QR pixels, so the
//! rendered QR is half the module count tall in terminal rows.
//!
//! `y` yanks the address to the system clipboard via
//! [`crate::clipboard::ClipboardHandle`] (OSC 52 fallback works over SSH).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hodl_core::Address;
use qrcode::QrCode;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::clipboard::ClipboardHandle;

/// Action emitted to the parent app loop.
#[derive(Debug)]
pub enum ReceiveAction {
    /// Return to the account screen.
    Back,
    /// Quit the application.
    Quit,
    /// Open the contextual help overlay.
    ShowHelp,
}

pub struct ReceiveState {
    pub address: Address,
    pub deriv_path: String,
    qr_lines: Vec<String>,
    yank_flash: Option<String>,
}

impl ReceiveState {
    pub fn new(address: Address, deriv_path: String) -> Self {
        let qr_lines = render_qr(address.as_str());
        Self {
            address,
            deriv_path,
            qr_lines,
            yank_flash: None,
        }
    }

    /// Keybind reference for the contextual help overlay.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        vec![
            ("y".into(), "Copy address to clipboard".into()),
            ("q / Esc".into(), "Back to accounts".into()),
            ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
            ("?".into(), "Show this help".into()),
        ]
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        clipboard: &ClipboardHandle,
    ) -> Option<ReceiveAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            return Some(ReceiveAction::Quit);
        }

        match key.code {
            KeyCode::Char('y') => {
                clipboard.yank(self.address.as_str());
                self.yank_flash = Some("address copied to clipboard".into());
            }
            KeyCode::Char('q') | KeyCode::Esc => return Some(ReceiveAction::Back),
            KeyCode::Char('?') => return Some(ReceiveAction::ShowHelp),
            _ => {}
        }

        None
    }
}

// ── QR rendering ──────────────────────────────────────────────────────────

/// Encode `data` as a QR code and convert it into terminal lines using
/// half-block characters. Each output row encodes two rows of QR modules.
pub fn render_qr(data: &str) -> Vec<String> {
    let code = match QrCode::new(data.as_bytes()) {
        Ok(c) => c,
        Err(_) => return vec!["[QR encode failed]".into()],
    };

    let width = code.width();
    // `to_colors()` returns a flat `Vec<qrcode::Color>` in row-major order.
    let pixels: Vec<bool> = code
        .into_colors()
        .iter()
        .map(|&c| c == qrcode::Color::Dark)
        .collect();

    // Pad to even number of rows.
    let rows = pixels.len().div_ceil(width);
    let padded_rows = rows + (rows % 2);

    // Add one-module quiet zone column on each side.
    let qz = 1usize;
    let out_cols = width + 2 * qz;

    let mut lines = Vec::new();

    // Top quiet zone (one output row = two blank QR rows).
    lines.push(" ".repeat(out_cols));

    let mut y = 0usize;
    while y + 1 < padded_rows {
        let mut line = String::new();
        // Left quiet zone.
        line.push(' ');
        for x in 0..width {
            let top = pixel(&pixels, y, x, width, rows);
            let bot = pixel(&pixels, y + 1, x, width, rows);
            line.push(half_block(top, bot));
        }
        // Right quiet zone.
        line.push(' ');
        lines.push(line);
        y += 2;
    }

    // Bottom quiet zone.
    lines.push(" ".repeat(out_cols));

    lines
}

fn pixel(pixels: &[bool], row: usize, col: usize, width: usize, height: usize) -> bool {
    if row >= height {
        return false;
    }
    pixels.get(row * width + col).copied().unwrap_or(false)
}

/// Half-block encoding: top pixel in upper half, bottom in lower half.
///
/// | top | bot | char |
/// |-----|-----|------|
/// | 0   | 0   | ' '  |
/// | 0   | 1   | '▄'  |
/// | 1   | 0   | '▀'  |
/// | 1   | 1   | '█'  |
pub fn half_block(top: bool, bot: bool) -> char {
    match (top, bot) {
        (false, false) => ' ',
        (false, true) => '\u{2584}', // ▄
        (true, false) => '\u{2580}', // ▀
        (true, true) => '\u{2588}',  // █
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, area: Rect, state: &mut ReceiveState) {
    let block = Block::default()
        .title(" hodl • Receive ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let qr_height = state.qr_lines.len() as u16;
    let info_height = 4u16; // address + path + flash + hint

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(qr_height),
            Constraint::Length(info_height),
            Constraint::Min(0),
        ])
        .split(inner);

    // QR block.
    let qr_lines: Vec<Line> = state
        .qr_lines
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect();
    let qr_para = Paragraph::new(qr_lines)
        .style(Style::default().fg(Color::White).bg(Color::Black))
        .alignment(Alignment::Center);
    f.render_widget(qr_para, chunks[0]);

    // Info rows.
    let addr_style = Style::default().fg(Color::White);
    let path_style = Style::default().fg(Color::DarkGray);
    let flash_style = Style::default().fg(Color::Green);
    let hint_style = Style::default().fg(Color::DarkGray);

    let info_lines = vec![
        Line::from(Span::styled(state.address.as_str().to_string(), addr_style)),
        Line::from(Span::styled(state.deriv_path.clone(), path_style)),
        Line::from(Span::styled(
            state.yank_flash.as_deref().unwrap_or("").to_string(),
            flash_style,
        )),
        Line::from(Span::styled("y copy • q / Esc back", hint_style)),
    ];

    let info_para = Paragraph::new(info_lines).alignment(Alignment::Center);
    f.render_widget(info_para, chunks[1]);
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_block_all_cases() {
        assert_eq!(half_block(false, false), ' ');
        assert_eq!(half_block(false, true), '\u{2584}');
        assert_eq!(half_block(true, false), '\u{2580}');
        assert_eq!(half_block(true, true), '\u{2588}');
    }

    #[test]
    fn qr_render_cell_count_for_known_input() {
        // A known short address — QR version depends on content length.
        let addr = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let lines = render_qr(addr);
        // QR must produce at least one row and each row must be non-empty.
        assert!(!lines.is_empty(), "expected non-empty QR");
        assert!(!lines[0].is_empty(), "first QR row must be non-empty");
        // All rows have the same char-count (the QR is square + quiet zone).
        let widths: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
        let first_w = widths[0];
        assert!(
            widths.iter().all(|&w| w == first_w),
            "QR rows have inconsistent widths: {widths:?}"
        );
    }

    #[test]
    fn qr_render_produces_half_block_chars() {
        let addr = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let lines = render_qr(addr);
        let all: String = lines.join("");
        let half_blocks = [' ', '\u{2584}', '\u{2580}', '\u{2588}'];
        for ch in all.chars() {
            assert!(
                half_blocks.contains(&ch),
                "unexpected char U+{:04X} in QR output",
                ch as u32
            );
        }
    }
}
