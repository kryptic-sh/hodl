//! Lock-screen UI: password entry, unlock, idle auto-lock.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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
enum Mode {
    Locked,
    Unlocked,
}

/// Outcome reported back to the caller. Currently we just exit cleanly in
/// both paths; this type lets future M2 wire-up hand off the unlocked wallet.
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
) -> Result<Outcome> {
    let mut state = LockState::new();

    loop {
        terminal.draw(|f| draw(f, &state))?;

        // Idle check before polling.
        if state.mode == Mode::Unlocked && state.last_activity.elapsed() >= idle_timeout {
            // Drop the unlocked wallet — `ZeroizeOnDrop` scrubs the seed.
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

struct LockState {
    mode: Mode,
    /// Password buffer. Zeroized on submit / clear.
    password: String,
    message: Option<(String, MessageKind)>,
    unlocked: Option<UnlockedWallet>,
    last_activity: Instant,
}

impl LockState {
    fn new() -> Self {
        Self {
            mode: Mode::Locked,
            password: String::new(),
            message: None,
            unlocked: None,
            last_activity: Instant::now(),
        }
    }

    fn clear_password(&mut self) {
        self.password.zeroize();
    }
}

impl Drop for LockState {
    fn drop(&mut self) {
        self.clear_password();
        // unlocked: ZeroizeOnDrop runs automatically.
    }
}

fn handle_key(state: &mut LockState, wallet: &Wallet, k: KeyEvent) -> Option<Outcome> {
    // Ctrl-C / Ctrl-D quits from anywhere.
    if k.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
    {
        return Some(Outcome::Quit);
    }

    match state.mode {
        Mode::Locked => match k.code {
            KeyCode::Esc => return Some(Outcome::Quit),
            KeyCode::Enter => {
                let pw = state.password.as_bytes().to_vec();
                match wallet.unlock(&pw) {
                    Ok(u) => {
                        state.unlocked = Some(u);
                        state.mode = Mode::Unlocked;
                        state.message = Some(("unlocked — M1 done".into(), MessageKind::Info));
                    }
                    Err(e) => {
                        state.message = Some((format!("{e}"), MessageKind::Error));
                    }
                }
                state.clear_password();
                let mut zeroed = pw;
                zeroed.zeroize();
            }
            KeyCode::Backspace => {
                state.password.pop();
            }
            // Ignore other ctrl-modified chars.
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                state.password.push(c);
            }
            _ => {}
        },
        Mode::Unlocked => match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Some(Outcome::Quit),
            KeyCode::Char('l') => {
                // Manual lock.
                state.unlocked = None;
                state.mode = Mode::Locked;
                state.message = Some(("locked".into(), MessageKind::Info));
            }
            _ => {}
        },
    }
    None
}

fn draw(f: &mut ratatui::Frame, state: &LockState) {
    let area = f.area();
    match state.mode {
        Mode::Locked => draw_locked(f, area, state),
        Mode::Unlocked => draw_unlocked(f, area, state),
    }
}

fn draw_locked(f: &mut ratatui::Frame, area: Rect, state: &LockState) {
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

    let masked: String = "*".repeat(state.password.chars().count());
    let prompt = Paragraph::new(Line::from(vec![Span::raw("password: "), Span::raw(masked)]))
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
        "enter to submit • esc to quit",
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
