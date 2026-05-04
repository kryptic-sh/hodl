//! Settings screen — hjkl-form with endpoint picker, Tor toggle,
//! KDF selector, and lock-timeout editor.
//!
//! The only place config is written to disk is the explicit Save submit.
//! Per PLAN.md: never auto-write on missing config.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use hjkl_form::{
    CheckboxField, Field, FieldMeta, Form, FormMode, Input, SelectField, SubmitField,
    TextFieldEditor,
};
use hjkl_ratatui::form::{FormPalette, draw_form};
use hodl_config::{Config, KdfPreset, LockConfig, TorConfig};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::help::{HelpAction, HelpOverlay};

/// Action emitted to the parent app loop.
#[derive(Debug)]
pub enum SettingsAction {
    /// Settings saved successfully; carry updated config back to app.
    Saved(Config),
    /// User dismissed without saving.
    Back,
    /// Quit the application.
    Quit,
}

// ── Field indices ──────────────────────────────────────────────────────────

const F_TOR: usize = 0;
const F_KDF: usize = 1;
const F_TIMEOUT: usize = 2;

// ── State ──────────────────────────────────────────────────────────────────

pub struct SettingsState {
    form: Form,
    config_path: PathBuf,
    message: Option<(String, bool)>,
}

impl SettingsState {
    pub fn new(config: &Config, config_path: PathBuf) -> Self {
        let kdf_options = vec!["default".into(), "hardened".into(), "paranoid".into()];
        let kdf_index = match config.kdf {
            KdfPreset::Default => 0,
            KdfPreset::Hardened => 1,
            KdfPreset::Paranoid => 2,
        };
        let mut kdf_select = SelectField::new(FieldMeta::new("KDF strength"), kdf_options);
        kdf_select.index = kdf_index;

        let timeout_field = TextFieldEditor::with_meta(FieldMeta::new("lock timeout (seconds)"), 1)
            .with_initial(&config.lock.idle_timeout_secs.to_string());

        let form = Form::new()
            .with_title("Settings")
            .with_field(Field::Checkbox(
                CheckboxField::new(FieldMeta::new("enable Tor")).with_value(config.tor.enabled),
            ))
            .with_field(Field::Select(kdf_select))
            .with_field(Field::SingleLineText(timeout_field))
            .with_field(Field::Submit(SubmitField::new(FieldMeta::new("Save"))));

        Self {
            form,
            config_path,
            message: None,
        }
    }

    /// Build a new `Config` from form values.
    fn read_config(&self, base: &Config) -> Config {
        let tor_enabled =
            matches!(self.form.fields.get(F_TOR), Some(Field::Checkbox(c)) if c.value);
        let kdf = match self.form.fields.get(F_KDF) {
            Some(Field::Select(s)) => match s.selected() {
                Some("hardened") => KdfPreset::Hardened,
                Some("paranoid") => KdfPreset::Paranoid,
                _ => KdfPreset::Default,
            },
            _ => KdfPreset::Default,
        };
        let idle_secs: u64 = match self.form.fields.get(F_TIMEOUT) {
            Some(Field::SingleLineText(f)) => f.text().parse().unwrap_or(300),
            _ => 300,
        };

        Config {
            chains: base.chains.clone(),
            tor: TorConfig {
                enabled: tor_enabled,
                socks5: base.tor.socks5.clone(),
            },
            lock: LockConfig {
                idle_timeout_secs: idle_secs,
            },
            kdf,
        }
    }

