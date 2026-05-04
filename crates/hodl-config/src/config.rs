//! Config structs and TOML loader.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use hodl_core::ChainId;

use crate::error::ConfigError;

/// Endpoint variant for a chain backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Endpoint {
    Electrum { url: String, tls: bool },
    JsonRpc { url: String },
    Lws { url: String },
}

/// Per-chain configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainConfig {
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    #[serde(default = "default_gap_limit")]
    pub gap_limit: u32,
}

fn default_gap_limit() -> u32 {
    20
}

/// Tor proxy config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorConfig {
    pub enabled: bool,
    pub socks5: String,
}

impl Default for TorConfig {
    fn default() -> Self {
        TorConfig {
            enabled: false,
            socks5: "socks5://127.0.0.1:9050".to_string(),
        }
    }
}

/// Idle auto-lock config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockConfig {
    pub idle_timeout_secs: u64,
}

impl Default for LockConfig {
    fn default() -> Self {
        LockConfig {
            idle_timeout_secs: 300,
        }
    }
}

/// Argon2id parameter preset.
///
/// | Preset   | m (MiB) | t | p |
/// |----------|---------|---|---|
/// | Default  | 64      | 3 | 1 |
/// | Hardened | 256     | 4 | 1 |
/// | Paranoid | 1024    | 6 | 1 |
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KdfPreset {
    #[default]
    Default,
    Hardened,
    Paranoid,
}

/// Top-level hodl config.
///
/// `Config::default()` populates `chains` with curated public Electrum
/// endpoints for the BTC family (BTC mainnet + testnet, BCH, LTC, DOGE,
/// NAV). The wallet still does not phone home on its own — it only contacts
/// these servers when the user opens the accounts / receive / send screens.
/// EVM (ETH/BSC) and Monero have no built-in defaults: EVM JSON-RPC needs
/// per-user API keys (Infura/Alchemy/etc.), and Monero LWS leaks the view
/// key to the operator so the privacy-conservative default is "self-host".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub chains: HashMap<ChainId, ChainConfig>,
    #[serde(default)]
    pub tor: TorConfig,
    #[serde(default)]
    pub lock: LockConfig,
    #[serde(default)]
    pub kdf: KdfPreset,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            chains: default_chains(),
            tor: TorConfig::default(),
            lock: LockConfig::default(),
            kdf: KdfPreset::default(),
        }
    }
}

/// Curated public-Electrum endpoint defaults. All TLS. Sourced from
/// `1209k.com/bitcoin-eye` reliability monitor on 2026-05-04.
fn default_chains() -> HashMap<ChainId, ChainConfig> {
    use Endpoint::Electrum;

    fn cc(endpoints: Vec<Endpoint>) -> ChainConfig {
        ChainConfig {
            endpoints,
            gap_limit: default_gap_limit(),
        }
    }
    fn ssl(host: &str, port: u16) -> Endpoint {
        Electrum {
            url: format!("ssl://{host}:{port}"),
            tls: true,
        }
    }

    let mut m = HashMap::new();
    m.insert(
        ChainId::Bitcoin,
        cc(vec![
            ssl("electrum.blockstream.info", 50002),
            ssl("electrum.bullbitcoin.com", 50002),
            ssl("electrum.acinq.co", 50002),
            ssl("electrum.bitaroo.net", 50002),
            ssl("electrum.emzy.de", 50002),
        ]),
    );
    m.insert(
        ChainId::BitcoinTestnet,
        cc(vec![
            ssl("testnet.aranguren.org", 51002),
            ssl("testnet.qtornado.com", 51002),
            ssl("electrum.blockstream.info", 60002),
            ssl("ax101.blockeng.ch", 60002),
            ssl("v22019051929289916.bestsrv.de", 50002),
        ]),
    );
    m.insert(
        ChainId::Litecoin,
        cc(vec![
            ssl("electrum1.cipig.net", 20063),
            ssl("electrum2.cipig.net", 20063),
            ssl("electrum3.cipig.net", 20063),
            ssl("backup.electrum-ltc.org", 50002),
            ssl("litecoin.stackwallet.com", 20063),
        ]),
    );
    m.insert(
        ChainId::BitcoinCash,
        cc(vec![
            ssl("bch.soul-dev.com", 50002),
            ssl("electrum.imaginary.cash", 50002),
            ssl("fulcrum.aglauck.com", 50002),
            ssl("electroncash.dk", 50002),
            ssl("bch0.kister.net", 50002),
        ]),
    );
    m.insert(
        ChainId::Dogecoin,
        cc(vec![
            ssl("dogecoin.stackwallet.com", 50022),
            ssl("electrum1.cipig.net", 20060),
            ssl("electrum2.cipig.net", 20060),
            ssl("electrum3.cipig.net", 20060),
            ssl("doge.aftrek.org", 50002),
        ]),
    );
    m.insert(
        ChainId::NavCoin,
        cc(vec![
            ssl("electrum.nav.community", 40002),
            ssl("electrum1.nav.community", 40002),
            ssl("electrum2.nav.community", 40002),
            ssl("electrum3.nav.community", 40002),
            ssl("electrum4.nav.community", 40002),
        ]),
    );
    m
}

