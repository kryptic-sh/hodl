//! Encrypted on-disk vault format.
//!
//! Layout (big-endian for fixed widths, little-endian inside the params block):
//!
//! ```text
//! magic(8) | version(2) | argon2_params(16) | salt(16) | nonce(12) | ciphertext(N) | tag(16)
//! ```
//!
//! - `magic`        = `b"HODLVLT\0"` (literal byte string).
//! - `version`      = `1` (big-endian u16).
//! - `argon2_params` = `m_cost(4) | t_cost(4) | parallelism(4) | reserved(4)`,
//!   each little-endian `u32`. Reserved must be zero.
//! - `salt`         = 16 random bytes for Argon2id.
//! - `nonce`        = 12 random bytes for ChaCha20-Poly1305.
//! - `ciphertext + tag` — `chacha20poly1305::ChaCha20Poly1305::encrypt`
//!   produces ciphertext with the 16-byte Poly1305 tag appended.
//!
//! KDF: Argon2id, default `m=64 MiB, t=3, p=1` per PLAN.md. Output is the
//! 32-byte ChaCha20-Poly1305 key.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Result, WalletError};

pub const MAGIC: [u8; 8] = *b"HODLVLT\0";
pub const VERSION: u16 = 1;
pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
pub const PARAMS_LEN: usize = 16;
pub const HEADER_LEN: usize = 8 + 2 + PARAMS_LEN + SALT_LEN + NONCE_LEN;
pub const KEY_LEN: usize = 32;

/// Argon2id parameters. PLAN defaults: 64 MiB, t=3, p=1.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory cost in KiB.
    pub m_cost: u32,
    /// Time cost (iterations).
    pub t_cost: u32,
    /// Parallelism / lanes.
    pub parallelism: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        // 64 MiB, t=3, p=1.
        Self {
            m_cost: 64 * 1024,
            t_cost: 3,
            parallelism: 1,
        }
    }
}

impl KdfParams {
    /// Lighter params for unit tests so the suite doesn't take 30 seconds.
    /// Memory must satisfy Argon2's `8 * parallelism` lower bound.
    pub fn testing() -> Self {
        Self {
            m_cost: 16,
            t_cost: 1,
            parallelism: 1,
        }
    }

    fn to_bytes(self) -> [u8; PARAMS_LEN] {
        let mut out = [0u8; PARAMS_LEN];
        out[0..4].copy_from_slice(&self.m_cost.to_le_bytes());
        out[4..8].copy_from_slice(&self.t_cost.to_le_bytes());
        out[8..12].copy_from_slice(&self.parallelism.to_le_bytes());
        // bytes 12..16 = reserved (zero).
        out
    }

    fn from_bytes(b: &[u8; PARAMS_LEN]) -> Result<Self> {
        let m_cost = u32::from_le_bytes(b[0..4].try_into().unwrap());
        let t_cost = u32::from_le_bytes(b[4..8].try_into().unwrap());
        let parallelism = u32::from_le_bytes(b[8..12].try_into().unwrap());
        let reserved = u32::from_le_bytes(b[12..16].try_into().unwrap());
        if reserved != 0 {
            return Err(WalletError::VaultFormat(
                "reserved KDF param bytes are non-zero".into(),
            ));
        }
        Ok(Self {
            m_cost,
            t_cost,
            parallelism,
        })
    }
}

/// Material we plan to encrypt. Stored as the inner blob inside the vault.
/// Format: `[1-byte len][seed bytes (32 or 64)]`. We keep this simple — the
/// outer file format already encodes versioning.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct VaultPlaintext {
    pub seed: Vec<u8>,
}

impl core::fmt::Debug for VaultPlaintext {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never log the seed bytes.
        f.debug_struct("VaultPlaintext")
            .field("seed_len", &self.seed.len())
            .finish()
    }
}

impl VaultPlaintext {
    pub fn new(seed: Vec<u8>) -> Self {
        Self { seed }
    }

