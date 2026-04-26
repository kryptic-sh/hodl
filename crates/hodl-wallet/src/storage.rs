//! Vault file paths under `$XDG_DATA_HOME/hodl/wallets/`.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::{Result, WalletError};

/// Default data root: `$XDG_DATA_HOME/hodl/`.
pub fn default_data_dir() -> Result<PathBuf> {
    let pd = ProjectDirs::from("sh", "kryptic", "hodl")
        .ok_or_else(|| WalletError::Storage("could not resolve project data directory".into()))?;
    Ok(pd.data_dir().to_path_buf())
}

/// `<data_root>/wallets/`.
pub fn wallets_dir(data_root: &Path) -> PathBuf {
    data_root.join("wallets")
}

/// `<data_root>/wallets/<name>.vault`.
pub fn vault_path(data_root: &Path, name: &str) -> PathBuf {
    wallets_dir(data_root).join(format!("{name}.vault"))
}

/// Ensure `wallets/` exists.
pub fn ensure_wallets_dir(data_root: &Path) -> Result<()> {
    let dir = wallets_dir(data_root);
    std::fs::create_dir_all(&dir)?;
    Ok(())
}
