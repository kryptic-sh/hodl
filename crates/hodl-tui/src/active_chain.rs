//! Per-chain dispatch enum. Picks the right concrete chain crate based on
//! `ChainId` and the user's config (endpoint type + URL + Tor toggle).

use hodl_chain_bitcoin::electrum::{ElectrumClient, Utxo};
use hodl_chain_bitcoin::{BitcoinChain, InputHint, NetworkParams as BtcNetworkParams, Purpose};
use hodl_chain_ethereum::{EthRpcClient, EthereumChain, NetworkParams as EthNetworkParams};
use hodl_chain_monero::{LwsClient, MoneroChain, NetworkParams as XmrNetworkParams};
use hodl_config::{Config, Endpoint};
use hodl_core::error::{Error, Result};
use hodl_core::{Address, Amount, Chain, ChainId, FeeRate, SendParams, TxId, UnsignedTx};

pub enum ActiveChain {
    Bitcoin(BitcoinChain),
    Ethereum(EthereumChain),
    Monero(MoneroChain),
}

/// Pre-built send pipeline payload.
pub enum PreparedSend {
    Bitcoin {
        utxos: Vec<Utxo>,
        hints: Vec<InputHint>,
        change_sats: u64,
        rbf: bool,
    },
    Ethereum {
        unsigned: UnsignedTx,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SendOpts {
    pub rbf: bool,
    pub gap_limit: u32,
}

impl ActiveChain {
    pub fn from_chain_id(id: ChainId, config: &Config) -> Result<Self> {
        let proxy = if config.tor.enabled {
            Some(config.tor.socks5.as_str())
        } else {
            None
        };

        match id {
            ChainId::Bitcoin
            | ChainId::BitcoinTestnet
            | ChainId::Litecoin
            | ChainId::Dogecoin
            | ChainId::BitcoinCash
            | ChainId::NavCoin => {
                let params = btc_network_params(id);
                let chain_cfg = config.chains.get(&id).cloned().unwrap_or_default();
                let endpoint = chain_cfg
                    .endpoints
                    .iter()
                    .find(|ep| matches!(ep, Endpoint::Electrum { .. }))
                    .ok_or_else(|| {
                        Error::Endpoint(format!(
                            "no Electrum endpoint configured for {}",
                            id.display_name()
                        ))
                    })?;
                let electrum = electrum_connect(endpoint, proxy)?;
                Ok(ActiveChain::Bitcoin(
                    BitcoinChain::new(params, electrum).with_purpose(Purpose::Bip84),
                ))
            }
            ChainId::Ethereum | ChainId::BscMainnet => {
                let params = eth_network_params(id);
                let chain_cfg = config.chains.get(&id).cloned().unwrap_or_default();
                let endpoint = chain_cfg
                    .endpoints
                    .iter()
                    .find(|ep| matches!(ep, Endpoint::JsonRpc { .. }))
                    .ok_or_else(|| {
                        Error::Endpoint(format!(
                            "no JsonRpc endpoint configured for {}",
                            id.display_name()
                        ))
                    })?;
                let url = match endpoint {
                    Endpoint::JsonRpc { url } => url.clone(),
                    _ => unreachable!(),
                };
                let rpc = match proxy {
                    Some(p) => EthRpcClient::with_socks5(url, p)?,
                    None => EthRpcClient::new(url),
                };
                Ok(ActiveChain::Ethereum(EthereumChain::new(params, rpc)))
            }
            ChainId::Monero => {
                let chain_cfg = config.chains.get(&id).cloned().unwrap_or_default();
                let lws_endpoint = chain_cfg
                    .endpoints
                    .iter()
                    .find(|ep| matches!(ep, Endpoint::Lws { .. }))
                    .ok_or_else(|| {
                        Error::Endpoint("no Lws endpoint configured for Monero".into())
                    })?;
                let lws_url = match lws_endpoint {
                    Endpoint::Lws { url } => url.clone(),
                    _ => unreachable!(),
                };
                let lws = LwsClient::new(lws_url);
                Ok(ActiveChain::Monero(MoneroChain::new(
                    XmrNetworkParams::MAINNET,
                    Some(lws),
                    None,
                )))
            }
        }
    }

    pub fn chain_id(&self) -> ChainId {
        match self {
            ActiveChain::Bitcoin(c) => c.id(),
            ActiveChain::Ethereum(c) => c.id(),
            ActiveChain::Monero(c) => c.id(),
        }
    }

    pub fn derive(&self, seed: &[u8; 64], account: u32, index: u32) -> Result<Address> {
        match self {
            ActiveChain::Bitcoin(c) => c.derive(seed, account, index),
            ActiveChain::Ethereum(c) => c.derive(seed, account, index),
            ActiveChain::Monero(c) => c.derive(seed, account, index),
        }
    }

    pub fn balance(&self, addr: &Address) -> Result<Amount> {
        match self {
            ActiveChain::Bitcoin(c) => c.balance(addr),
            ActiveChain::Ethereum(c) => c.balance(addr),
            ActiveChain::Monero(c) => c.balance(addr),
        }
    }

    pub fn estimate_fee(&self, target_blocks: u32) -> Result<FeeRate> {
        match self {
            ActiveChain::Bitcoin(c) => c.estimate_fee(target_blocks),
            ActiveChain::Ethereum(c) => c.estimate_fee(target_blocks),
            ActiveChain::Monero(c) => c.estimate_fee(target_blocks),
        }
    }

    pub fn build_send(
        &self,
        seed: &[u8; 64],
        account: u32,
        params: &SendParams,
        opts: SendOpts,
    ) -> Result<PreparedSend> {
        match self {
            ActiveChain::Bitcoin(c) => {
                let chain_id = c.id();
                // Only BTC mainnet and testnet have segwit-v0 PSBT send support.
                // DOGE/BCH/NAV/LTC all share this code path but legacy P2PKH signing
                // (needed for DOGE, BCH, NAV) is post-v1 — gate here so the dispatch
                // shape is in place for future match arms.
                if !matches!(chain_id, ChainId::Bitcoin | ChainId::BitcoinTestnet) {
                    return Err(Error::Chain(format!(
                        "send for {} not yet implemented; only BTC/BTC-testnet have \
                         segwit-v0 PSBT support today",
                        chain_id.display_name()
                    )));
                }
                let (utxos, hints, change_sats) =
                    c.build_tx_multi_source(seed, account, params, opts.rbf, opts.gap_limit)?;
                Ok(PreparedSend::Bitcoin {
                    utxos,
                    hints,
                    change_sats,
                    rbf: opts.rbf,
                })
            }
            ActiveChain::Ethereum(c) => {
                let unsigned = c.build_tx(params.clone())?;
                Ok(PreparedSend::Ethereum { unsigned })
            }
            ActiveChain::Monero(_) => {
                Err(Error::Chain("Monero send not implemented (post-v1)".into()))
            }
        }
    }

    pub fn sign_and_broadcast(
        &self,
        seed: &[u8; 64],
        account: u32,
        params: &SendParams,
        prepared: PreparedSend,
    ) -> Result<TxId> {
        match (self, prepared) {
            (
                ActiveChain::Bitcoin(c),
                PreparedSend::Bitcoin {
                    utxos,
                    hints,
                    change_sats,
                    rbf,
                },
            ) => {
                let signed =
                    c.sign_multi_source(seed, account, params, rbf, &hints, &utxos, change_sats)?;
                c.broadcast(signed)
            }
            (ActiveChain::Ethereum(c), PreparedSend::Ethereum { unsigned }) => {
                let key = c.derive_private_key(seed, account, 0, 0)?;
                let signed = c.sign(unsigned, &key)?;
                c.broadcast(signed)
            }
            _ => Err(Error::Chain("send-prepared/chain mismatch".into())),
        }
    }
}

fn btc_network_params(id: ChainId) -> BtcNetworkParams {
    match id {
        ChainId::Bitcoin => BtcNetworkParams::BITCOIN_MAINNET,
        ChainId::BitcoinTestnet => BtcNetworkParams::BITCOIN_TESTNET,
        ChainId::Litecoin => BtcNetworkParams::LITECOIN_MAINNET,
        ChainId::Dogecoin => BtcNetworkParams::DOGECOIN_MAINNET,
        ChainId::BitcoinCash => BtcNetworkParams::BITCOIN_CASH_MAINNET,
        ChainId::NavCoin => BtcNetworkParams::NAVCOIN_MAINNET,
        _ => unreachable!("non-Bitcoin chain passed to btc_network_params"),
    }
}

fn eth_network_params(id: ChainId) -> EthNetworkParams {
    match id {
        ChainId::Ethereum => EthNetworkParams::ETHEREUM_MAINNET,
        ChainId::BscMainnet => EthNetworkParams::BSC_MAINNET,
        _ => unreachable!("non-Ethereum chain passed to eth_network_params"),
    }
}

/// Connect to an Electrum server from a URL like `ssl://host:60002` or
/// `tcp://host:50001`. Routes through SOCKS5 if `proxy` is `Some("socks5://…")`.
pub fn electrum_connect(endpoint: &Endpoint, proxy: Option<&str>) -> Result<ElectrumClient> {
    let url = match endpoint {
        Endpoint::Electrum { url, .. } => url.as_str(),
        other => {
            return Err(Error::Endpoint(format!(
                "expected Electrum endpoint, got {other:?}"
            )));
        }
    };

    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| Error::Network(format!("invalid Electrum URL (missing scheme): {url}")))?;
    let (host, port_str) = rest
        .rsplit_once(':')
        .ok_or_else(|| Error::Network(format!("invalid Electrum URL (missing port): {url}")))?;
    let port: u16 = port_str
        .parse()
        .map_err(|_| Error::Network(format!("invalid port in Electrum URL: {url}")))?;

    match (scheme, proxy) {
        ("ssl" | "tls", Some(p)) => ElectrumClient::connect_tls_via_socks5(host, port, p),
        ("ssl" | "tls", None) => ElectrumClient::connect_tls(host, port),
        (_, Some(p)) => ElectrumClient::connect_tcp_via_socks5(host, port, p),
        (_, None) => ElectrumClient::connect_tcp(host, port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hodl_config::{ChainConfig, Config, Endpoint, LockConfig, TorConfig};
    use std::collections::HashMap;

    fn config_with_endpoint(chain: ChainId, endpoint: Endpoint) -> Config {
        let mut chains = HashMap::new();
        chains.insert(
            chain,
            ChainConfig {
                endpoints: vec![endpoint],
                gap_limit: 20,
            },
        );
        Config {
            chains,
            tor: TorConfig::default(),
            lock: LockConfig::default(),
            kdf: Default::default(),
        }
    }

    #[test]
    fn from_chain_id_picks_btc_for_bitcoin() {
        // A real Electrum connect would fail — we're testing the factory
        // dispatch logic only. Since `from_chain_id` connects eagerly, we
        // verify it attempts the right path by checking the error message
        // comes from the network stack (not from "no endpoint configured").
        let cfg = config_with_endpoint(
            ChainId::Bitcoin,
            Endpoint::Electrum {
                url: "tcp://127.0.0.1:19999".into(),
                tls: false,
            },
        );
        let result = ActiveChain::from_chain_id(ChainId::Bitcoin, &cfg);
        // Either succeeds (unlikely in test env) or fails with a network
        // error — not an "endpoint" config error. That proves dispatch ran.
        match &result {
            Ok(ac) => assert!(matches!(ac, ActiveChain::Bitcoin(_))),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("no Electrum endpoint"),
                    "factory should not fail on missing endpoint; got: {msg}"
                );
            }
        }
    }

    #[test]
    fn from_chain_id_picks_eth_for_ethereum() {
        // JsonRpc connects lazily (no actual TCP dial in constructor), so this
        // succeeds and we can assert the variant.
        let cfg = config_with_endpoint(
            ChainId::Ethereum,
            Endpoint::JsonRpc {
                url: "http://127.0.0.1:18545".into(),
            },
        );
        let result = ActiveChain::from_chain_id(ChainId::Ethereum, &cfg);
        assert!(
            matches!(result, Ok(ActiveChain::Ethereum(_))),
            "expected Ok(Ethereum)"
        );
    }

    #[test]
    fn from_chain_id_errors_when_no_endpoint() {
        // Empty config for Bitcoin: should error with the chain name.
        let cfg = config_with_endpoint(
            ChainId::Ethereum,
            Endpoint::JsonRpc {
                url: "http://127.0.0.1:18545".into(),
            },
        );
        // Ask for Bitcoin which has no endpoint in this config.
        let result = ActiveChain::from_chain_id(ChainId::Bitcoin, &cfg);
        assert!(
            result.is_err(),
            "expected error for missing Bitcoin endpoint"
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("Bitcoin") || msg.contains("Electrum"),
            "error should mention chain or endpoint type: {msg}"
        );
    }
}
