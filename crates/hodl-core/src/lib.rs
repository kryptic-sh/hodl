//! Shared types and errors for hodl.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("wallet error: {0}")]
    Wallet(String),
}

pub type Result<T> = std::result::Result<T, Error>;
