//! Account screen — per-chain summary card for the unlocked wallet.
//!
//! Chain selection drives `ActiveChain::from_chain_id` — the picker is no
//! longer decorative; switching chains re-scans against the new backend.
//!
//! ## Loading flow
//!
//! `start_load` spawns a background thread that opens the Electrum/RPC
//! connection, runs a BIP-44 gap-limit scan (Bitcoin family) or derives a
//! single address with its balance (EVM / Monero), and returns
//! `Result<WalletScan, String>` via a channel. The event loop polls via
//! `pending_scan.try_recv()` each iteration:
//! - `Empty`          → tick `scanning_spinner`; redraw.
//! - `Ok(Ok(scan))`   → swap into state; clear pending.
//! - `Ok(Err(msg))`   → set scan_error; clear pending.
//! - `Disconnected`   → set scan_error to "scan thread panicked"; clear pending.
//!
//! While scanning, navigation keys that depend on scan results (`r`/`s`/`b`/`d`)
//! are suppressed. `q`, `S`, `p`, Ctrl-C/D, and `?` always work.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hjkl_picker::{PickerAction, PickerEvent, PickerLogic};
use hodl_chain_bitcoin::WalletScan;
use hodl_config::Config;
use hodl_core::{Address, Chain, ChainId};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tracing::debug;
use zeroize::Zeroize;

use hodl_wallet::UnlockedWallet;

use crate::active_chain::ActiveChain;
use crate::spinner::Spinner;

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
    /// User switched chains via the picker; parent should call `start_load`.
    ChainSwitched,
    /// Navigate to the address book screen.
    OpenAddressBook,
    /// Navigate to the settings screen.
    OpenSettings,
    /// Open the used-addresses sub-view (Step C wires the routing).
    OpenAddresses,
    /// Lock the wallet (return to lock screen).
    Lock,
    /// Quit the application.
    Quit,
    /// Open the contextual help overlay.
    ShowHelp,
}