    /// Keybind reference for the contextual help overlay.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        vec![
            ("j / k".into(), "Move focus between fields".into()),
            ("Space".into(), "Toggle checkbox".into()),
            ("h / l".into(), "Cycle select options".into()),
            ("i".into(), "Edit text field (Insert mode)".into()),
            ("Esc".into(), "Normal mode / back without saving".into()),
            ("Enter".into(), "Save settings (on Save field)".into()),
            ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
            ("?".into(), "Show this help".into()),
        ]
    }

    fn try_save(&mut self, base: &Config) -> Option<Config> {
        let cfg = self.read_config(base);
        let toml_str = match toml::to_string_pretty(&cfg) {
            Ok(s) => s,
            Err(e) => {
                self.message = Some((format!("serialize error: {e}"), true));
                return None;
            }
        };
        if let Err(e) = std::fs::write(&self.config_path, toml_str.as_bytes()) {
            self.message = Some((format!("write failed: {e}"), true));
            return None;
        }
        self.message = Some(("settings saved".into(), false));
        Some(cfg)
    }
}

// ── Event loop ─────────────────────────────────────────────────────────────

pub fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut SettingsState,
    base_config: &Config,
) -> Result<SettingsAction>
where
    B::Error: Send + Sync + 'static,
{
    let mut help_overlay: Option<HelpOverlay> = None;

    loop {
        terminal.draw(|f| {
            let area = f.area();
            draw(f, area, state);
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
                    return Ok(SettingsAction::Quit);
                }

                // Overlay absorbs all keys when open.
                if let Some(ref mut overlay) = help_overlay {
                    if overlay.handle_key(k) == HelpAction::Close {
                        help_overlay = None;
                    }
                    continue;
                }

                // `?` in Normal mode opens the help overlay.
                if k.code == KeyCode::Char('?') && state.form.mode == FormMode::Normal {
                    help_overlay = Some(HelpOverlay::new("Settings", state.help_lines()));
                    continue;
                }

                if k.code == KeyCode::Enter && state.form.mode == FormMode::Normal {
                    let is_submit = matches!(
                        state.form.fields.get(state.form.focused()),
                        Some(Field::Submit(_))
                    );
                    if is_submit {
                        if let Some(cfg) = state.try_save(base_config) {
                            return Ok(SettingsAction::Saved(cfg));
                        }
                        continue;
                    }
                }

                if k.code == KeyCode::Esc && state.form.mode == FormMode::Normal {
                    return Ok(SettingsAction::Back);
                }

                state.form.handle_input(Input::from(k));
            }
            Event::Resize(_, _) => continue,
            _ => continue,
        }
    }
}

// ── Drawing ────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, area: Rect, state: &mut SettingsState) {
    let block = Block::default()
        .title(" hodl • Settings ")
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

    let bottom_text = if let Some((msg, is_error)) = &state.message {
        let style = if *is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        Paragraph::new(Line::from(Span::styled(msg.clone(), style))).alignment(Alignment::Center)
    } else {
        Paragraph::new(Line::from(Span::styled(
            "j/k focus • Space toggle • h/l cycle select • Enter save • Esc back",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center)
    };
    f.render_widget(bottom_text, chunks[1]);
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_state(config: &Config, tmp: &TempDir) -> SettingsState {
        let path = tmp.path().join("config.toml");
        SettingsState::new(config, path)
    }

    #[test]
    fn settings_round_trip_defaults() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let state = make_state(&cfg, &dir);
        let out = state.read_config(&cfg);
        assert_eq!(out.kdf, cfg.kdf);
        assert_eq!(out.tor.enabled, cfg.tor.enabled);
        assert_eq!(out.lock.idle_timeout_secs, cfg.lock.idle_timeout_secs);
    }

    #[test]
    fn settings_tor_toggle_reflected() {
        let dir = TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.tor.enabled = true;
        let state = make_state(&cfg, &dir);
        let out = state.read_config(&cfg);
        assert!(out.tor.enabled);
    }

    #[test]
    fn settings_save_writes_toml() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut state = make_state(&cfg, &dir);
        let saved = state.try_save(&cfg);
        assert!(saved.is_some());
        assert!(state.config_path.exists());
        let content = std::fs::read_to_string(&state.config_path).unwrap();
        // Should be valid TOML that round-trips.
        let back: Config = toml::from_str(&content).expect("saved TOML must parse");
        assert_eq!(back.kdf, cfg.kdf);
    }
}
