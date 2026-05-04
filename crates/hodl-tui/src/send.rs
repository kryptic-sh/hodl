//! Send screen — build + sign + broadcast a transaction on the active chain.
//!
//! Driven by `hjkl-form`. Fields:
//!   0 = recipient (address TextFieldEditor + per-chain validator)
//!   1 = amount (TextFieldEditor + validator)
//!   2 = fee tier SelectField (Slow / Normal / Fast / Custom)
//!   3 = custom sat/vB (TextFieldEditor; Bitcoin only, ignored for EVM)
//!   4 = RBF checkbox (BIP-125; Bitcoin only, ignored for non-BTC)
//!   5 = "Sign & broadcast" SubmitField
//!
//! Submit pipeline (all network I/O off the UI thread):
//!   1. Validate recipient address per active chain codec.
//!   2. Build `ActiveChain`, run `estimate_fee` + `build_send` → `PreparedSend`.
//!      Shown as `building…  ⠋` spinner.
//!   3. `sign_and_broadcast` → `TxId` displayed in result pane.
//!      Shown as `broadcasting…  ⠋` spinner.
//!
//! After broadcast: result pane shows TxId.
//! `q` / Esc returns to Accounts.
//!
//! Tab/Enter are blocked while building or broadcasting so the user cannot
//! double-submit. Text editing in form fields is still allowed during the
//! build phase (the build thread has already captured the values).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use hjkl_form::{
    CheckboxField, Field, FieldMeta, Form, FormMode, Input, SelectField, SubmitField,
    TextFieldEditor, Validator,
};
use hjkl_picker::{PickerAction, PickerEvent, PickerLogic};
use hjkl_ratatui::form::{FormPalette, draw_form};
use hodl_config::{AddressBook, Config, Contact, KnownHosts};
use hodl_core::{Address, Amount, ChainId, FeeRate};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tracing::debug;
use zeroize::Zeroize;

use hodl_wallet::UnlockedWallet;

use crate::active_chain::{ActiveChain, PreparedSend, SendOpts};
use crate::help::{HelpAction, HelpOverlay};
use crate::retry::{self, MAX_ATTEMPTS as MAX_SEND_ATTEMPTS};

// ── Field indices ──────────────────────────────────────────────────────────

const FIELD_RECIPIENT: usize = 0;
const FIELD_AMOUNT: usize = 1;
const FIELD_FEE_TIER: usize = 2;
const FIELD_CUSTOM_FEE: usize = 3;
const FIELD_RBF: usize = 4;

const FEE_TIERS: &[&str] = &[
    "Slow (12 blocks)",
    "Normal (6 blocks)",
    "Fast (2 blocks)",
    "Custom",
];

/// Action emitted by the send screen.
#[derive(Debug)]
pub enum SendAction {
    Back,
    Quit,
}

// ── Validators ─────────────────────────────────────────────────────────────

/// Validate a recipient address for the given chain.
///
/// Per chain:
/// - BTC / BTC-testnet / LTC / NAV: bech32 segwit v0 (P2WPKH) preferred,
///   legacy P2PKH base58check accepted as fallback.
/// - DOGE: legacy P2PKH base58check only — DOGE never deployed bech32.
/// - BCH: CashAddr (`bitcoincash:q…`) only — BCH replaced base58 P2PKH.
/// - ETH / BSC: EIP-55 hex.
/// - Monero: not yet implemented.
///
/// The chain crate's `decode_address_to_script` re-validates against the
/// chain's prefix bytes so a wrong-chain address is caught at signing time
/// even if it slips past this front-door check.
pub fn validate_recipient(s: &str, chain: ChainId) -> std::result::Result<(), String> {
    if s.is_empty() {
        return Err("recipient cannot be empty".into());
    }
    use ChainId::*;
    match chain {
        Bitcoin | BitcoinTestnet | Litecoin => {
            // Try bech32 first (the wallet's preferred default).
            if let Ok((_, ver, prog)) = bech32::segwit::decode(s)
                && ver == bech32::segwit::VERSION_0
                && prog.len() == 20
            {
                return Ok(());
            }
            // Fall back to legacy base58check P2PKH.
            validate_base58_p2pkh(s)
        }
        // NavCoin and Dogecoin: legacy P2PKH only — bech32/segwit not
        // deployed in upstream node software.
        Dogecoin | NavCoin => validate_base58_p2pkh(s),
        BitcoinCash => {
            if !s.starts_with("bitcoincash:") {
                return Err("BCH recipient must be CashAddr (bitcoincash:q…)".into());
            }
            // Quick shape check; full polymod verify happens at sign time.
            if s.len() < 14 {
                return Err("BCH CashAddr too short".into());
            }
            Ok(())
        }
        Ethereum | BscMainnet => hodl_chain_ethereum::address::from_str_normalized(s)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Monero => Err("Monero send not implemented".into()),
    }
}

/// Validate a base58check-encoded P2PKH address shape (21 bytes after decode).
///
/// Doesn't verify the version byte against any specific chain — that check
/// happens at the chain crate's `decode_address_to_script` boundary, which
/// has access to the chain's expected `p2pkh_prefix`.
fn validate_base58_p2pkh(s: &str) -> std::result::Result<(), String> {
    let decoded = bs58::decode(s)
        .with_check(None)
        .into_vec()
        .map_err(|e| format!("base58 decode: {e}"))?;
    if decoded.len() != 21 {
        return Err(format!(
            "P2PKH address must decode to 21 bytes (got {})",
            decoded.len()
        ));
    }
    Ok(())
}

