//! Send screen — build + sign + broadcast a Bitcoin P2WPKH transaction.
//!
//! Driven by `hjkl-form`. Fields:
//!   0 = recipient (bech32 P2WPKH TextFieldEditor + validator)
//!   1 = amount in BTC (TextFieldEditor + validator)
//!   2 = fee tier SelectField (Slow / Normal / Fast / Custom)
//!   3 = custom sat/vB (TextFieldEditor, only meaningful when tier = Custom)
//!   4 = "Sign & broadcast" SubmitField
//!
//! Submit pipeline:
//!   1. Re-fetch UTXOs for the source address.
//!   2. Map tier → block target → estimate_fee; Custom reads field 3.
//!   3. BitcoinChain::build_tx_for_address → UnsignedTx + per-input keys.
//!   4. BitcoinChain::sign_with_keys → SignedTx.
//!   5. Chain::broadcast → TxId displayed in result pane.
//!
//! After broadcast: result pane shows TxId + mempool.space hint URL.
//! `q` / Esc returns to Accounts.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use hjkl_form::{
    Field, FieldMeta, Form, FormMode, Input, SelectField, SubmitField, TextFieldEditor, Validator,
};
use hjkl_ratatui::form::{FormPalette, draw_form};
use hodl_chain_bitcoin::{BitcoinChain, NetworkParams, Purpose};
use hodl_config::{ChainConfig, Config, Endpoint};
use hodl_core::{Address, Amount, Chain, ChainId, FeeRate};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tracing::debug;

use hodl_wallet::UnlockedWallet;

// ── Field indices ──────────────────────────────────────────────────────────

const FIELD_RECIPIENT: usize = 0;
const FIELD_AMOUNT: usize = 1;
const FIELD_FEE_TIER: usize = 2;
const FIELD_CUSTOM_FEE: usize = 3;

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

