//! Lock-screen UI: password entry, unlock, idle auto-lock.
//!
//! The password field is a single-field [`hjkl_form::Form`] backed by a
//! [`hjkl_form::TextFieldEditor`]. Vim-modal input (Normal / Insert) routes
//! through the form FSM; control keys (Ctrl-C/D, Esc-quit, manual lock) are
//! intercepted before forwarding. The field text is rendered as `*` characters
//! — hjkl-form has no native masked type, so we read the char count from the
//! buffer and render a masked paragraph ourselves.
//!
//! `w` opens a wallet-switcher overlay (`hjkl-picker`) listing all `.vault`
//! files discovered in the data root. Selecting one emits `Outcome::SwitchWallet`.
//!
//! ## Unlock flow
//!
//! When the user submits the password, the argon2id KDF (~1–2 s on production
//! params) is run **off the UI thread** via `std::thread::spawn`. A
//! `std::sync::mpsc::channel` carries the result back. While a decrypt attempt
//! is in flight, `pending_unlock` holds the `Receiver` end. The event loop
//! polls it each tick with `try_recv` and renders an animated braille spinner
//! so the user knows the app is working.
//!
//! Key presses are ignored while `pending_unlock` is `Some` — argon2id is
//! uninterruptible, so cancellation is not meaningful; ignoring keys prevents
//! queueing a second submit attempt on top of the in-flight one.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use hjkl_form::{Field, FieldMeta, Form, FormMode, Input, TextFieldEditor};
use hjkl_picker::{PickerAction, PickerEvent, PickerLogic};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use zeroize::Zeroize;

use hodl_wallet::{UnlockedWallet, Wallet, storage::list_wallets};

use crate::help::{HelpAction, HelpOverlay};
use crate::spinner::Spinner;

/// Outcome reported back to the caller.
#[derive(Debug)]
pub enum Outcome {
    Quit,
    AutoLocked,
    Unlocked(UnlockedWallet),
    /// User selected a different wallet; re-enter lock screen for that wallet.
    SwitchWallet(String),
}