/// Validate an amount string: positive decimal number of BTC.
pub fn validate_amount(s: &str) -> std::result::Result<(), String> {
    if s.is_empty() {
        return Err("amount cannot be empty".into());
    }
    match s.parse::<f64>() {
        Ok(v) if v > 0.0 => Ok(()),
        Ok(_) => Err("amount must be > 0".into()),
        Err(_) => Err(format!("'{s}' is not a valid decimal amount")),
    }
}

/// Validate a custom sat/vB rate: positive integer.
pub fn validate_custom_fee(s: &str) -> std::result::Result<(), String> {
    match s.parse::<u64>() {
        Ok(v) if v > 0 => Ok(()),
        Ok(_) => Err("fee rate must be > 0 sat/vB".into()),
        Err(_) => Err(format!("'{s}' is not a valid integer sat/vB")),
    }
}

fn mk_validator<F>(f: F) -> Validator
where
    F: Fn(&str) -> std::result::Result<(), String> + Send + 'static,
{
    Box::new(f)
}

// ── Payload sent between build and broadcast threads ───────────────────────

/// Everything the broadcast thread needs, after the build thread succeeds.
struct BroadcastPayload {
    chain: ChainId,
    config: Config,
    seed: [u8; 64],
    account: u32,
    send_params: hodl_core::SendParams,
    prepared: PreparedSend,
    known_hosts: Arc<Mutex<KnownHosts>>,
    data_root: PathBuf,
}

// ── State machine ──────────────────────────────────────────────────────────

/// Outcome of one send attempt (build or broadcast). Parallel to
/// `retry::AttemptResult` but carries a success value `T` on `Done` so the
/// send threads can return their result by value rather than via a channel.
enum SendAttempt<T> {
    /// Attempt succeeded; carry the result.
    Done(T),
    /// Non-retryable error; stop immediately.
    Fatal(String),
    /// Retryable error; outer loop will try again.
    Retry(String),
}

enum Phase {
    Form,
    /// Off-thread: estimate_fee + build_send. Carries the channel and an
    /// atomic attempt counter updated by the worker on each retry.
    Building(Receiver<Result<BroadcastPayload, String>>, Arc<AtomicU32>),
    /// Off-thread: sign_and_broadcast. Carries the channel and an atomic
    /// attempt counter updated by the worker on each retry.
    Broadcasting(Receiver<Result<String, String>>, Arc<AtomicU32>),
    /// Broadcast succeeded; hold TxId string.
    Result(String),
    /// Submit failed; error message.
    Error(String),
}

pub struct SendState {
    chain: ChainId,
    account: u32,
    total_balance_sats: u64,
    form: Form,
    phase: Phase,
    config: Config,
    known_hosts: Arc<Mutex<KnownHosts>>,
    data_root: PathBuf,
    book: AddressBook,
    book_picker: Option<hjkl_picker::Picker>,
    flash: Option<String>,
}

impl SendState {
    pub fn new(
        chain: ChainId,
        account: u32,
        total_balance_sats: u64,
        config: Config,
        known_hosts: Arc<Mutex<KnownHosts>>,
        data_root: PathBuf,
    ) -> Self {
        let form = make_send_form(chain, total_balance_sats);
        let book_path =
            AddressBook::default_path().unwrap_or_else(|_| data_root.join("address_book.toml"));
        let book = AddressBook::load(&book_path).unwrap_or_default();
        Self {
            chain,
            account,
            total_balance_sats,
            form,
            phase: Phase::Form,
            config,
            known_hosts,
            data_root,
            book,
            book_picker: None,
            flash: None,
        }
    }

    #[cfg(test)]
    fn new_with_book(chain: ChainId, total_balance_sats: u64, book: AddressBook) -> Self {
        let form = make_send_form(chain, total_balance_sats);
        Self {
            chain,
            account: 0,
            total_balance_sats,
            form,
            phase: Phase::Form,
            config: Config::default(),
            known_hosts: Arc::new(Mutex::new(KnownHosts::default())),
            data_root: PathBuf::from("/tmp"),
            book,
            book_picker: None,
            flash: None,
        }
    }

    /// Open the address-book picker filtered to contacts matching `self.chain`.
    /// Sets `self.flash` and returns without opening if no matching contacts exist.
    fn open_book_picker(&mut self) {
        let contacts: Vec<Contact> = self
            .book
            .entries
            .iter()
            .filter(|c| c.chain == self.chain)
            .cloned()
            .collect();
        if contacts.is_empty() {
            self.flash = Some(format!("no contacts for {}", self.chain.display_name()));
            return;
        }
        self.flash = None;
        let source = ContactPickerSource { contacts };
        self.book_picker = Some(hjkl_picker::Picker::new(Box::new(source)));
    }

