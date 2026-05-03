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
}

pub type Result<T> = std::result::Result<T, Error>;
