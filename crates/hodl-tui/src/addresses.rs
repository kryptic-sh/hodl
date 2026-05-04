//! Addresses screen — read-only list of used wallet addresses for the
//! currently-selected chain, populated from a cached WalletScan.
//!
//! Opened from the Accounts screen via `d`. Esc/q returns to Accounts.
//! No background work — the scan is owned by AccountState; this screen
//! reads it via cloned data passed to AddressesState::new.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hodl_chain_bitcoin::WalletScan;
use hodl_core::ChainId;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, TableState};

/// Action emitted to the parent (app.rs) when the addresses screen wants to
/// transition.
#[derive(Debug)]
pub enum AddressesAction {
    /// Return to the Accounts screen.
    Close,
    /// Quit the application.
    Quit,
    /// Open the contextual help overlay.
    ShowHelp,
}

/// One row in the address table, pre-formatted for rendering.
struct AddressRow {
    index: u32,
    change: u32,
    path: String,
    address: String,
    confirmed: u64,
    pending: u64,
}

pub struct AddressesState {
    rows: Vec<AddressRow>,
    chain: ChainId,
    table_state: TableState,
}

impl AddressesState {
    /// Build from a completed `WalletScan`.
    ///
    /// `derivation_path_fn` is called with `(change, index)` for each row and
    /// returns the human-readable BIP-44 path string. Passing the closure from
    /// app.rs avoids a hard dependency on `ActiveChain` here.
    ///
    /// Sort order: receive (change=0) first by index ascending, then change
    /// (change=1) by index ascending. The first row is selected by default.
    pub fn new(
        scan: &WalletScan,
        chain: ChainId,
        derivation_path_fn: impl Fn(u32, u32) -> String,
    ) -> Self {
        let mut rows: Vec<AddressRow> = scan
            .used
            .iter()
            .map(|u| AddressRow {
                index: u.index,
                change: u.change,
                path: derivation_path_fn(u.change, u.index),
                address: u.address.clone(),
                confirmed: u.balance.confirmed,
                pending: u.balance.pending,
            })
            .collect();

        // Receive (change=0) first sorted by index, then change (change=1) by index.
        rows.sort_by_key(|r| (r.change, r.index));

        let mut table_state = TableState::default();
        if !rows.is_empty() {
            table_state.select(Some(0));
        }

        Self {
            rows,
            chain,
            table_state,
        }
    }

    /// Move the table selection by `delta` rows, wrapping around.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(self.rows.len() as i32) as usize;
        self.table_state.select(Some(next));
    }

    /// Keybind reference for the contextual help overlay.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        vec![
            ("j / ↓".into(), "Move selection down".into()),
            ("k / ↑".into(), "Move selection up".into()),
            ("g / Home".into(), "Jump to first row".into()),
            ("G / End".into(), "Jump to last row".into()),
            ("q / Esc".into(), "Return to Accounts".into()),
            ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
            ("?".into(), "Show this help".into()),
        ]
    }

    /// Route a keypress. Returns an action when the screen wants to transition.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AddressesAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            return Some(AddressesAction::Quit);
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                None
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if !self.rows.is_empty() {
                    self.table_state.select(Some(0));
                }
                None
            }
            KeyCode::Char('G') | KeyCode::End => {
                if !self.rows.is_empty() {
                    self.table_state.select(Some(self.rows.len() - 1));
                }
                None
            }
            KeyCode::Char('q') | KeyCode::Esc => Some(AddressesAction::Close),
            KeyCode::Char('?') => Some(AddressesAction::ShowHelp),
            _ => None,
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

/// Format a satoshi amount as a decimal coin string (e.g. `1.23456789 BTC`).
///
/// All currently-supported chains use 8 decimal places (atoms / 1e8).
fn format_atoms(atoms: u64, chain: ChainId) -> String {
    let symbol = chain.ticker();
    let whole = atoms / 100_000_000;
    let frac = atoms % 100_000_000;
    format!("{whole}.{frac:08} {symbol}")
}

pub fn draw(f: &mut Frame, area: Rect, state: &mut AddressesState) {
    let chain_name = state.chain.display_name();
    let title = format!(" hodl • Addresses — {chain_name} ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if state.rows.is_empty() {
        // Defensive: `d` is gated on scan being non-empty in account.rs, but
        // guard here anyway so we never render a blank or panic.
        let p = Paragraph::new(Line::from(Span::styled(
            "no used addresses",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center);
        f.render_widget(p, chunks[0]);
    } else {
        let header = Row::new(vec![
            ratatui::widgets::Cell::from("idx")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("type")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("path")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("address")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("confirmed")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("pending")
                .style(Style::default().add_modifier(Modifier::BOLD)),
        ])
        .style(Style::default().fg(Color::Cyan));

        let chain = state.chain;
        let rows: Vec<Row> = state
            .rows
            .iter()
            .map(|r| {
                let (type_label, type_color) = if r.change == 0 {
                    ("recv", Color::Green)
                } else {
                    ("chg", Color::Yellow)
                };
                Row::new(vec![
                    ratatui::widgets::Cell::from(r.index.to_string()),
                    ratatui::widgets::Cell::from(type_label).style(Style::default().fg(type_color)),
                    ratatui::widgets::Cell::from(r.path.clone()),
                    ratatui::widgets::Cell::from(r.address.clone()),
                    ratatui::widgets::Cell::from(format_atoms(r.confirmed, chain)),
                    ratatui::widgets::Cell::from(format_atoms(r.pending, chain)),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(5),  // idx
            Constraint::Length(5),  // type
            Constraint::Length(22), // path
            Constraint::Min(20),    // address
            Constraint::Length(22), // confirmed
            Constraint::Length(22), // pending
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("> ");

        f.render_stateful_widget(table, chunks[0], &mut state.table_state);
    }

    let footer = Paragraph::new(Line::from(Span::styled(
        "j/k move • g/G top/bottom • q/Esc back • ? help",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(footer, chunks[1]);
}