    /// Route a key event into the open book picker. Returns `true` if the picker
    /// handled the key and the caller should skip normal form handling.
    fn handle_book_picker_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        let Some(picker) = &mut self.book_picker else {
            return false;
        };
        match picker.handle_key(key) {
            PickerEvent::Cancel => {
                self.book_picker = None;
            }
            PickerEvent::Select(PickerAction::OpenPath(addr_path)) => {
                let addr = addr_path.to_string_lossy().into_owned();
                if let Some(Field::SingleLineText(f)) = self.form.fields.get_mut(FIELD_RECIPIENT) {
                    f.set_text(&addr);
                }
                self.book_picker = None;
            }
            PickerEvent::Select(_) | PickerEvent::None => {
                if let Some(p) = &mut self.book_picker {
                    p.refresh();
                }
            }
        }
        true
    }

    fn field_text(&self, idx: usize) -> String {
        match self.form.fields.get(idx) {
            Some(Field::SingleLineText(f)) => f.text(),
            _ => String::new(),
        }
    }

    fn selected_tier(&self) -> Option<&str> {
        match self.form.fields.get(FIELD_FEE_TIER) {
            Some(Field::Select(s)) => s.selected(),
            _ => None,
        }
    }

    fn fee_target_blocks(&self) -> u32 {
        match self.selected_tier().unwrap_or("Normal") {
            t if t.starts_with("Slow") => 12,
            t if t.starts_with("Fast") => 2,
            _ => 6,
        }
    }

    fn rbf_checked(&self) -> bool {
        match self.form.fields.get(FIELD_RBF) {
            Some(Field::Checkbox(c)) => c.value,
            _ => false,
        }
    }

    /// `true` while a background operation is in flight.
    pub fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Building(_, _) | Phase::Broadcasting(_, _)
        )
    }

    /// Poll in-flight channels. Returns `true` if the phase changed.
    pub fn poll(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;
        match &mut self.phase {
            Phase::Building(rx, _attempt) => {
                match rx.try_recv() {
                    Ok(Ok(payload)) => {
                        // Build succeeded — kick off broadcast thread.
                        let (tx, brx) = mpsc::channel();
                        let bcast_attempt = Arc::new(AtomicU32::new(1));
                        let bcast_attempt_worker = Arc::clone(&bcast_attempt);
                        std::thread::spawn(move || {
                            let result = broadcast_thread(payload, bcast_attempt_worker);
                            let _ = tx.send(result);
                        });
                        self.phase = Phase::Broadcasting(brx, bcast_attempt);
                        true
                    }
                    Ok(Err(msg)) => {
                        self.phase = Phase::Error(msg);
                        true
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.phase = Phase::Error("build thread panicked — try again".into());
                        true
                    }
                    Err(TryRecvError::Empty) => false,
                }
            }
            Phase::Broadcasting(rx, _attempt) => match rx.try_recv() {
                Ok(Ok(txid)) => {
                    self.phase = Phase::Result(txid);
                    true
                }
                Ok(Err(msg)) => {
                    self.phase = Phase::Error(msg);
                    true
                }
                Err(TryRecvError::Disconnected) => {
                    self.phase = Phase::Error("broadcast thread panicked — try again".into());
                    true
                }
                Err(TryRecvError::Empty) => false,
            },
            _ => false,
        }
    }

    /// Keybind reference for the contextual help overlay.
    /// Mode-aware: form-input binds when in Insert mode, navigation binds otherwise.
    ///
    /// `F1` is used as the help trigger instead of `?` so `?` can still be typed
    /// in form fields while in Insert mode.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        match &self.phase {
            Phase::Form => {
                if self.form.mode == FormMode::Insert {
                    vec![
                        ("Esc".into(), "Return to Normal mode".into()),
                        ("F1".into(), "Show this help".into()),
                    ]
                } else {
                    vec![
                        ("i".into(), "Enter Insert mode to edit field".into()),
                        ("Tab / j / k".into(), "Move focus between fields".into()),
                        ("h / l".into(), "Cycle select/checkbox options".into()),
                        ("Ctrl+B".into(), "Pick from address book".into()),
                        ("Enter".into(), "Submit (on Sign & broadcast field)".into()),
                        ("Esc".into(), "Back to accounts".into()),
                        ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
                        ("F1".into(), "Show this help".into()),
                    ]
                }
            }
            Phase::Building(_, _) | Phase::Broadcasting(_, _) => vec![
                ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
                ("F1".into(), "Show this help".into()),
            ],
            Phase::Result(_) => vec![
                ("Enter / q / Esc".into(), "Return to accounts".into()),
                ("F1".into(), "Show this help".into()),
            ],
            Phase::Error(_) => vec![
                ("q / Esc".into(), "Back to accounts".into()),
                ("any other".into(), "Clear error, return to form".into()),
                ("F1".into(), "Show this help".into()),
            ],
        }
    }

    /// Validate inputs and spawn the build thread. Transitions to
    /// `Phase::Building` on success, `Phase::Error` on validation failure.
    fn start_build(&mut self, wallet: &UnlockedWallet) {
        let recipient_str = self.field_text(FIELD_RECIPIENT);
        if let Err(e) = validate_recipient(&recipient_str, self.chain) {
            self.phase = Phase::Error(format!("recipient: {e}"));
            return;
        }

        let amount_str = self.field_text(FIELD_AMOUNT);
        let amount_val: f64 = match amount_str.parse() {
            Ok(v) if v > 0.0 => v,
            _ => {
                self.phase = Phase::Error("invalid amount".into());
                return;
            }
        };
        let amount_atoms = chain_amount_atoms(self.chain, amount_val);

        if amount_atoms > self.total_balance_sats as u128 {
            self.phase = Phase::Error(format!(
                "amount exceeds balance ({} available)",
                self.total_balance_sats
            ));
            return;
        }

        let is_custom = matches!(self.chain, ChainId::Bitcoin | ChainId::BitcoinTestnet)
            && self.selected_tier().unwrap_or("").starts_with("Custom");
        let custom_fee_sats = if is_custom {
            let s = self.field_text(FIELD_CUSTOM_FEE);
            match s.parse::<u64>() {
                Ok(v) if v > 0 => Some(v),
                _ => {
                    self.phase = Phase::Error("invalid custom fee rate".into());
                    return;
                }
            }
        } else {
            None
        };

        let chain = self.chain;
        let config = self.config.clone();
        let seed: [u8; 64] = *wallet.seed().as_bytes();
        let account = self.account;
        let fee_target = self.fee_target_blocks();
        let rbf = self.rbf_checked();
        let chain_cfg = config.chains.get(&chain).cloned().unwrap_or_default();
        let to_addr = Address::new(recipient_str, chain);
        let amount = Amount::from_atoms(amount_atoms, chain);
        let send_params = hodl_core::SendParams {
            from: Address::new(String::new(), chain),
            to: to_addr,
            amount,
            fee: FeeRate::SatsPerVbyte { sats: 1, chain }, // placeholder; overwritten in thread
        };
        let known_hosts = Arc::clone(&self.known_hosts);
        let data_root = self.data_root.clone();

        let (tx, rx) = mpsc::channel();
        let build_attempt = Arc::new(AtomicU32::new(1));
        let build_attempt_worker = Arc::clone(&build_attempt);
        std::thread::spawn(move || {
            // Mutable rebinding so we can zeroize the actual captured array
            // (not a fresh Copy) after the worker returns. The build_thread
            // takes `&[u8; 64]` so it doesn't make its own stack copy.
            let mut seed = seed;
            let result = build_thread(
                chain,
                &config,
                &seed,
                account,
                send_params,
                fee_target,
                custom_fee_sats,
                rbf,
                chain_cfg.gap_limit,
                &known_hosts,
                &data_root,
                build_attempt_worker,
            );
            seed.zeroize();
            let _ = tx.send(result);
        });

        self.phase = Phase::Building(rx, build_attempt);
    }
}

