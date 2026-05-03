//! Account screen — shows addresses + balances for the unlocked wallet.
//!
//! For M2 we support one chain (Bitcoin). A `hjkl_picker::Picker` overlay
//! lists configured chains; if none are configured, an empty-state banner
//! is shown instead.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hjkl_picker::{PickerAction, PickerEvent, PickerLogic};
use hodl_chain_bitcoin::{BitcoinChain, NetworkParams, Purpose};
use hodl_config::{ChainConfig, Config, Endpoint};
use hodl_core::{Address, Amount, Chain, ChainId};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use tracing::debug;

use hodl_wallet::UnlockedWallet;

/// Action emitted by the account screen to the parent app loop.
#[derive(Debug)]
pub enum AccountAction {
    /// Navigate to the receive screen for the given address.
    OpenReceive(Address),
    /// Navigate to the send screen for the given address + derivation index.
    OpenSend {
        address: Address,
        account: u32,
        change_branch: u32,
        index: u32,
        balance_sats: u64,
    },
    /// Navigate to the settings screen.
    OpenSettings,
    /// Lock the wallet (return to lock screen).
    Lock,
    /// Quit the application.
    Quit,
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
    flash: Option<String>,
    config: Config,
}

impl AccountState {
    pub fn new(_data_root: PathBuf, config: Config) -> Self {
        Self {
            rows: Vec::new(),
            table_state: TableState::default(),
            picker: None,
            flash: None,
            config,
        }
    }

    /// Populate the account rows by scanning the first gap-limit addresses.
    ///
    /// If `Config.chains[Bitcoin]` has no endpoints configured, leaves rows
    /// empty and sets a flash message.
    pub fn load_accounts(&mut self, wallet: &UnlockedWallet) {
        let chain_cfg = self
            .config
            .chains
            .get(&ChainId::Bitcoin)
            .cloned()
            .unwrap_or_default();

        if chain_cfg.endpoints.is_empty() {
            self.flash = Some("no Electrum endpoints configured — edit settings to add one".into());
            return;
        }

        let Some(endpoint_url) = first_electrum_url(&chain_cfg) else {
            self.flash = Some("no valid Electrum endpoint found in config".into());
            return;
        };

        debug!("connecting to Electrum: {endpoint_url}");

        let electrum = match electrum_connect(&endpoint_url) {
            Ok(c) => c,
            Err(e) => {
                self.flash = Some(format!("Electrum connect failed: {e}"));
                return;
            }
        };

        let chain = BitcoinChain::new(NetworkParams::BITCOIN_MAINNET, electrum)
            .with_purpose(Purpose::Bip84);

        let seed = wallet.seed().as_bytes().to_owned();
        let mut rows = Vec::new();

        for index in 0..5u32 {
            let addr = match chain.derive(&seed, 0, index) {
                Ok(a) => a,
                Err(e) => {
                    debug!("derive account {index} failed: {e}");
                    continue;
                }
            };
            let balance = match chain.balance(&addr) {
                Ok(b) => {
                    debug!("balance {index}: {b:?}");
                    Some(b)
                }
                Err(e) => {
                    debug!("balance {index} failed: {e}");
                    None
                }
            };
            let path = format!("m/84'/0'/0'/0/{index}");
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
            self.table_state.select(Some(0));
        }
    }

    fn selected_address(&self) -> Option<&Address> {
        let idx = self.table_state.selected()?;
        self.rows.get(idx).map(|r| &r.address)
    }

    fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(self.rows.len() as i32) as usize;
        self.table_state.select(Some(next));
    }

    /// Open the chain switcher picker.
    fn open_picker(&mut self) {
        let chains: Vec<ChainId> = self.config.chains.keys().cloned().collect();
        if chains.is_empty() {
            self.flash = Some("no chains configured — edit settings".into());
            return;
        }
        let source = ChainPickerSource::new(chains);
        self.picker = Some(hjkl_picker::Picker::new(Box::new(source)));
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
                PickerEvent::Select(_) => {
                    // For M2 the pick just closes the overlay; chain
                    // switching is post-M2.
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
                if let Some(idx) = self.table_state.selected()
                    && let Some(row) = self.rows.get(idx)
                {
                    let balance_sats = row.balance.as_ref().map(|b| b.atoms() as u64).unwrap_or(0);
                    return Some(AccountAction::OpenSend {
                        address: row.address.clone(),
                        account: 0,
                        change_branch: 0,
                        index: row.index,
                        balance_sats,
                    });
                }
            }
            KeyCode::Char('S') => return Some(AccountAction::OpenSettings),
            KeyCode::Char('p') => self.open_picker(),
            KeyCode::Char('q') | KeyCode::Esc => return Some(AccountAction::Lock),
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
        .constraints([Constraint::Min(1), Constraint::Length(2)])
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

    let hint = Paragraph::new(Line::from(Span::styled(
        "j/k move • r receive • s send • S settings • p picker • q lock",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(hint, chunks[1]);

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
        let _ = idx;
        PickerAction::None
    }

    fn enumerate(
        &mut self,
        _query: Option<&str>,
        _cancel: Arc<AtomicBool>,
    ) -> Option<JoinHandle<()>> {
        None
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Connect to an Electrum server from a URL like `ssl://host:60002` or `tcp://host:50001`.
fn electrum_connect(url: &str) -> hodl_core::Result<hodl_chain_bitcoin::electrum::ElectrumClient> {
    use hodl_chain_bitcoin::electrum::ElectrumClient;
    use hodl_core::error::Error;

    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| Error::Network(format!("invalid Electrum URL (missing scheme): {url}")))?;
    let (host, port_str) = rest
        .rsplit_once(':')
        .ok_or_else(|| Error::Network(format!("invalid Electrum URL (missing port): {url}")))?;
    let port: u16 = port_str
        .parse()
        .map_err(|_| Error::Network(format!("invalid port in Electrum URL: {url}")))?;

    match scheme {
        "ssl" | "tls" => ElectrumClient::connect_tls(host, port),
        _ => ElectrumClient::connect_tcp(host, port),
    }
}

fn first_electrum_url(cfg: &ChainConfig) -> Option<String> {
    cfg.endpoints.iter().find_map(|ep| {
        if let Endpoint::Electrum { url, .. } = ep {
            Some(url.clone())
        } else {
            None
        }
    })
}
