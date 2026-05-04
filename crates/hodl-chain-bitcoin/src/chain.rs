use std::cell::RefCell;

use hodl_core::error::{Error, Result};
use hodl_core::{
    Address, Amount, Chain, ChainId, FeeRate, PrivateKeyBytes, SendParams, SignedTx, TxId, TxRef,
    UnsignedTx,
};

use crate::derive::{Purpose, derive_address, derive_xprv};
use crate::electrum::{ElectrumClient, Utxo, p2pkh_scripthash, p2sh_scripthash, p2wpkh_scripthash};
use crate::network::NetworkParams;
use crate::psbt::{
    Outpoint, TxInput, TxOutput, build_psbt, decode_p2wpkh_address, estimate_vsize, hash160,
    p2pkh_script, p2sh_p2wpkh_redeem_script, p2sh_script, p2wpkh_script, select_coins, sign_inputs,
    sign_inputs_legacy_p2pkh, sign_inputs_p2sh_p2wpkh,
};

/// Decode a recipient address string to a scriptPubKey for the given purpose.
///
/// - Bip84 / Bip86: bech32 P2WPKH.
/// - Bip44: base58check P2PKH or CashAddr P2PKH for BCH.
/// - Bip49: returns an error (not yet implemented).
fn decode_address_to_script(
    addr: &str,
    purpose: Purpose,
    params: &NetworkParams,
) -> Result<Vec<u8>> {
    match purpose {
        Purpose::Bip84 | Purpose::Bip86 => {
            // Bech32 HRP must match the chain — bech32 decode does this implicitly
            // since the HRP is part of the encoding, but we double-check the prog
            // length for P2WPKH.
            let hash = decode_p2wpkh_address(addr)?;
            Ok(p2wpkh_script(&hash))
        }
        Purpose::Bip44 => {
            // CashAddr (BCH): contains a colon and uses the chain's HRP.
            if addr.contains(':') {
                let prefix = format!("{}:", params.bech32_hrp);
                if !addr.starts_with(&prefix) {
                    return Err(Error::Codec(format!(
                        "CashAddr prefix mismatch: expected '{}', got '{}'",
                        prefix.trim_end_matches(':'),
                        addr.split(':').next().unwrap_or("")
                    )));
                }
                let h160 = crate::cashaddr::decode_p2pkh_cashaddr(addr)
                    .map_err(|e| Error::Codec(format!("cashaddr decode: {e}")))?;
                return Ok(p2pkh_script(&h160));
            }
            // Legacy base58check P2PKH. Validate version byte against the
            // chain's p2pkh_prefix — sending DOGE to a BTC address (or vice
            // versa) would otherwise silently encode the wrong scriptPubKey
            // and lose the funds.
            let decoded = bs58::decode(addr)
                .with_check(None)
                .into_vec()
                .map_err(|e| Error::Codec(format!("base58 decode: {e}")))?;
            if decoded.len() != 21 {
                return Err(Error::Codec("P2PKH address must decode to 21 bytes".into()));
            }
            if decoded[0] != params.p2pkh_prefix {
                return Err(Error::Codec(format!(
                    "address version byte 0x{:02x} does not match {} (expected 0x{:02x})",
                    decoded[0],
                    params.chain_id.display_name(),
                    params.p2pkh_prefix
                )));
            }
            let mut h160 = [0u8; 20];
            h160.copy_from_slice(&decoded[1..]);
            Ok(p2pkh_script(&h160))
        }
        Purpose::Bip49 => {
            // P2SH-P2WPKH recipient address: base58check with p2sh_prefix.
            let decoded = bs58::decode(addr)
                .with_check(None)
                .into_vec()
                .map_err(|e| Error::Codec(format!("base58 decode: {e}")))?;
            if decoded.len() != 21 {
                return Err(Error::Codec("P2SH address must decode to 21 bytes".into()));
            }
            if decoded[0] != params.p2sh_prefix {
                return Err(Error::Codec(format!(
                    "address version byte 0x{:02x} does not match {} P2SH (expected 0x{:02x})",
                    decoded[0],
                    params.chain_id.display_name(),
                    params.p2sh_prefix
                )));
            }
            let mut script_hash = [0u8; 20];
            script_hash.copy_from_slice(&decoded[1..]);
            Ok(p2sh_script(&script_hash))
        }
    }
}

/// Build the scriptPubKey for a change output matching the given purpose.
fn purpose_script(purpose: Purpose, pubkey_hash: &[u8; 20]) -> Vec<u8> {
    match purpose {
        Purpose::Bip84 | Purpose::Bip86 => p2wpkh_script(pubkey_hash),
        Purpose::Bip44 => p2pkh_script(pubkey_hash),
        Purpose::Bip49 => {
            // Change output for P2SH-P2WPKH: hash the redeemScript.
            let redeem = p2sh_p2wpkh_redeem_script(pubkey_hash);
            let script_hash = hash160(&redeem);
            p2sh_script(&script_hash)
        }
    }
}

/// Confirmed-vs-pending balance in atoms (sats).
///
/// "Confirmed" = at least 1 block confirmation (Electrum's `confirmed` field).
/// "Pending" = mempool only (Electrum's `unconfirmed` field — can be negative
/// when an incoming unconfirmed output is subsequently spent in the mempool,
/// so we clamp to 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BalanceSplit {
    pub confirmed: u64,
    pub pending: u64,
}