// ── Worker functions ────────────────────────────────────────────────────────

/// Build thread: open chain, estimate fee, build_send. Returns a `BroadcastPayload`.
/// Retries up to `MAX_SEND_ATTEMPTS` times on transient network errors.
#[allow(clippy::too_many_arguments)]
fn build_thread(
    chain: ChainId,
    config: &Config,
    seed: &[u8; 64],
    account: u32,
    send_params: hodl_core::SendParams,
    fee_target: u32,
    custom_fee_sats: Option<u64>,
    rbf: bool,
    gap_limit: u32,
    known_hosts: &Arc<Mutex<KnownHosts>>,
    data_root: &std::path::Path,
    attempt_counter: Arc<AtomicU32>,
) -> Result<BroadcastPayload, String> {
    debug!("build_thread for chain {:?}", chain);

    for attempt in 1..=MAX_SEND_ATTEMPTS {
        attempt_counter.store(attempt, Ordering::Relaxed);
        match try_build_once(
            chain,
            config,
            seed,
            account,
            send_params.clone(),
            fee_target,
            custom_fee_sats,
            rbf,
            gap_limit,
            known_hosts,
            data_root,
        ) {
            SendAttempt::Done(payload) => return Ok(payload),
            SendAttempt::Fatal(msg) => return Err(msg),
            SendAttempt::Retry(reason) => {
                debug!("build attempt {attempt} failed: {reason}; retrying");
                if attempt == MAX_SEND_ATTEMPTS {
                    return Err(format!(
                        "all {MAX_SEND_ATTEMPTS} endpoints failed — last: {reason}"
                    ));
                }
            }
        }
    }
    unreachable!("loop always returns before exhausting attempts")
}

/// One attempt at building a transaction. Never retries internally.
#[allow(clippy::too_many_arguments)]
fn try_build_once(
    chain: ChainId,
    config: &Config,
    seed: &[u8; 64],
    account: u32,
    mut send_params: hodl_core::SendParams,
    fee_target: u32,
    custom_fee_sats: Option<u64>,
    rbf: bool,
    gap_limit: u32,
    known_hosts: &Arc<Mutex<KnownHosts>>,
    data_root: &std::path::Path,
) -> SendAttempt<BroadcastPayload> {
    let active = match ActiveChain::from_chain_id(chain, config, known_hosts, data_root) {
        Ok(a) => a,
        Err(e) => return send_classify(chain, "connect", e),
    };

    let fee_rate = if let Some(sats) = custom_fee_sats {
        FeeRate::SatsPerVbyte { sats, chain }
    } else {
        debug!("estimating fee for {fee_target} blocks on {chain:?}");
        match active.estimate_fee(fee_target) {
            Ok(r) => r,
            Err(e) => return send_classify(chain, "fee estimate", e),
        }
    };

    send_params.fee = fee_rate;

    debug!("build_send for account {account} on {chain:?}");
    match active.build_send(seed, account, &send_params, SendOpts { rbf, gap_limit }) {
        Ok(prepared) => SendAttempt::Done(BroadcastPayload {
            chain,
            config: config.clone(),
            seed: *seed,
            account,
            send_params,
            prepared,
            known_hosts: Arc::clone(known_hosts),
            data_root: data_root.to_path_buf(),
        }),
        Err(e) => send_classify(chain, "build", e),
    }
}

