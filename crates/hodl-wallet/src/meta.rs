//! Per-wallet sidecar metadata — `<data_root>/wallets/<name>.meta.toml`.
//!
//! The file is optional: a missing file returns [`WalletMeta::default()`].
//! This ensures all wallets created before this feature shipped keep working
//! without any migration step.

use std::path::{Path, PathBuf};

use crate::error::{Result, WalletError};
use crate::storage;

/// Persisted metadata stored alongside each vault file.
///
/// New fields must have `#[serde(default)]` so old meta files parse cleanly.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WalletMeta {
    /// When `true`, the vault password is stored in the OS keyring and
    /// the lock screen will attempt a keyring auto-unlock before prompting.
    #[serde(default)]
    pub keyring: bool,
}

impl WalletMeta {
    /// Read the meta file. Returns [`WalletMeta::default()`] if the file is
    /// missing — covers all wallets created before this feature shipped.
    pub fn load(data_root: &Path, name: &str) -> Result<Self> {
        let path = meta_path(data_root, name);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        toml::from_str(&raw).map_err(|e| WalletError::VaultFormat(e.to_string()))
    }

    /// Persist the meta file.
    pub fn save(&self, data_root: &Path, name: &str) -> Result<()> {
        storage::ensure_wallets_dir(data_root)?;
        let path = meta_path(data_root, name);
        let raw = toml::to_string_pretty(self).map_err(|e| WalletError::Serde(e.to_string()))?;
        std::fs::write(&path, raw)?;
        Ok(())
    }
}

/// `<data_root>/wallets/<name>.meta.toml`
pub fn meta_path(data_root: &Path, name: &str) -> PathBuf {
    storage::wallets_dir(data_root).join(format!("{name}.meta.toml"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let meta = WalletMeta::load(dir.path(), "nonexistent").unwrap();
        assert!(!meta.keyring);
    }

    #[test]
    fn round_trip_keyring_true() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(storage::wallets_dir(dir.path())).unwrap();

        let meta = WalletMeta { keyring: true };
        meta.save(dir.path(), "mywallet").unwrap();

        let loaded = WalletMeta::load(dir.path(), "mywallet").unwrap();
        assert!(loaded.keyring);
    }

    #[test]
    fn round_trip_keyring_false() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(storage::wallets_dir(dir.path())).unwrap();

        let meta = WalletMeta { keyring: false };
        meta.save(dir.path(), "mywallet").unwrap();

        let loaded = WalletMeta::load(dir.path(), "mywallet").unwrap();
        assert!(!loaded.keyring);
    }

    #[test]
    fn bad_toml_returns_error() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(storage::wallets_dir(dir.path())).unwrap();
        let path = meta_path(dir.path(), "broken");
        std::fs::write(&path, b"not valid toml [[[").unwrap();

        let err = WalletMeta::load(dir.path(), "broken").unwrap_err();
        assert!(matches!(err, WalletError::VaultFormat(_)));
    }
}