impl Config {
    /// Resolve the default config file path via `hjkl-config`.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        hjkl_config::config_dir("hodl")
            .map(|d| d.join("config.toml"))
            .map_err(|e| ConfigError::Other(e.to_string()))
    }

    /// Load config from `path`. Returns `Config::default()` if the file does
    /// not exist. Never writes to disk.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let src = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str::<Config>(&src).map_err(|e| {
            let span = e.span().unwrap_or(0..0);
            let (line, col, snippet) = locate(&src, span.start);
            ConfigError::Parse {
                path: path.to_path_buf(),
                line,
                col,
                message: e.message().to_string(),
                snippet,
            }
        })
    }
}

/// Extract (1-based line, 1-based col, snippet) from a byte offset in `src`.
fn locate(src: &str, offset: usize) -> (usize, usize, String) {
    let before = &src[..offset.min(src.len())];
    let line = before.lines().count().max(1);
    let col = before.rfind('\n').map(|p| offset - p).unwrap_or(offset + 1);
    let snippet = src
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or("")
        .to_string();
    (line, col, snippet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trip() {
        let cfg = Config::default();
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn defaults_populate_btc_family() {
        let cfg = Config::default();
        for chain in [
            ChainId::Bitcoin,
            ChainId::BitcoinTestnet,
            ChainId::Litecoin,
            ChainId::BitcoinCash,
            ChainId::Dogecoin,
            ChainId::NavCoin,
        ] {
            let cc = cfg.chains.get(&chain).expect("chain in defaults");
            assert!(
                !cc.endpoints.is_empty(),
                "{chain:?} should have at least one default endpoint"
            );
            for ep in &cc.endpoints {
                match ep {
                    Endpoint::Electrum { tls, url } => {
                        assert!(*tls, "{chain:?} default endpoint must be TLS: {url}");
                        assert!(
                            url.starts_with("ssl://"),
                            "{chain:?} url must start with ssl://: {url}"
                        );
                    }
                    other => panic!("{chain:?} default must be Electrum, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn defaults_skip_evm_and_monero() {
        let cfg = Config::default();
        assert!(
            !cfg.chains.contains_key(&ChainId::Ethereum),
            "ETH must not have a default RPC (needs user API key)"
        );
        assert!(
            !cfg.chains.contains_key(&ChainId::BscMainnet),
            "BSC must not have a default RPC"
        );
        assert!(
            !cfg.chains.contains_key(&ChainId::Monero),
            "Monero must not have a default LWS endpoint (privacy)"
        );
    }

    #[test]
    fn sample_toml_parses() {
        let src = r#"
[chains.bitcoin-testnet]
gap_limit = 30

[[chains.bitcoin-testnet.endpoints]]
kind = "electrum"
url = "ssl://electrum.blockstream.info:60002"
tls = true
"#;
        let cfg: Config = toml::from_str(src).expect("parse");
        let chain = cfg.chains.get(&ChainId::BitcoinTestnet).expect("chain");
        assert_eq!(chain.gap_limit, 30);
        assert_eq!(chain.endpoints.len(), 1);
        assert!(matches!(
            &chain.endpoints[0],
            Endpoint::Electrum { tls: true, .. }
        ));
    }

    #[test]
    fn missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let cfg = Config::load(&path).expect("load");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn unknown_chain_key_errors() {
        let src = r#"
[chains.not-a-real-chain]
gap_limit = 10
"#;
        let result = toml::from_str::<Config>(src);
        assert!(
            result.is_err(),
            "expected error for unknown chain key, got Ok"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not-a-real-chain") || msg.contains("unknown"),
            "error message should mention the bad key: {msg}"
        );
    }
}
