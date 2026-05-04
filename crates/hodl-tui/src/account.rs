//! Account screen — per-chain summary card for the unlocked wallet.
//!
//! Chain selection drives `ActiveChain::from_chain_id` — the picker is no
//! longer decorative; switching chains re-scans against the new backend.
//!
//! ## Loading flow
//!
//! `start_load` spawns a background thread that opens the Electrum/RPC
//! connection, runs a BIP-44 gap-limit scan (Bitcoin family) or derives a
//! single address with its balance (EVM / Monero), and sends `ScanEvent`
//! messages over a channel. The event loop polls via `poll_scan()` each
//! iteration:
//! - `ScanEvent::Used(used)` → append to `partial_scan`; redraw immediately.
//! - `ScanEvent::Done(scan)` → swap `partial_scan` into `scan`; clear pending.
//! - `ScanEvent::Error(msg)` → set scan_error; clear pending.
//! - `TryRecvError::Empty`   → tick `scanning_spinner`; no redraw.
//! - `TryRecvError::Disconnected` → set scan_error to "scan thread panicked".
//!
//! While scanning, navigation keys that depend on scan results (`r`/`s`/`b`/`d`)
//! are suppressed. `q`, `S`, `p`, Ctrl-C/D, and `?` always work.
//!
//! The summary card shows partial results (live count + running balance) while
//! scanning, with the spinner ticking next to the numbers. The Addresses
//! sub-view (`d`) is still gated on a completed scan snapshot.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hjkl_picker::{PickerAction, PickerEvent, PickerLogic};
use hodl_chain_bitcoin::{UsedAddress, WalletScan};
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

/// Events sent from the scan worker thread to the UI.
enum ScanEvent {
    /// New used address discovered. Append to partial_scan and bump running total.
    Used(UsedAddress),
    /// Scan finished. Final WalletScan replaces the partial accumulator.
    Done(WalletScan),
    /// Scan errored. Surface via scan_error; clear pending.
    Error(String),
    /// Mid-scan network failure — UI should clear `partial_scan` so the
    /// retry attempt rebuilds from scratch (avoids duplicate Used entries
    /// from the previous server's partial result). The worker logs the
    /// underlying reason via `tracing::debug!`; the UI surfaces only the
    /// attempt count in the status line.
    Reset { attempt: u32 },
}

/// Maximum scan attempts before giving up. Each attempt rebuilds the
/// `ActiveChain` (via `from_chain_id` → `try_endpoints`), which re-shuffles
/// the endpoint list, so successive attempts try different servers.
const MAX_SCAN_ATTEMPTS: u32 = 3;

/// Action emitted by the account screen to the parent app loop.
#[derive(Debug)]
pub enum AccountAction {
    /// Navigate to the receive screen — app.rs picks the address via
    /// `pick_receive_address` (first used receive address, or derive 0).
    OpenReceive,
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
    /// In-flight scan event channel. `Some` while the background thread is running.
    pending_scan: Option<Receiver<ScanEvent>>,
    /// Partial result accumulated from `ScanEvent::Used` events while
    /// `pending_scan` is `Some`. Promoted to `self.scan` on `ScanEvent::Done`.
    partial_scan: WalletScan,
    /// Current retry attempt (1 = first attempt, increments on Reset).
    /// Surfaced in the status line so the user can see the wallet is
    /// failing over to a different Electrum server.
    scan_attempt: u32,
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
            partial_scan: WalletScan::default(),
            scan_attempt: 0,
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

    /// Tick the scanning spinner (called by the event loop on `TryRecvError::Empty`).
    pub fn tick_spinner(&mut self) {
        if let Some(ref mut s) = self.scanning_spinner {
            s.tick();
        }
    }