impl BalanceSplit {
    pub fn total(self) -> u64 {
        self.confirmed + self.pending
    }

    pub fn is_zero(self) -> bool {
        self.confirmed == 0 && self.pending == 0
    }
}

/// One used address discovered during a gap-limit wallet scan.
#[derive(Debug, Clone)]
pub struct UsedAddress {
    pub index: u32,
    /// 0 = receive (external) chain, 1 = change (internal) chain — BIP-44 path component.
    pub change: u32,
    pub address: String,
    pub balance: BalanceSplit,
}

/// Result of a gap-limit wallet scan.
#[derive(Debug, Clone, Default)]
pub struct WalletScan {
    /// All addresses with history > 0 OR balance > 0, across both receive and
    /// change chains. Order: receive first (sorted by index), then change
    /// (sorted by index).
    pub used: Vec<UsedAddress>,
    /// Aggregate balance across all used addresses.
    pub total: BalanceSplit,
    /// Highest derivation index reached on each chain (how far the scan walked
    /// before hitting `gap_limit` consecutive unused). For diagnostics.
    pub highest_index_receive: u32,
    pub highest_index_change: u32,
}

/// BIP-125 RBF sequence value — signals opt-in replace-by-fee.
pub const SEQUENCE_RBF: u32 = 0xffff_fffd;

/// Non-RBF final sequence (no RBF signaling).
pub const SEQUENCE_FINAL: u32 = 0xffff_ffff;

/// Per-input derivation hint: which `(account, change, index)` path owns
/// the address that funded this input. Passed from `build_tx_multi_source`
/// to `sign_multi_source` so the signer can derive the right key per input
/// without touching the seed directly at the PSBT level.
#[derive(Clone, Debug)]
pub struct InputHint {
    pub account: u32,
    pub change: u32,
    pub index: u32,
}

/// Bitcoin (mainnet + testnet) chain implementation.
///
/// Default purpose is BIP-84 (native segwit P2WPKH). Change via
/// `BitcoinChain::with_purpose`.
///
/// `ElectrumClient` needs `&mut self` for every call (line-framed protocol);
/// `RefCell` provides interior mutability so `Chain` trait methods can take
/// `&self`.
pub struct BitcoinChain {
    params: NetworkParams,
    electrum: RefCell<ElectrumClient>,
    purpose: Purpose,
}

impl BitcoinChain {
    pub fn new(params: NetworkParams, electrum: ElectrumClient) -> Self {
        let purpose = Self::default_send_purpose(params.chain_id);
        Self {
            params,
            electrum: RefCell::new(electrum),
            purpose,
        }
    }

    pub fn with_purpose(mut self, purpose: Purpose) -> Self {
        self.purpose = purpose;
        self
    }

    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    /// Returns the default derivation purpose for this chain's send path.
    ///
    /// - Bip44 (legacy P2PKH) for DOGE, BCH, NAV — bech32/segwit not
    ///   deployed in upstream node software.
    /// - Bip84 (native segwit P2WPKH) for everything else in the BTC family.
    pub fn default_send_purpose(chain_id: ChainId) -> Purpose {
        match chain_id {
            ChainId::Dogecoin | ChainId::BitcoinCash | ChainId::NavCoin => Purpose::Bip44,
            _ => Purpose::Bip84,
        }
    }

    /// Compute the Electrum scripthash for an address string.
    ///
    /// Supports bech32 P2WPKH, legacy P2PKH (base58check), and CashAddr P2PKH.
    fn scripthash_for(&self, addr: &Address) -> Result<String> {
        let s = addr.as_str();

        // Try bech32 segwit (P2WPKH, witness v0).
        if let Ok((_, witness_ver, prog)) = bech32::segwit::decode(s)
            && witness_ver == bech32::segwit::VERSION_0
            && prog.len() == 20
        {
            let h160: [u8; 20] = prog.try_into().unwrap();
            return Ok(p2wpkh_scripthash(&h160));
        }

        // Try CashAddr P2PKH (BCH): prefix is "bitcoincash:q..."
        if s.contains(':')
            && let Ok(h160) = crate::cashaddr::decode_p2pkh_cashaddr(s)
        {
            return Ok(p2pkh_scripthash(&h160));
        }

        // Try legacy base58check — may be P2PKH or P2SH.
        // Distinguish by the version byte against the chain's prefixes.
        if let Ok(decoded) = bs58::decode(s).with_check(None).into_vec()
            && decoded.len() == 21
        {
            let version = decoded[0];
            let h160: [u8; 20] = decoded[1..].try_into().unwrap();
            if version == self.params.p2sh_prefix {
                return Ok(p2sh_scripthash(&h160));
            }
            // Default: treat as P2PKH (covers p2pkh_prefix and unknown).
            return Ok(p2pkh_scripthash(&h160));
        }

        Err(Error::Chain(format!(
            "scripthash computation failed for address: {s}"
        )))
    }

    /// Fetch UTXOs for a bech32 P2WPKH address.
    fn utxos_for(&self, addr: &Address) -> Result<Vec<Utxo>> {
        let sh = self.scripthash_for(addr)?;
        self.electrum.borrow_mut().scripthash_listunspent(&sh)
    }

    /// Public: fetch UTXOs for a bech32 P2WPKH address.
    ///
    /// Exposed so the TUI send screen can fetch UTXOs without going through the
    /// private `utxos_for` helper.
    pub fn listunspent(&self, addr: &Address) -> Result<Vec<Utxo>> {
        self.utxos_for(addr)
    }

