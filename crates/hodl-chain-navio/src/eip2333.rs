//! EIP-2333 key derivation for BLS12-381, as Navio implements it.
//!
//! Port of `src/blsct/eip_2333/bls12_381_keygen.cpp` in nav-io/navio-core.
//! Navio follows the final (v4) EIP-2333 draft: the KeyGen salt is re-hashed
//! with SHA-256 on every round, including the first.

use bls12_381::Scalar;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::scalar::{scalar_from_be_bytes, scalar_to_be_bytes};

type HmacSha256 = Hmac<Sha256>;

const DIGEST_SIZE: usize = 32;
const LAMPORT_CHUNKS: usize = 255;
/// `ceil((3 * ceil(log2(r))) / 16)` for the BLS12-381 scalar field.
const OKM_LEN: usize = 48;

fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; DIGEST_SIZE] {
    let mut mac = HmacSha256::new_from_slice(salt).expect("HMAC accepts any key length");
    mac.update(ikm);
    mac.finalize().into_bytes().into()
}

/// HKDF-Expand with an empty-or-short `info`, writing `out.len()` bytes.
fn hkdf_expand(prk: &[u8; DIGEST_SIZE], info: &[u8], out: &mut [u8]) {
    let mut prev = [0u8; DIGEST_SIZE];
    let rounds = out.len().div_ceil(DIGEST_SIZE);
    assert!(rounds <= 255, "HKDF-Expand output too long");
    for i in 1..=rounds {
        let mut mac = HmacSha256::new_from_slice(prk).expect("HMAC accepts any key length");
        if i > 1 {
            mac.update(&prev);
        }
        mac.update(info);
        mac.update(&[i as u8]);
        prev = mac.finalize().into_bytes().into();

        let start = (i - 1) * DIGEST_SIZE;
        let end = usize::min(start + DIGEST_SIZE, out.len());
        out[start..end].copy_from_slice(&prev[..end - start]);
    }
    prev.zeroize();
}

/// EIP-2333 `hkdf_mod_r`. Never returns zero.
fn hkdf_mod_r(ikm: &[u8]) -> Scalar {
    let mut salt: [u8; DIGEST_SIZE] = Sha256::digest(b"BLS-SIG-KEYGEN-SALT-").into();
    let mut ikm_zero = Vec::with_capacity(ikm.len() + 1);
    ikm_zero.extend_from_slice(ikm);
    ikm_zero.push(0);

    loop {
        let mut prk = hkdf_extract(&salt, &ikm_zero);
        let mut okm = [0u8; OKM_LEN];
        // info = I2OSP(48, 2)
        hkdf_expand(&prk, &[0x00, OKM_LEN as u8], &mut okm);
        prk.zeroize();

        let sk = scalar_from_be_bytes(&okm);
        okm.zeroize();
        if sk != Scalar::zero() {
            ikm_zero.zeroize();
            salt.zeroize();
            return sk;
        }
        salt = Sha256::digest(salt).into();
    }
}

/// 255 chunks of 32 bytes, derived from `ikm` under `salt`.
fn ikm_to_lamport_sk(ikm: &[u8], salt: &[u8; 4], out: &mut [u8; LAMPORT_CHUNKS * DIGEST_SIZE]) {
    let mut prk = hkdf_extract(salt, ikm);
    hkdf_expand(&prk, &[], out);
    prk.zeroize();
}

fn parent_sk_to_lamport_pk(parent_sk: &Scalar, index: u32) -> [u8; DIGEST_SIZE] {
    let salt = index.to_be_bytes();
    let mut ikm = scalar_to_be_bytes(parent_sk);
    let mut not_ikm = ikm;
    for b in not_ikm.iter_mut() {
        *b = !*b;
    }

    let mut lamport = [0u8; LAMPORT_CHUNKS * DIGEST_SIZE];
    let mut hasher = Sha256::new();

    for source in [&ikm, &not_ikm] {
        ikm_to_lamport_sk(source, &salt, &mut lamport);
        for chunk in lamport.as_chunks::<DIGEST_SIZE>().0 {
            hasher.update(Sha256::digest(chunk));
        }
    }

    lamport.zeroize();
    ikm.zeroize();
    not_ikm.zeroize();
    hasher.finalize().into()
}

/// Derive the master secret key from at least 32 bytes of seed material.
///
/// Navio calls this with either the 32-byte BIP-39 entropy (no passphrase) or
/// the 64-byte BIP-39 seed (with passphrase); see [`crate::keys`].
pub fn derive_master_sk(seed: &[u8]) -> Option<Scalar> {
    if seed.len() < 32 {
        return None;
    }
    Some(hkdf_mod_r(seed))
}

/// Derive the child secret key at `index`.
pub fn derive_child_sk(parent_sk: &Scalar, index: u32) -> Scalar {
    let mut lamport_pk = parent_sk_to_lamport_pk(parent_sk, index);
    let sk = hkdf_mod_r(&lamport_pk);
    lamport_pk.zeroize();
    sk
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EIP-2333 test case 0. Navio's `derive_master_SK` is the spec function
    /// unchanged, so the published vectors pin our port.
    #[test]
    fn eip2333_case_0_master_and_child() {
        let seed = hex::decode("c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04").unwrap();
        let master = derive_master_sk(&seed).unwrap();
        assert_eq!(
            hex::encode(scalar_to_be_bytes(&master)),
            "0d7359d57963ab8fbbde1852dcf553fedbc31f464d80ee7d40ae683122b45070"
        );
        let child = derive_child_sk(&master, 0);
        assert_eq!(
            hex::encode(scalar_to_be_bytes(&child)),
            "2d18bd6c14e6d15bf8b5085c9b74f3daae3b03cc2014770a599d8c1539e50f8e"
        );
    }

    /// EIP-2333 test case 2 — a 32-byte seed and a large child index.
    #[test]
    fn eip2333_case_2_master_and_child() {
        let seed = hex::decode("3141592653589793238462643383279502884197169399375105820974944592")
            .unwrap();
        let master = derive_master_sk(&seed).unwrap();
        assert_eq!(
            hex::encode(scalar_to_be_bytes(&master)),
            "41c9e07822b092a93fd6797396338c3ada4170cc81829fdfce6b5d34bd5e7ec7"
        );
        let child = derive_child_sk(&master, 3141592653);
        assert_eq!(
            hex::encode(scalar_to_be_bytes(&child)),
            "384843fad5f3d777ea39de3e47a8f999ae91f89e42bffa993d91d9782d152a0f"
        );
    }

    #[test]
    fn seed_shorter_than_32_bytes_rejected() {
        assert!(derive_master_sk(&[0u8; 31]).is_none());
    }

    #[test]
    fn hkdf_expand_matches_rfc5869_vector() {
        // RFC 5869 A.1: SHA-256, L = 42.
        let prk: [u8; 32] =
            hex::decode("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5")
                .unwrap()
                .try_into()
                .unwrap();
        let info = hex::decode("f0f1f2f3f4f5f6f7f8f9").unwrap();
        let mut okm = [0u8; 42];
        hkdf_expand(&prk, &info, &mut okm);
        assert_eq!(
            hex::encode(okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }
}
