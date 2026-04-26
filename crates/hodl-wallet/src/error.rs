//! Wallet error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalletError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid mnemonic: {0}")]
    Mnemonic(String),

    #[error("invalid derivation path: {0}")]
    DerivationPath(String),

    #[error("BIP-32 derivation failed: {0}")]
    Derive(String),

    #[error("vault format error: {0}")]
    VaultFormat(String),

    #[error("KDF error: {0}")]
    Kdf(String),

    #[error("decryption failed (wrong password or corrupted vault)")]
    Decrypt,

    #[error("encryption failed: {0}")]
    Encrypt(String),

    #[error("vault already exists: {0}")]
    VaultExists(String),

    #[error("vault not found: {0}")]
    VaultMissing(String),

    #[error("storage path resolution failed: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serde(String),
}

pub type Result<T> = std::result::Result<T, WalletError>;