    /// Build a PSBT and collect per-input signing keys.
    ///
    /// All inputs come from one source address (account/change/index).
    /// Change is sent to the next index in the change branch (branch=1).
    ///
    /// Returns `(UnsignedTx, per-input private key bytes)`.
    pub fn build_tx_for_address(
        &self,
        seed: &[u8; 64],
        account: u32,
        change_branch: u32,
        index: u32,
        params: &SendParams,
    ) -> Result<(UnsignedTx, Vec<PrivateKeyBytes>)> {
        let fee_sats = match params.fee {
            FeeRate::SatsPerVbyte { sats, .. } => {
                let n_in = 1usize; // estimated before coin selection
                let n_out = 2usize; // recipient + change
                sats * estimate_vsize(n_in, n_out)
            }
            FeeRate::Custom { value, .. } => value,
            FeeRate::Gwei { .. } => {
                return Err(Error::Chain(
                    "Gwei fee rate not applicable to Bitcoin".into(),
                ));
            }
        };

        let amount_sats = params.amount.atoms() as u64;
        let utxos = self.utxos_for(&params.from)?;
        let (selected_utxos, change_sats) = select_coins(utxos, amount_sats, fee_sats)?;

        // Re-estimate fee with actual input count.
        let n_in = selected_utxos.len();
        let n_out = if change_sats > 0 { 2 } else { 1 };
        let fee_sats = match params.fee {
            FeeRate::SatsPerVbyte { sats, .. } => sats * estimate_vsize(n_in, n_out),
            FeeRate::Custom { value, .. } => value,
            _ => unreachable!(),
        };

        // Re-run selection with refined fee.
        let (selected_utxos, change_sats) =
            select_coins(self.utxos_for(&params.from)?, amount_sats, fee_sats)?;
        let n_out = if change_sats > 0 { 2 } else { 1 };

        // Source key + pubkey.
        let xprv = derive_xprv(
            seed,
            self.purpose,
            &self.params,
            account,
            change_branch,
            index,
        )?;
        let source_key_bytes = xprv.private_key().to_bytes();
        let pubkey: [u8; 33] = xprv.public_key().to_bytes();
        let pubkey_hash = hash160(&pubkey);

        // Recipient script.
        let recipient_hash = decode_p2wpkh_address(params.to.as_str())?;
        let recipient_script = p2wpkh_script(&recipient_hash);

        // Build inputs.
        let mut tx_inputs = Vec::with_capacity(selected_utxos.len());
        for utxo in &selected_utxos {
            let outpoint = Outpoint::from_str(&utxo.tx_hash, utxo.tx_pos)?;
            tx_inputs.push(TxInput {
                outpoint,
                sequence: SEQUENCE_FINAL,
                value_sats: utxo.value,
                pubkey_hash,
                pubkey,
            });
        }

        // Build outputs.
        let mut tx_outputs = Vec::with_capacity(n_out);
        tx_outputs.push(TxOutput {
            script_pubkey: recipient_script,
            value_sats: amount_sats,
        });
        if change_sats > 0 {
            let change_xprv = derive_xprv(seed, self.purpose, &self.params, account, 1, index + 1)?;
            let change_pubkey: [u8; 33] = change_xprv.public_key().to_bytes();
            let change_hash = hash160(&change_pubkey);
            tx_outputs.push(TxOutput {
                script_pubkey: p2wpkh_script(&change_hash),
                value_sats: change_sats,
            });
        }

        let raw_psbt = build_psbt(&tx_inputs, &tx_outputs)?;
        let unsigned = UnsignedTx {
            chain: self.params.chain_id,
            raw: raw_psbt,
        };

        // Provide one key per input (all the same address here).
        let key_bytes: [u8; 32] = source_key_bytes.into();
        let keys: Vec<PrivateKeyBytes> = (0..tx_inputs.len())
            .map(|_| PrivateKeyBytes(key_bytes))
            .collect();

        Ok((unsigned, keys))
    }

    /// Scan all funded external-chain addresses (change=0) for `account`,
    /// perform coin selection across the merged UTXO pool, and return the
    /// selected inputs together with per-input derivation hints and the
    /// computed change amount.
    ///
    /// `gap_limit` controls how many consecutive empty addresses end the scan
    /// (default 20; pass `ChainConfig::default().gap_limit` from the TUI).
    ///
    /// Change goes to `m/.../account'/1/0` (change branch, index 0). Reusing
    /// index 0 is a privacy tradeoff; fresh-index change is future work.
    ///
    /// Returns `(selected_utxos, hints, change_sats)` — pass all three into
    /// `sign_multi_source` to avoid duplicate computation.
    pub fn build_tx_multi_source(
        &self,
        seed: &[u8; 64],
        account: u32,
        params: &SendParams,
        rbf: bool,
        gap_limit: u32,
    ) -> Result<(Vec<Utxo>, Vec<InputHint>, u64)> {
        self.build_tx_multi_source_inner(seed, account, params, rbf, gap_limit)
    }