/// Broadcast thread: sign **once**, then broadcast with retry. Returns the
/// TxId string.
///
/// The signing step is deterministic and local (no network), so it runs
/// outside the retry loop: on success the seed is immediately zeroized and
/// the resulting `SignedTx` (raw bytes, `Clone`-able) is what we re-submit
/// against alternate endpoints on broadcast failure.
///
/// Connect or broadcast errors classified as retryable trigger a fresh
/// `ActiveChain::from_chain_id` (which re-shuffles endpoints) up to
/// `MAX_SEND_ATTEMPTS` times. The signed bytes are reused — re-signing on
/// retry would waste CPU and would not change the tx (deterministic).
///
/// Zeroizes `payload.seed` on **every** exit path so a failed broadcast
/// doesn't leave a live seed copy in dropped stack memory.
fn broadcast_thread(
    mut payload: BroadcastPayload,
    attempt_counter: Arc<AtomicU32>,
) -> Result<String, String> {
    debug!("broadcast_thread for chain {:?}", payload.chain);

    // ── Phase 1: connect once + sign locally ────────────────────────────────
    //
    // Signing needs `&self` on the BitcoinChain/EthereumChain wrapper, so
    // we have to build an ActiveChain to sign. The connection that ships
    // with it is then thrown away — broadcasting will reconnect anyway so
    // each attempt picks a fresh server via `try_endpoints`.
    attempt_counter.store(1, Ordering::Relaxed);
    let signing_chain = match ActiveChain::from_chain_id(
        payload.chain,
        &payload.config,
        &payload.known_hosts,
        &payload.data_root,
    ) {
        Ok(a) => a,
        Err(e) => {
            payload.seed.zeroize();
            return match send_classify(payload.chain, "connect", e) {
                SendAttempt::Done(_) => unreachable!(),
                SendAttempt::Fatal(msg) | SendAttempt::Retry(msg) => Err(msg),
            };
        }
    };

    let signed = match signing_chain.sign_only(
        &payload.seed,
        payload.account,
        &payload.send_params,
        payload.prepared,
    ) {
        Ok(s) => s,
        Err(e) => {
            payload.seed.zeroize();
            // Sign failures are always fatal — re-trying with a different
            // server won't change the result (signing is local and
            // deterministic).
            return Err(format!("{}: sign: {e}", payload.chain.display_name()));
        }
    };
    // Seed is no longer needed once signing is done. Zero it immediately
    // so the broadcast retry loop can't accidentally leak it.
    payload.seed.zeroize();
    drop(signing_chain);

    // ── Phase 2: broadcast with retry against fresh endpoints ───────────────
    let mut last_reason: Option<String> = None;
    for attempt in 1..=MAX_SEND_ATTEMPTS {
        attempt_counter.store(attempt, Ordering::Relaxed);
        let active = match ActiveChain::from_chain_id(
            payload.chain,
            &payload.config,
            &payload.known_hosts,
            &payload.data_root,
        ) {
            Ok(a) => a,
            Err(e) => match send_classify(payload.chain, "connect", e) {
                SendAttempt::Done(_) => unreachable!(),
                SendAttempt::Fatal(msg) => return Err(msg),
                SendAttempt::Retry(reason) => {
                    debug!("broadcast attempt {attempt} connect failed: {reason}");
                    last_reason = Some(reason);
                    continue;
                }
            },
        };

        match active.broadcast_only(signed.clone()) {
            Ok(txid) => return Ok(txid.0),
            Err(e) => match send_classify(payload.chain, "broadcast", e) {
                SendAttempt::Done(_) => unreachable!(),
                SendAttempt::Fatal(msg) => return Err(msg),
                SendAttempt::Retry(reason) => {
                    debug!("broadcast attempt {attempt} failed: {reason}");
                    last_reason = Some(reason);
                }
            },
        }
    }
    let reason = last_reason.unwrap_or_else(|| "no detail".into());
    Err(format!(
        "all {MAX_SEND_ATTEMPTS} endpoints failed — last: {reason}"
    ))
}

/// Classify a `hodl_core::Error` for a send attempt (build or connect stage).
fn send_classify(
    chain: ChainId,
    stage: &str,
    e: hodl_core::error::Error,
) -> SendAttempt<BroadcastPayload> {
    match retry::classify(chain, stage, e) {
        crate::retry::AttemptResult::Done => unreachable!(),
        crate::retry::AttemptResult::Fatal(msg) => SendAttempt::Fatal(msg),
        crate::retry::AttemptResult::Retry(msg) => SendAttempt::Retry(msg),
    }
}

// ── Form builder ───────────────────────────────────────────────────────────

