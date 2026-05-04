//! Addresses screen — read-only list of used wallet addresses for the
//! currently-selected chain.
//!
//! Opened from the Accounts screen via `d`. Esc/q returns to Accounts.
//!
//! ## Streaming model
//!
//! `AddressesState` is **data-less** — it holds only the chain identity
//! plus the table cursor. The actual `WalletScan` is owned by
//! `AccountState`, which is kept alive (not stashed) by the App while
//! the Addresses sub-view is open. Each draw call passes the current
//! scan in, so as the background scan worker streams new used addresses
//! into `AccountState.partial_scan`, the table picks them up
//! automatically — no manual refresh, no parallel state to keep in sync.
//!
//! Path strings are computed inside `draw` from `(chain, change, index)`
//! via a pure formula (`m/{purpose}'/{coin}'/0'/{change}/{index}`) — no
//! network access required.

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

pub struct AddressesState {
    chain: ChainId,
    /// BIP-44 purpose for this chain's addresses (44 / 49 / 84 / 86).
    /// Cached so each draw doesn't re-compute it.
    purpose: u32,
    /// SLIP-44 coin type (e.g. 0 for BTC, 60 for ETH). Cached for the same reason.
    coin: u32,
    /// Selection is clamped on each draw to the current row count, so it
    /// remains valid even as the streaming scan grows the table beneath it.
    table_state: TableState,
}

impl AddressesState {
    /// Build a fresh sub-view for `chain`. The selection starts at the first row;
    /// rendering is driven by the live `WalletScan` passed to [`draw`].
    pub fn new(chain: ChainId, purpose: u32, coin: u32) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));
        Self {
            chain,
            purpose,
            coin,
            table_state,
        }
    }

    /// Move the table selection by `delta` rows, wrapping around.
    /// `len` is the current row count (caller passes it from the live scan).
    pub(crate) fn move_selection(&mut self, delta: i32, len: usize) {
        if len == 0 {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(len as i32) as usize;
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
    /// `len` is the live row count (caller pulls from the active scan).
    pub fn handle_key(&mut self, key: KeyEvent, len: usize) -> Option<AddressesAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            return Some(AddressesAction::Quit);
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1, len);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1, len);
                None
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if len > 0 {
                    self.table_state.select(Some(0));
                }
                None
            }
            KeyCode::Char('G') | KeyCode::End => {
                if len > 0 {
                    self.table_state.select(Some(len - 1));
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

/// Render the Addresses table.
///
/// `scan` is the live wallet scan owned by `AccountState` — pass
/// `account.scan.as_ref().unwrap_or(&account.partial_scan)` so a
/// completed snapshot wins over an in-flight partial when both exist.
/// `scanning` controls the footer label (live spinner hint vs. static).
pub fn draw(
    f: &mut Frame,
    area: Rect,
    state: &mut AddressesState,
    scan: &WalletScan,
    scanning: bool,
) {
    let chain_name = state.chain.display_name();
    let suffix = if scanning { "  (live)" } else { "" };
    let title = format!(" hodl • Addresses — {chain_name}{suffix} ");

    let border_color = if scanning { Color::Cyan } else { Color::Green };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    // Sort the live scan into receive-first / index-ascending order. The
    // worker streams in receive-then-change order already, but a partial
    // mid-scan view may be incomplete — sort defensively.
    let mut sorted: Vec<&hodl_chain_bitcoin::UsedAddress> = scan.used.iter().collect();
    sorted.sort_by_key(|u| (u.change, u.index));

    // Clamp the cursor so streaming row deletions / insertions can't
    // leave the selection past the end. Selecting None when empty so the
    // table renders cleanly with no highlight.
    let len = sorted.len();
    if len == 0 {
        state.table_state.select(None);
    } else {
        let sel = state.table_state.selected().unwrap_or(0).min(len - 1);
        state.table_state.select(Some(sel));
    }

    if sorted.is_empty() {
        let msg = if scanning {
            "scanning… no used addresses found yet"
        } else {
            "no used addresses"
        };
        let p = Paragraph::new(Line::from(Span::styled(
            msg,
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
        let purpose = state.purpose;
        let coin = state.coin;
        let rows: Vec<Row> = sorted
            .iter()
            .map(|u| {
                let (type_label, type_color) = if u.change == 0 {
                    ("recv", Color::Green)
                } else {
                    ("chg", Color::Yellow)
                };
                let path = format!("m/{purpose}'/{coin}'/0'/{}/{}", u.change, u.index);
                Row::new(vec![
                    ratatui::widgets::Cell::from(u.index.to_string()),
                    ratatui::widgets::Cell::from(type_label).style(Style::default().fg(type_color)),
                    ratatui::widgets::Cell::from(path),
                    ratatui::widgets::Cell::from(u.address.clone()),
                    ratatui::widgets::Cell::from(format_atoms(u.balance.confirmed, chain)),
                    ratatui::widgets::Cell::from(format_atoms(u.balance.pending, chain)),
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

    let footer_text = if scanning {
        "j/k move • g/G top/bottom • q/Esc back • ? help • streaming live"
    } else {
        "j/k move • g/G top/bottom • q/Esc back • ? help"
    };
    let footer = Paragraph::new(Line::from(Span::styled(
        footer_text,
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(footer, chunks[1]);
}