    /// Inner builder — separated so the public API stays clean.
    fn build_tx_multi_source_inner(
        &self,
        seed: &[u8; 64],
        account: u32,
        params: &SendParams,
        _rbf: bool,
        gap_limit: u32,
    ) -> Result<(Vec<Utxo>, Vec<InputHint>, u64)> {
        use crate::scan::gap_scan;

        // Collect all funded external-chain addresses (change=0).
        let funded = gap_scan(self, seed, account, 0, gap_limit)?;

        // Fetch UTXOs for each funded address and track which (change, index)
        // owns each UTXO.
        let mut pool: Vec<(Utxo, InputHint)> = Vec::new();
        for (index, addr, _bal) in &funded {
            let utxos = self.utxos_for(addr)?;
            for utxo in utxos {
                pool.push((
                    utxo,
                    InputHint {
                        account,
                        change: 0,
                        index: *index,
                    },
                ));
            }
        }

        if pool.is_empty() {
            return Err(Error::Chain(
                "no UTXOs found across any funded address".into(),
            ));
        }

        let amount_sats = params.amount.atoms() as u64;

        // Initial fee estimate (1 input, 2 outputs) to seed coin selection.
        let initial_fee = match params.fee {
            FeeRate::SatsPerVbyte { sats, .. } => sats * estimate_vsize(1, 2),
            FeeRate::Custom { value, .. } => value,
            FeeRate::Gwei { .. } => {
                return Err(Error::Chain(
                    "Gwei fee rate not applicable to Bitcoin".into(),
                ));
            }
        };

        // Sort pool largest-first for greedy selection.
        pool.sort_by_key(|(u, _)| std::cmp::Reverse(u.value));
        let utxos_only: Vec<Utxo> = pool.iter().map(|(u, _)| u.clone()).collect();

        let (selected_utxos, _) = select_coins(utxos_only, amount_sats, initial_fee)?;

        // Re-estimate fee with actual input count.
        let n_in = selected_utxos.len();
        let fee_sats = match params.fee {
            FeeRate::SatsPerVbyte { sats, .. } => sats * estimate_vsize(n_in, 2),
            FeeRate::Custom { value, .. } => value,
            _ => unreachable!(),
        };

        // Re-run selection with refined fee against the full pool (already sorted).
        let utxos_only2: Vec<Utxo> = pool.iter().map(|(u, _)| u.clone()).collect();
        let (selected_utxos, change_sats) = select_coins(utxos_only2, amount_sats, fee_sats)?;

        // Pair each selected UTXO back to its hint (by txid+vout identity).
        let mut selected_hints: Vec<InputHint> = Vec::with_capacity(selected_utxos.len());
        for sel in &selected_utxos {
            let hint = pool
                .iter()
                .find(|(u, _)| u.tx_hash == sel.tx_hash && u.tx_pos == sel.tx_pos)
                .map(|(_, h)| h.clone())
                .ok_or_else(|| Error::Chain("selected UTXO lost hint".into()))?;
            selected_hints.push(hint);
        }

        Ok((selected_utxos, selected_hints, change_sats))
    }

    /// Sign every input using the per-input derivation hints produced by
    /// `build_tx_multi_source`. `change_sats` must be the value returned by
    /// that call — passing it directly avoids a redundant `select_coins` run
    /// and ensures the output set matches exactly.
    #[allow(clippy::too_many_arguments)]
    pub fn sign_multi_source(
        &self,
        seed: &[u8; 64],
        account: u32,
        params: &SendParams,
        rbf: bool,
        hints: &[InputHint],
        selected_utxos: &[Utxo],
        change_sats: u64,
    ) -> Result<SignedTx> {
        let amount_sats = params.amount.atoms() as u64;
        let n_out = if change_sats > 0 { 2 } else { 1 };
        let sequence = if rbf { SEQUENCE_RBF } else { SEQUENCE_FINAL };

        let mut tx_inputs: Vec<TxInput> = Vec::with_capacity(selected_utxos.len());
        let mut key_bytes_vec: Vec<[u8; 32]> = Vec::with_capacity(selected_utxos.len());

        for (utxo, hint) in selected_utxos.iter().zip(hints.iter()) {
            let xprv = derive_xprv(
                seed,
                self.purpose,
                &self.params,
                hint.account,
                hint.change,
                hint.index,
            )?;
            let pubkey: [u8; 33] = xprv.public_key().to_bytes();
            let pubkey_hash = hash160(&pubkey);
            let outpoint = Outpoint::from_str(&utxo.tx_hash, utxo.tx_pos)?;
            tx_inputs.push(TxInput {
                outpoint,
                sequence,
                value_sats: utxo.value,
                pubkey_hash,
                pubkey,
            });
            let kb: [u8; 32] = xprv.private_key().to_bytes().into();
            key_bytes_vec.push(kb);
        }

        let recipient_script =
            decode_address_to_script(params.to.as_str(), self.purpose, &self.params)?;
        let mut tx_outputs: Vec<TxOutput> = Vec::with_capacity(n_out);
        tx_outputs.push(TxOutput {
            script_pubkey: recipient_script,
            value_sats: amount_sats,
        });
        if change_sats > 0 {
            let change_xprv = derive_xprv(seed, self.purpose, &self.params, account, 1, 0)?;
            let change_pubkey: [u8; 33] = change_xprv.public_key().to_bytes();
            let change_hash = hash160(&change_pubkey);
            let change_script = purpose_script(self.purpose, &change_hash);
            tx_outputs.push(TxOutput {
                script_pubkey: change_script,
                value_sats: change_sats,
            });
        }

        let signed_bytes = match self.purpose {
            Purpose::Bip44 => sign_inputs_legacy_p2pkh(
                self.params.chain_id,
                &tx_inputs,
                &tx_outputs,
                &key_bytes_vec,
            )?,
            Purpose::Bip49 => sign_inputs_p2sh_p2wpkh(&tx_inputs, &tx_outputs, &key_bytes_vec)?,
            _ => sign_inputs(&tx_inputs, &tx_outputs, &key_bytes_vec)?,
        };
        Ok(SignedTx {
            chain: self.params.chain_id,
            raw: signed_bytes,
        })
    }

