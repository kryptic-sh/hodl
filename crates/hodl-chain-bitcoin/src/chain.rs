use std::cell::RefCell;

use hodl_core::error::{Error, Result};
use hodl_core::{
    Address, Amount, Chain, ChainId, FeeRate, PrivateKeyBytes, SendParams, SignedTx, TxId, TxRef,
    UnsignedTx,
};

use crate::derive::{Purpose, derive_address};
use crate::electrum::{ElectrumClient, p2wpkh_scripthash};
use crate::network::NetworkParams;

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
        // Fallback: treat address as a raw script (not decoded here — PE scope).
        Err(Error::Chain(
            "scripthash computation for non-P2WPKH addresses not yet implemented".into(),
        ))
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
        Err(Error::Chain("not implemented — see PE (M3 send)".into()))
    }

    fn sign(&self, _tx: UnsignedTx, _key: &PrivateKeyBytes) -> Result<SignedTx> {
        Err(Error::Chain("not implemented — see PE (M3 send)".into()))
    }

    fn broadcast(&self, _tx: SignedTx) -> Result<TxId> {
        Err(Error::Chain("not implemented — see PE (M3 send)".into()))
    }
}