fn make_send_form(chain: ChainId, balance_atoms: u64) -> Form {
    let mut recipient_field = TextFieldEditor::with_meta(
        FieldMeta::new("recipient address")
            .required(true)
            .placeholder(recipient_placeholder(chain)),
        1,
    );
    // Inline form validator; closes over chain. Authoritative check is in start_build.
    recipient_field.validator = Some(mk_validator(move |s| validate_recipient(s, chain)));

    let amount_placeholder = format!("0.0 (max {} {})", balance_atoms, chain.ticker());
    let mut amount_field = TextFieldEditor::with_meta(
        FieldMeta::new(format!("amount ({})", chain.ticker()))
            .required(true)
            .placeholder(amount_placeholder),
        1,
    );
    amount_field.validator = Some(mk_validator(validate_amount));

    let mut custom_fee_field =
        TextFieldEditor::with_meta(FieldMeta::new("custom fee (sat/vB)").placeholder("10"), 1);
    custom_fee_field.validator = Some(mk_validator(validate_custom_fee));

    Form::new()
        .with_title(format!("Send {}", chain.display_name()))
        .with_field(Field::SingleLineText(recipient_field))
        .with_field(Field::SingleLineText(amount_field))
        .with_field(Field::Select(SelectField::new(
            FieldMeta::new("fee tier"),
            FEE_TIERS.iter().map(|s| s.to_string()).collect(),
        )))
        .with_field(Field::SingleLineText(custom_fee_field))
        .with_field(Field::Checkbox(CheckboxField::new(FieldMeta::new(
            "RBF (replace-by-fee)",
        ))))
        .with_field(Field::Submit(SubmitField::new(FieldMeta::new(
            "Sign & broadcast",
        ))))
}

fn recipient_placeholder(chain: ChainId) -> &'static str {
    use ChainId::*;
    match chain {
        Bitcoin | BitcoinTestnet => "bc1q…",
        Litecoin => "ltc1q…",
        Dogecoin => "D…",
        BitcoinCash => "bitcoincash:q…",
        NavCoin => "N…",
        Ethereum | BscMainnet => "0x…",
        Monero => "4…",
    }
}

// ── Event loop ─────────────────────────────────────────────────────────────

pub fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut SendState,
    wallet: &UnlockedWallet,
) -> Result<SendAction>
where
    B::Error: Send + Sync + 'static,
{
    let mut help_overlay: Option<HelpOverlay> = None;

    loop {
        // Drain in-flight channel events first so phase/spinner state is
        // current before we render.
        state.poll();

        // Single render per iteration — covers both the timeout (spinner
        // animation) and post-event (form echo) paths uniformly.
        terminal.draw(|f| {
            let area = f.area();
            draw(f, state);
            if let Some(ref mut overlay) = help_overlay {
                overlay.draw(f, area);
            }
        })?;

        // Short timeout while busy so spinner animates; idle otherwise.
        let wait = if state.is_busy() {
            std::time::Duration::from_millis(80)
        } else {
            std::time::Duration::from_millis(250)
        };

        if !event::poll(wait)? {
            continue;
        }

        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
                {
                    return Ok(SendAction::Quit);
                }

                // Overlay absorbs all keys when open.
                if let Some(ref mut overlay) = help_overlay {
                    if overlay.handle_key(k) == HelpAction::Close {
                        help_overlay = None;
                    }
                    continue;
                }

                // F1 opens the help overlay from any phase/mode.
                if k.code == KeyCode::F(1) {
                    help_overlay = Some(HelpOverlay::new("Send", state.help_lines()));
                    continue;
                }

                // Result pane: Enter or q returns to accounts.
                if matches!(state.phase, Phase::Result(_)) {
                    if matches!(k.code, KeyCode::Enter | KeyCode::Char('q') | KeyCode::Esc) {
                        return Ok(SendAction::Back);
                    }
                    continue;
                }

                // Error pane: q/Esc goes back; any other key clears error.
                if matches!(state.phase, Phase::Error(_)) {
                    if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                        return Ok(SendAction::Back);
                    }
                    state.phase = Phase::Form;
                    continue;
                }

                // While building or broadcasting: block Tab/Enter (no double-submit);
                // allow text editing and Esc-to-Normal so user can still type.
                if state.is_busy() {
                    // Only allow Esc to return to Normal mode (cosmetic).
                    if k.code == KeyCode::Esc {
                        state.form.handle_input(Input::from(k));
                    }
                    continue;
                }

                if k.code == KeyCode::Esc && state.form.mode == FormMode::Normal {
                    return Ok(SendAction::Back);
                }

                if k.code == KeyCode::Enter && state.form.mode == FormMode::Normal {
                    let focused_is_submit = matches!(
                        state.form.fields.get(state.form.focused()),
                        Some(Field::Submit(_))
                    );
                    if focused_is_submit {
                        state.start_build(wallet);
                        continue;
                    }
                }

                // Picker open: route all keys through picker, skip form.
                if state.book_picker.is_some() {
                    state.handle_book_picker_key(k);
                    continue;
                }

                // Ctrl+B in Normal mode on the recipient field opens the picker.
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && k.code == KeyCode::Char('b')
                    && state.form.mode == FormMode::Normal
                    && state.form.focused() == FIELD_RECIPIENT
                    && matches!(state.phase, Phase::Form)
                {
                    state.open_book_picker();
                    continue;
                }

                state.form.handle_input(Input::from(k));
            }
            Event::Resize(_, _) => continue,
            _ => continue,
        }
    }
}

// ── Drawing ────────────────────────────────────────────────────────────────

