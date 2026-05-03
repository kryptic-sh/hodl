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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Per-chain endpoint lists and gap-limit overrides. Empty by default —
    /// the user must opt in to endpoints. Never phones home.
    #[serde(default)]
    pub chains: HashMap<ChainId, ChainConfig>,
    #[serde(default)]
    pub tor: TorConfig,
    #[serde(default)]
    pub lock: LockConfig,
    #[serde(default)]
    pub kdf: KdfPreset,
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