    fn encode(&self) -> Result<Vec<u8>> {
        if self.seed.len() > u8::MAX as usize {
            return Err(WalletError::Encrypt(
                "seed too long for plaintext encoding".into(),
            ));
        }
        let mut out = Vec::with_capacity(1 + self.seed.len());
        out.push(self.seed.len() as u8);
        out.extend_from_slice(&self.seed);
        Ok(out)
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() {
            return Err(WalletError::VaultFormat("empty plaintext blob".into()));
        }
        let len = bytes[0] as usize;
        if 1 + len != bytes.len() {
            return Err(WalletError::VaultFormat(
                "plaintext length prefix does not match payload".into(),
            ));
        }
        Ok(Self {
            seed: bytes[1..].to_vec(),
        })
    }
}

/// Run Argon2id KDF with the supplied params, returning a 32-byte key.
fn derive_key(password: &[u8], salt: &[u8], params: KdfParams) -> Result<[u8; KEY_LEN]> {
    let p = Params::new(
        params.m_cost,
        params.t_cost,
        params.parallelism,
        Some(KEY_LEN),
    )
    .map_err(|e| WalletError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(password, salt, &mut out)
        .map_err(|e| WalletError::Kdf(e.to_string()))?;
    Ok(out)
}

/// Encrypt the plaintext with a password and return the full vault blob.
pub fn encrypt(plaintext: &VaultPlaintext, password: &[u8], params: KdfParams) -> Result<Vec<u8>> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    encrypt_with(plaintext, password, params, &salt, &nonce_bytes)
}

/// Deterministic encrypt — caller supplies salt + nonce. Used only by tests.
pub fn encrypt_with(
    plaintext: &VaultPlaintext,
    password: &[u8],
    params: KdfParams,
    salt: &[u8; SALT_LEN],
    nonce_bytes: &[u8; NONCE_LEN],
) -> Result<Vec<u8>> {
    let mut key_bytes = derive_key(password, salt, params)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let nonce = Nonce::from_slice(nonce_bytes);
    let pt = plaintext.encode()?;

    // Bind header (magic|version|params|salt|nonce) as AEAD associated data so
    // any tampering with header bytes invalidates the tag.
    let aad = build_header(params, salt, nonce_bytes);
    let ct_and_tag = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &pt,
                aad: &aad,
            },
        )
        .map_err(|e| WalletError::Encrypt(e.to_string()))?;
    key_bytes.zeroize();

    let mut out = Vec::with_capacity(HEADER_LEN + ct_and_tag.len());
    out.extend_from_slice(&aad);
    out.extend_from_slice(&ct_and_tag);
    Ok(out)
}

/// Decrypt a vault blob with a password.
pub fn decrypt(blob: &[u8], password: &[u8]) -> Result<VaultPlaintext> {
    if blob.len() < HEADER_LEN + TAG_LEN {
        return Err(WalletError::VaultFormat("vault blob too short".into()));
    }
    if blob[0..8] != MAGIC {
        return Err(WalletError::VaultFormat("bad magic".into()));
    }
    let version = u16::from_be_bytes(blob[8..10].try_into().unwrap());
    if version != VERSION {
        return Err(WalletError::VaultFormat(format!(
            "unsupported vault version {version}"
        )));
    }
    let params = KdfParams::from_bytes(blob[10..10 + PARAMS_LEN].try_into().unwrap())?;
    let salt: [u8; SALT_LEN] = blob[10 + PARAMS_LEN..10 + PARAMS_LEN + SALT_LEN]
        .try_into()
        .unwrap();
    let nonce_bytes: [u8; NONCE_LEN] = blob[10 + PARAMS_LEN + SALT_LEN..HEADER_LEN]
        .try_into()
        .unwrap();
    let ct_and_tag = &blob[HEADER_LEN..];

    let mut key_bytes = derive_key(password, &salt, params)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let nonce = Nonce::from_slice(&nonce_bytes);
    let aad = build_header(params, &salt, &nonce_bytes);

    let pt = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ct_and_tag,
                aad: &aad,
            },
        )
        .map_err(|_| WalletError::Decrypt)?;
    key_bytes.zeroize();
    VaultPlaintext::decode(&pt)
}

