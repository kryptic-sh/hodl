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

/// List wallet names (file stems of `*.vault` files) in `<data_root>/wallets/`.
///
/// Returns an empty vec if the directory does not exist yet.
pub fn list_wallets(data_root: &Path) -> Result<Vec<String>> {
    let dir = wallets_dir(data_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("vault")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn list_wallets_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let names = list_wallets(tmp.path()).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn list_wallets_two_vaults() {
        let tmp = TempDir::new().unwrap();
        let dir = wallets_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.vault"), b"x").unwrap();
        std::fs::write(dir.join("beta.vault"), b"x").unwrap();
        // Non-vault file should be ignored.
        std::fs::write(dir.join("notes.txt"), b"ignore").unwrap();

        let names = list_wallets(tmp.path()).unwrap();
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