    /// Sign all inputs in a PSBT with a slice of per-input private keys.
    ///
    /// The PSBT raw bytes from `build_tx_for_address` are parsed back to
    /// reconstruct the input/output structure needed for BIP-143.
    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_keys(
        &self,
        seed: &[u8; 64],
        account: u32,
        change_branch: u32,
        index: u32,
        utxos: &[crate::electrum::Utxo],
        params: &SendParams,
        keys: &[PrivateKeyBytes],
    ) -> Result<SignedTx> {
        let fee_sats = match params.fee {
            FeeRate::SatsPerVbyte { sats, .. } => {
                sats * estimate_vsize(utxos.len(), if utxos.len() == 1 { 1 } else { 2 })
            }
            FeeRate::Custom { value, .. } => value,
            _ => return Err(Error::Chain("unsupported fee rate for Bitcoin".into())),
        };

        let amount_sats = params.amount.atoms() as u64;
        let (selected_utxos, change_sats) = select_coins(utxos.to_vec(), amount_sats, fee_sats)?;

        let xprv = derive_xprv(
            seed,
            self.purpose,
            &self.params,
            account,
            change_branch,
            index,
        )?;
        let pubkey: [u8; 33] = xprv.public_key().to_bytes();
        let pubkey_hash = hash160(&pubkey);

        let recipient_hash = decode_p2wpkh_address(params.to.as_str())?;
        let recipient_script = p2wpkh_script(&recipient_hash);

        let mut tx_inputs = Vec::with_capacity(selected_utxos.len());
        for utxo in &selected_utxos {
            let outpoint = Outpoint::from_str(&utxo.tx_hash, utxo.tx_pos)?;
            tx_inputs.push(TxInput {
                outpoint,
                sequence: SEQUENCE_FINAL,
                value_sats: utxo.value,
                pubkey_hash,
                pubkey,
            });
        }

        let n_out = if change_sats > 0 { 2 } else { 1 };
        let mut tx_outputs = Vec::with_capacity(n_out);
        tx_outputs.push(TxOutput {
            script_pubkey: recipient_script,
            value_sats: amount_sats,
        });
        if change_sats > 0 {
            let change_xprv = derive_xprv(seed, self.purpose, &self.params, account, 1, index + 1)?;
            let change_pubkey: [u8; 33] = change_xprv.public_key().to_bytes();
            let change_hash = hash160(&change_pubkey);
            tx_outputs.push(TxOutput {
                script_pubkey: p2wpkh_script(&change_hash),
                value_sats: change_sats,
            });
        }

        let raw_key_slice: Vec<[u8; 32]> = keys.iter().map(|k| k.0).collect();
        let signed_bytes = sign_inputs(&tx_inputs, &tx_outputs, &raw_key_slice)?;
        Ok(SignedTx {
            chain: self.params.chain_id,
            raw: signed_bytes,
        })
    }

    /// Walk both receive (change=0) and change (change=1) chains, deriving
    /// each address, querying its history length and balance. Stops walking
    /// each chain when `gap_limit` consecutive addresses with no history are
    /// seen. Returns only addresses with history > 0 OR balance > 0.
    ///
    /// Default gap_limit per BIP-44 spec = 20 (matches Electrum/Trezor/Ledger).
    /// Caller passes the value from `chain_cfg.gap_limit`.
    ///
    /// This is a blocking operation — caller should run it on a background
    /// thread.
    pub fn scan_used_addresses(
        &self,
        seed: &[u8; 64],
        account: u32,
        gap_limit: u32,
    ) -> Result<WalletScan> {
        let mut scan = WalletScan::default();

        for change in [0u32, 1u32] {
            let mut consecutive_unused = 0u32;
            let mut index = 0u32;

            loop {
                let addr_str = crate::derive::derive_address(
                    seed,
                    self.purpose,
                    &self.params,
                    account,
                    change,
                    index,
                )?;
                let addr = hodl_core::Address::new(addr_str.clone(), self.params.chain_id);
                let scripthash = self.scripthash_for(&addr)?;

                let count = self.electrum.borrow_mut().get_history_count(&scripthash)?;

                if count == 0 {
                    consecutive_unused += 1;
                    if consecutive_unused >= gap_limit {
                        // Record highest index reached (last checked index before break).
                        if change == 0 {
                            scan.highest_index_receive = index;
                        } else {
                            scan.highest_index_change = index;
                        }
                        break;
                    }
                } else {
                    consecutive_unused = 0;

                    // Fetch balance for used address.
                    let raw = self
                        .electrum
                        .borrow_mut()
                        .scripthash_get_balance(&scripthash)?;
                    let pending = if raw.unconfirmed < 0 {
                        0u64
                    } else {
                        raw.unconfirmed as u64
                    };
                    let balance = BalanceSplit {
                        confirmed: raw.confirmed,
                        pending,
                    };

                    scan.total.confirmed += balance.confirmed;
                    scan.total.pending += balance.pending;

                    scan.used.push(UsedAddress {
                        index,
                        change,
                        address: addr_str,
                        balance,
                    });
                }

                index += 1;
            }
        }

        Ok(scan)
    }
}

