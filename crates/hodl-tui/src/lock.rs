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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
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

    loop {
        terminal.draw(|f| draw_locked(f, f.area(), &mut state))?;

        if state.last_activity.elapsed() >= idle_timeout {
            state.last_activity = Instant::now();
            state.message = Some(("auto-locked (idle)".into(), MessageKind::Info));
            continue;
        }

        let wait = Duration::from_millis(250);
        if !event::poll(wait)? {
            continue;
        }

        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                state.last_activity = Instant::now();
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
        }
    }

    /// Extract the password bytes, wipe the field, then attempt unlock.
    /// Returns the `UnlockedWallet` on success, or populates the error message.
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

    /// Rebuild the form field to an empty state, zeroizing any backing memory.
    fn wipe_field(&mut self) {
        if let Some(Field::SingleLineText(f)) = self.form.fields.first_mut() {
            f.set_text("");
        }
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
        if let Some(unlocked) = state.submit_password(wallet) {
            return Some(Outcome::Unlocked(unlocked));
        }
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