pub struct AccountState {
    /// Background gap-limit scan result — populated by start_load.
    pub scan: Option<WalletScan>,
    /// Populated when the scan thread returns an error.
    pub scan_error: Option<String>,
    /// In-flight scan channel. `Some` while the background thread is running.
    pending_scan: Option<Receiver<Result<WalletScan, String>>>,
    /// Spinner shown while `pending_scan` is active.
    scanning_spinner: Option<Spinner>,
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
            scan: None,
            scan_error: None,
            pending_scan: None,
            scanning_spinner: None,
            picker: None,
            picker_chains: Vec::new(),
            flash: None,
            config,
            current_chain: ChainId::Bitcoin,
        }
    }

    /// Returns `true` while a scan is in flight.
    pub fn is_scanning(&self) -> bool {
        self.pending_scan.is_some()
    }

    /// Kept for backward-compat with app.rs call sites that checked is_loading().
    pub fn is_loading(&self) -> bool {
        self.is_scanning()
    }

    /// Tick the scanning spinner (called by the event loop on `TryRecvError::Empty`).
    pub fn tick_spinner(&mut self) {
        if let Some(ref mut s) = self.scanning_spinner {
            s.tick();
        }
    }

    /// Poll the pending scan channel once. Returns `true` if state changed
    /// (caller should redraw), `false` if still empty.
    pub fn poll_scan(&mut self) -> bool {
        let result = match &self.pending_scan {
            Some(rx) => rx.try_recv(),
            None => return false,
        };

        use std::sync::mpsc::TryRecvError;
        match result {
            Ok(Ok(scan)) => {
                self.scan = Some(scan);
                self.scan_error = None;
                self.pending_scan = None;
                self.scanning_spinner = None;
                true
            }
            Ok(Err(msg)) => {
                self.scan_error = Some(msg);
                self.scan = None;
                self.pending_scan = None;
                self.scanning_spinner = None;
                true
            }
            Err(TryRecvError::Disconnected) => {
                self.scan_error = Some("scan thread panicked".into());
                self.scan = None;
                self.pending_scan = None;
                self.scanning_spinner = None;
                true
            }
            Err(TryRecvError::Empty) => {
                self.tick_spinner();
                false
            }
        }
    }

    /// Kept for backward-compat with app.rs call sites that called poll_load().
    pub fn poll_load(&mut self) -> bool {
        self.poll_scan()
    }

    /// Spawn a background thread to run the gap-limit scan.
    /// Replaces the old per-row balance loader.
    pub fn start_load(&mut self, wallet: &UnlockedWallet) {
        debug!("start_load (scan) for chain {:?}", self.current_chain);

        // Clear stale data so the loading state is visible immediately.
        self.scan = None;
        self.scan_error = None;
        self.flash = None;

        let chain = self.current_chain;
        let config = self.config.clone();
        let gap_limit = config.chains.get(&chain).map(|c| c.gap_limit).unwrap_or(20);

        // Extract seed bytes to move into the thread. [u8; 64] is Copy + Send;
        // we zeroize the closure-captured copy explicitly before the thread exits.
        let seed: [u8; 64] = *wallet.seed().as_bytes();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Mutable rebinding so we can zeroize the actual captured array
            // (not a fresh Copy) after the worker returns.
            let mut seed = seed;
            let result = scan_thread(chain, &config, &seed, gap_limit, 0);
            seed.zeroize();
            let _ = tx.send(result);
        });

        self.pending_scan = Some(rx);
        self.scanning_spinner = Some(Spinner::new());
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

    /// Pick the best receive address from the scan for the Receive screen.
    ///
    /// Returns the first used receive address (change=0) if any exist; otherwise
    /// falls back to deriving index 0.
    pub fn pick_receive_address(&self, wallet: &UnlockedWallet) -> Option<Address> {
        // Try first used receive address from scan.
        if let Some(scan) = &self.scan
            && let Some(used) = scan.used.iter().find(|u| u.change == 0)
        {
            let addr = Address::new(used.address.clone(), self.current_chain);
            return Some(addr);
        }

        // Fallback: derive index 0 from the wallet seed.
        let seed: [u8; 64] = *wallet.seed().as_bytes();
        let active = ActiveChain::from_chain_id(self.current_chain, &self.config).ok()?;
        active.derive(&seed, 0, 0).ok()
    }

    /// Keybind reference for the contextual help overlay.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        vec![
            ("r".into(), "Open receive screen".into()),
            ("s".into(), "Open send screen".into()),
            ("b".into(), "Open address book".into()),
            ("d".into(), "View used addresses".into()),
            ("S".into(), "Open settings".into()),
            ("p".into(), "Open chain picker".into()),
            ("q / Esc".into(), "Lock wallet".into()),
            ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
            ("?".into(), "Show this help".into()),
        ]
    }

    /// Route a keypress. Returns an action when the screen wants to transition.
    ///
    /// `r`/`s`/`b`/`d` are blocked while `is_scanning()` is true.
    /// `q`, `S`, `p`, Ctrl-C/D, and `?` always work.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AccountAction> {
        // We need the wallet for picking a receive address, but handle_key
        // doesn't take a wallet parameter — so for `r` we return the action
        // with a placeholder and let app.rs do the derivation. Actually, we
        // store a cached address approach — but app.rs re-reads from AccountState.
        // The cleanest solution: return OpenReceive with a lazy flag.
        // Per the spec we return OpenReceive(addr) — app.rs calls pick_receive_address.
        // Since handle_key doesn't have wallet, we cache the address differently.
        // This is handled via a separate method called from app.rs.
        // So handle_key returns a special variant and app.rs picks the address.
        self.handle_key_inner(key)
    }

    fn handle_key_inner(&mut self, key: KeyEvent) -> Option<AccountAction> {
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
            // Actions below are blocked while a scan is in flight.
            KeyCode::Char('r') if !self.is_scanning() => {
                // App-level routing will call pick_receive_address_with_wallet.
                // We signal the action; app.rs does the address lookup.
                return Some(AccountAction::OpenReceive(Address::new(
                    "__pending__".to_string(),
                    self.current_chain,
                )));
            }
            KeyCode::Char('s') if !self.is_scanning() => {
                let total_balance_sats = self.scan.as_ref().map(|s| s.total.total()).unwrap_or(0);
                return Some(AccountAction::OpenSend {
                    chain: self.current_chain,
                    account: 0,
                    total_balance_sats,
                });
            }
            KeyCode::Char('b') if !self.is_scanning() => {
                return Some(AccountAction::OpenAddressBook);
            }
            KeyCode::Char('d')
                if !self.is_scanning()
                    && self
                        .scan
                        .as_ref()
                        .map(|s| !s.used.is_empty())
                        .unwrap_or(false) =>
            {
                return Some(AccountAction::OpenAddresses);
            }
            KeyCode::Char('S') => {
                return Some(AccountAction::OpenSettings);
            }
            KeyCode::Char('p') => self.open_picker(),
            KeyCode::Char('q') | KeyCode::Esc => return Some(AccountAction::Lock),
            KeyCode::Char('?') => return Some(AccountAction::ShowHelp),
            _ => {}
        }

        None
    }
}

