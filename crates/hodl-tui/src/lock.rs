//! Lock-screen UI: password entry, unlock, idle auto-lock.
//!
//! The password field is a single-field [`hjkl_form::Form`] backed by a
//! [`hjkl_form::TextFieldEditor`]. Vim-modal input (Normal / Insert) routes
//! through the form FSM; control keys (Ctrl-C/D, Esc-quit, manual lock) are
//! intercepted before forwarding. The field text is rendered as `*` characters
//! — hjkl-form has no native masked type, so we read the char count from the
//! buffer and render a masked paragraph ourselves (option b from the plan).

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use hjkl_form::{Field, FieldMeta, Form, FormMode, Input, TextFieldEditor};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use zeroize::Zeroize;

use hodl_wallet::{UnlockedWallet, Wallet};

/// What screen we're currently rendering.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Mode {
    Locked,
    Unlocked,
}

/// Outcome reported back to the caller.
#[derive(Debug)]
pub enum Outcome {
    Quit,
    AutoLocked,
}

/// Run the event loop until the user quits or the wallet auto-locks.
pub fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    wallet: &Wallet,
    idle_timeout: Duration,
) -> Result<Outcome>
where
    B::Error: Send + Sync + 'static,
{
    let mut state = LockState::new();

    loop {
        terminal.draw(|f| draw(f, &mut state))?;

        // Idle check before polling.
        if state.mode == Mode::Unlocked && state.last_activity.elapsed() >= idle_timeout {
            state.unlocked = None;
            state.mode = Mode::Locked;
            state.message = Some(("auto-locked (idle)".into(), MessageKind::Info));
            state.last_activity = Instant::now();
            continue;
        }

        // Cap poll wait so we re-check the idle clock.
        let wait = Duration::from_millis(250);
        if !event::poll(wait)? {
            continue;
        }

        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                state.last_activity = Instant::now();
                match handle_key(&mut state, wallet, k) {
                    Some(o) => return Ok(o),
                    None => continue,
                }
            }
            Event::Resize(_, _) => continue,
            _ => continue,
        }
    }
}

#[derive(Debug, Copy, Clone)]
enum MessageKind {
    Info,
    Error,
}

pub(crate) struct LockState {
    pub(crate) mode: Mode,
    /// Single-field form — the TextFieldEditor at index 0 holds the password.
    form: Form,
    message: Option<(String, MessageKind)>,
    unlocked: Option<UnlockedWallet>,
    last_activity: Instant,
}

fn make_password_form() -> Form {
    Form::new().with_field(Field::SingleLineText(TextFieldEditor::with_meta(
        FieldMeta::new("password"),
        1,
    )))
}

impl LockState {
    pub(crate) fn new() -> Self {
        Self {
            mode: Mode::Locked,
            form: make_password_form(),
            message: None,
            unlocked: None,
            last_activity: Instant::now(),
        }
    }

    /// Extract the password bytes, wipe the field, then attempt unlock.
    fn submit_password(&mut self, wallet: &Wallet) {
        let pw_text = match self.form.fields.first() {
            Some(Field::SingleLineText(f)) => f.text(),
            _ => String::new(),
        };

        let mut pw_bytes: Vec<u8> = pw_text.into_bytes();

        match wallet.unlock(&pw_bytes) {
            Ok(u) => {
                self.unlocked = Some(u);
                self.mode = Mode::Unlocked;
                self.message = Some(("unlocked — M1 done".into(), MessageKind::Info));
            }
            Err(e) => {
                self.message = Some((format!("{e}"), MessageKind::Error));
            }
        }

        pw_bytes.zeroize();
        self.wipe_field();
    }

    /// Rebuild the form field to an empty state, zeroizing any backing memory.
    fn wipe_field(&mut self) {
        // set_text("") rebuilds the inner Editor with a fresh empty Buffer,
        // dropping the previous one. The previous String allocation is freed by
        // Rust's normal drop; the Buffer rope's internal Vec is also dropped.
        if let Some(Field::SingleLineText(f)) = self.form.fields.first_mut() {
            f.set_text("");
        }
    }
}

impl Drop for LockState {
    fn drop(&mut self) {
        self.wipe_field();
        // unlocked: ZeroizeOnDrop runs automatically.
    }
}

fn handle_key(
    state: &mut LockState,
    wallet: &Wallet,
    k: crossterm::event::KeyEvent,
) -> Option<Outcome> {
    // Ctrl-C / Ctrl-D quit from anywhere.
    if k.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
    {
        return Some(Outcome::Quit);
    }

    match state.mode {
        Mode::Locked => {
            // Enter submits regardless of form mode.
            if k.code == KeyCode::Enter {
                state.submit_password(wallet);
                return None;
            }

            // Esc in Normal mode quits; in Insert mode let the form handle it
            // (it will return to Normal).
            if k.code == KeyCode::Esc && state.form.mode == FormMode::Normal {
                return Some(Outcome::Quit);
            }

            state.form.handle_input(Input::from(k));
            None
        }
        Mode::Unlocked => {
            if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                return Some(Outcome::Quit);
            }
            if k.code == KeyCode::Char('l') {
                state.unlocked = None;
                state.mode = Mode::Locked;
                state.message = Some(("locked".into(), MessageKind::Info));
            }
            None
        }
    }
}