/// Run the event loop until the user quits, wallet auto-locks, or unlocks.
pub fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    wallet: &Wallet,
    idle_timeout: Duration,
    data_root: &Path,
) -> Result<Outcome>
where
    B::Error: Send + Sync + 'static,
{
    let mut state = LockState::new();
    let mut help_overlay: Option<HelpOverlay> = None;

    loop {
        // ── Poll pending unlock ───────────────────────────────────────────
        if let Some(rx) = &state.pending_unlock {
            match rx.try_recv() {
                Ok(Ok(unlocked)) => {
                    state.pending_unlock = None;
                    return Ok(Outcome::Unlocked(unlocked));
                }
                Ok(Err(e)) => {
                    state.pending_unlock = None;
                    state.message = Some((format!("{e}"), MessageKind::Error));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Still computing — advance spinner and redraw below.
                    state.spinner.tick();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.pending_unlock = None;
                    state.message = Some((
                        "unlock thread panicked — try again".into(),
                        MessageKind::Error,
                    ));
                }
            }
        }

        // ── Render ────────────────────────────────────────────────────────
        terminal.draw(|f| {
            let area = f.area();
            draw_locked(f, area, &mut state);
            if let Some(ref mut overlay) = help_overlay {
                overlay.draw(f, area);
            }
        })?;

        // ── Idle auto-lock ────────────────────────────────────────────────
        if state.last_activity.elapsed() >= idle_timeout {
            state.last_activity = Instant::now();
            state.message = Some(("auto-locked (idle)".into(), MessageKind::Info));
            continue;
        }

        // ── Event polling ─────────────────────────────────────────────────
        // Use a short 80 ms timeout while the spinner is running so animation
        // stays smooth; fall back to 250 ms when idle to save CPU.
        let wait = if state.pending_unlock.is_some() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        };

        if !event::poll(wait)? {
            continue;
        }

        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                // Ignore ALL keypresses while an unlock is in flight.
                // argon2id is uninterruptible; queuing another submit would
                // cause a double-attempt the moment the first finishes.
                if state.pending_unlock.is_some() {
                    continue;
                }

                state.last_activity = Instant::now();

                // Overlay absorbs all keys when open.
                if let Some(ref mut overlay) = help_overlay {
                    if overlay.handle_key(k) == HelpAction::Close {
                        help_overlay = None;
                    }
                    continue;
                }

                // `?` in Normal mode opens the help overlay.
                if k.code == KeyCode::Char('?') && state.form.mode == FormMode::Normal {
                    help_overlay = Some(HelpOverlay::new("Lock", state.help_lines()));
                    continue;
                }

                match handle_key(&mut state, wallet, k, data_root) {
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
    /// Single-field form — the TextFieldEditor at index 0 holds the password.
    form: Form,
    message: Option<(String, MessageKind)>,
    pub(crate) last_activity: Instant,
    /// Wallet switcher picker overlay. `None` when closed.
    picker: Option<hjkl_picker::Picker>,
    /// In-flight unlock attempt channel. `Some` while argon2id is running.
    /// All key input is ignored while this is `Some` because the KDF is
    /// uninterruptible — accepting more input would only queue a race.
    pending_unlock: Option<Receiver<Result<UnlockedWallet>>>,
    /// Animated spinner shown while decrypting.
    spinner: Spinner,
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
            form: make_password_form(),
            message: None,
            last_activity: Instant::now(),
            picker: None,
            pending_unlock: None,
            spinner: Spinner::new(),
        }
    }

    /// Keybind reference for the contextual help overlay.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        vec![
            ("i".into(), "Enter insert mode (type password)".into()),
            ("Esc".into(), "Return to Normal mode / quit".into()),
            ("Enter".into(), "Submit password".into()),
            ("w".into(), "Open wallet switcher".into()),
            ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
            ("?".into(), "Show this help".into()),
        ]
    }

    /// Spawn the unlock attempt on a background thread and store the receiver
    /// so the event loop can poll for the result. The password bytes are moved
    /// into the thread; the field is wiped immediately after extraction.
    fn start_unlock(&mut self, wallet: &Wallet) {
        let pw_text = match self.form.fields.first() {
            Some(Field::SingleLineText(f)) => f.text(),
            _ => String::new(),
        };
        let pw_bytes: Vec<u8> = pw_text.into_bytes();
        self.wipe_field();

        // Clone the Wallet handle — it only holds a name + PathBuf, no secrets.
        let wallet_clone = wallet.clone();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = wallet_clone.unlock(&pw_bytes);
            // Safety: pw_bytes is zeroized here, before the thread exits.
            let mut pw = pw_bytes;
            pw.zeroize();
            // If the receiver has already been dropped (shouldn't happen), ignore.
            let _ = tx.send(result.map_err(anyhow::Error::from));
        });

        self.pending_unlock = Some(rx);
        self.spinner = Spinner::new();
        self.message = None;
    }

    /// Rebuild the form field to an empty state, zeroizing any backing memory.
    fn wipe_field(&mut self) {
        if let Some(Field::SingleLineText(f)) = self.form.fields.first_mut() {
            f.set_text("");
        }
    }

    /// Synchronous unlock — used only by unit tests where threading is overkill.
    #[cfg(test)]
    fn submit_password(&mut self, wallet: &Wallet) -> Option<UnlockedWallet> {
        let pw_text = match self.form.fields.first() {
            Some(Field::SingleLineText(f)) => f.text(),
            _ => String::new(),
        };
        let mut pw_bytes: Vec<u8> = pw_text.into_bytes();

        let result = match wallet.unlock(&pw_bytes) {
            Ok(u) => {
                self.message = Some(("unlocked".into(), MessageKind::Info));
                Some(u)
            }
            Err(e) => {
                self.message = Some((format!("{e}"), MessageKind::Error));
                None
            }
        };

        pw_bytes.zeroize();
        self.wipe_field();
        result
    }
}

impl Drop for LockState {
    fn drop(&mut self) {
        self.wipe_field();
    }
}

fn handle_key(
    state: &mut LockState,
    wallet: &Wallet,
    k: crossterm::event::KeyEvent,
    data_root: &Path,
) -> Option<Outcome> {
    if k.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
    {
        return Some(Outcome::Quit);
    }

    // Wallet switcher picker absorbs keys when open.
    if let Some(picker) = &mut state.picker {
        match picker.handle_key(k) {
            PickerEvent::Cancel => {
                state.picker = None;
            }
            PickerEvent::Select(PickerAction::None) | PickerEvent::None => {
                picker.refresh();
            }
            PickerEvent::Select(PickerAction::OpenPath(p)) => {
                state.picker = None;
                // The path carries the wallet name as a plain component.
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                return Some(Outcome::SwitchWallet(name));
            }
            PickerEvent::Select(_) => {
                state.picker = None;
            }
        }
        return None;
    }

    if k.code == KeyCode::Enter {
        state.start_unlock(wallet);
        return None;
    }

    if k.code == KeyCode::Esc && state.form.mode == FormMode::Normal {
        return Some(Outcome::Quit);
    }

    // `w` opens the wallet switcher picker.
    if k.code == KeyCode::Char('w') && state.form.mode == FormMode::Normal {
        let names = list_wallets(data_root).unwrap_or_default();
        if names.is_empty() {
            state.message = Some(("no other wallets found".into(), MessageKind::Info));
        } else {
            let source = WalletPickerSource::new(names);
            state.picker = Some(hjkl_picker::Picker::new(Box::new(source)));
        }
        return None;
    }

    state.form.handle_input(Input::from(k));
    None
}

