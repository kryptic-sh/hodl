//! Key management, address derivation, signing.
//!
//! BIP-39 mnemonics, BIP-32 hierarchical derivation, and an encrypted
//! Argon2id + ChaCha20-Poly1305 vault. All sensitive material is wrapped in
//! `Zeroize` / `ZeroizeOnDrop` types so it scrubs on drop.

pub mod cache;
pub mod derive;
pub mod error;
pub mod mnemonic;
pub mod storage;
pub mod vault;

use std::path::{Path, PathBuf};

use zeroize::ZeroizeOnDrop;

use crate::derive::{ExtendedPrivKey, derive_path, master_from_seed};
use crate::error::{Result, WalletError};
use crate::mnemonic::Seed;
use crate::vault::{KdfParams, VaultPlaintext};

/// On-disk representation of a wallet — a path to a vault file and the file's
/// encrypted bytes. Holds nothing sensitive in plaintext.
#[derive(Debug, Clone)]
pub struct Wallet {
    pub name: String,
    pub vault_path: PathBuf,
}

impl Wallet {
    /// Create and persist a new vault at `<data_root>/wallets/<name>.vault`.
    /// Refuses to overwrite an existing vault.
    pub fn create(
        data_root: &Path,
        name: &str,
        mnemonic_phrase: &str,
        passphrase: &str,
        password: &[u8],
        kdf: KdfParams,
    ) -> Result<Wallet> {
        let m = mnemonic::parse(mnemonic_phrase)?;
        let seed = mnemonic::to_seed(&m, passphrase);
        let path = storage::vault_path(data_root, name);
        if path.exists() {
            return Err(WalletError::VaultExists(path.display().to_string()));
        }
        storage::ensure_wallets_dir(data_root)?;

        let pt = VaultPlaintext::new(seed.as_bytes().to_vec());
        let blob = vault::encrypt(&pt, password, kdf)?;
        // Atomic-ish write: write to tmp + rename within the same dir.
        let tmp = path.with_extension("vault.tmp");
        std::fs::write(&tmp, &blob)?;
        std::fs::rename(&tmp, &path)?;

        Ok(Wallet {
            name: name.to_string(),
            vault_path: path,
        })
    }

    /// Open a wallet handle for an existing vault file. Does not decrypt.
    pub fn open(data_root: &Path, name: &str) -> Result<Wallet> {
        let path = storage::vault_path(data_root, name);
        if !path.exists() {
            return Err(WalletError::VaultMissing(path.display().to_string()));
        }
        Ok(Wallet {
            name: name.to_string(),
            vault_path: path,
        })
    }

    /// Decrypt the vault and return an in-memory unlocked wallet.
    pub fn unlock(&self, password: &[u8]) -> Result<UnlockedWallet> {
        let blob = std::fs::read(&self.vault_path)?;
        let pt = vault::decrypt(&blob, password)?;
        if pt.seed.len() != 64 {
            return Err(WalletError::VaultFormat(format!(
                "expected 64-byte seed, got {}",
                pt.seed.len()
            )));
        }
        let mut seed_bytes = [0u8; 64];
        seed_bytes.copy_from_slice(&pt.seed);
        Ok(UnlockedWallet {
            name: self.name.clone(),
            seed: Seed(seed_bytes),
        })
    }
}

/// In-memory unlocked wallet. Drops zeroize the seed automatically.
#[derive(ZeroizeOnDrop)]
pub struct UnlockedWallet {
    #[zeroize(skip)]
    pub name: String,
    seed: Seed,
}

impl UnlockedWallet {
    pub fn seed(&self) -> &Seed {
        &self.seed
    }

    /// BIP-32 master extended private key.
    pub fn master(&self) -> Result<ExtendedPrivKey> {
        master_from_seed(&self.seed)
    }

    /// Derive a child key at the given path.
    pub fn derive(&self, path: &str) -> Result<ExtendedPrivKey> {
        derive_path(&self.seed, path)
    }

    /// Explicitly drop the unlocked wallet, scrubbing the seed.
    pub fn lock(self) {
        // Drop runs ZeroizeOnDrop.
        drop(self);
    }

    /// Derive a 32-byte ChaCha20-Poly1305 key for encrypting on-disk caches
    /// (scan results, etc) under this wallet. Cheap (single SHA-256) — safe to
    /// recompute as needed; the seed is the only sensitive input.
    pub fn cache_key(&self) -> [u8; cache::KEY_LEN] {
        cache::derive_cache_key(&self.seed.0)
    }
}

impl std::fmt::Debug for UnlockedWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnlockedWallet")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn create_unlock_round_trip() {
        let tmp = TempDir::new().unwrap();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let w = Wallet::create(
            tmp.path(),
            "default",
            phrase,
            "",
            b"correct horse battery staple",
            KdfParams::testing(),
        )
        .unwrap();
        assert!(w.vault_path.exists());

        let opened = Wallet::open(tmp.path(), "default").unwrap();
        let unlocked = opened.unlock(b"correct horse battery staple").unwrap();
        let xprv = unlocked.master().unwrap();
        assert!(xprv.to_extended_key_string().starts_with("xprv"));

        let derived = unlocked.derive("m/44'/0'/0'/0/0").unwrap();
        assert!(derived.to_extended_key_string().starts_with("xprv"));
    }

    #[test]
    fn unlock_wrong_password() {
        let tmp = TempDir::new().unwrap();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let w = Wallet::create(tmp.path(), "n", phrase, "", b"good", KdfParams::testing()).unwrap();
        match w.unlock(b"bad").unwrap_err() {
            WalletError::Decrypt => {}
            other => panic!("expected Decrypt, got {other:?}"),
        }
    }

    #[test]
    fn create_refuses_overwrite() {
        let tmp = TempDir::new().unwrap();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        Wallet::create(tmp.path(), "n", phrase, "", b"pw", KdfParams::testing()).unwrap();
        let err =
            Wallet::create(tmp.path(), "n", phrase, "", b"pw", KdfParams::testing()).unwrap_err();
        match err {
            WalletError::VaultExists(_) => {}
            other => panic!("expected VaultExists, got {other:?}"),
        }
    }

    #[test]
    fn open_missing_fails() {
        let tmp = TempDir::new().unwrap();
        match Wallet::open(tmp.path(), "nope").unwrap_err() {
            WalletError::VaultMissing(_) => {}
            other => panic!("expected VaultMissing, got {other:?}"),
        }
    }
}
