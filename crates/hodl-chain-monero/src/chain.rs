//! MoneroChain — implements the hodl-core Chain trait.
//!
//! M7 scope: derive (BIP-39 → Ledger-compat address), balance + history via LWS,
//! broadcast via daemon RPC.
//!
//! build_tx and sign return clear errors — ring signatures, bulletproofs, and
//! output stealth address construction are post-v1.

use hodl_core::error::{Error, Result};
use hodl_core::{
    Address, Amount, Chain, ChainId, FeeRate, PrivateKeyBytes, SendParams, SignedTx, TxId, TxRef,
    UnsignedTx,
};

use crate::derive::{derive_keys, pubkey_from_secret, standard_address};
use crate::lws::LwsClient;
use crate::network::NetworkParams;
use crate::rpc::DaemonRpcClient;

/// Monero chain implementation.
///
/// `lws` is required for balance + history. `daemon` is required for broadcast.
/// Either may be None if the user has not configured an endpoint — the
/// corresponding methods return a descriptive error in that case.
pub struct MoneroChain {
    params: NetworkParams,
    lws: Option<LwsClient>,
    daemon: Option<DaemonRpcClient>,
}

impl MoneroChain {
    pub fn new(
        params: NetworkParams,
        lws: Option<LwsClient>,
        daemon: Option<DaemonRpcClient>,
    ) -> Self {
        Self {
            params,
            lws,
            daemon,
        }
    }
}

impl Chain for MoneroChain {
    fn id(&self) -> ChainId {
        self.params.chain_id
    }

    fn slip44(&self) -> u32 {
        self.params.chain_id.slip44()
    }

    /// Derive the primary Monero address from a BIP-39 seed.
    ///
    /// `account` is accepted for trait compatibility but Monero's primary
    /// address is always at m/44'/128'/0'/0/0 — subaddresses are post-v1.
    /// `index` is ignored for the same reason.
    fn derive(&self, seed: &[u8; 64], _account: u32, _index: u32) -> Result<Address> {
        let keys = derive_keys(seed)?;
        let spend_pub = pubkey_from_secret(&keys.spend);
        let view_pub = pubkey_from_secret(&keys.view);
        let addr = standard_address(&spend_pub, &view_pub, self.params.address_prefix);
        Ok(Address::new(addr, self.params.chain_id))
    }

    fn balance(&self, addr: &Address) -> Result<Amount> {
        let lws = self.lws.as_ref().ok_or_else(|| {
            Error::Endpoint(
                "Monero balance requires an LWS endpoint. Configure one via config.toml \
                 (Endpoint::Lws). No default endpoint is provided — privacy policy requires \
                 self-hosting open-monero-server."
                    .to_string(),
            )
        })?;
        // We need the view key to authenticate with LWS. Re-derive from seed
        // is not possible here (Chain::balance only receives Address). Callers
        // that need live balance should use MoneroChain directly via a higher-
        // level wallet layer that holds the seed. For now surface the info we
        // can compute from the address alone: address info via LWS.
        // LWS login/auth with a view key is the wallet layer's responsibility;
        // balance() here is a protocol stub that will be wired up in M8 when
        // the wallet layer passes through the view key alongside the address.
        tracing::debug!(
            "MoneroChain::balance called for {}; LWS endpoint present",
            addr.as_str()
        );
        let _ = lws; // used above in is_some check
        Err(Error::Chain(
            "MoneroChain::balance requires the view key for LWS authentication. \
             Wire up via the wallet layer (M8). The LWS client is available — \
             configure your endpoint and call lws.get_address_info() directly."
                .to_string(),
        ))
    }

    fn history(&self, addr: &Address) -> Result<Vec<TxRef>> {
        let lws = self.lws.as_ref().ok_or_else(|| {
            Error::Endpoint(
                "Monero history requires an LWS endpoint. Configure one via config.toml."
                    .to_string(),
            )
        })?;
        tracing::debug!(
            "MoneroChain::history called for {}; LWS endpoint present",
            addr.as_str()
        );
        let _ = lws;
        Err(Error::Chain(
            "MoneroChain::history requires the view key for LWS authentication. \
             Wire up via the wallet layer (M8)."
                .to_string(),
        ))
    }

    fn estimate_fee(&self, _target_blocks: u32) -> Result<FeeRate> {
        // Monero uses a dynamic fee based on the fee per byte from the daemon.
        // A proper implementation queries /get_fee_estimate from the daemon.
        // For M7 we return a coarse default (0.0001 XMR = 100_000_000 piconero).
        tracing::debug!(
            "MoneroChain::estimate_fee: returning coarse default (0.0001 XMR); \
             wire up daemon fee estimation in M8"
        );
        Ok(FeeRate::Custom {
            value: 100_000_000,
            chain: self.params.chain_id,
        })
    }