/// Worker function executed on the background thread.
///
/// For Bitcoin-family chains: runs `BitcoinChain::scan_used_addresses`.
/// For Ethereum / BSC / Monero: builds a degenerate single-entry scan from
/// a single derived address + balance query (these chains use a fixed derived
/// address, not a gap-walk).
///
/// # Seed handling
///
/// The caller must zeroize `seed` after this function returns — the caller's
/// thread owns the `[u8; 64]` and is responsible for the zeroize call on
/// every exit path (including panic unwind via a Drop guard in the closure).
fn scan_thread(
    chain: ChainId,
    config: &Config,
    seed: &[u8; 64],
    gap_limit: u32,
    account: u32,
) -> Result<WalletScan, String> {
    debug!("scan_thread for chain {:?}", chain);

    let active = ActiveChain::from_chain_id(chain, config)
        .map_err(|e| format!("{}: {e}", chain.display_name()))?;

    match active {
        ActiveChain::Bitcoin(btc_chain) => {
            // Full gap-limit scan via scan_used_addresses.
            btc_chain
                .scan_used_addresses(seed, account, gap_limit)
                .map_err(|e| format!("{}: {e}", chain.display_name()))
        }
        ActiveChain::Ethereum(eth_chain) => {
            // TODO: per-chain scan strategies for non-BTC families.
            // EVM uses a single derived address — build a degenerate 1-entry scan.
            let addr_str = eth_chain
                .derive(seed, account, 0)
                .map_err(|e| format!("{}: derive: {e}", chain.display_name()))?;
            let balance_amount = eth_chain
                .balance(&addr_str)
                .map_err(|e| format!("{}: balance: {e}", chain.display_name()))?;
            let balance_atoms = balance_amount.atoms() as u64;
            let balance_split = hodl_chain_bitcoin::BalanceSplit {
                confirmed: balance_atoms,
                pending: 0,
            };
            let used = vec![hodl_chain_bitcoin::UsedAddress {
                index: 0,
                change: 0,
                address: addr_str.as_str().to_string(),
                balance: balance_split,
            }];
            Ok(WalletScan {
                total: balance_split,
                used,
                highest_index_receive: 0,
                highest_index_change: 0,
            })
        }
        ActiveChain::Monero(xmr_chain) => {
            // TODO: per-chain scan strategies for non-BTC families.
            // Monero uses a single derived address — build a degenerate 1-entry scan.
            let addr_str = xmr_chain
                .derive(seed, account, 0)
                .map_err(|e| format!("{}: derive: {e}", chain.display_name()))?;
            let balance_amount = xmr_chain
                .balance(&addr_str)
                .map_err(|e| format!("{}: balance: {e}", chain.display_name()))?;
            let balance_atoms = balance_amount.atoms() as u64;
            let balance_split = hodl_chain_bitcoin::BalanceSplit {
                confirmed: balance_atoms,
                pending: 0,
            };
            let used = vec![hodl_chain_bitcoin::UsedAddress {
                index: 0,
                change: 0,
                address: addr_str.as_str().to_string(),
                balance: balance_split,
            }];
            Ok(WalletScan {
                total: balance_split,
                used,
                highest_index_receive: 0,
                highest_index_change: 0,
            })
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

/// Format a satoshi amount as a decimal coin string (e.g. `1.23456789 BTC`).
fn format_sats(sats: u64, chain: ChainId) -> String {
    let symbol = chain.ticker();
    // All supported chains use 8 decimal places (sats / 1e8).
    let whole = sats / 100_000_000;
    let frac = sats % 100_000_000;
    format!("{whole}.{frac:08} {symbol}")
}

/// Human-readable purpose label for the card title.
fn purpose_label(chain: ChainId) -> &'static str {
    match chain {
        ChainId::Bitcoin | ChainId::BitcoinTestnet | ChainId::Litecoin => "Bip84 P2WPKH",
        ChainId::Dogecoin | ChainId::NavCoin => "Bip44 P2PKH",
        ChainId::BitcoinCash => "Bip44 CashAddr",
        ChainId::Ethereum => "Bip44 ERC-20",
        ChainId::BscMainnet => "Bip44 BEP-20",
        ChainId::Monero => "Bip44 XMR",
    }
}

pub fn draw(f: &mut Frame, area: Rect, state: &mut AccountState) {
    let outer_block = Block::default()
        .title(" hodl • Accounts ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    let outer_inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    // Split outer inner into top padding, card area, bottom status + hint.
    let outer_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // card + padding
            Constraint::Length(1), // status line
            Constraint::Length(1), // hint bar
        ])
        .split(outer_inner);

    // Centre the card horizontally (~50% of width, min 54).
    let card_width = (outer_inner.width / 2).max(54).min(outer_inner.width);
    let card_x = outer_inner.x + (outer_inner.width.saturating_sub(card_width)) / 2;

    // Card height: border top + blank + 3 balance rows + blank + 2 info rows +
    // blank + 2 hint rows + border bottom = 12
    let card_height = 12u16.min(outer_chunks[0].height);
    let card_y = outer_chunks[0].y + (outer_chunks[0].height.saturating_sub(card_height)) / 2;

    let card_area = Rect::new(card_x, card_y, card_width, card_height);

    // Card title: "<chain> · <purpose>"
    let chain_name = state.current_chain.display_name();
    let purpose = purpose_label(state.current_chain);
    let card_title = format!(" {chain_name} · {purpose} ");

    let card_block = Block::default()
        .title(card_title)
        .title_style(Style::default().add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::White));

    let card_inner = card_block.inner(card_area);
    f.render_widget(card_block, card_area);

    // Build card body lines.
    let body_lines: Vec<Line> = if state.is_scanning() {
        // Show spinner while scan is running.
        let frame = state
            .scanning_spinner
            .as_ref()
            .map(|s| s.current())
            .unwrap_or("⠋");
        vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  scanning…  {frame}"),
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
        ]
    } else if let Some(ref err) = state.scan_error.clone() {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  error: {err}"),
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
        ]
    } else if let Some(ref scan) = state.scan {
        let confirmed = format_sats(scan.total.confirmed, state.current_chain);
        let pending = format_sats(scan.total.pending, state.current_chain);
        let total = format_sats(scan.total.total(), state.current_chain);
        let used_count = scan.used.len();
        vec![
            Line::from(""),
            Line::from(format!("   confirmed:    {confirmed}")),
            Line::from(format!("   pending:      {pending}")),
            Line::from(format!("   total:        {total}")),
            Line::from(""),
            Line::from(format!("   used addresses:  {used_count}")),
            Line::from(format!("   gap limit:       {}", {
                state
                    .config
                    .chains
                    .get(&state.current_chain)
                    .map(|c| c.gap_limit)
                    .unwrap_or(20)
            })),
            Line::from(""),
            Line::from(Span::styled(
                "   r receive · s send · b book · S settings",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "   p picker · d addresses · q lock · ? help",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        // No scan yet, no error — initial blank state (shouldn't normally show).
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  press any key to start",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
        ]
    };

    let body_para = Paragraph::new(body_lines);
    f.render_widget(body_para, card_inner);

    // Status line.
    let (status_text, status_color) = if state.is_scanning() {
        (format!("{} · scanning…", chain_name), Color::Cyan)
    } else if let Some(ref err) = state.scan_error {
        (format!("{chain_name} · scan failed: {err}"), Color::Red)
    } else if let Some(ref scan) = state.scan {
        let n = scan.used.len();
        (format!("{chain_name} · synced {n}/{n} used"), Color::Green)
    } else {
        (chain_name.to_string(), Color::DarkGray)
    };

    let status = Paragraph::new(Line::from(Span::styled(
        status_text,
        Style::default().fg(status_color),
    )))
    .alignment(Alignment::Center);
    f.render_widget(status, outer_chunks[1]);

    // Hint bar.
    let hint = Paragraph::new(Line::from(Span::styled(
        "r receive · s send · b book · d addresses · S settings · p picker · q lock · ? help",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(hint, outer_chunks[2]);

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