    /// Poll the pending scan channel, draining all queued events in one pass.
    ///
    /// Returns `true` if state changed (caller should redraw), `false` if the
    /// channel was empty and only the spinner was ticked.
    pub fn poll_scan(&mut self) -> bool {
        let rx = match &self.pending_scan {
            Some(rx) => rx,
            None => return false,
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(ScanEvent::Used(used)) => {
                    self.partial_scan.total.confirmed += used.balance.confirmed;
                    self.partial_scan.total.pending += used.balance.pending;
                    self.partial_scan.used.push(used);
                    changed = true;
                }
                Ok(ScanEvent::Done(final_scan)) => {
                    self.scan = Some(final_scan);
                    self.scan_error = None;
                    self.partial_scan = WalletScan::default();
                    self.pending_scan = None;
                    self.scanning_spinner = None;
                    return true;
                }
                Ok(ScanEvent::Error(msg)) => {
                    self.scan_error = Some(msg);
                    self.scan = None;
                    self.partial_scan = WalletScan::default();
                    self.pending_scan = None;
                    self.scanning_spinner = None;
                    return true;
                }
                Ok(ScanEvent::Reset { attempt }) => {
                    // Worker hit a network error and is failing over to a
                    // different Electrum server. Drop the partial state from
                    // the previous attempt; the retry rebuilds from scratch.
                    self.partial_scan = WalletScan::default();
                    self.scan_attempt = attempt;
                    changed = true;
                }
                Err(TryRecvError::Empty) => {
                    if !changed {
                        self.tick_spinner();
                    }
                    return changed;
                }
                Err(TryRecvError::Disconnected) => {
                    self.scan_error = Some("scan thread panicked".into());
                    self.scan = None;
                    self.partial_scan = WalletScan::default();
                    self.pending_scan = None;
                    self.scanning_spinner = None;
                    return true;
                }
            }
        }
    }

    /// Spawn a background thread to run the gap-limit scan.
    ///
    /// For Bitcoin-family chains the worker calls
    /// `scan_used_addresses_streaming` and sends a `ScanEvent::Used` for every
    /// discovered address before a final `ScanEvent::Done`. For EVM/Monero the
    /// degenerate single-address case sends one `Used` followed by `Done` so
    /// the consumer's event loop is uniform across all chain types.
    pub fn start_load(&mut self, wallet: &UnlockedWallet) {
        debug!("start_load (scan) for chain {:?}", self.current_chain);

        // Clear stale data so the loading state is visible immediately.
        self.scan = None;
        self.scan_error = None;
        self.partial_scan = WalletScan::default();
        self.scan_attempt = 1;
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
            // (not a fresh Copy) after the worker returns on every exit path.
            let mut seed = seed;
            scan_thread_streaming(chain, &config, &seed, gap_limit, 0, &tx);
            seed.zeroize();
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

    /// Pick the best receive address + its derivation path for the Receive
    /// screen.
    ///
    /// Returns the first used receive address (change=0) if any exist;
    /// otherwise falls back to deriving index 0. The path matches the chain's
    /// actual purpose via `ActiveChain::derivation_path`.
    pub fn pick_receive(&self, wallet: &UnlockedWallet) -> Option<(Address, String)> {
        let active = ActiveChain::from_chain_id(self.current_chain, &self.config).ok()?;

        // Try first used receive address (change=0) from the scan.
        if let Some(scan) = &self.scan
            && let Some(used) = scan.used.iter().find(|u| u.change == 0)
        {
            let addr = Address::new(used.address.clone(), self.current_chain);
            let path = active.derivation_path(0, used.index);
            return Some((addr, path));
        }

        // Fallback: derive index 0 from the wallet seed. Zeroize the local
        // copy once the derive returns (see AGENTS.md security rules).
        let mut seed: [u8; 64] = *wallet.seed().as_bytes();
        let result = active.derive(&seed, 0, 0).ok();
        seed.zeroize();
        let addr = result?;
        let path = active.derivation_path(0, 0);
        Some((addr, path))
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
                return Some(AccountAction::OpenReceive);
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

/// Streaming worker function executed on the background thread.
///
/// For Bitcoin-family chains: calls `scan_used_addresses_streaming`, sending a
/// `ScanEvent::Used` for each discovered address as it is found, then
/// `ScanEvent::Done` with the final `WalletScan` once the walk completes.
///
/// For Ethereum / BSC / Monero: builds a degenerate single-entry scan from
/// a single derived address + balance query, sends one `ScanEvent::Used`
/// followed by `ScanEvent::Done` to keep the consumer uniform.
///
/// On any error, sends `ScanEvent::Error(msg)` and returns.
///
/// # Seed handling
///
/// The caller must zeroize `seed` after this function returns — the caller's
/// thread owns the `[u8; 64]` and is responsible for the zeroize call on
/// every exit path. The callback closure receives only `&UsedAddress` (no
/// seed material).
fn scan_thread_streaming(
    chain: ChainId,
    config: &Config,
    seed: &[u8; 64],
    gap_limit: u32,
    account: u32,
    tx: &std::sync::mpsc::Sender<ScanEvent>,
) {
    debug!("scan_thread_streaming for chain {:?}", chain);

    // Outer retry loop: each attempt rebuilds the ActiveChain via
    // `from_chain_id` → `try_endpoints`, which re-shuffles the endpoint
    // list. So consecutive attempts try different servers (with a small
    // random chance of repeating). Capped at MAX_SCAN_ATTEMPTS.
    for attempt in 1..=MAX_SCAN_ATTEMPTS {
        // After the first attempt, signal a reset so the UI clears any
        // partial Used entries from the previous server's run.
        if attempt > 1 {
            let _ = tx.send(ScanEvent::Reset { attempt });
        }

        let outcome = run_scan_attempt(chain, config, seed, gap_limit, account, tx);
        match outcome {
            AttemptResult::Done => return,
            AttemptResult::Fatal(msg) => {
                let _ = tx.send(ScanEvent::Error(msg));
                return;
            }
            AttemptResult::Retry(reason) => {
                debug!("scan attempt {attempt} failed: {reason}; retrying");
                if attempt == MAX_SCAN_ATTEMPTS {
                    // `reason` is already prefixed with the chain name by
                    // classify(); don't double-prefix.
                    let _ = tx.send(ScanEvent::Error(format!(
                        "all {MAX_SCAN_ATTEMPTS} endpoints failed — last: {reason}"
                    )));
                    return;
                }
            }
        }
    }
}

/// Outcome of a single scan attempt against a freshly-built ActiveChain.
enum AttemptResult {
    /// Scan completed successfully — `ScanEvent::Done` was already sent.
    Done,
    /// Non-retryable error (config, codec, chain logic). Surface to UI as
    /// the final scan_error and stop trying.
    Fatal(String),
    /// Retryable error (network/IO). Outer loop will reconnect to a
    /// different server and try again.
    Retry(String),
}

fn run_scan_attempt(
    chain: ChainId,
    config: &Config,
    seed: &[u8; 64],
    gap_limit: u32,
    account: u32,
    tx: &std::sync::mpsc::Sender<ScanEvent>,
) -> AttemptResult {
    let active = match ActiveChain::from_chain_id(chain, config) {
        Ok(a) => a,
        Err(e) => {
            // Connect failure across all endpoints — try_endpoints already
            // retried internally, no point in retrying at this layer.
            return classify(chain, "connect", e);
        }
    };

    match active {
        ActiveChain::Bitcoin(btc_chain) => {
            let tx_clone = tx.clone();
            let mut on_used = |used: &UsedAddress| {
                let _ = tx_clone.send(ScanEvent::Used(used.clone()));
            };
            match btc_chain.scan_used_addresses_streaming(seed, account, gap_limit, &mut on_used) {
                Ok(scan) => {
                    let _ = tx.send(ScanEvent::Done(scan));
                    AttemptResult::Done
                }
                Err(e) => classify(chain, "scan", e),
            }
        }
        ActiveChain::Ethereum(eth_chain) => single_address_scan(chain, tx, || {
            let addr = eth_chain
                .derive(seed, account, 0)
                .map_err(|e| (e, "derive"))?;
            let amount = eth_chain.balance(&addr).map_err(|e| (e, "balance"))?;
            Ok((addr.as_str().to_string(), amount.atoms() as u64))
        }),
        ActiveChain::Monero(xmr_chain) => single_address_scan(chain, tx, || {
            let addr = xmr_chain
                .derive(seed, account, 0)
                .map_err(|e| (e, "derive"))?;
            let amount = xmr_chain.balance(&addr).map_err(|e| (e, "balance"))?;
            Ok((addr.as_str().to_string(), amount.atoms() as u64))
        }),
    }
}

/// Map a `hodl_core::Error` into an `AttemptResult` via its retry-ability:
/// network/IO failures retry; everything else is fatal.
fn classify(chain: ChainId, stage: &str, e: hodl_core::error::Error) -> AttemptResult {
    use hodl_core::error::Error;
    let msg = format!("{}: {stage}: {e}", chain.display_name());
    match e {
        Error::Network(_) | Error::Io(_) | Error::Endpoint(_) => AttemptResult::Retry(msg),
        Error::Codec(_) | Error::Chain(_) | Error::Config(_) => AttemptResult::Fatal(msg),
    }
}

/// Run a single derive-and-balance call as one Used + one Done event.
/// Used by EVM/Monero where the wallet has only one externally-derived
/// address. The closure returns `(address_string, balance_atoms)`.
fn single_address_scan(
    chain: ChainId,
    tx: &std::sync::mpsc::Sender<ScanEvent>,
    work: impl FnOnce() -> std::result::Result<(String, u64), (hodl_core::error::Error, &'static str)>,
) -> AttemptResult {
    match work() {
        Ok((address, atoms)) => {
            let balance = hodl_chain_bitcoin::BalanceSplit {
                confirmed: atoms,
                pending: 0,
            };
            let used = UsedAddress {
                index: 0,
                change: 0,
                address,
                balance,
            };
            let _ = tx.send(ScanEvent::Used(used.clone()));
            let _ = tx.send(ScanEvent::Done(WalletScan {
                total: balance,
                used: vec![used],
                highest_index_receive: 0,
                highest_index_change: 0,
            }));
            AttemptResult::Done
        }
        Err((e, stage)) => classify(chain, stage, e),
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

    let gap_limit = state
        .config
        .chains
        .get(&state.current_chain)
        .map(|c| c.gap_limit)
        .unwrap_or(20);

    // Build card body lines.
    let body_lines: Vec<Line> =
        if !state.is_scanning() && state.scan.is_none() && state.scan_error.is_none() {
            // Initial state pre-scan — render placeholder.
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  press any key to start",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
            ]
        } else if !state.is_scanning() {
            if let Some(ref err) = state.scan_error.clone() {
                // Scan failed — render error in the card body.
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("  error: {err}"),
                        Style::default().fg(Color::Red),
                    )),
                    Line::from(""),
                ]
            } else if let Some(ref scan) = state.scan {
                // Scan complete — render full card.
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
                    Line::from(format!("   gap limit:       {gap_limit}")),
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
                vec![]
            }
        } else {
            // is_scanning() — render PARTIAL state from self.partial_scan with
            // spinner ticking next to the numbers.
            let frame = state
                .scanning_spinner
                .as_ref()
                .map(|s| s.current())
                .unwrap_or("⠋");
            let confirmed = format_sats(state.partial_scan.total.confirmed, state.current_chain);
            let pending = format_sats(state.partial_scan.total.pending, state.current_chain);
            let total = format_sats(state.partial_scan.total.total(), state.current_chain);
            let used_count = state.partial_scan.used.len();
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw(format!("   confirmed:    {confirmed}  ")),
                    Span::styled(frame, Style::default().fg(Color::Cyan)),
                ]),
                Line::from(vec![
                    Span::raw(format!("   pending:      {pending}  ")),
                    Span::styled(frame, Style::default().fg(Color::Cyan)),
                ]),
                Line::from(vec![
                    Span::raw(format!("   total:        {total}  ")),
                    Span::styled(frame, Style::default().fg(Color::Cyan)),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::raw(format!("   used addresses:  {used_count}  ")),
                    Span::styled(frame, Style::default().fg(Color::Cyan)),
                ]),
                Line::from(format!("   gap limit:       {gap_limit}")),
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
        };

    let body_para = Paragraph::new(body_lines);
    f.render_widget(body_para, card_inner);

    // Status line.
    let (status_text, status_color) = if state.is_scanning() {
        let frame = state
            .scanning_spinner
            .as_ref()
            .map(|s| s.current())
            .unwrap_or("⠋");
        let n = state.partial_scan.used.len();
        let attempt_suffix = if state.scan_attempt > 1 {
            format!(" (attempt {}/{MAX_SCAN_ATTEMPTS})", state.scan_attempt)
        } else {
            String::new()
        };
        (
            format!("{chain_name} · scanning {n} used so far{attempt_suffix}  {frame}"),
            Color::Cyan,
        )
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