/// Validate a bech32 P2WPKH address.
pub fn validate_recipient(s: &str) -> std::result::Result<(), String> {
    if s.is_empty() {
        return Err("recipient cannot be empty".into());
    }
    match bech32::segwit::decode(s) {
        Ok((_, ver, prog)) if ver == bech32::segwit::VERSION_0 && prog.len() == 20 => Ok(()),
        Ok(_) => Err("address must be a P2WPKH bech32 (witness v0, 20-byte program)".into()),
        Err(e) => Err(format!("invalid bech32 address: {e}")),
    }
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

// ── State machine ──────────────────────────────────────────────────────────

enum Phase {
    Form,
    /// Broadcast succeeded; hold TxId string.
    Result(String),
    /// Submit failed; error message.
    Error(String),
}

pub struct SendState {
    source_address: Address,
    source_account: u32,
    source_change_branch: u32,
    source_index: u32,
    source_balance_sats: u64,
    form: Form,
    phase: Phase,
    config: Config,
}

impl SendState {
    pub fn new(
        source_address: Address,
        source_account: u32,
        source_change_branch: u32,
        source_index: u32,
        source_balance_sats: u64,
        config: Config,
    ) -> Self {
        let form = make_send_form(source_balance_sats);
        Self {
            source_address,
            source_account,
            source_change_branch,
            source_index,
            source_balance_sats,
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

    fn try_submit(&mut self, wallet: &UnlockedWallet) {
        let recipient_str = self.field_text(FIELD_RECIPIENT);
        if let Err(e) = validate_recipient(&recipient_str) {
            self.phase = Phase::Error(format!("recipient: {e}"));
            return;
        }

        let amount_str = self.field_text(FIELD_AMOUNT);
        let amount_btc: f64 = match amount_str.parse() {
            Ok(v) if v > 0.0 => v,
            _ => {
                self.phase = Phase::Error("invalid amount".into());
                return;
            }
        };
        let amount_sats = (amount_btc * 1e8).round() as u64;

        if amount_sats > self.source_balance_sats {
            self.phase = Phase::Error(format!(
                "amount ({amount_sats} sats) exceeds balance ({} sats)",
                self.source_balance_sats
            ));
            return;
        }

        let chain_cfg = self
            .config
            .chains
            .get(&ChainId::Bitcoin)
            .cloned()
            .unwrap_or_default();

        let endpoint_url = match first_electrum_url(&chain_cfg) {
            Some(u) => u,
            None => {
                self.phase = Phase::Error("no Electrum endpoint configured".into());
                return;
            }
        };

        let fee_rate = if self.selected_tier().unwrap_or("").starts_with("Custom") {
            let custom_str = self.field_text(FIELD_CUSTOM_FEE);
            match custom_str.parse::<u64>() {
                Ok(v) if v > 0 => FeeRate::SatsPerVbyte {
                    sats: v,
                    chain: ChainId::Bitcoin,
                },
                _ => {
                    self.phase = Phase::Error("invalid custom fee rate".into());
                    return;
                }
            }
        } else {
            let target = self.fee_target_blocks();
            debug!("estimating fee for {target} blocks");
            let electrum_fee = match electrum_connect(&endpoint_url) {
                Ok(c) => c,
                Err(e) => {
                    self.phase = Phase::Error(format!("Electrum connect (fee): {e}"));
                    return;
                }
            };
            let chain_fee = BitcoinChain::new(NetworkParams::BITCOIN_MAINNET, electrum_fee)
                .with_purpose(Purpose::Bip84);
            match chain_fee.estimate_fee(target) {
                Ok(r) => r,
                Err(e) => {
                    self.phase = Phase::Error(format!("fee estimate failed: {e}"));
                    return;
                }
            }
        };

        let seed = wallet.seed().as_bytes().to_owned();
        let to_addr = Address::new(recipient_str, ChainId::Bitcoin);
        let amount = Amount::from_atoms(amount_sats as u128, ChainId::Bitcoin);

        let send_params = hodl_core::SendParams {
            from: self.source_address.clone(),
            to: to_addr,
            amount,
            fee: fee_rate,
        };

        // Fetch UTXOs.
        let electrum_utxo = match electrum_connect(&endpoint_url) {
            Ok(c) => c,
            Err(e) => {
                self.phase = Phase::Error(format!("Electrum connect (utxo): {e}"));
                return;
            }
        };
        let chain_utxo = BitcoinChain::new(NetworkParams::BITCOIN_MAINNET, electrum_utxo)
            .with_purpose(Purpose::Bip84);
        let utxos = match chain_utxo.listunspent(&self.source_address) {
            Ok(u) => u,
            Err(e) => {
                self.phase = Phase::Error(format!("listunspent: {e}"));
                return;
            }
        };

        debug!("building tx from source index {}", self.source_index);
        let electrum_build = match electrum_connect(&endpoint_url) {
            Ok(c) => c,
            Err(e) => {
                self.phase = Phase::Error(format!("Electrum connect (build): {e}"));
                return;
            }
        };
        let chain_build = BitcoinChain::new(NetworkParams::BITCOIN_MAINNET, electrum_build)
            .with_purpose(Purpose::Bip84);
        let (_unsigned, keys) = match chain_build.build_tx_for_address(
            &seed,
            self.source_account,
            self.source_change_branch,
            self.source_index,
            &send_params,
        ) {
            Ok(r) => r,
            Err(e) => {
                self.phase = Phase::Error(format!("build tx: {e}"));
                return;
            }
        };

        // Sign.
        let electrum_sign = match electrum_connect(&endpoint_url) {
            Ok(c) => c,
            Err(e) => {
                self.phase = Phase::Error(format!("Electrum connect (sign): {e}"));
                return;
            }
        };
        let chain_sign = BitcoinChain::new(NetworkParams::BITCOIN_MAINNET, electrum_sign)
            .with_purpose(Purpose::Bip84);
        let signed = match chain_sign.sign_with_keys(
            &seed,
            self.source_account,
            self.source_change_branch,
            self.source_index,
            &utxos,
            &send_params,
            &keys,
        ) {
            Ok(s) => s,
            Err(e) => {
                self.phase = Phase::Error(format!("sign: {e}"));
                return;
            }
        };

        // Broadcast.
        let electrum_bc = match electrum_connect(&endpoint_url) {
            Ok(c) => c,
            Err(e) => {
                self.phase = Phase::Error(format!("Electrum connect (broadcast): {e}"));
                return;
            }
        };
        let chain_bc = BitcoinChain::new(NetworkParams::BITCOIN_MAINNET, electrum_bc)
            .with_purpose(Purpose::Bip84);
        match chain_bc.broadcast(signed) {
            Ok(txid) => {
                self.phase = Phase::Result(txid.0);
            }
            Err(e) => {
                self.phase = Phase::Error(format!("broadcast: {e}"));
            }
        }
    }
}

// ── Form builder ───────────────────────────────────────────────────────────

fn make_send_form(balance_sats: u64) -> Form {
    let mut recipient_field = TextFieldEditor::with_meta(
        FieldMeta::new("recipient address")
            .required(true)
            .placeholder("bc1q..."),
        1,
    );
    recipient_field.validator = Some(mk_validator(validate_recipient));

    let balance_btc = balance_sats as f64 / 1e8;
    let amount_placeholder = format!("0.0 (max {balance_btc:.8} BTC)");
    let mut amount_field = TextFieldEditor::with_meta(
        FieldMeta::new("amount (BTC)")
            .required(true)
            .placeholder(amount_placeholder),
        1,
    );
    amount_field.validator = Some(mk_validator(validate_amount));

    let mut custom_fee_field =
        TextFieldEditor::with_meta(FieldMeta::new("custom fee (sat/vB)").placeholder("10"), 1);
    custom_fee_field.validator = Some(mk_validator(validate_custom_fee));

    Form::new()
        .with_title("Send Bitcoin")
        .with_field(Field::SingleLineText(recipient_field))
        .with_field(Field::SingleLineText(amount_field))
        .with_field(Field::Select(SelectField::new(
            FieldMeta::new("fee tier"),
            FEE_TIERS.iter().map(|s| s.to_string()).collect(),
        )))
        .with_field(Field::SingleLineText(custom_fee_field))
        .with_field(Field::Submit(SubmitField::new(FieldMeta::new(
            "Sign & broadcast",
        ))))
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
    loop {
        terminal.draw(|f| draw(f, state))?;

        if !event::poll(std::time::Duration::from_millis(250))? {
            continue;
        }

        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
                {
                    return Ok(SendAction::Quit);
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

                if k.code == KeyCode::Esc && state.form.mode == FormMode::Normal {
                    return Ok(SendAction::Back);
                }

                if k.code == KeyCode::Enter && state.form.mode == FormMode::Normal {
                    let focused_is_submit = matches!(
                        state.form.fields.get(state.form.focused()),
                        Some(Field::Submit(_))
                    );
                    if focused_is_submit {
                        state.try_submit(wallet);
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
        Phase::Form => draw_form_phase(f, area, state),
    }
}

fn draw_form_phase(f: &mut ratatui::Frame, area: Rect, state: &mut SendState) {
    let block = Block::default()
        .title(format!(
            " hodl • Send — from {} ",
            state.source_address.as_str()
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

fn electrum_connect(url: &str) -> hodl_core::Result<hodl_chain_bitcoin::electrum::ElectrumClient> {
    use hodl_chain_bitcoin::electrum::ElectrumClient;
    use hodl_core::error::Error;

    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| Error::Network(format!("invalid Electrum URL: {url}")))?;
    let (host, port_str) = rest
        .rsplit_once(':')
        .ok_or_else(|| Error::Network(format!("invalid Electrum URL: {url}")))?;
    let port: u16 = port_str
        .parse()
        .map_err(|_| Error::Network(format!("invalid port: {url}")))?;

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

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_recipient_rejects_empty() {
        assert!(validate_recipient("").is_err());
    }

    #[test]
    fn validate_recipient_rejects_non_bech32() {
        assert!(validate_recipient("1A1zP1eP5QGefi2DMPTfTL5SLmv7Divf").is_err());
    }

    #[test]
    fn validate_recipient_accepts_p2wpkh() {
        let addr = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
        assert!(validate_recipient(addr).is_ok());
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