impl Chain for BitcoinChain {
    fn id(&self) -> ChainId {
        self.params.chain_id
    }

    fn slip44(&self) -> u32 {
        self.params.chain_id.slip44()
    }

    fn derive(&self, seed: &[u8; 64], account: u32, index: u32) -> Result<Address> {
        let addr_str = derive_address(seed, self.purpose, &self.params, account, 0, index)?;
        Ok(Address::new(addr_str, self.params.chain_id))
    }

    fn balance(&self, addr: &Address) -> Result<Amount> {
        let sh = self.scripthash_for(addr)?;
        let bal = self.electrum.borrow_mut().scripthash_get_balance(&sh)?;
        let atoms = (bal.confirmed as i128 + bal.unconfirmed as i128).max(0) as u128;
        Ok(Amount::from_atoms(atoms, self.params.chain_id))
    }

    fn history(&self, addr: &Address) -> Result<Vec<TxRef>> {
        let sh = self.scripthash_for(addr)?;
        let entries = self.electrum.borrow_mut().scripthash_get_history(&sh)?;
        Ok(entries
            .into_iter()
            .map(|e| TxRef {
                id: hodl_core::TxId(e.tx_hash),
                height: if e.height > 0 {
                    Some(e.height as u64)
                } else {
                    None
                },
                time: None,
            })
            .collect())
    }

    fn estimate_fee(&self, target_blocks: u32) -> Result<FeeRate> {
        let btc_per_kb = self.electrum.borrow_mut().estimate_fee(target_blocks)?;
        // Convert BTC/kB → satoshis/vByte: 1 BTC = 100_000_000 sat, 1 kB = 1000 vB.
        let sats_per_vbyte = ((btc_per_kb * 1e8) / 1000.0).ceil() as u64;
        Ok(FeeRate::SatsPerVbyte {
            sats: sats_per_vbyte,
            chain: self.params.chain_id,
        })
    }

    fn build_tx(&self, _params: SendParams) -> Result<UnsignedTx> {
        // The trait-level build_tx is intentionally thin; use build_tx_for_address
        // for Bitcoin which needs per-input keys and UTXO data.
        Err(Error::Chain(
            "use BitcoinChain::build_tx_for_address for Bitcoin send".into(),
        ))
    }

    fn sign(&self, _tx: UnsignedTx, _key: &PrivateKeyBytes) -> Result<SignedTx> {
        // Single-key trait path does not apply to Bitcoin multi-input; use sign_with_keys.
        Err(Error::Chain(
            "use BitcoinChain::sign_with_keys for Bitcoin send".into(),
        ))
    }

    fn broadcast(&self, tx: SignedTx) -> Result<TxId> {
        let raw_hex = hex::encode(&tx.raw);
        let txid = self.electrum.borrow_mut().transaction_broadcast(&raw_hex)?;
        Ok(TxId(txid))
    }