    fn build_tx(&self, _params: SendParams) -> Result<UnsignedTx> {
        Err(Error::Chain(
            "MoneroChain::build_tx is not implemented. Full Monero send requires \
             ring signature construction, bulletproof range proofs, and output \
             stealth address derivation — these are post-v1 (after M7). \
             M7 covers receive and balance only."
                .to_string(),
        ))
    }

    fn sign(&self, _tx: UnsignedTx, _key: &PrivateKeyBytes) -> Result<SignedTx> {
        Err(Error::Chain(
            "MoneroChain::sign is not implemented. Full Monero signing requires \
             ring signature construction (post-v1)."
                .to_string(),
        ))
    }

    fn broadcast(&self, tx: SignedTx) -> Result<TxId> {
        let daemon = self.daemon.as_ref().ok_or_else(|| {
            Error::Endpoint(
                "Monero broadcast requires a daemon endpoint. Configure one via config.toml \
                 (Endpoint::JsonRpc pointing at your own Monero node). No default endpoint \
                 is provided."
                    .to_string(),
            )
        })?;
        let tx_hex = hex::encode(&tx.raw);
        daemon.send_raw_transaction(&tx_hex)
    }

    /// Returns the spend key (32 bytes) for this seed at the Monero derivation path.
    ///
    /// `account`, `change`, and `index` are ignored — Monero uses a single
    /// fixed path (m/44'/128'/0'/0/0) for the primary spend key.
    fn derive_private_key(
        &self,
        seed: &[u8; 64],
        _account: u32,
        _change: u32,
        _index: u32,
    ) -> Result<PrivateKeyBytes> {
        let keys = derive_keys(seed)?;
        Ok(PrivateKeyBytes(keys.spend))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABANDON_SEED_HEX: &str = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";

    fn seed_bytes() -> [u8; 64] {
        hex::decode(ABANDON_SEED_HEX).unwrap().try_into().unwrap()
    }

    fn chain() -> MoneroChain {
        MoneroChain::new(NetworkParams::MAINNET, None, None)
    }

    #[test]
    fn chain_id_and_slip44() {
        let c = chain();
        assert_eq!(c.id(), ChainId::Monero);
        assert_eq!(c.slip44(), 128);
    }

    #[test]
    fn derive_address_shape() {
        let c = chain();
        let seed = seed_bytes();
        let addr = c.derive(&seed, 0, 0).unwrap();
        assert_eq!(addr.chain(), ChainId::Monero);
        assert_eq!(addr.as_str().len(), 95);
        assert!(
            addr.as_str().starts_with('4'),
            "mainnet address must start with '4'"
        );
    }

    #[test]
    fn derive_address_deterministic() {
        let c = chain();
        let seed = seed_bytes();
        let a1 = c.derive(&seed, 0, 0).unwrap();
        let a2 = c.derive(&seed, 0, 0).unwrap();
        assert_eq!(a1, a2);
    }

    #[test]
    fn balance_without_lws_returns_endpoint_error() {
        let c = chain();
        let addr = Address::new("4fake", ChainId::Monero);
        let err = c.balance(&addr).unwrap_err();
        assert!(
            err.to_string().contains("LWS endpoint"),
            "expected LWS error, got: {err}"
        );
    }

    #[test]
    fn build_tx_stub_error() {
        let c = chain();
        let params = SendParams {
            from: Address::new("4from", ChainId::Monero),
            to: Address::new("4to", ChainId::Monero),
            amount: Amount::from_atoms(1_000_000_000_000, ChainId::Monero),
            fee: FeeRate::Custom {
                value: 100_000_000,
                chain: ChainId::Monero,
            },
        };
        let err = c.build_tx(params).unwrap_err();
        assert!(
            err.to_string().contains("not implemented"),
            "expected stub error, got: {err}"
        );
    }

    #[test]
    fn sign_stub_error() {
        let c = chain();
        let tx = UnsignedTx {
            chain: ChainId::Monero,
            raw: vec![],
        };
        let key = PrivateKeyBytes([0u8; 32]);
        let err = c.sign(tx, &key).unwrap_err();
        assert!(
            err.to_string().contains("not implemented"),
            "expected stub error, got: {err}"
        );
    }

    #[test]
    fn derive_private_key_returns_spend_key() {
        let c = chain();
        let seed = seed_bytes();
        let pk = c.derive_private_key(&seed, 0, 0, 0).unwrap();
        // spend key must not be all zeros.
        assert_ne!(pk.0, [0u8; 32]);
    }

    #[test]
    fn estimate_fee_returns_custom() {
        let c = chain();
        let fee = c.estimate_fee(6).unwrap();
        assert!(matches!(
            fee,
            FeeRate::Custom {
                chain: ChainId::Monero,
                ..
            }
        ));
    }
}