// ── Wallet picker source ───────────────────────────────────────────────────

struct WalletPickerSource {
    names: Vec<String>,
}

impl WalletPickerSource {
    fn new(names: Vec<String>) -> Self {
        Self { names }
    }
}

impl PickerLogic for WalletPickerSource {
    fn title(&self) -> &str {
        "wallets"
    }

    fn item_count(&self) -> usize {
        self.names.len()
    }

    fn label(&self, idx: usize) -> String {
        self.names.get(idx).cloned().unwrap_or_default()
    }

    fn match_text(&self, idx: usize) -> String {
        self.label(idx)
    }

    fn has_preview(&self) -> bool {
        false
    }

    fn select(&self, idx: usize) -> PickerAction {
        // Encode the wallet name as an `OpenPath` so we can retrieve it.
        let name = self.names.get(idx).cloned().unwrap_or_default();
        PickerAction::OpenPath(PathBuf::from(name))
    }

    fn enumerate(
        &mut self,
        _query: Option<&str>,
        _cancel: Arc<AtomicBool>,
    ) -> Option<JoinHandle<()>> {
        None
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

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

    // Show spinner while decrypting, otherwise show any status message.
    if state.pending_unlock.is_some() {
        state.spinner.draw(f, chunks[2], "decrypting…", Color::Cyan);
    } else if let Some((msg, kind)) = &state.message {
        let style = match kind {
            MessageKind::Info => Style::default().fg(Color::Cyan),
            MessageKind::Error => Style::default().fg(Color::Red),
        };
        let p = Paragraph::new(Line::from(Span::styled(msg.clone(), style)))
            .alignment(Alignment::Center);
        f.render_widget(p, chunks[2]);
    }

    let hint = Paragraph::new(Line::from(Span::styled(
        "i to type • enter to submit • w wallets • esc to quit",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(hint, chunks[3]);

    // Picker overlay drawn on top.
    if state.picker.is_some() {
        draw_wallet_picker_overlay(f, area, state);
    }
}

fn draw_wallet_picker_overlay(f: &mut ratatui::Frame, area: Rect, state: &mut LockState) {
    let Some(picker) = &mut state.picker else {
        return;
    };
    picker.refresh();

    let w = area.width.min(50);
    let h = area.height.min(14);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let overlay = Rect::new(x, y, w, h);

    let block = Block::default()
        .title(format!(" {} ", picker.title()))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(overlay);
    f.render_widget(Clear, overlay);
    f.render_widget(block, overlay);

    let entries = picker.visible_entries();
    let selected = picker.selected;

    let rows: Vec<Line> = entries
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            if i == selected {
                Line::from(Span::styled(
                    format!("> {label}"),
                    Style::default().bg(Color::DarkGray),
                ))
            } else {
                Line::from(Span::raw(format!("  {label}")))
            }
        })
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);

    let query = picker.query.text();
    let query_para = Paragraph::new(query)
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White));
    f.render_widget(query_para, chunks[0]);
    f.render_widget(Paragraph::new(rows), chunks[1]);
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

    fn type_password(state: &mut LockState, password: &str) {
        state.form.handle_input(ki('i'));
        for c in password.chars() {
            state.form.handle_input(ki(c));
        }
        state.form.handle_input(special(Key::Esc));
    }

    #[test]
    fn wrong_password_returns_none() {
        let dir = TempDir::new().unwrap();
        let wallet = test_wallet(&dir);
        let mut state = LockState::new();

        type_password(&mut state, "wrong-password");
        let result = state.submit_password(&wallet);

        assert!(result.is_none());
        assert!(matches!(&state.message, Some((_, MessageKind::Error))));
    }

    #[test]
    fn correct_password_returns_unlocked() {
        let dir = TempDir::new().unwrap();
        let wallet = test_wallet(&dir);
        let mut state = LockState::new();

        type_password(&mut state, "correct-password");
        let result = state.submit_password(&wallet);

        assert!(result.is_some());
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
