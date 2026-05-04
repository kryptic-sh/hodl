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

use std::sync::mpsc::{self, Receiver};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use hjkl_form::{
    CheckboxField, Field, FieldMeta, Form, FormMode, Input, SelectField, SubmitField,
    TextFieldEditor, Validator,
};
use hjkl_ratatui::form::{FormPalette, draw_form};
use hodl_config::Config;
use hodl_core::{Address, Amount, ChainId, FeeRate};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tracing::debug;
use zeroize::Zeroize;

use hodl_wallet::UnlockedWallet;

use crate::active_chain::{ActiveChain, PreparedSend, SendOpts};
use crate::help::{HelpAction, HelpOverlay};
use crate::spinner::Spinner;

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
        Bitcoin | BitcoinTestnet | Litecoin | NavCoin => {
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
        Dogecoin => validate_base58_p2pkh(s),
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
}

// ── State machine ──────────────────────────────────────────────────────────

enum Phase {
    Form,
    /// Off-thread: estimate_fee + build_send. Carries the channel + spinner.
    Building(Receiver<Result<BroadcastPayload, String>>, Spinner),
    /// Off-thread: sign_and_broadcast. Carries the channel + spinner.
    Broadcasting(Receiver<Result<String, String>>, Spinner),
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
}

impl SendState {
    pub fn new(chain: ChainId, account: u32, total_balance_sats: u64, config: Config) -> Self {
        let form = make_send_form(chain, total_balance_sats);
        Self {
            chain,
            account,
            total_balance_sats,
            form,
            phase: Phase::Form,
            config,
        }
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
            Phase::Building(rx, spinner) => {
                match rx.try_recv() {
                    Ok(Ok(payload)) => {
                        // Build succeeded — kick off broadcast thread.
                        let (tx, brx) = mpsc::channel();
                        std::thread::spawn(move || {
                            let result = broadcast_thread(payload);
                            let _ = tx.send(result);
                        });
                        self.phase = Phase::Broadcasting(brx, Spinner::new());
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
                    Err(TryRecvError::Empty) => {
                        spinner.tick();
                        false
                    }
                }
            }
            Phase::Broadcasting(rx, spinner) => match rx.try_recv() {
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
                Err(TryRecvError::Empty) => {
                    spinner.tick();
                    false
                }
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

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = build_thread(
                chain,
                &config,
                seed,
                account,
                send_params,
                fee_target,
                custom_fee_sats,
                rbf,
                chain_cfg.gap_limit,
            );
            // Zeroize local seed copy before thread exits.
            let mut seed_copy = seed;
            seed_copy.zeroize();
            let _ = tx.send(result);
        });

        self.phase = Phase::Building(rx, Spinner::new());
    }
}

// ── Worker functions ────────────────────────────────────────────────────────

/// Build thread: open chain, estimate fee, build_send. Returns a `BroadcastPayload`.
#[allow(clippy::too_many_arguments)]
fn build_thread(
    chain: ChainId,
    config: &Config,
    seed: [u8; 64],
    account: u32,
    mut send_params: hodl_core::SendParams,
    fee_target: u32,
    custom_fee_sats: Option<u64>,
    rbf: bool,
    gap_limit: u32,
) -> Result<BroadcastPayload, String> {
    debug!("build_thread for chain {:?}", chain);

    let active =
        ActiveChain::from_chain_id(chain, config).map_err(|e| format!("chain connect: {e}"))?;

    let fee_rate = if let Some(sats) = custom_fee_sats {
        FeeRate::SatsPerVbyte { sats, chain }
    } else {
        debug!("estimating fee for {fee_target} blocks on {chain:?}");
        active
            .estimate_fee(fee_target)
            .map_err(|e| format!("fee estimate failed: {e}"))?
    };

    send_params.fee = fee_rate;

    debug!("build_send for account {account} on {chain:?}");
    let prepared = active
        .build_send(&seed, account, &send_params, SendOpts { rbf, gap_limit })
        .map_err(|e| format!("build: {e}"))?;

    Ok(BroadcastPayload {
        chain,
        config: config.clone(),
        seed,
        account,
        send_params,
        prepared,
    })
}

/// Broadcast thread: sign + broadcast. Returns the TxId string.
fn broadcast_thread(mut payload: BroadcastPayload) -> Result<String, String> {
    debug!("broadcast_thread for chain {:?}", payload.chain);

    let active = ActiveChain::from_chain_id(payload.chain, &payload.config)
        .map_err(|e| format!("chain connect: {e}"))?;

    let txid = active
        .sign_and_broadcast(
            &payload.seed,
            payload.account,
            &payload.send_params,
            payload.prepared,
        )
        .map_err(|e| format!("sign/broadcast: {e}"))?;

    // Zeroize seed before returning.
    payload.seed.zeroize();

    Ok(txid.0)
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
        NavCoin => "nav1q…",
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
        // Poll in-flight channels; redraw if phase changed.
        let changed = state.poll();
        if changed {
            terminal.draw(|f| {
                draw(f, state);
                if let Some(ref mut overlay) = help_overlay {
                    overlay.draw(f, f.area());
                }
            })?;
        }

        // Short timeout while busy so spinner animates; idle otherwise.
        let wait = if state.is_busy() {
            std::time::Duration::from_millis(80)
        } else {
            std::time::Duration::from_millis(250)
        };

        if !event::poll(wait)? {
            terminal.draw(|f| {
                let area = f.area();
                draw(f, state);
                if let Some(ref mut overlay) = help_overlay {
                    overlay.draw(f, area);
                }
            })?;
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

    let mode_hint = if state.form.mode == FormMode::Insert {
        "Esc Normal • Tab/j/k focus • h/l select tier"
    } else {
        "i edit • Tab/j/k focus • h/l tier • Enter submit • Esc back"
    };
    let p = Paragraph::new(Line::from(Span::styled(
        mode_hint,
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(p, chunks[1]);
}

/// Render the form in the background with a spinner overlay at the bottom.
fn draw_busy(f: &mut ratatui::Frame, area: Rect, state: &mut SendState) {
    let (label, spinner) = match &state.phase {
        Phase::Building(_, s) => ("building…", s),
        Phase::Broadcasting(_, s) => ("broadcasting…", s),
        _ => return,
    };

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

    spinner.draw(f, chunks[1], label, Color::Cyan);
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
///
/// BTC-family: value is interpreted as the major unit (BTC/LTC/etc.), scaled
/// by 1e8 to satoshis. ETH-family: value is in ETH, scaled by 1e18 to wei.
/// Monero: value is in XMR, scaled by 1e12 to piconero.
fn chain_amount_atoms(chain: ChainId, value: f64) -> u128 {
    use ChainId::*;
    let scale: f64 = match chain {
        Bitcoin | BitcoinTestnet | Litecoin | Dogecoin | BitcoinCash | NavCoin => 1e8,
        Ethereum | BscMainnet => 1e18,
        Monero => 1e12,
    };
    (value * scale).round() as u128
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
}
