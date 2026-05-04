//! Cross-cutting error type for hodl chain implementations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("chain error: {0}")]
    Chain(String),
    #[error("endpoint error: {0}")]
    Endpoint(String),
    #[error("address codec error: {0}")]
    Codec(String),
    #[error("network error: {0}")]
    Network(String),
    /// TOFU cert mismatch — the server presented a different leaf certificate
    /// than the one pinned on first connect. This is a security signal, not a
    /// transient network failure. Do NOT retry automatically.
    ///
    /// To recover: verify the cert rotation was intentional, then remove the
    /// entry from `<data_root>/known_hosts.toml` and reconnect to re-pin.
    #[error(
        "TOFU mismatch for {host}: pinned {pinned}, server presented {presented}. \
         Cert changed since first connect — potential MitM or operator key rotation. \
         Remove the entry from known_hosts.toml to re-pin."
    )]
    TofuMismatch {
        host: String,
        pinned: String,
        presented: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
