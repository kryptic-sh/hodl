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
//! Submit pipeline:
//!   1. Validate recipient address per active chain codec.
//!   2. Build `ActiveChain` from `chain_id` + config.
//!   3. `estimate_fee` via the chain (BTC → SatsPerVbyte, ETH → Gwei). The
//!      form's fee tier always maps to `estimate_fee`; custom sat/vB is BTC-only.
//!   4. `build_send` → `PreparedSend`.
//!   5. `sign_and_broadcast` → `TxId` displayed in result pane.
//!
//! After broadcast: result pane shows TxId.
//! `q` / Esc returns to Accounts.

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

use hodl_wallet::UnlockedWallet;

use crate::active_chain::{ActiveChain, SendOpts};

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
/// Bitcoin family uses bech32 segwit v0 P2WPKH. Note: DOGE/BCH/NAV don't
/// actually use bech32 in practice, but the TUI gates those chains' send
/// path earlier (build_send returns an error), so the validator is moot.
/// Ethereum family uses EIP-55 hex. Monero send is not yet implemented.
pub fn validate_recipient(s: &str, chain: ChainId) -> std::result::Result<(), String> {
    if s.is_empty() {
        return Err("recipient cannot be empty".into());
    }
    use ChainId::*;
    match chain {
        Bitcoin | BitcoinTestnet | Litecoin | Dogecoin | BitcoinCash | NavCoin => {
            match bech32::segwit::decode(s) {
                Ok((_, ver, prog)) if ver == bech32::segwit::VERSION_0 && prog.len() == 20 => {
                    Ok(())
                }
                Ok(_) => {
                    Err("address must be a P2WPKH bech32 (witness v0, 20-byte program)".into())
                }
                Err(e) => Err(format!("invalid bech32 address: {e}")),
            }
        }
        Ethereum | BscMainnet => hodl_chain_ethereum::address::from_str_normalized(s)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Monero => Err("Monero send not implemented".into()),
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

    fn try_submit(&mut self, wallet: &UnlockedWallet) {
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
        // Scale to smallest unit: sats for BTC-family, wei for ETH, piconero for XMR.
        let amount_atoms = chain_amount_atoms(self.chain, amount_val);

        if amount_atoms > self.total_balance_sats as u128 {
            self.phase = Phase::Error(format!(
                "amount exceeds balance ({} available)",
                self.total_balance_sats
            ));
            return;
        }

        let active = match ActiveChain::from_chain_id(self.chain, &self.config) {
            Ok(a) => a,
            Err(e) => {
                self.phase = Phase::Error(format!("chain connect: {e}"));
                return;
            }
        };

        // Fee: for Bitcoin, Custom tier reads the sat/vB field. For all other
        // chains we always call estimate_fee — EVM fee tier semantics differ and
        // the custom sat/vB field is Bitcoin-only.
        let fee_rate = if matches!(self.chain, ChainId::Bitcoin | ChainId::BitcoinTestnet)
            && self.selected_tier().unwrap_or("").starts_with("Custom")
        {
            let custom_str = self.field_text(FIELD_CUSTOM_FEE);
            match custom_str.parse::<u64>() {
                Ok(v) if v > 0 => FeeRate::SatsPerVbyte {
                    sats: v,
                    chain: self.chain,
                },
                _ => {
                    self.phase = Phase::Error("invalid custom fee rate".into());
                    return;
                }
            }
        } else {
            let target = self.fee_target_blocks();
            debug!("estimating fee for {target} blocks on {:?}", self.chain);
            match active.estimate_fee(target) {
                Ok(r) => r,
                Err(e) => {
                    self.phase = Phase::Error(format!("fee estimate failed: {e}"));
                    return;
                }
            }
        };

        let rbf = self.rbf_checked();
        let seed = wallet.seed().as_bytes().to_owned();
        let to_addr = Address::new(recipient_str, self.chain);
        let amount = Amount::from_atoms(amount_atoms, self.chain);
        let chain_cfg = self
            .config
            .chains
            .get(&self.chain)
            .cloned()
            .unwrap_or_default();

        let send_params = hodl_core::SendParams {
            from: Address::new(String::new(), self.chain),
            to: to_addr,
            amount,
            fee: fee_rate,
        };

        debug!(
            "build_send for account {} on {:?}",
            self.account, self.chain
        );
        let prepared = match active.build_send(
            &seed,
            self.account,
            &send_params,
            SendOpts {
                rbf,
                gap_limit: chain_cfg.gap_limit,
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                self.phase = Phase::Error(format!("build: {e}"));
                return;
            }
        };

        match active.sign_and_broadcast(&seed, self.account, &send_params, prepared) {
            Ok(txid) => {
                self.phase = Phase::Result(txid.0);
            }
            Err(e) => {
                self.phase = Phase::Error(format!("sign/broadcast: {e}"));
            }
        }
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
    // Inline form validator; closes over chain. Authoritative check is in try_submit.
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
    fn validate_recipient_rejects_non_bech32() {
        assert!(validate_recipient("1A1zP1eP5QGefi2DMPTfTL5SLmv7Divf", ChainId::Bitcoin).is_err());
    }

    #[test]
    fn validate_recipient_accepts_p2wpkh() {
        let addr = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
        assert!(validate_recipient(addr, ChainId::Bitcoin).is_ok());
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
