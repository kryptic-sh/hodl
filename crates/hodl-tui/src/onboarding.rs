//! Onboarding TUI: create-wallet and restore-wallet flows.
//!
//! Both flows are `hjkl_form::Form` dialogs. After a successful Create
//! submit the mnemonic is shown on a confirmation pane ("write this down");
//! pressing Enter there gates the final vault persist. All sensitive field
//! text is zeroized on submit regardless of outcome.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use hjkl_form::{
    Field, FieldMeta, Form, FormMode, Input, SelectField, SubmitField, TextFieldEditor, Validator,
};
use hjkl_ratatui::form::{FormPalette, draw_form};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use zeroize::Zeroize;

use hodl_wallet::Wallet;
use hodl_wallet::mnemonic::{self, WordCount};
use hodl_wallet::vault::KdfParams;

use crate::help::{HelpAction, HelpOverlay};

/// Result of onboarding — either a ready wallet or a user quit.
#[derive(Debug)]
pub enum OnboardingOutcome {
    Created(Wallet),
    Restored(Wallet),
    Quit,
}

/// Top-level onboarding mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingMode {
    Create,
    Restore,
}

// ── Field indices for Create form ──────────────────────────────────────────

const CREATE_PASSPHRASE: usize = 1;
const CREATE_PASSWORD: usize = 2;
const CREATE_CONFIRM: usize = 3;

// ── Field indices for Restore form ─────────────────────────────────────────

const RESTORE_MNEMONIC: usize = 1;
const RESTORE_PASSPHRASE: usize = 2;
const RESTORE_PASSWORD: usize = 3;
const RESTORE_CONFIRM: usize = 4;

// ── Validators ─────────────────────────────────────────────────────────────

/// Validator: 12 or 24 BIP-39 words, valid checksum.
pub fn validate_mnemonic(s: &str) -> Result<(), String> {
    let count = s.split_whitespace().count();
    if count != 12 && count != 24 {
        return Err(format!("expected 12 or 24 words, got {count}"));
    }
    mnemonic::parse(s).map(|_| ()).map_err(|e| e.to_string())
}

/// Validator: non-empty password.
pub fn validate_password_nonempty(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("password cannot be empty".into());
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Create flow
// ─────────────────────────────────────────────────────────────────────────────

fn make_create_form() -> Form {
    let mut password_field =
        TextFieldEditor::with_meta(FieldMeta::new("vault password").required(true), 1);
    password_field.validator = Some(mk_validator(validate_password_nonempty));

    Form::new()
        .with_title("Create wallet")
        .with_field(Field::Select(SelectField::new(
            FieldMeta::new("word count"),
            vec!["12".into(), "24".into()],
        )))
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("BIP-39 passphrase (optional)"),
            1,
        )))
        .with_field(Field::SingleLineText(password_field))
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("confirm password").required(true),
            1,
        )))
        .with_field(Field::Submit(SubmitField::new(FieldMeta::new(
            "Create wallet",
        ))))
}

fn mk_validator<F>(f: F) -> Validator
where
    F: Fn(&str) -> Result<(), String> + Send + 'static,
{
    Box::new(f)
}

// ─────────────────────────────────────────────────────────────────────────────
// Restore flow
// ─────────────────────────────────────────────────────────────────────────────

fn make_restore_form() -> Form {
    let mut mnemonic_field = TextFieldEditor::with_meta(
        FieldMeta::new("mnemonic phrase")
            .required(true)
            .placeholder("enter 12 or 24 BIP-39 words separated by spaces"),
        3,
    );
    mnemonic_field.validator = Some(mk_validator(validate_mnemonic));

    let mut password_field =
        TextFieldEditor::with_meta(FieldMeta::new("vault password").required(true), 1);
    password_field.validator = Some(mk_validator(validate_password_nonempty));

    Form::new()
        .with_title("Restore wallet")
        .with_field(Field::Select(SelectField::new(
            FieldMeta::new("word count"),
            vec!["12".into(), "24".into()],
        )))
        .with_field(Field::MultiLineText(mnemonic_field))
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("BIP-39 passphrase (optional)"),
            1,
        )))
        .with_field(Field::SingleLineText(password_field))
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("confirm password").required(true),
            1,
        )))
        .with_field(Field::Submit(SubmitField::new(
            SubmitField::new(FieldMeta::new("Restore wallet")).meta,
        )))
}

// ─────────────────────────────────────────────────────────────────────────────
// State machine
// ─────────────────────────────────────────────────────────────────────────────

enum Phase {
    Form,
    /// Mnemonic confirmation gate (create-only). Text = generated phrase.
    Confirm(String),
}

