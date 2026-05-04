//! Encrypted blob format for non-vault data (scan caches, etc).
//!
//! Distinct from the vault format because the inputs are different:
//! the vault encrypts a high-value secret with a low-entropy password
//! (so it pays the Argon2id cost), whereas a cache blob is encrypted
//! with a key **derived from the already-unlocked seed** — no KDF
//! needed because the seed is already 512 bits of high-entropy material.
//!
//! Layout:
//!
//! ```text
//! magic(4) | version(2) | nonce(12) | ciphertext(N) | tag(16)
//! ```
//!
//! - `magic`   = `b"HCAC"`
//! - `version` = `1` (big-endian u16)
//! - `nonce`   = 12 random bytes for ChaCha20-Poly1305
//! - `ciphertext + tag` — `ChaCha20Poly1305::encrypt` output (tag appended)
//!
//! AAD = the 6-byte header (magic|version) — any tampering with the
//! magic/version invalidates the tag.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::error::{Result, WalletError};

pub const MAGIC: [u8; 4] = *b"HCAC";
pub const VERSION: u16 = 1;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
pub const HEADER_LEN: usize = 4 + 2;
pub const KEY_LEN: usize = 32;

/// Domain-separation tag for the cache key derivation. Bumping this string
/// invalidates every existing cache blob (forces a re-scan after upgrade).
const CACHE_KEY_DOMAIN: &[u8] = b"hodl-cache-v1\0";

/// Derive a 32-byte ChaCha20-Poly1305 key from an unlocked wallet seed.
///
/// The seed is already 512 bits of high-entropy material (BIP-39
/// PBKDF2-HMAC-SHA512), so a plain SHA-256 over `domain || seed` is
/// sufficient. No password / Argon2 cost — the unlock step already paid it.
pub fn derive_cache_key(seed: &[u8; 64]) -> [u8; KEY_LEN] {
    let mut h = Sha256::new();
    h.update(CACHE_KEY_DOMAIN);
    h.update(seed);
    h.finalize().into()
}

/// Encrypt a plaintext payload with a cache key. Returns the full blob
/// (header + ciphertext + tag).
pub fn encrypt(plaintext: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    encrypt_with(plaintext, key, &nonce_bytes)
}

/// Deterministic encrypt — caller supplies the nonce. Used by tests.
pub fn encrypt_with(
    plaintext: &[u8],
    key: &[u8; KEY_LEN],
    nonce_bytes: &[u8; NONCE_LEN],
) -> Result<Vec<u8>> {
    let mut key_copy = *key;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_copy));
    let nonce = Nonce::from_slice(nonce_bytes);
    let aad = build_header();
    let ct_and_tag = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|e| WalletError::Encrypt(e.to_string()))?;
    key_copy.zeroize();

    let mut out = Vec::with_capacity(HEADER_LEN + NONCE_LEN + ct_and_tag.len());
    out.extend_from_slice(&aad);
    out.extend_from_slice(nonce_bytes);
    out.extend_from_slice(&ct_and_tag);
    Ok(out)
}

/// Decrypt a cache blob. Wrong key → `WalletError::Decrypt`.
pub fn decrypt(blob: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    if blob.len() < HEADER_LEN + NONCE_LEN + TAG_LEN {
        return Err(WalletError::VaultFormat("cache blob too short".into()));
    }
    if blob[0..4] != MAGIC {
        return Err(WalletError::VaultFormat("bad cache magic".into()));
    }
    let version = u16::from_be_bytes(blob[4..6].try_into().unwrap());
    if version != VERSION {
        return Err(WalletError::VaultFormat(format!(
            "unsupported cache version {version}"
        )));
    }

    let nonce_bytes: [u8; NONCE_LEN] = blob[HEADER_LEN..HEADER_LEN + NONCE_LEN].try_into().unwrap();
    let ct = &blob[HEADER_LEN + NONCE_LEN..];

    let mut key_copy = *key;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_copy));
    let aad = build_header();
    let pt = cipher
        .decrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload { msg: ct, aad: &aad },
        )
        .map_err(|_| WalletError::Decrypt);
    key_copy.zeroize();
    pt
}

fn build_header() -> [u8; HEADER_LEN] {
    let mut out = [0u8; HEADER_LEN];
    out[0..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let seed = [7u8; 64];
        let key = derive_cache_key(&seed);
        let pt = b"hello cache world";
        let blob = encrypt(pt, &key).unwrap();
        let recovered = decrypt(&blob, &key).unwrap();
        assert_eq!(recovered, pt);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = derive_cache_key(&[1u8; 64]);
        let key2 = derive_cache_key(&[2u8; 64]);
        let blob = encrypt(b"data", &key1).unwrap();
        assert!(matches!(decrypt(&blob, &key2), Err(WalletError::Decrypt)));
    }

    #[test]
    fn tampered_blob_fails() {
        let key = derive_cache_key(&[3u8; 64]);
        let mut blob = encrypt(b"payload", &key).unwrap();
        // Flip a bit in the ciphertext region.
        let n = blob.len();
        blob[n - 5] ^= 0x01;
        assert!(matches!(decrypt(&blob, &key), Err(WalletError::Decrypt)));
    }

    #[test]
    fn deterministic_with_fixed_nonce() {
        let key = derive_cache_key(&[5u8; 64]);
        let nonce = [9u8; NONCE_LEN];
        let a = encrypt_with(b"x", &key, &nonce).unwrap();
        let b = encrypt_with(b"x", &key, &nonce).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn key_is_deterministic_per_seed() {
        let s = [42u8; 64];
        assert_eq!(derive_cache_key(&s), derive_cache_key(&s));
    }

    #[test]
    fn key_changes_with_seed() {
        assert_ne!(derive_cache_key(&[1u8; 64]), derive_cache_key(&[2u8; 64]));
    }
}
