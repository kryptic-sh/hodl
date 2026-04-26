//! BIP-32 hierarchical key derivation.
//!
//! Thin wrapper around the `bip32` crate. Master key is derived from a
//! 64-byte BIP-39 seed via HMAC-SHA512 with key `"Bitcoin seed"`. Derivation
//! paths follow BIP-44: `m/44'/coin_type'/account'/change/address_index`.

use core::str::FromStr;

use bip32::{DerivationPath, XPrv, XPub};
use zeroize::ZeroizeOnDrop;

use crate::error::{Result, WalletError};
use crate::mnemonic::Seed;

/// Extended private key wrapper. The underlying `XPrv` already zeroizes its
/// secret bytes on drop; we add `ZeroizeOnDrop` derivation at the wrapper
/// level for any additional cached state we add later.
#[derive(Clone, ZeroizeOnDrop)]
pub struct ExtendedPrivKey {
    #[zeroize(skip)]
    inner: XPrv,
}

impl ExtendedPrivKey {
    pub fn xprv(&self) -> &XPrv {
        &self.inner
    }

    pub fn public_key(&self) -> XPub {
        self.inner.public_key()
    }

    /// Encode as BIP-32 base58check string ("xprv..." on mainnet defaults).
    pub fn to_extended_key_string(&self) -> String {
        self.inner.to_string(bip32::Prefix::XPRV).to_string()
    }
}

impl core::fmt::Debug for ExtendedPrivKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExtendedPrivKey").finish_non_exhaustive()
    }
}

/// Derive the BIP-32 master key from a 64-byte BIP-39 seed.
pub fn master_from_seed(seed: &Seed) -> Result<ExtendedPrivKey> {
    let xprv = XPrv::new(seed.as_bytes()).map_err(|e| WalletError::Derive(e.to_string()))?;
    Ok(ExtendedPrivKey { inner: xprv })
}

/// Derive a child key at the given path from the seed.
pub fn derive_path(seed: &Seed, path: &str) -> Result<ExtendedPrivKey> {
    let parsed =
        DerivationPath::from_str(path).map_err(|e| WalletError::DerivationPath(e.to_string()))?;
    let xprv = XPrv::derive_from_path(seed.as_bytes(), &parsed)
        .map_err(|e| WalletError::Derive(e.to_string()))?;
    Ok(ExtendedPrivKey { inner: xprv })
}

/// Format a BIP-44 path: `m/44'/coin'/account'/change/index`.
pub fn bip44_path(coin_type: u32, account: u32, change: u32, index: u32) -> String {
    format!("m/44'/{coin_type}'/{account}'/{change}/{index}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BIP-32 Test Vector 1.
    /// https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki#test-vector-1
    const TV1_SEED: &str = "000102030405060708090a0b0c0d0e0f";

    /// BIP-32 Test Vector 2.
    const TV2_SEED: &str = "fffcf9f6f3f0edeae7e4e1dedbd8d5d2cfccc9c6c3c0bdbab7b4b1aeaba8a5a29f9c999693908d8a8784817e7b7875726f6c696663605d5a5754514e4b484542";

    // BIP-32 vectors below feed raw bytes directly to `XPrv::new` (which
    // accepts 16..=64 bytes), bypassing our fixed-64-byte `Seed` holder so
    // the standard vector seeds (16 / 64 byte) compare exactly.

    #[test]
    fn bip32_test_vector_1_master() {
        // Vector 1 master xprv.
        let raw = hex::decode(TV1_SEED).unwrap();
        let xprv = XPrv::new(&raw).unwrap();
        let s = xprv.to_string(bip32::Prefix::XPRV).to_string();
        assert_eq!(
            s,
            "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi"
        );
    }

    #[test]
    fn bip32_test_vector_1_m_0h() {
        let raw = hex::decode(TV1_SEED).unwrap();
        let path = DerivationPath::from_str("m/0'").unwrap();
        let xprv = XPrv::derive_from_path(&raw, &path).unwrap();
        let s = xprv.to_string(bip32::Prefix::XPRV).to_string();
        assert_eq!(
            s,
            "xprv9uHRZZhk6KAJC1avXpDAp4MDc3sQKNxDiPvvkX8Br5ngLNv1TxvUxt4cV1rGL5hj6KCesnDYUhd7oWgT11eZG7XnxHrnYeSvkzY7d2bhkJ7"
        );
    }

    #[test]
    fn bip32_test_vector_1_full_path() {
        // m/0'/1/2'/2/1000000000
        let raw = hex::decode(TV1_SEED).unwrap();
        let path = DerivationPath::from_str("m/0'/1/2'/2/1000000000").unwrap();
        let xprv = XPrv::derive_from_path(&raw, &path).unwrap();
        let s = xprv.to_string(bip32::Prefix::XPRV).to_string();
        assert_eq!(
            s,
            "xprvA41z7zogVVwxVSgdKUHDy1SKmdb533PjDz7J6N6mV6uS3ze1ai8FHa8kmHScGpWmj4WggLyQjgPie1rFSruoUihUZREPSL39UNdE3BBDu76"
        );
    }

    #[test]
    fn bip32_test_vector_2_master() {
        let raw = hex::decode(TV2_SEED).unwrap();
        let xprv = XPrv::new(&raw).unwrap();
        let s = xprv.to_string(bip32::Prefix::XPRV).to_string();
        assert_eq!(
            s,
            "xprv9s21ZrQH143K31xYSDQpPDxsXRTUcvj2iNHm5NUtrGiGG5e2DtALGdso3pGz6ssrdK4PFmM8NSpSBHNqPqm55Qn3LqFtT2emdEXVYsCzC2U"
        );
    }

    #[test]
    fn bip32_test_vector_2_full_path() {
        // m/0/2147483647'/1/2147483646'/2
        let raw = hex::decode(TV2_SEED).unwrap();
        let path = DerivationPath::from_str("m/0/2147483647'/1/2147483646'/2").unwrap();
        let xprv = XPrv::derive_from_path(&raw, &path).unwrap();
        let s = xprv.to_string(bip32::Prefix::XPRV).to_string();
        assert_eq!(
            s,
            "xprvA2nrNbFZABcdryreWet9Ea4LvTJcGsqrMzxHx98MMrotbir7yrKCEXw7nadnHM8Dq38EGfSh6dqA9QWTyefMLEcBYJUuekgW4BYPJcr9E7j"
        );
    }

    #[test]
    fn bip44_path_format() {
        assert_eq!(bip44_path(0, 0, 0, 0), "m/44'/0'/0'/0/0");
        assert_eq!(bip44_path(60, 1, 0, 5), "m/44'/60'/1'/0/5");
    }

    #[test]
    fn derive_via_seed_wrapper() {
        // Smoke test that our wrapper composes correctly. We can't easily test
        // exact byte equality with BIP-32 vectors via the wrapper because
        // `Seed` is fixed 64 bytes — but we can confirm round-trip + path
        // parsing.
        let seed = {
            // Use a real BIP-39-derived 64-byte seed.
            let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
            let m = bip39::Mnemonic::parse_in(bip39::Language::English, phrase).unwrap();
            crate::mnemonic::to_seed(&m, "")
        };
        let master = master_from_seed(&seed).unwrap();
        let _ = master.to_extended_key_string();
        let derived = derive_path(&seed, "m/44'/0'/0'/0/0").unwrap();
        assert!(derived.to_extended_key_string().starts_with("xprv"));
    }
}
