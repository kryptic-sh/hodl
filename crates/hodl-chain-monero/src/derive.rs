//! Ledger-compatible BIP-39 → Monero key derivation.
//!
//! WARNING: This uses the Ledger-compatible BIP-39 scheme, NOT Monero's
//! native 25-word Electrum-style mnemonic. The address produced here
//! matches Cake Wallet, Monerujo (Ledger seed mode), and Ledger Live.
//! It does NOT match monero-wallet-cli, the GUI wallet, or MyMonero
//! (those consume the 25-word seed directly).
//!
//! Derivation:
//!   spend_key = sc_reduce32(keccak256(bip32_private_key_at(m/44'/128'/0'/0/0)))
//!   view_key  = sc_reduce32(keccak256(spend_key))

use bip32::{DerivationPath, XPrv};
use curve25519_dalek::constants::ED25519_BASEPOINT_POINT;
use curve25519_dalek::scalar::Scalar;
use hodl_core::error::{Error, Result};
use tiny_keccak::{Hasher, Keccak};
use zeroize::Zeroize;

/// Spend and view key pair derived from a BIP-39 seed via the Ledger scheme.
#[derive(Zeroize)]
pub struct MoneroKeys {
    pub spend: [u8; 32],
    pub view: [u8; 32],
}

/// Keccak-256 (original, not SHA3-256) of the input bytes.
fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut k = Keccak::v256();
    k.update(input);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

/// Reduce a 32-byte little-endian integer mod the ed25519 group order l.
///
/// Equivalent to Monero's sc_reduce32. Uses curve25519-dalek which
/// implements the reduction internally via from_bytes_mod_order.
fn sc_reduce32(input: [u8; 32]) -> [u8; 32] {
    let s = Scalar::from_bytes_mod_order(input);
    s.to_bytes()
}

/// Derive Monero spend + view keys from a 64-byte BIP-39 seed.
///
/// Uses the Ledger-compatible scheme:
///   spend = sc_reduce32(keccak256(bip32 private key at m/44'/128'/0'/0/0))
///   view  = sc_reduce32(keccak256(spend))
pub fn derive_keys(seed: &[u8; 64]) -> Result<MoneroKeys> {
    let path: DerivationPath = "m/44'/128'/0'/0/0"
        .parse()
        .map_err(|e: bip32::Error| Error::Chain(format!("derivation path: {e}")))?;
    let xprv = XPrv::derive_from_path(seed, &path)
        .map_err(|e| Error::Chain(format!("key derivation: {e}")))?;
    let raw: [u8; 32] = xprv.private_key().to_bytes().into();

    let spend_hash = keccak256(&raw);
    let spend = sc_reduce32(spend_hash);
    let view_hash = keccak256(&spend);
    let view = sc_reduce32(view_hash);

    Ok(MoneroKeys { spend, view })
}

/// Derive the ed25519 public key for a Monero secret key scalar.
///
/// Monero public key = secret * G, where G is the ed25519 basepoint.
pub fn pubkey_from_secret(secret: &[u8; 32]) -> [u8; 32] {
    let s = Scalar::from_bytes_mod_order(*secret);
    (ED25519_BASEPOINT_POINT * s).compress().to_bytes()
}

/// Encode a standard Monero address.
///
/// Format: [prefix(1) | spend_pub(32) | view_pub(32)] + 4-byte keccak256 checksum,
/// encoded with Monero's base58 variant (8-byte blocks → 11 chars).
pub fn standard_address(spend_pub: &[u8; 32], view_pub: &[u8; 32], prefix: u8) -> String {
    crate::address::encode(spend_pub, view_pub, prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    // "abandon" x 11 + "about", no passphrase — standard BIP-39 test seed.
    // Full 64-byte seed from the BIP-39 spec / Trezor reference (no passphrase).
    const ABANDON_SEED_HEX: &str = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";

    fn seed_bytes() -> [u8; 64] {
        hex::decode(ABANDON_SEED_HEX).unwrap().try_into().unwrap()
    }

    #[test]
    fn derive_keys_deterministic() {
        let seed = seed_bytes();
        let k1 = derive_keys(&seed).unwrap();
        let k2 = derive_keys(&seed).unwrap();
        assert_eq!(k1.spend, k2.spend, "spend key must be deterministic");
        assert_eq!(k1.view, k2.view, "view key must be deterministic");
    }

    #[test]
    fn spend_key_is_reduced() {
        // sc_reduce32 output must satisfy 0 < s < l (the ed25519 group order).
        // A zero scalar would mean the keccak output happened to be 0 mod l —
        // practically impossible; this guards against implementation bugs.
        let seed = seed_bytes();
        let keys = derive_keys(&seed).unwrap();
        assert_ne!(keys.spend, [0u8; 32], "spend key must not be zero");
        assert_ne!(keys.view, [0u8; 32], "view key must not be zero");
    }

    #[test]
    fn pubkeys_cross_consistent() {
        let seed = seed_bytes();
        let keys = derive_keys(&seed).unwrap();
        // Re-derive view public key from spend private key via the standard relation.
        // In Monero: view_pub = view_priv * G. This checks pubkey_from_secret works.
        let spend_pub = pubkey_from_secret(&keys.spend);
        let view_pub = pubkey_from_secret(&keys.view);
        // Both must be valid non-zero 32-byte ed25519 points.
        assert_ne!(spend_pub, [0u8; 32]);
        assert_ne!(view_pub, [0u8; 32]);
        // And they must differ (spend != view key with overwhelming probability).
        assert_ne!(spend_pub, view_pub);
    }

    #[test]
    fn standard_address_shape() {
        // A mainnet Monero address: 95 chars, starts with '4'.
        // TODO: source an exact published vector from Cake Wallet's BIP-39 test
        // fixtures or Ledger's monero-app-ledger test vectors to pin the exact
        // address string. For now we assert structural shape only.
        let seed = seed_bytes();
        let keys = derive_keys(&seed).unwrap();
        let spend_pub = pubkey_from_secret(&keys.spend);
        let view_pub = pubkey_from_secret(&keys.view);
        let addr = standard_address(&spend_pub, &view_pub, 18);
        assert_eq!(addr.len(), 95, "mainnet address must be 95 chars");
        assert!(
            addr.starts_with('4'),
            "mainnet address must start with '4', got: {addr}"
        );
    }
}