pub fn draw(f: &mut ratatui::Frame, state: &mut SendState) {
    let area = f.area();
    match &state.phase {
        Phase::Result(txid) => draw_result(f, area, txid.clone()),
        Phase::Error(msg) => draw_error(f, area, msg.clone(), state),
        Phase::Building(_, _) | Phase::Broadcasting(_, _) => draw_busy(f, area, state),
        Phase::Form => draw_form_phase(f, area, state),
    }
}

fn draw_form_phase(f: &mut ratatui::Frame, area: Rect, state: &mut SendState) {
    let block = Block::default()
        .title(format!(
            " hodl • Send {} — account {} (total {} atoms) ",
            state.chain.display_name(),
            state.account,
            state.total_balance_sats,
        ))
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));
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

    let hint_content: Line = if let Some(msg) = &state.flash {
        Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        let mode_hint = if state.form.mode == FormMode::Insert {
            "Esc Normal • Tab/j/k focus • h/l select tier"
        } else {
            "i edit • Tab/j/k focus • h/l tier • Ctrl+B book • Enter submit • Esc back"
        };
        Line::from(Span::styled(
            mode_hint,
            Style::default().fg(Color::DarkGray),
        ))
    };
    let p = Paragraph::new(hint_content).alignment(Alignment::Center);
    f.render_widget(p, chunks[1]);

    if state.book_picker.is_some() {
        draw_book_picker_overlay(f, area, state);
    }
}

