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

/// ERC-20 / BEP-20 token specification for per-chain balance reads.
///
/// Configure under `[[chains.ethereum.tokens]]` or
/// `[[chains.bsc_mainnet.tokens]]` in `~/.config/hodl/config.toml`.
///
/// ```toml
/// [[chains.ethereum.tokens]]
/// address  = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
/// symbol   = "USDC"
/// decimals = 6
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSpec {
    /// Lower-case `0x…` 40-hex-char contract address.
    pub address: String,
    /// Display symbol (e.g. "USDC", "USDT", "DAI").
    pub symbol: String,
    /// Decimal places (USDC=6, DAI=18, etc.).
    pub decimals: u8,
}

/// Per-chain configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainConfig {
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    #[serde(default = "default_gap_limit")]
    pub gap_limit: u32,
    /// ERC-20 / BEP-20 token contracts to track. Defaults to empty (no tokens).
    /// Absent key in TOML deserialises as empty vec — existing configs are unaffected.
    #[serde(default)]
    pub tokens: Vec<TokenSpec>,
}

impl Default for ChainConfig {
    fn default() -> Self {
        ChainConfig {
            endpoints: Vec::new(),
            gap_limit: default_gap_limit(),
            tokens: Vec::new(),
        }
    }
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

/// UI behaviour config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiConfig {
    /// Show the splash animation at TUI launch. Default: true.
    pub splash: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig { splash: true }
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
/// Navio). The wallet still does not phone home on its own — it only contacts
/// these servers when the user opens the accounts / receive / send screens.
/// EVM (ETH/BSC) and Monero have no built-in defaults: EVM JSON-RPC needs
/// per-user API keys (Infura/Alchemy/etc.), and Monero LWS leaks the view
/// key to the operator so the privacy-conservative default is "self-host".
///
/// ## Override semantics
///
/// User overrides are **per-chain key**, not per-endpoint. If the user's
/// `config.toml` is missing a `[chains.X]` block for some chain `X`, the
/// curated default endpoint list for `X` is used. If the user provides a
/// `[chains.X]` block — even an empty one — it fully replaces the default
/// for that chain. Other chains keep their defaults independently.
///
/// Example: writing only `[chains.bitcoin] endpoints = [...]` keeps DOGE,
/// LTC, BCH, Navio, and BTC-testnet on their defaults; only BTC is replaced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, deserialize_with = "deserialize_chains")]
    pub chains: HashMap<ChainId, ChainConfig>,
    #[serde(default)]
    pub tor: TorConfig,
    #[serde(default)]
    pub lock: LockConfig,
    #[serde(default)]
    pub kdf: KdfPreset,
    #[serde(default)]
    pub ui: UiConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            chains: default_chains(),
            tor: TorConfig::default(),
            lock: LockConfig::default(),
            kdf: KdfPreset::default(),
            ui: UiConfig::default(),
        }
    }
}

/// Curated public-Electrum endpoint defaults. All TLS. Sourced from
/// `1209k.com/bitcoin-eye` reliability monitor on 2026-05-04.
///
/// Policy: do not curate down endpoints out on first smoke alert.
/// `try_endpoints` (electrum.rs) handles failover; defaults stay until
/// sustained downtime + curated replacement. See hodl#19.
fn default_chains() -> HashMap<ChainId, ChainConfig> {
    use Endpoint::Electrum;

    fn cc(endpoints: Vec<Endpoint>) -> ChainConfig {
        ChainConfig {
            endpoints,
            gap_limit: default_gap_limit(),
            tokens: Vec::new(),
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
    // Navio ElectrumX. nav-io/navio-electrum ships exactly one mainnet
    // server (electrum/chains/mainnet/servers.json), which lists 50002 for
    // TLS rather than the 40002 its DEFAULT_PORTS names.
    m.insert(ChainId::Navio, cc(vec![ssl("electrum.nav.io", 50002)]));
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
    ///
    /// User overrides merge **per chain key** over the curated defaults: any
    /// `[chains.X]` block the user provides fully replaces the default for
    /// that chain, but other chains keep their defaults. Top-level fields
    /// (`tor`, `lock`, `kdf`) work normally — present overrides default,
    /// absent uses the default value.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let src = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut user: Config = toml::from_str(&src).map_err(|e| {
            let span = e.span().unwrap_or(0..0);
            let (line, col, snippet) = locate(&src, span.start);
            ConfigError::Parse {
                path: path.to_path_buf(),
                line,
                col,
                message: e.message().to_string(),
                snippet,
            }
        })?;
        // Per-chain merge: any chain the user did NOT override gets its
        // curated default. Chains the user did override keep the user's
        // value, untouched.
        for (chain, default_cc) in default_chains() {
            user.chains.entry(chain).or_insert(default_cc);
        }
        Ok(user)
    }
}