fn draw(f: &mut ratatui::Frame, state: &mut LockState) {
    let area = f.area();
    match state.mode {
        Mode::Locked => draw_locked(f, area, state),
        Mode::Unlocked => draw_unlocked(f, area, state),
    }
}

fn draw_locked(f: &mut ratatui::Frame, area: Rect, state: &mut LockState) {
    let block = Block::default()
        .title(" hodl • LOCKED ")
        .borders(Borders::ALL)
        .style(Style::default());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(inner);

    let banner = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "enter vault password to unlock",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ])
    .alignment(Alignment::Center);
    f.render_widget(banner, chunks[0]);

    // Render the password field masked. The form drives vim-modal input;
    // we read char count and display asterisks instead of plain text.
    let char_count = match state.form.fields.first() {
        Some(Field::SingleLineText(field)) => field.text().chars().count(),
        _ => 0,
    };
    let mode_indicator = if state.form.mode == FormMode::Insert {
        "-- INSERT --  "
    } else {
        ""
    };
    let masked = "*".repeat(char_count);
    let prompt = Paragraph::new(Line::from(vec![
        Span::styled(mode_indicator, Style::default().fg(Color::Yellow)),
        Span::raw("password: "),
        Span::raw(masked),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(prompt, chunks[1]);

    if let Some((msg, kind)) = &state.message {
        let style = match kind {
            MessageKind::Info => Style::default().fg(Color::Cyan),
            MessageKind::Error => Style::default().fg(Color::Red),
        };
        let p = Paragraph::new(Line::from(Span::styled(msg.clone(), style)))
            .alignment(Alignment::Center);
        f.render_widget(p, chunks[2]);
    }

    let hint = Paragraph::new(Line::from(Span::styled(
        "i to type • enter to submit • esc to quit",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(hint, chunks[3]);
}

fn draw_unlocked(f: &mut ratatui::Frame, area: Rect, state: &LockState) {
    let block = Block::default()
        .title(" hodl • UNLOCKED ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "M1 placeholder — wallet unlocked",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "wallet: {}",
            state
                .unlocked
                .as_ref()
                .map(|u| u.name.as_str())
                .unwrap_or("?")
        )),
        Line::from(""),
        Line::from(Span::styled(
            "press l to lock • q or esc to quit",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let body = Paragraph::new(lines).alignment(Alignment::Center);
    f.render_widget(body, inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use hjkl_form::Key;
    use hodl_wallet::{Wallet, vault::KdfParams};
    use tempfile::TempDir;

    fn test_wallet(dir: &TempDir) -> Wallet {
        Wallet::create(
            dir.path(),
            "test",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "",
            b"correct-password",
            KdfParams::testing(),
        )
        .unwrap()
    }

    fn ki(c: char) -> Input {
        Input {
            key: Key::Char(c),
            ..Input::default()
        }
    }

    fn special(k: Key) -> Input {
        Input {
            key: k,
            ..Input::default()
        }
    }

    /// Type a string into the form via Insert mode.
    fn type_password(state: &mut LockState, password: &str) {
        state.form.handle_input(ki('i')); // enter Insert
        for c in password.chars() {
            state.form.handle_input(ki(c));
        }
        state.form.handle_input(special(Key::Esc)); // back to Normal
    }

    #[test]
    fn wrong_password_stays_locked() {
        let dir = TempDir::new().unwrap();
        let wallet = test_wallet(&dir);
        let mut state = LockState::new();

        type_password(&mut state, "wrong-password");
        state.submit_password(&wallet);

        assert_eq!(state.mode, Mode::Locked);
        assert!(matches!(&state.message, Some((_, MessageKind::Error))));
    }

    #[test]
    fn correct_password_unlocks() {
        let dir = TempDir::new().unwrap();
        let wallet = test_wallet(&dir);
        let mut state = LockState::new();

        type_password(&mut state, "correct-password");
        state.submit_password(&wallet);

        assert_eq!(state.mode, Mode::Unlocked);
        assert!(state.unlocked.is_some());
    }

    #[test]
    fn field_cleared_after_failed_unlock() {
        let dir = TempDir::new().unwrap();
        let wallet = test_wallet(&dir);
        let mut state = LockState::new();

        type_password(&mut state, "wrong-password");
        state.submit_password(&wallet);

        let text = match state.form.fields.first() {
            Some(Field::SingleLineText(f)) => f.text(),
            _ => panic!("expected text field"),
        };
        assert!(text.is_empty(), "field must be empty after failed unlock");
    }
}