fn draw_book_picker_overlay(f: &mut ratatui::Frame, area: Rect, state: &mut SendState) {
    let Some(picker) = &mut state.book_picker else {
        return;
    };
    picker.refresh();

    let w = area.width.min(60);
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

/// Render the form in the background with a spinner overlay at the bottom.
fn draw_busy(f: &mut ratatui::Frame, area: Rect, state: &mut SendState) {
    let (base_label, attempt) = match &state.phase {
        Phase::Building(_, a) => ("building…", a.load(Ordering::Relaxed)),
        Phase::Broadcasting(_, a) => ("broadcasting…", a.load(Ordering::Relaxed)),
        _ => return,
    };
    let attempt_suffix = if attempt > 1 {
        format!(" (attempt {attempt}/{MAX_SEND_ATTEMPTS})")
    } else {
        String::new()
    };
    let label_owned = format!("{base_label}{attempt_suffix}");
    let label = label_owned.as_str();

    let block = Block::default()
        .title(format!(
            " hodl • Send {} — account {} ",
            state.chain.display_name(),
            state.account,
        ))
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    // Render the (now read-only) form behind the spinner so the user can see
    // what they submitted.
    draw_form(f, chunks[0], &mut state.form, &FormPalette::dark());

    let spinner_text = format!("{label}  {}", hjkl_ratatui::spinner::frame());
    let spinner_line = Line::from(Span::styled(spinner_text, Style::default().fg(Color::Cyan)));
    let p = Paragraph::new(spinner_line).alignment(Alignment::Center);
    f.render_widget(p, chunks[1]);
}

fn draw_result(f: &mut ratatui::Frame, area: Rect, txid: String) {
    let block = Block::default()
        .title(" hodl • Send — broadcast successful ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let url = format!("https://mempool.space/tx/{txid}");
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Transaction broadcast!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("TxID:", Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled(txid, Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(Span::styled(
            "View on mempool.space:",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(url, Style::default().fg(Color::Cyan))),
        Line::from(""),
        Line::from(Span::styled(
            "Enter / q to return to accounts",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let p = Paragraph::new(lines).alignment(Alignment::Center);
    f.render_widget(p, inner);
}

fn draw_error(f: &mut ratatui::Frame, area: Rect, msg: String, state: &mut SendState) {
    let block = Block::default()
        .title(" hodl • Send — error ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Red));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(1)])
        .split(inner);

    let error_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            msg,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to edit  •  q / Esc to go back",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let p = Paragraph::new(error_lines).alignment(Alignment::Center);
    f.render_widget(p, chunks[0]);

    let result = draw_form(f, chunks[1], &mut state.form, &FormPalette::dark());
    if let Some((cx, cy)) = result.cursor {
        f.set_cursor_position((cx, cy));
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Scale a human-readable decimal amount to the chain's smallest unit.
fn chain_amount_atoms(chain: ChainId, value: f64) -> u128 {
    let scale = 10f64.powi(chain.decimals() as i32);
    (value * scale).round() as u128
}

// ── Contact picker source ──────────────────────────────────────────────────

fn short_address(addr: &str) -> String {
    if addr.len() <= 16 {
        return addr.to_string();
    }
    format!("{}…{}", &addr[..6], &addr[addr.len() - 4..])
}

struct ContactPickerSource {
    contacts: Vec<Contact>,
}

impl PickerLogic for ContactPickerSource {
    fn title(&self) -> &str {
        "address book"
    }

    fn item_count(&self) -> usize {
        self.contacts.len()
    }

    fn label(&self, idx: usize) -> String {
        self.contacts
            .get(idx)
            .map(|c| format!("{} — {}", c.label, short_address(&c.address)))
            .unwrap_or_default()
    }

    fn match_text(&self, idx: usize) -> String {
        self.contacts
            .get(idx)
            .map(|c| format!("{} {}", c.label, c.address))
            .unwrap_or_default()
    }

    fn has_preview(&self) -> bool {
        false
    }

    fn select(&self, idx: usize) -> PickerAction {
        let addr = self
            .contacts
            .get(idx)
            .map(|c| c.address.clone())
            .unwrap_or_default();
        PickerAction::OpenPath(PathBuf::from(addr))
    }

    fn enumerate(
        &mut self,
        _query: Option<&str>,
        _cancel: Arc<AtomicBool>,
    ) -> Option<JoinHandle<()>> {
        None
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_recipient_rejects_empty() {
        assert!(validate_recipient("", ChainId::Bitcoin).is_err());
    }

    #[test]
    fn validate_recipient_rejects_garbage() {
        assert!(validate_recipient("not-an-address", ChainId::Bitcoin).is_err());
    }

    #[test]
    fn validate_recipient_accepts_p2wpkh() {
        let addr = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
        assert!(validate_recipient(addr, ChainId::Bitcoin).is_ok());
    }

    #[test]
    fn validate_recipient_accepts_legacy_btc() {
        // Satoshi's address — legacy P2PKH, base58check, version byte 0x00.
        let addr = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        assert!(validate_recipient(addr, ChainId::Bitcoin).is_ok());
    }

    #[test]
    fn validate_recipient_accepts_doge_legacy() {
        // Random valid DOGE address (base58 P2PKH, prefix 0x1e → "D...").
        let addr = "DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L";
        assert!(validate_recipient(addr, ChainId::Dogecoin).is_ok());
    }

    #[test]
    fn validate_recipient_accepts_bch_cashaddr() {
        let addr = "bitcoincash:qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
        assert!(validate_recipient(addr, ChainId::BitcoinCash).is_ok());
    }

    #[test]
    fn validate_recipient_rejects_legacy_for_bch() {
        // BCH only accepts CashAddr; a base58 P2PKH must fail.
        let addr = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        assert!(validate_recipient(addr, ChainId::BitcoinCash).is_err());
    }

    #[test]
    fn validate_recipient_accepts_eth_address() {
        // All-lowercase ETH address (no checksum required for all-lower).
        let addr = "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed";
        assert!(validate_recipient(addr, ChainId::Ethereum).is_ok());
    }

    #[test]
    fn validate_recipient_rejects_monero() {
        assert!(validate_recipient("4anything", ChainId::Monero).is_err());
    }

    #[test]
    fn validate_amount_rejects_zero() {
        assert!(validate_amount("0").is_err());
        assert!(validate_amount("0.0").is_err());
    }

    #[test]
    fn validate_amount_rejects_empty() {
        assert!(validate_amount("").is_err());
    }

    #[test]
    fn validate_amount_rejects_negative() {
        assert!(validate_amount("-1.0").is_err());
    }

    #[test]
    fn validate_amount_accepts_positive() {
        assert!(validate_amount("0.001").is_ok());
        assert!(validate_amount("1.5").is_ok());
    }

    #[test]
    fn validate_custom_fee_rejects_zero() {
        assert!(validate_custom_fee("0").is_err());
    }

    #[test]
    fn validate_custom_fee_accepts_positive() {
        assert!(validate_custom_fee("10").is_ok());
        assert!(validate_custom_fee("1").is_ok());
    }

    fn make_book() -> AddressBook {
        AddressBook {
            entries: vec![
                Contact {
                    label: "Alice BTC".into(),
                    address: "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu".into(),
                    chain: ChainId::Bitcoin,
                    note: None,
                },
                Contact {
                    label: "Bob ETH".into(),
                    address: "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".into(),
                    chain: ChainId::Ethereum,
                    note: None,
                },
            ],
        }
    }

    #[test]
    fn book_picker_filters_to_matching_chain() {
        let book = make_book();
        let mut state = SendState::new_with_book(ChainId::Bitcoin, 1_000_000, book);
        state.open_book_picker();
        assert!(
            state.book_picker.is_some(),
            "picker should open for Bitcoin send with a BTC contact"
        );
        assert!(
            state.flash.is_none(),
            "no flash when contacts exist for the chain"
        );
        let picker = state.book_picker.as_mut().unwrap();
        assert_eq!(picker.visible_entries().len(), 1, "only the BTC contact");
        let (label, _) = &picker.visible_entries()[0];
        assert!(label.contains("Alice BTC"), "label contains contact name");
    }

    #[test]
    fn book_picker_shows_flash_when_no_matching_contacts() {
        let book = make_book();
        let mut state = SendState::new_with_book(ChainId::Litecoin, 500_000, book);
        state.open_book_picker();
        assert!(
            state.book_picker.is_none(),
            "picker must not open when no contacts match"
        );
        assert!(
            state.flash.is_some(),
            "flash message must be set when no contacts match"
        );
        let flash = state.flash.as_ref().unwrap();
        assert!(flash.contains("Litecoin"), "flash names the chain");
    }

    #[test]
    fn set_recipient_from_picker_writes_to_field() {
        let btc_addr = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
        let book = make_book();
        let mut state = SendState::new_with_book(ChainId::Bitcoin, 1_000_000, book);
        if let Some(Field::SingleLineText(f)) = state.form.fields.get_mut(FIELD_RECIPIENT) {
            f.set_text(btc_addr);
        }
        let actual = state.field_text(FIELD_RECIPIENT);
        assert_eq!(actual, btc_addr);
    }
}