/// Chain keys that named a chain hodl used to support.
///
/// A key here is dropped with a warning instead of failing the load. `hodl`
/// loads its config with `unwrap_or_default()`, so a hard parse error would
/// silently discard every *other* setting the user has — their Tor toggle,
/// lock timeout, KDF preset and custom endpoints — over one obsolete block.
/// Any key that is neither current nor listed here is still an error, so a
/// typo'd chain name is reported rather than quietly ignored.
const LEGACY_CHAIN_KEYS: &[&str] = &[
    // Removed in favour of `navio`. Its endpoints were NavCoin Electrum
    // servers, which do not serve the Navio chain, so the block is dropped
    // rather than carried over; `load`'s per-chain merge then supplies the
    // curated Navio default.
    "nav-coin",
];

/// Deserialize `[chains.*]`, dropping [`LEGACY_CHAIN_KEYS`].
fn deserialize_chains<'de, D>(de: D) -> Result<HashMap<ChainId, ChainConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Error as _, IntoDeserializer};

    let raw: HashMap<String, ChainConfig> = HashMap::deserialize(de)?;
    let mut out = HashMap::with_capacity(raw.len());
    for (key, cc) in raw {
        let parsed: Result<ChainId, serde::de::value::Error> =
            ChainId::deserialize(key.as_str().into_deserializer());
        match parsed {
            Ok(id) => {
                out.insert(id, cc);
            }
            Err(_) if LEGACY_CHAIN_KEYS.contains(&key.as_str()) => tracing::warn!(
                "config: [chains.{key}] names a chain hodl no longer supports; \
                 ignoring it"
            ),
            Err(e) => return Err(D::Error::custom(e)),
        }
    }
    Ok(out)
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
            ChainId::Navio,
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
    fn user_override_merges_per_chain() {
        // User overrides only Bitcoin. All other BTC-family defaults must
        // survive the merge intact.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[chains.bitcoin]
gap_limit = 50

[[chains.bitcoin.endpoints]]
kind = "electrum"
url = "ssl://my-private-electrum.example:50002"
tls = true
"#,
        )
        .unwrap();

        let cfg = Config::load(&path).expect("load");

        // Bitcoin: user's value wins entirely.
        let btc = cfg.chains.get(&ChainId::Bitcoin).expect("btc");
        assert_eq!(btc.gap_limit, 50);
        assert_eq!(btc.endpoints.len(), 1);
        match &btc.endpoints[0] {
            Endpoint::Electrum { url, .. } => {
                assert!(url.contains("my-private-electrum.example"));
            }
            other => panic!("expected user's electrum endpoint, got {other:?}"),
        }

        // Other BTC-family chains: defaults preserved.
        for chain in [
            ChainId::BitcoinTestnet,
            ChainId::Litecoin,
            ChainId::BitcoinCash,
            ChainId::Dogecoin,
            ChainId::Navio,
        ] {
            let cc = cfg.chains.get(&chain).expect("default preserved");
            assert!(
                !cc.endpoints.is_empty(),
                "{chain:?} default endpoints lost during merge"
            );
        }
    }

    /// A config written before the NavCoin → Navio rename must not take the
    /// user's other settings down with it.
    #[test]
    fn legacy_chain_key_is_dropped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[tor]
enabled = true
socks5 = "socks5://127.0.0.1:9050"

[chains.nav-coin]
gap_limit = 7

[[chains.nav-coin.endpoints]]
kind = "electrum"
url = "ssl://electrum.nav.community:40002"
tls = true

[chains.bitcoin]
gap_limit = 50