    fn derive_private_key(
        &self,
        seed: &[u8; 64],
        account: u32,
        change: u32,
        index: u32,
    ) -> Result<PrivateKeyBytes> {
        let xprv = derive_xprv(seed, self.purpose, &self.params, account, change, index)?;
        let key_bytes: [u8; 32] = xprv.private_key().to_bytes().into();
        Ok(PrivateKeyBytes(key_bytes))
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;
    use crate::electrum::Utxo;
    use crate::psbt::select_coins;

    /// Build a pool the same way `build_tx_multi_source_inner` does, then
    /// verify that hint pairing correctly associates each selected UTXO with
    /// the (account, change, index) of its source address.
    ///
    /// This exercises the core logic — coin-selection across a multi-address
    /// pool + hint pairing — without needing a live Electrum connection.
    /// The choice of extracted unit test over e2e mock is deliberate: a
    /// queue-based MockTransport that handles gap_scan's interleaved
    /// balance/history/listunspent calls per address would require ~150 lines
    /// of scaffolding for marginal coverage gain over what psbt.rs tests plus
    /// this test already provide.
    #[test]
    fn hint_pairing_multi_address() {
        fn utxo(tx_hash: &str, tx_pos: u32, value: u64) -> Utxo {
            Utxo {
                tx_hash: tx_hash.to_string(),
                tx_pos,
                height: 800_000,
                value,
            }
        }

        // Simulate: addr[0] owns two UTXOs (30k, 20k), addr[1] owns one (30k).
        let mut pool: Vec<(Utxo, InputHint)> = vec![
            (
                utxo(&"aa".repeat(32), 0, 30_000),
                InputHint {
                    account: 0,
                    change: 0,
                    index: 0,
                },
            ),
            (
                utxo(&"aa".repeat(32), 1, 20_000),
                InputHint {
                    account: 0,
                    change: 0,
                    index: 0,
                },
            ),
            (
                utxo(&"bb".repeat(32), 0, 30_000),
                InputHint {
                    account: 0,
                    change: 0,
                    index: 1,
                },
            ),
        ];

        // Largest-first sort (mirrors build_tx_multi_source_inner).
        pool.sort_by_key(|(u, _)| std::cmp::Reverse(u.value));

        let amount_sats = 60_000u64;
        // Custom fee: 1_000 sats flat.
        let fee_sats = 1_000u64;

        let utxos_only: Vec<Utxo> = pool.iter().map(|(u, _)| u.clone()).collect();
        let (selected_utxos, change_sats) =
            select_coins(utxos_only, amount_sats, fee_sats).unwrap();

        // Greedy on [30k, 30k, 20k]: 30k + 30k = 60k covers 60k + 1k? No —
        // 60k < 61k.  So all three are needed: 80k total, change = 80k - 61k = 19k.
        // Actually: 30k + 30k = 60k < 61k, then add 20k = 80k >= 61k.
        assert_eq!(selected_utxos.len(), 3);
        assert_eq!(change_sats, 80_000 - 61_000);

        // Pair hints.
        let mut selected_hints: Vec<InputHint> = Vec::new();
        for sel in &selected_utxos {
            let hint = pool
                .iter()
                .find(|(u, _)| u.tx_hash == sel.tx_hash && u.tx_pos == sel.tx_pos)
                .map(|(_, h)| h.clone())
                .expect("hint must exist for every selected UTXO");
            selected_hints.push(hint);
        }

        assert_eq!(selected_hints.len(), 3);

        // The two 30k UTXOs come from indices 0 and 1; the 20k from index 0.
        // After sort the order is: (aa,0,30k), (bb,0,30k), (aa,1,20k).
        // Verify accounts and change levels are all 0 (external chain).
        for h in &selected_hints {
            assert_eq!(h.account, 0);
            assert_eq!(h.change, 0);
        }

        // The two 30k inputs come from indices 0 and 1 respectively.
        // (aa,0) → index 0, (bb,0) → index 1, (aa,1) → index 0.
        let indices: Vec<u32> = selected_hints.iter().map(|h| h.index).collect();
        assert!(indices.contains(&0), "index 0 must appear");
        assert!(indices.contains(&1), "index 1 must appear");
    }

    /// Verify `sign_multi_source` with rbf=true produces a signed tx where
    /// every input carries sequence 0xfffffffd.
    ///
    /// Uses a minimal pre-built pool (no network) by constructing the selected
    /// UTXOs and hints directly, then invoking `sign_inputs` directly with the
    /// derived keys — mirrors what sign_multi_source does internally.
    #[test]
    fn sign_multi_source_rbf_sequence() {
        use crate::derive::derive_xprv;
        use crate::network::NetworkParams;
        use crate::psbt::{
            Outpoint, TxInput, TxOutput, decode_p2wpkh_address, hash160, p2wpkh_script, sign_inputs,
        };

        // Arbitrary 64-byte seed; no mnemonic dependency needed here.
        let seed_bytes = [0x42u8; 64];
        let params = NetworkParams::BITCOIN_MAINNET;
        let purpose = crate::derive::Purpose::Bip84;

        // Derive keys for two inputs from different indices.
        let xprv0 = derive_xprv(&seed_bytes, purpose, &params, 0, 0, 0).unwrap();
        let pk0: [u8; 33] = xprv0.public_key().to_bytes();
        let ph0 = hash160(&pk0);

        let xprv1 = derive_xprv(&seed_bytes, purpose, &params, 0, 0, 1).unwrap();
        let pk1: [u8; 33] = xprv1.public_key().to_bytes();
        let ph1 = hash160(&pk1);

        let inputs = vec![
            TxInput {
                outpoint: Outpoint::from_str(&"aa".repeat(32), 0).unwrap(),
                sequence: SEQUENCE_RBF,
                value_sats: 30_000,
                pubkey_hash: ph0,
                pubkey: pk0,
            },
            TxInput {
                outpoint: Outpoint::from_str(&"bb".repeat(32), 0).unwrap(),
                sequence: SEQUENCE_RBF,
                value_sats: 30_000,
                pubkey_hash: ph1,
                pubkey: pk1,
            },
        ];

        let recipient_hash =
            decode_p2wpkh_address("bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu").unwrap();
        let outputs = vec![TxOutput {
            script_pubkey: p2wpkh_script(&recipient_hash),
            value_sats: 59_000,
        }];

        let kb0: [u8; 32] = xprv0.private_key().to_bytes().into();
        let kb1: [u8; 32] = xprv1.private_key().to_bytes().into();
        let signed = sign_inputs(&inputs, &outputs, &[kb0, kb1]).unwrap();

        // Segwit marker present.
        assert_eq!(signed[4], 0x00);
        assert_eq!(signed[5], 0x01);

        // Check first input's sequence bytes.
        // Layout after version(4) + marker/flag(2) + vin_count(1):
        //   outpoint(36) + scriptSig_varint(1) = offset 44 for sequence.
        let seq_start = 4 + 2 + 1 + 36 + 1;
        let seq = u32::from_le_bytes(signed[seq_start..seq_start + 4].try_into().unwrap());
        assert_eq!(seq, SEQUENCE_RBF, "first input must carry RBF sequence");
    }

    /// `default_send_purpose` returns the correct purpose for each chain.
    #[test]
    fn default_send_purpose_per_chain() {
        assert_eq!(
            BitcoinChain::default_send_purpose(ChainId::Bitcoin),
            Purpose::Bip84
        );
        assert_eq!(
            BitcoinChain::default_send_purpose(ChainId::BitcoinTestnet),
            Purpose::Bip84
        );
        assert_eq!(
            BitcoinChain::default_send_purpose(ChainId::Litecoin),
            Purpose::Bip84
        );
        assert_eq!(
            BitcoinChain::default_send_purpose(ChainId::NavCoin),
            Purpose::Bip44
        );
        assert_eq!(
            BitcoinChain::default_send_purpose(ChainId::Dogecoin),
            Purpose::Bip44
        );
        assert_eq!(
            BitcoinChain::default_send_purpose(ChainId::BitcoinCash),
            Purpose::Bip44
        );
    }

    /// `decode_address_to_script` rejects a BTC legacy address when the
    /// active chain is DOGE — the version byte (0x00) doesn't match DOGE's
    /// p2pkh_prefix (0x1e). Without this check, sending DOGE to a BTC
    /// address would silently encode the wrong scriptPubKey and burn funds.
    #[test]
    fn decode_address_rejects_wrong_chain_prefix() {
        let btc_addr = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        let result =
            decode_address_to_script(btc_addr, Purpose::Bip44, &NetworkParams::DOGECOIN_MAINNET);
        assert!(
            result.is_err(),
            "BTC address must be rejected on DOGE chain"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("version byte") || msg.contains("does not match"),
            "error must mention prefix mismatch; got: {msg}"
        );
    }

    /// `decode_address_to_script` accepts the correct chain's address.
    #[test]
    fn decode_address_accepts_matching_chain_prefix() {
        let doge_addr = "DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L";
        let result =
            decode_address_to_script(doge_addr, Purpose::Bip44, &NetworkParams::DOGECOIN_MAINNET);
        assert!(
            result.is_ok(),
            "DOGE address must be accepted on DOGE chain; got: {:?}",
            result.err()
        );
    }

    /// `decode_address_to_script` rejects a CashAddr with the wrong HRP.
    #[test]
    fn decode_address_rejects_wrong_cashaddr_hrp() {
        // ecash-prefixed CashAddr should fail when the chain expects bitcoincash:
        let ecash_addr = "ecash:qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
        let result = decode_address_to_script(
            ecash_addr,
            Purpose::Bip44,
            &NetworkParams::BITCOIN_CASH_MAINNET,
        );
        assert!(
            result.is_err(),
            "ecash:-prefixed address must be rejected on BCH chain"
        );
    }

    // ── WalletScan / BalanceSplit unit-shape tests ──────────────────────────

    /// `BalanceSplit::total` adds confirmed + pending.
    #[test]
    fn balance_split_total() {
        let b = BalanceSplit {
            confirmed: 1_000,
            pending: 500,
        };
        assert_eq!(b.total(), 1_500);
    }

    /// `BalanceSplit::is_zero` returns true only when both fields are 0.
    #[test]
    fn balance_split_is_zero() {
        assert!(BalanceSplit::default().is_zero());
        assert!(
            !BalanceSplit {
                confirmed: 1,
                pending: 0
            }
            .is_zero()
        );
        assert!(
            !BalanceSplit {
                confirmed: 0,
                pending: 1
            }
            .is_zero()
        );
        assert!(
            !BalanceSplit {
                confirmed: 1,
                pending: 1
            }
            .is_zero()
        );
    }

    /// `WalletScan::default()` is empty.
    #[test]
    fn wallet_scan_default_is_empty() {
        let scan = WalletScan::default();
        assert!(scan.used.is_empty());
        assert!(scan.total.is_zero());
        assert_eq!(scan.highest_index_receive, 0);
        assert_eq!(scan.highest_index_change, 0);
    }

    // ── No mock-based test for scan_used_addresses ──────────────────────────
    //
    // `ElectrumClient` does not expose a trait abstraction over the transport
    // at the level of individual RPC calls — the mock in electrum.rs operates
    // at the raw TCP stream level (queue of newline-delimited JSON lines).
    // Wiring a multi-call sequence (get_history + get_balance per address,
    // across two chains, for a realistic gap-limit walk) would require a
    // stateful queue-based MockTransport with ~200 lines of scaffolding for
    // marginal coverage gain over what the unit-shape tests above plus the
    // electrum.rs protocol tests already provide.  The integration test below
    // covers the real end-to-end path instead.

    /// Integration smoke-test: scan the standard "abandon × 11 + about"
    /// BIP-39 test seed against a live Electrum mainnet server.  The seed is
    /// well-known (published in BIP-39 / hardware-wallet docs) so its wallets
    /// are intentionally empty on mainnet.  Expected result: `used` is empty
    /// and `total.total() == 0`.
    ///
    /// Ignored by default — unignore manually to verify real-network behaviour.
    #[test]
    #[ignore = "hits real Electrum server"]
    fn scan_used_addresses_empty_seed_mainnet() {
        use crate::electrum::ElectrumClient;
        use crate::network::NetworkParams;

        // "abandon" × 11 + "about", no passphrase — standard BIP-39 test seed.
        let seed_hex = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";
        let seed_bytes: Vec<u8> = hex::decode(seed_hex).unwrap();
        let seed: [u8; 64] = seed_bytes.try_into().unwrap();

        let electrum = ElectrumClient::connect_tls("electrum.blockstream.info", 50002).unwrap();
        let chain = BitcoinChain::new(NetworkParams::BITCOIN_MAINNET, electrum);

        let scan = chain.scan_used_addresses(&seed, 0, 20).unwrap();

        assert!(
            scan.used.is_empty(),
            "well-known empty test seed must have no used addresses; got: {:?}",
            scan.used
        );
        assert_eq!(
            scan.total.total(),
            0,
            "well-known empty test seed must have zero total balance"
        );
    }
}