pub struct OnboardingState {
    mode: OnboardingMode,
    form: Form,
    phase: Phase,
    message: Option<(String, bool)>, // (text, is_error)
    data_root: PathBuf,
    wallet_name: String,
}

impl OnboardingState {
    pub fn new(mode: OnboardingMode, data_root: PathBuf, wallet_name: String) -> Self {
        let form = match mode {
            OnboardingMode::Create => make_create_form(),
            OnboardingMode::Restore => make_restore_form(),
        };
        Self {
            mode,
            form,
            phase: Phase::Form,
            message: None,
            data_root,
            wallet_name,
        }
    }

    /// Keybind reference for the contextual help overlay.
    /// Sub-state-aware: Confirm pane binds differ from Form binds.
    ///
    /// `F1` is used as the help trigger instead of `?` so `?` can still be
    /// typed in form fields while in Insert mode.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        match &self.phase {
            Phase::Confirm(_) => vec![
                ("Enter".into(), "Confirm mnemonic written — continue".into()),
                ("Esc".into(), "Back to form".into()),
                ("F1".into(), "Show this help".into()),
            ],
            Phase::Form => {
                if self.form.mode == FormMode::Insert {
                    vec![
                        ("Esc".into(), "Return to Normal mode".into()),
                        ("F1".into(), "Show this help".into()),
                    ]
                } else {
                    let mode_name = match self.mode {
                        OnboardingMode::Create => "Create wallet",
                        OnboardingMode::Restore => "Restore wallet",
                    };
                    vec![
                        (
                            "i".into(),
                            format!("Enter Insert mode to fill {}", mode_name),
                        ),
                        ("Tab / j / k".into(), "Move focus between fields".into()),
                        ("h / l".into(), "Cycle select options".into()),
                        ("Enter".into(), "Submit (on Submit field)".into()),
                        ("Esc".into(), "Quit onboarding".into()),
                        ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
                        ("F1".into(), "Show this help".into()),
                    ]
                }
            }
        }
    }

    fn field_text(&self, idx: usize) -> String {
        match self.form.fields.get(idx) {
            Some(Field::SingleLineText(f)) | Some(Field::MultiLineText(f)) => f.text(),
            _ => String::new(),
        }
    }

    fn word_count(&self) -> WordCount {
        match self.form.fields.first() {
            Some(Field::Select(s)) => {
                if s.selected() == Some("24") {
                    WordCount::TwentyFour
                } else {
                    WordCount::Twelve
                }
            }
            _ => WordCount::Twelve,
        }
    }

    /// Run the create submit: validate password match, generate mnemonic,
    /// move to the confirmation pane.
    fn try_create_submit(&mut self) -> Option<OnboardingOutcome> {
        let mut pw = self.field_text(CREATE_PASSWORD);
        let mut confirm = self.field_text(CREATE_CONFIRM);

        if pw.is_empty() {
            self.message = Some(("password cannot be empty".into(), true));
            pw.zeroize();
            confirm.zeroize();
            return None;
        }
        if pw != confirm {
            self.message = Some(("passwords do not match".into(), true));
            pw.zeroize();
            confirm.zeroize();
            return None;
        }
        pw.zeroize();
        confirm.zeroize();

        let wc = self.word_count();
        match mnemonic::generate(wc) {
            Ok(m) => {
                self.phase = Phase::Confirm(m.to_string());
                self.message = Some((
                    "WRITE THIS DOWN — then press Enter to continue".into(),
                    false,
                ));
                None
            }
            Err(e) => {
                self.message = Some((format!("generate failed: {e}"), true));
                None
            }
        }
    }

    /// Persist the wallet after confirmation gate.
    fn finalize_create(&mut self, phrase: &str) -> Option<OnboardingOutcome> {
        let mut passphrase = self.field_text(CREATE_PASSPHRASE);
        let mut pw = self.field_text(CREATE_PASSWORD);

        let result = Wallet::create(
            &self.data_root,
            &self.wallet_name,
            phrase,
            &passphrase,
            pw.as_bytes(),
            KdfParams::default(),
        );

        passphrase.zeroize();
        pw.zeroize();
        self.wipe_sensitive_fields_create();

        match result {
            Ok(w) => Some(OnboardingOutcome::Created(w)),
            Err(e) => {
                self.message = Some((format!("create failed: {e}"), true));
                self.phase = Phase::Form;
                None
            }
        }
    }

    fn try_restore_submit(&mut self) -> Option<OnboardingOutcome> {
        let phrase = self.field_text(RESTORE_MNEMONIC);
        let mut passphrase = self.field_text(RESTORE_PASSPHRASE);
        let mut pw = self.field_text(RESTORE_PASSWORD);
        let mut confirm = self.field_text(RESTORE_CONFIRM);

        if pw != confirm {
            self.message = Some(("passwords do not match".into(), true));
            passphrase.zeroize();
            pw.zeroize();
            confirm.zeroize();
            return None;
        }
        confirm.zeroize();

        if let Err(e) = validate_mnemonic(&phrase) {
            self.message = Some((e, true));
            passphrase.zeroize();
            pw.zeroize();
            return None;
        }

        let result = Wallet::create(
            &self.data_root,
            &self.wallet_name,
            &phrase,
            &passphrase,
            pw.as_bytes(),
            KdfParams::default(),
        );

        passphrase.zeroize();
        pw.zeroize();
        self.wipe_sensitive_fields_restore();

        match result {
            Ok(w) => Some(OnboardingOutcome::Restored(w)),
            Err(e) => {
                self.message = Some((format!("restore failed: {e}"), true));
                None
            }
        }
    }

    fn wipe_sensitive_fields_create(&mut self) {
        for idx in [CREATE_PASSPHRASE, CREATE_PASSWORD, CREATE_CONFIRM] {
            if let Some(Field::SingleLineText(f)) = self.form.fields.get_mut(idx) {
                f.set_text("");
            }
        }
    }

    fn wipe_sensitive_fields_restore(&mut self) {
        for idx in [
            RESTORE_MNEMONIC,
            RESTORE_PASSPHRASE,
            RESTORE_PASSWORD,
            RESTORE_CONFIRM,
        ] {
            match self.form.fields.get_mut(idx) {
                Some(Field::SingleLineText(f)) | Some(Field::MultiLineText(f)) => {
                    f.set_text("");
                }
                _ => {}
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Event loop
// ─────────────────────────────────────────────────────────────────────────────

/// Run the onboarding modal. Returns when the user creates/restores a wallet
/// or explicitly quits.
pub fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut OnboardingState,
) -> Result<OnboardingOutcome>
where
    B::Error: Send + Sync + 'static,
{
    let mut help_overlay: Option<HelpOverlay> = None;

    loop {
        terminal.draw(|f| {
            let area = f.area();
            draw(f, state);
            if let Some(ref overlay) = help_overlay {
                overlay.draw(f, area);
            }
        })?;

        if !event::poll(std::time::Duration::from_millis(250))? {
            continue;
        }

        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
                {
                    return Ok(OnboardingOutcome::Quit);
                }

                // Overlay absorbs all keys when open.
                if let Some(ref mut overlay) = help_overlay {
                    if overlay.handle_key(k) == HelpAction::Close {
                        help_overlay = None;
                    }
                    continue;
                }

                // F1 opens the help overlay from any sub-state/mode.
                // `?` is not used here so it can still be typed in form fields.
                if k.code == KeyCode::F(1) {
                    help_overlay = Some(HelpOverlay::new(
                        match state.mode {
                            OnboardingMode::Create => "Create wallet",
                            OnboardingMode::Restore => "Restore wallet",
                        },
                        state.help_lines(),
                    ));
                    continue;
                }

                match &state.phase {
                    Phase::Confirm(_) => {
                        if k.code == KeyCode::Enter {
                            // Take the phrase out of Phase::Confirm.
                            let phrase = match &state.phase {
                                Phase::Confirm(p) => p.clone(),
                                _ => unreachable!(),
                            };
                            if let Some(o) = state.finalize_create(&phrase) {
                                return Ok(o);
                            }
                        } else if k.code == KeyCode::Esc {
                            state.phase = Phase::Form;
                            state.message = None;
                        }
                        continue;
                    }
                    Phase::Form => {}
                }

                // Enter on a Submit field fires the submit handler.
                if k.code == KeyCode::Enter && state.form.mode == FormMode::Normal {
                    let focused_is_submit = matches!(
                        state.form.fields.get(state.form.focused()),
                        Some(Field::Submit(_))
                    );
                    if focused_is_submit {
                        let outcome = match state.mode {
                            OnboardingMode::Create => state.try_create_submit(),
                            OnboardingMode::Restore => state.try_restore_submit(),
                        };
                        if let Some(o) = outcome {
                            return Ok(o);
                        }
                        continue;
                    }
                }

                if k.code == KeyCode::Esc && state.form.mode == FormMode::Normal {
                    return Ok(OnboardingOutcome::Quit);
                }

                state.form.handle_input(Input::from(k));
            }
            Event::Resize(_, _) => continue,
            _ => continue,
        }
    }
}

