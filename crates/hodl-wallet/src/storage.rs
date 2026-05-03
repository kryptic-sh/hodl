//! Vault file paths under `$XDG_DATA_HOME/hodl/wallets/`.

use std::path::{Path, PathBuf};

use crate::error::{Result, WalletError};

/// Default data root: `$XDG_DATA_HOME/hodl/`.
///
/// Routes through `hjkl_config::data_dir` for XDG-everywhere resolution
/// (Linux/macOS/Windows all honor `$XDG_DATA_HOME` with `~/.local/share`
/// fallback). Replaces the prior `directories::ProjectDirs` lookup, which
/// used a `sh.kryptic.hodl` Bundle ID layout on macOS/Windows.
pub fn default_data_dir() -> Result<PathBuf> {
    hjkl_config::data_dir("hodl")
        .map_err(|e| WalletError::Storage(format!("could not resolve data dir: {e}")))
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
