//! Reusable braille spinner widget.
//!
//! Shared between the lock screen (decrypting…), accounts screen (loading…),
//! and send screen (building / broadcasting…). Each screen that has an
//! in-flight background operation stores a `Spinner` and calls `tick()` each
//! time the `TryRecvError::Empty` arm fires; `draw()` renders the label +
//! current frame centred in the provided area.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Braille animation frames — cycle at ~80 ms per frame.
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// Animated braille spinner.
pub struct Spinner {
    frame: usize,
}

impl Spinner {
    /// Create a new spinner starting at frame 0.
    pub fn new() -> Self {
        Self { frame: 0 }
    }

    /// Advance by one frame (wraps around).
    pub fn tick(&mut self) {
        self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
    }

    /// Current frame character.
    pub fn current(&self) -> &'static str {
        SPINNER_FRAMES[self.frame]
    }

    /// Render `<label>  <frame>` centred in `area`, in `color`.
    pub fn draw(&self, f: &mut Frame, area: Rect, label: &str, color: Color) {
        let text = format!("{label}  {}", self.current());
        let line = Line::from(Span::styled(text, Style::default().fg(color)));
        let p = Paragraph::new(line).alignment(Alignment::Center);
        f.render_widget(p, area);
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_frame_is_zero() {
        let s = Spinner::new();
        assert_eq!(s.current(), SPINNER_FRAMES[0]);
    }

    #[test]
    fn tick_advances_frame() {
        let mut s = Spinner::new();
        s.tick();
        assert_eq!(s.current(), SPINNER_FRAMES[1]);
        s.tick();
        assert_eq!(s.current(), SPINNER_FRAMES[2]);
    }

    #[test]
    fn tick_wraps_at_end() {
        let mut s = Spinner::new();
        for _ in 0..SPINNER_FRAMES.len() {
            s.tick();
        }
        // After a full cycle we are back to frame 0.
        assert_eq!(s.current(), SPINNER_FRAMES[0]);
    }
}