fn draw(f: &mut ratatui::Frame, state: &mut OnboardingState) {
    let area = f.area();
    match &state.phase.clone_phrase() {
        Some(phrase) => draw_confirm(f, area, phrase, &state.message),
        None => draw_form_phase(f, area, state),
    }
}

trait ClonePhrase {
    fn clone_phrase(&self) -> Option<String>;
}

impl ClonePhrase for Phase {
    fn clone_phrase(&self) -> Option<String> {
        match self {
            Phase::Confirm(p) => Some(p.clone()),
            Phase::Form => None,
        }
    }
}

fn draw_form_phase(f: &mut ratatui::Frame, area: Rect, state: &mut OnboardingState) {
    let block = Block::default()
        .title(match state.mode {
            OnboardingMode::Create => " hodl • Create wallet ",
            OnboardingMode::Restore => " hodl • Restore wallet ",
        })
        .borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    let result = draw_form(f, chunks[0], &mut state.form, &FormPalette::dark());
    if let Some((cx, cy)) = result.cursor {
        f.set_cursor_position((cx, cy));
    }

    if let Some((msg, is_error)) = &state.message {
        let style = if *is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Cyan)
        };
        let p = Paragraph::new(Line::from(Span::styled(msg.clone(), style)))
            .alignment(Alignment::Center);
        f.render_widget(p, chunks[1]);
    } else {
        let mode_hint = if state.form.mode == FormMode::Insert {
            "Esc to Normal • Tab/j/k focus • h/l select option"
        } else {
            "i to edit • Tab/j/k focus • h/l select option • Esc quit"
        };
        let p = Paragraph::new(Line::from(Span::styled(
            mode_hint,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center);
        f.render_widget(p, chunks[1]);
    }
}

fn draw_confirm(
    f: &mut ratatui::Frame,
    area: Rect,
    phrase: &str,
    message: &Option<(String, bool)>,
) {
    let block = Block::default()
        .title(" hodl • Write down your mnemonic ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let words: Vec<&str> = phrase.split_whitespace().collect();
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "YOUR SEED PHRASE — WRITE THIS DOWN AND KEEP IT SAFE:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Display 4 words per line.
    for chunk in words.chunks(4) {
        lines.push(Line::from(Span::styled(
            chunk.join("  "),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));
    if let Some((msg, is_error)) = message {
        let style = if *is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(Span::styled(msg.clone(), style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press Enter when written down  •  Esc to go back",
        Style::default().fg(Color::DarkGray),
    )));

    let p = Paragraph::new(lines).alignment(Alignment::Center);
    f.render_widget(p, inner);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_mnemonic_rejects_short() {
        let err = validate_mnemonic("abandon abandon").unwrap_err();
        assert!(err.contains("expected 12 or 24 words"));
    }

    #[test]
    fn validate_mnemonic_rejects_18() {
        let phrase = "abandon ".repeat(17) + "about";
        let err = validate_mnemonic(&phrase).unwrap_err();
        assert!(err.contains("18") || err.contains("expected"));
    }

    #[test]
    fn validate_mnemonic_rejects_bad_checksum() {
        let bad = "abandon ".repeat(11) + "abandon";
        assert!(validate_mnemonic(&bad).is_err());
    }

    #[test]
    fn validate_mnemonic_accepts_valid_12() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        assert!(validate_mnemonic(phrase).is_ok());
    }

    #[test]
    fn validate_mnemonic_accepts_valid_24() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        assert!(validate_mnemonic(phrase).is_ok());
    }

    #[test]
    fn validate_password_nonempty_rejects_empty() {
        assert!(validate_password_nonempty("").is_err());
    }

    #[test]
    fn validate_password_nonempty_accepts_nonempty() {
        assert!(validate_password_nonempty("hunter2").is_ok());
    }

    #[test]
    fn onboarding_create_mismatch_stays_in_form() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut state = OnboardingState::new(
            OnboardingMode::Create,
            dir.path().to_path_buf(),
            "test".into(),
        );
        // Inject mismatched passwords.
        if let Some(Field::SingleLineText(f)) = state.form.fields.get_mut(CREATE_PASSWORD) {
            f.set_text("aaa");
        }
        if let Some(Field::SingleLineText(f)) = state.form.fields.get_mut(CREATE_CONFIRM) {
            f.set_text("bbb");
        }
        let outcome = state.try_create_submit();
        assert!(outcome.is_none());
        assert!(
            state
                .message
                .as_ref()
                .map(|(m, _)| m.contains("match"))
                .unwrap_or(false)
        );
    }
}
