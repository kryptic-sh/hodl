//! Account screen — shows addresses + balances for the unlocked wallet.
//!
//! Chain selection drives `ActiveChain::from_chain_id` — the picker is no
//! longer decorative; switching chains re-derives rows against the new backend.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hjkl_picker::{PickerAction, PickerEvent, PickerLogic};
use hodl_config::Config;
use hodl_core::{Address, Amount, ChainId};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use tracing::debug;

use hodl_wallet::UnlockedWallet;

use crate::active_chain::ActiveChain;

/// Action emitted by the account screen to the parent app loop.
#[derive(Debug)]
pub enum AccountAction {
    /// Navigate to the receive screen for the given address.
    OpenReceive(Address),
    /// Navigate to the send screen for the given HD account.
    ///
    /// `chain` carries the currently-selected chain so `SendState` builds
    /// the right `ActiveChain` without re-reading account state.
    OpenSend {
        chain: ChainId,
        account: u32,
        total_balance_sats: u64,
    },
    /// User switched chains via the picker; parent should call `load_accounts`.
    ChainSwitched,
    /// Navigate to the address book screen.
    OpenAddressBook,
    /// Navigate to the settings screen.
    OpenSettings,
    /// Lock the wallet (return to lock screen).
    Lock,
    /// Quit the application.
    Quit,
    /// Open the contextual help overlay.
    ShowHelp,
}

/// A single row in the account table.
struct AccountRow {
    index: u32,
    path: String,
    address: Address,
    balance: Option<Amount>,
}

pub struct AccountState {
    rows: Vec<AccountRow>,
    table_state: TableState,
    /// Chain picker overlay. `None` when closed.
    picker: Option<hjkl_picker::Picker>,
    /// Ordered chain list parallel to the open picker; used to resolve
    /// `PickerAction::SwitchSlot(idx)` back to a `ChainId`.
    picker_chains: Vec<ChainId>,
    flash: Option<String>,
    config: Config,
    /// Currently-selected chain. Defaults to Bitcoin; updated by the picker.
    pub current_chain: ChainId,
}

impl AccountState {
    pub fn new(_data_root: PathBuf, config: Config) -> Self {
        Self {
            rows: Vec::new(),
            table_state: TableState::default(),
            picker: None,
            picker_chains: Vec::new(),
            flash: None,
            config,
            current_chain: ChainId::Bitcoin,
        }
    }

    /// Populate the account rows for `current_chain` by deriving and querying
    /// the first 5 addresses. Switches chains when `current_chain` changes.
    pub fn load_accounts(&mut self, wallet: &UnlockedWallet) {
        debug!("load_accounts for chain {:?}", self.current_chain);

        let active = match ActiveChain::from_chain_id(self.current_chain, &self.config) {
            Ok(a) => a,
            Err(e) => {
                self.flash = Some(format!("{}: {e}", self.current_chain.display_name()));
                return;
            }
        };

        let seed = wallet.seed().as_bytes().to_owned();
        let mut rows = Vec::new();

        for index in 0..5u32 {
            let addr = match active.derive(&seed, 0, index) {
                Ok(a) => a,
                Err(e) => {
                    debug!("derive {index} failed: {e}");
                    continue;
                }
            };
            let balance = match active.balance(&addr) {
                Ok(b) => {
                    debug!("balance {index}: {b:?}");
                    Some(b)
                }
                Err(e) => {
                    debug!("balance {index} failed: {e}");
                    None
                }
            };
            let path = active.derivation_path(0, index);
            rows.push(AccountRow {
                index,
                path,
                address: addr,
                balance,
            });
        }

        if rows.is_empty() {
            self.flash = Some("no addresses derived — check chain config".into());
        } else {
            self.rows = rows;
            self.flash = None;
            self.table_state.select(Some(0));
        }
    }

    fn selected_address(&self) -> Option<&Address> {
        let idx = self.table_state.selected()?;
        self.rows.get(idx).map(|r| &r.address)
    }