[[chains.bitcoin.endpoints]]
kind = "electrum"
url = "ssl://my-private-electrum.example:50002"
tls = true
"#,
        )
        .unwrap();

        let cfg = Config::load(&path).expect("legacy chain key must not fail the load");

        // Unrelated settings survive.
        assert!(cfg.tor.enabled, "tor setting lost to the stale chain key");
        let btc = cfg.chains.get(&ChainId::Bitcoin).expect("btc");
        assert_eq!(btc.gap_limit, 50);

        // Navio falls back to its curated default rather than inheriting the
        // dead NavCoin endpoints.
        let navio = cfg.chains.get(&ChainId::Navio).expect("navio default");
        assert_eq!(navio.gap_limit, ChainConfig::default().gap_limit);
        for ep in &navio.endpoints {
            match ep {
                Endpoint::Electrum { url, .. } => assert!(
                    !url.contains("nav.community"),
                    "Navio must not inherit NavCoin endpoints: {url}"
                ),
                other => panic!("expected Electrum endpoint, got {other:?}"),
            }
        }
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
    fn explicit_empty_endpoints_overrides_defaults() {
        // User writing `[chains.bitcoin] endpoints = []` is an explicit
        // "no servers for this chain" signal — the merge must NOT fall
        // back to the defaults for that chain.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[chains.bitcoin]
endpoints = []
"#,
        )
        .unwrap();
        let cfg = Config::load(&path).expect("load");
        let btc = cfg.chains.get(&ChainId::Bitcoin).expect("btc present");
        assert!(
            btc.endpoints.is_empty(),
            "explicit empty list must defeat defaults; got {:?}",
            btc.endpoints
        );
        // Other BTC-family chains keep their defaults via per-key merge.
        let doge = cfg.chains.get(&ChainId::Dogecoin).expect("doge default");
        assert!(!doge.endpoints.is_empty(), "DOGE default lost");
    }

    #[test]
    fn missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let cfg = Config::load(&path).expect("load");
        assert_eq!(cfg, Config::default());
    }

    /// A key that is neither current nor a known legacy name is still an
    /// error — a typo must not be silently ignored.
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

    #[test]
    fn token_spec_round_trips_with_tokens_array() {
        let src = r#"
[chains.ethereum]

[[chains.ethereum.tokens]]
address  = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
symbol   = "USDC"
decimals = 6

[[chains.ethereum.tokens]]
address  = "0x6b175474e89094c44da98b954eedeac495271d0f"
symbol   = "DAI"
decimals = 18
"#;
        let cfg: Config = toml::from_str(src).expect("parse");
        let eth = cfg.chains.get(&ChainId::Ethereum).expect("ethereum chain");
        assert_eq!(eth.tokens.len(), 2);
        assert_eq!(eth.tokens[0].symbol, "USDC");
        assert_eq!(eth.tokens[0].decimals, 6);
        assert_eq!(
            eth.tokens[0].address,
            "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
        );
        assert_eq!(eth.tokens[1].symbol, "DAI");
        assert_eq!(eth.tokens[1].decimals, 18);

        // Round-trip: serialize then deserialize must be equal.
        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn token_spec_defaults_to_empty_when_absent() {
        // Configs with no tokens key must deserialize with an empty tokens vec.
        let src = r#"
[chains.ethereum]
"#;
        let cfg: Config = toml::from_str(src).expect("parse");
        let eth = cfg.chains.get(&ChainId::Ethereum).expect("ethereum chain");
        assert!(
            eth.tokens.is_empty(),
            "tokens should default to empty when not present in TOML"
        );
    }

    #[test]
    fn ui_splash_defaults_to_true() {
        // A config with no [ui] block must still get splash = true.
        let src = r#""#;
        let cfg: Config = toml::from_str(src).expect("parse");
        assert!(cfg.ui.splash, "splash should default to true");
    }

    #[test]
    fn ui_splash_false_round_trips() {
        let src = r#"
[ui]
splash = false
"#;
        let cfg: Config = toml::from_str(src).expect("parse");
        assert!(!cfg.ui.splash, "splash should be false when set to false");

        // Round-trip: serialize then deserialize must preserve false.
        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&serialized).expect("deserialize");
        assert!(!back.ui.splash, "splash false must survive round-trip");
    }

    #[test]
    fn ui_splash_true_round_trips() {
        let src = r#"
[ui]
splash = true
"#;
        let cfg: Config = toml::from_str(src).expect("parse");
        assert!(cfg.ui.splash, "splash should be true when set to true");

        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&serialized).expect("deserialize");
        assert!(back.ui.splash, "splash true must survive round-trip");
    }
}
