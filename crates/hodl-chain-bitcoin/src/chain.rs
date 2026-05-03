use std::cell::RefCell;

use hodl_core::error::{Error, Result};
use hodl_core::{
    Address, Amount, Chain, ChainId, FeeRate, PrivateKeyBytes, SendParams, SignedTx, TxId, TxRef,
    UnsignedTx,
};

use crate::derive::{Purpose, derive_address, derive_xprv};
use crate::electrum::{ElectrumClient, Utxo, p2wpkh_scripthash};
use crate::network::NetworkParams;
use crate::psbt::{
    Outpoint, TxInput, TxOutput, build_psbt, decode_p2wpkh_address, estimate_vsize, hash160,
    p2wpkh_script, select_coins, sign_inputs,
};

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
        Self {
            params,
            electrum: RefCell::new(electrum),
            purpose: Purpose::Bip84,
        }
    }

    pub fn with_purpose(mut self, purpose: Purpose) -> Self {
        self.purpose = purpose;
        self
    }

    /// Compute the Electrum scripthash for an address string.
    ///
    /// Only supports bech32 P2WPKH for now; the full UTXO path is PE scope.
    fn scripthash_for(&self, addr: &Address) -> Result<String> {
        let s = addr.as_str();
        // Decode bech32 witness program to get the pubkey hash.
        let (_, witness_ver, prog) =
            bech32::segwit::decode(s).map_err(|e| Error::Codec(format!("bech32 decode: {e}")))?;
        if witness_ver == bech32::segwit::VERSION_0 && prog.len() == 20 {
            let h160: [u8; 20] = prog.try_into().unwrap();
            return Ok(p2wpkh_scripthash(&h160));
        }
        Err(Error::Chain(
            "scripthash computation for non-P2WPKH addresses not yet implemented".into(),
        ))
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
                sequence: 0xffff_fffe,
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
                sequence: 0xffff_fffe,
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