fn build_header(
    params: KdfParams,
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..8].copy_from_slice(&MAGIC);
    h[8..10].copy_from_slice(&VERSION.to_be_bytes());
    h[10..10 + PARAMS_LEN].copy_from_slice(&params.to_bytes());
    h[10 + PARAMS_LEN..10 + PARAMS_LEN + SALT_LEN].copy_from_slice(salt);
    h[10 + PARAMS_LEN + SALT_LEN..HEADER_LEN].copy_from_slice(nonce);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let pt = VaultPlaintext::new(vec![0x11; 64]);
        let blob = encrypt(&pt, b"hunter2", KdfParams::testing()).unwrap();
        // Header sanity.
        assert_eq!(&blob[0..8], &MAGIC);
        assert_eq!(u16::from_be_bytes(blob[8..10].try_into().unwrap()), VERSION);
        assert!(blob.len() > HEADER_LEN + TAG_LEN);

        let recovered = decrypt(&blob, b"hunter2").unwrap();
        assert_eq!(recovered.seed, vec![0x11; 64]);
    }

    #[test]
    fn wrong_password_fails() {
        let pt = VaultPlaintext::new(vec![0xAB; 32]);
        let blob = encrypt(&pt, b"correct horse", KdfParams::testing()).unwrap();
        let err = decrypt(&blob, b"wrong horse").unwrap_err();
        match err {
            WalletError::Decrypt => {}
            other => panic!("expected Decrypt, got {other:?}"),
        }
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let pt = VaultPlaintext::new(vec![0xAB; 32]);
        let mut blob = encrypt(&pt, b"pw", KdfParams::testing()).unwrap();
        // Flip a bit in the ciphertext region.
        let i = HEADER_LEN + 1;
        blob[i] ^= 0x01;
        assert!(matches!(decrypt(&blob, b"pw"), Err(WalletError::Decrypt)));
    }

    #[test]
    fn tampered_header_fails() {
        let pt = VaultPlaintext::new(vec![0xAB; 32]);
        let mut blob = encrypt(&pt, b"pw", KdfParams::testing()).unwrap();
        // Flip a bit in the salt — header is bound as AAD, so this must fail.
        blob[10 + PARAMS_LEN] ^= 0x01;
        assert!(matches!(decrypt(&blob, b"pw"), Err(WalletError::Decrypt)));
    }

    #[test]
    fn deterministic_with_fixed_salt_and_nonce() {
        // Sanity: same password + salt + nonce + params + plaintext → same blob.
        let pt = VaultPlaintext::new(vec![1, 2, 3, 4, 5]);
        let salt = [7u8; SALT_LEN];
        let nonce = [9u8; NONCE_LEN];
        let a = encrypt_with(&pt, b"pw", KdfParams::testing(), &salt, &nonce).unwrap();
        let b = encrypt_with(&pt, b"pw", KdfParams::testing(), &salt, &nonce).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn bad_magic_rejected() {
        let pt = VaultPlaintext::new(vec![0; 32]);
        let mut blob = encrypt(&pt, b"pw", KdfParams::testing()).unwrap();
        blob[0] = b'X';
        match decrypt(&blob, b"pw").unwrap_err() {
            WalletError::VaultFormat(_) => {}
            other => panic!("expected VaultFormat, got {other:?}"),
        }
    }

    #[test]
    fn params_round_trip() {
        let p = KdfParams {
            m_cost: 65536,
            t_cost: 3,
            parallelism: 1,
        };
        let p2 = KdfParams::from_bytes(&p.to_bytes()).unwrap();
        assert_eq!(p, p2);
    }
}
