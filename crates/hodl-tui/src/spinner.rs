//! Reusable braille spinner widget.
//!
//! Shared between the lock screen (decrypting…), accounts screen (loading…),
//! and send screen (building / broadcasting…). Each screen that has an
//! in-flight background operation stores a `Spinner` and calls `tick()` each
//! time the `TryRecvError::Empty` arm fires; `draw()` renders the label +
//! current frame centred in the provided area.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Braille animation frames — cycle at ~80 ms per frame.
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// Minimum wall-clock gap between visible frame advances. Without this
/// floor, callers that tick on every event-loop iteration would speed up
/// the spinner whenever an event burst (e.g. mouse-move stream) makes
/// `event::poll` return early. Self-pacing keeps the apparent rate
/// constant regardless of caller cadence.
const TICK_INTERVAL: Duration = Duration::from_millis(80);

/// Animated braille spinner.
pub struct Spinner {
    frame: usize,
    last_tick: Instant,
}

impl Spinner {
    /// Create a new spinner starting at frame 0.
    pub fn new() -> Self {
        Self {
            frame: 0,
            // Set far enough in the past that the first `tick()` call
            // advances immediately; subsequent ticks observe the floor.
            last_tick: Instant::now()
                .checked_sub(TICK_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }

    /// Advance by one frame (wraps around) **iff** at least `TICK_INTERVAL`
    /// has elapsed since the last visible advance. Calling this on every
    /// event-loop iteration is intentionally cheap and idempotent within
    /// the interval.
    pub fn tick(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_tick) >= TICK_INTERVAL {
            self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
            self.last_tick = now;
        }
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
    fn tick_advances_frame_after_interval() {
        let mut s = Spinner::new();
        // First tick always lands (constructor sets last_tick in the past).
        s.tick();
        assert_eq!(s.current(), SPINNER_FRAMES[1]);
        // Second tick within interval is a no-op.
        s.tick();
        assert_eq!(s.current(), SPINNER_FRAMES[1]);
        // After waiting past the interval, the next tick advances.
        std::thread::sleep(TICK_INTERVAL + std::time::Duration::from_millis(5));
        s.tick();
        assert_eq!(s.current(), SPINNER_FRAMES[2]);
    }

    #[test]
    fn tick_wraps_at_end() {
        let mut s = Spinner::new();
        for _ in 0..SPINNER_FRAMES.len() {
            s.tick();
            std::thread::sleep(TICK_INTERVAL + std::time::Duration::from_millis(5));
        }
        // After a full cycle we are back to frame 0.
        assert_eq!(s.current(), SPINNER_FRAMES[0]);
    }

    #[test]
    fn rapid_ticks_do_not_speed_up_spinner() {
        let mut s = Spinner::new();
        s.tick(); // first tick advances to frame 1
        // Hammer tick() — none of these should advance because they all
        // land within TICK_INTERVAL of the last visible tick.
        for _ in 0..1000 {
            s.tick();
        }
        assert_eq!(s.current(), SPINNER_FRAMES[1]);
    }
}