    pub(crate) fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(self.rows.len() as i32) as usize;
        self.table_state.select(Some(next));
    }

    /// Open the chain switcher picker.
    fn open_picker(&mut self) {
        let mut chains: Vec<ChainId> = self.config.chains.keys().cloned().collect();
        if chains.is_empty() {
            self.flash = Some("no chains configured — edit settings".into());
            return;
        }
        // Stable sort so the list is deterministic across renders.
        chains.sort_by_key(|c| c.display_name());
        self.picker_chains = chains.clone();
        let source = ChainPickerSource::new(chains);
        self.picker = Some(hjkl_picker::Picker::new(Box::new(source)));
    }

    /// Keybind reference for the contextual help overlay.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        vec![
            ("j / ↓".into(), "Move selection down".into()),
            ("k / ↑".into(), "Move selection up".into()),
            ("r".into(), "Open receive screen".into()),
            ("s".into(), "Open send screen".into()),
            ("b".into(), "Open address book".into()),
            ("S".into(), "Open settings".into()),
            ("p".into(), "Open chain picker".into()),
            ("q / Esc".into(), "Lock wallet".into()),
            ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
            ("?".into(), "Show this help".into()),
        ]
    }

    /// Route a keypress. Returns an action when the screen wants to
    /// transition.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AccountAction> {
        // Ctrl-C / Ctrl-D quit.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            return Some(AccountAction::Quit);
        }

        // Picker overlay absorbs keys when open.
        if let Some(picker) = &mut self.picker {
            match picker.handle_key(key) {
                PickerEvent::Cancel => {
                    self.picker = None;
                }
                PickerEvent::Select(PickerAction::None) | PickerEvent::None => {
                    picker.refresh();
                }
                PickerEvent::Select(PickerAction::SwitchSlot(idx)) => {
                    self.picker = None;
                    if let Some(&chain) = self.picker_chains.get(idx) {
                        self.current_chain = chain;
                        return Some(AccountAction::ChainSwitched);
                    }
                }
                PickerEvent::Select(_) => {
                    self.picker = None;
                }
            }
            return None;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('r') => {
                if let Some(addr) = self.selected_address().cloned() {
                    return Some(AccountAction::OpenReceive(addr));
                }
            }
            KeyCode::Char('s') => {
                // Sum balance across all rows — multi-source send draws from
                // every funded address so total is what the user can actually spend.
                let total_balance_sats = self
                    .rows
                    .iter()
                    .filter_map(|r| r.balance.as_ref())
                    .map(|b| b.atoms() as u64)
                    .sum();
                return Some(AccountAction::OpenSend {
                    chain: self.current_chain,
                    account: 0,
                    total_balance_sats,
                });
            }
            KeyCode::Char('b') => return Some(AccountAction::OpenAddressBook),
            KeyCode::Char('S') => return Some(AccountAction::OpenSettings),
            KeyCode::Char('p') => self.open_picker(),
            KeyCode::Char('q') | KeyCode::Esc => return Some(AccountAction::Lock),
            KeyCode::Char('?') => return Some(AccountAction::ShowHelp),
            _ => {}
        }

        None
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, area: Rect, state: &mut AccountState) {
    let block = Block::default()
        .title(" hodl • Accounts ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    if let Some(msg) = &state.flash {
        let p = Paragraph::new(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Yellow),
        )))
        .alignment(Alignment::Center);
        f.render_widget(p, chunks[0]);
    } else if state.rows.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "loading accounts…",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center);
        f.render_widget(p, chunks[0]);
    } else {
        let header = Row::new(vec![
            Cell::from("idx").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("path").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("address").style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from("balance").style(Style::default().add_modifier(Modifier::BOLD)),
        ])
        .style(Style::default().fg(Color::Cyan));

        let rows: Vec<Row> = state
            .rows
            .iter()
            .map(|r| {
                Row::new(vec![
                    Cell::from(r.index.to_string()),
                    Cell::from(r.path.clone()),
                    Cell::from(r.address.as_str().to_string()),
                    Cell::from(
                        r.balance
                            .as_ref()
                            .map(|b| format!("{b}"))
                            .unwrap_or_else(|| "—".into()),
                    ),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(4),
            Constraint::Length(22),
            Constraint::Min(20),
            Constraint::Length(16),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("> ");

        f.render_stateful_widget(table, chunks[0], &mut state.table_state);
    }

    if !state.rows.is_empty() {
        let synced = state.rows.iter().filter(|r| r.balance.is_some()).count();
        let total = state.rows.len();
        let (label, color) = if synced == total {
            (format!("synced {synced}/{total}"), Color::Green)
        } else if synced == 0 {
            ("sync failed — endpoint unreachable".into(), Color::Red)
        } else {
            (format!("synced {synced}/{total} (partial)"), Color::Yellow)
        };
        let sync = Paragraph::new(Line::from(Span::styled(
            format!("{} · {}", state.current_chain.display_name(), label),
            Style::default().fg(color),
        )))
        .alignment(Alignment::Center);
        f.render_widget(sync, chunks[1]);
    }

    let hint = Paragraph::new(Line::from(Span::styled(
        "j/k move • r receive • s send • b book • S settings • p picker • q lock",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(hint, chunks[2]);

    // Picker overlay drawn on top.
    if state.picker.is_some() {
        draw_picker_overlay(f, area, state);
    }
}

fn draw_picker_overlay(f: &mut Frame, area: Rect, state: &mut AccountState) {
    let Some(picker) = &mut state.picker else {
        return;
    };
    picker.refresh();

    // Centre a 50×15 box.
    let w = area.width.min(60);
    let h = area.height.min(16);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let overlay = Rect::new(x, y, w, h);

    let block = Block::default()
        .title(format!(" {} ", picker.title()))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(overlay);
    f.render_widget(ratatui::widgets::Clear, overlay);
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

    // Query input.
    let query = picker.query.text();
    let query_para = Paragraph::new(query)
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White));
    f.render_widget(query_para, chunks[0]);

    let list = Paragraph::new(rows);
    f.render_widget(list, chunks[1]);
}

// ── Chain picker source ────────────────────────────────────────────────────

struct ChainPickerSource {
    chains: Vec<ChainId>,
}

impl ChainPickerSource {
    fn new(chains: Vec<ChainId>) -> Self {
        Self { chains }
    }
}

impl PickerLogic for ChainPickerSource {
    fn title(&self) -> &str {
        "chains"
    }

    fn item_count(&self) -> usize {
        self.chains.len()
    }

    fn label(&self, idx: usize) -> String {
        self.chains
            .get(idx)
            .map(|c| c.display_name().to_string())
            .unwrap_or_default()
    }

    fn match_text(&self, idx: usize) -> String {
        self.label(idx)
    }

    fn has_preview(&self) -> bool {
        false
    }

    fn select(&self, idx: usize) -> PickerAction {
        PickerAction::SwitchSlot(idx)
    }

    fn enumerate(
        &mut self,
        _query: Option<&str>,
        _cancel: Arc<AtomicBool>,
    ) -> Option<JoinHandle<()>> {
        None
    }
}
