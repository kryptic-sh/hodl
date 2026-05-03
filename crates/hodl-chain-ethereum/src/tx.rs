//! EIP-1559 (type 0x02) transaction encoding and signing.
//!
//! Hand-rolled RLP — no external RLP crate. Rules:
//!   Single byte 0x00–0x7f  → as-is
//!   Byte string len 0–55   → 0x80+len || data
//!   Byte string len > 55   → 0xb7+len_of_len || big-endian(len) || data
//!   List payload len 0–55  → 0xc0+len || payload
//!   List payload len > 55  → 0xf7+len_of_len || big-endian(len) || payload
//!   Integers               → minimal big-endian bytes (zero → empty)

use hodl_core::error::{Error, Result};
use k256::ecdsa::{RecoveryId, Signature, SigningKey};
use serde::{Deserialize, Serialize};
use tiny_keccak::{Hasher, Keccak};

/// EIP-1559 (type 0x02) unsigned transaction fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eip1559Tx {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u64,
    pub max_fee_per_gas: u64,
    pub gas_limit: u64,
    pub to: [u8; 20],
    pub value_wei: u128,
    /// Call data; empty for plain ETH transfers.
    pub data: Vec<u8>,
    /// Access list — always empty for v1 (ERC-2930 out of scope).
    pub access_list: Vec<()>,
}

// ── RLP primitives ────────────────────────────────────────────────────────────

fn rlp_length_prefix(base: u8, long_base: u8, len: usize) -> Vec<u8> {
    if len <= 55 {
        vec![base + len as u8]
    } else {
        let len_bytes = minimal_be_bytes_usize(len);
        let mut v = vec![long_base + len_bytes.len() as u8];
        v.extend_from_slice(&len_bytes);
        v
    }
}

fn minimal_be_bytes_usize(n: usize) -> Vec<u8> {
    if n == 0 {
        return vec![];
    }
    let b = n.to_be_bytes();
    let skip = b.iter().take_while(|&&x| x == 0).count();
    b[skip..].to_vec()
}

fn minimal_be_bytes_u64(n: u64) -> Vec<u8> {
    if n == 0 {
        return vec![];
    }
    let b = n.to_be_bytes();
    let skip = b.iter().take_while(|&&x| x == 0).count();
    b[skip..].to_vec()
}

fn minimal_be_bytes_u128(n: u128) -> Vec<u8> {
    if n == 0 {
        return vec![];
    }
    let b = n.to_be_bytes();
    let skip = b.iter().take_while(|&&x| x == 0).count();
    b[skip..].to_vec()
}

/// RLP-encode a byte string.
pub fn rlp_bytes(data: &[u8]) -> Vec<u8> {
    if data.len() == 1 && data[0] < 0x80 {
        // Single byte in [0x00, 0x7f] encodes as-is.
        return data.to_vec();
    }
    let mut out = rlp_length_prefix(0x80, 0xb7, data.len());
    out.extend_from_slice(data);
    out
}

/// RLP-encode a list given its already-encoded payload bytes.
pub fn rlp_list(payload: &[u8]) -> Vec<u8> {
    let mut out = rlp_length_prefix(0xc0, 0xf7, payload.len());
    out.extend_from_slice(payload);
    out
}

/// RLP-encode a u64 integer.
fn rlp_u64(n: u64) -> Vec<u8> {
    rlp_bytes(&minimal_be_bytes_u64(n))
}

/// RLP-encode a u128 integer.
fn rlp_u128(n: u128) -> Vec<u8> {
    rlp_bytes(&minimal_be_bytes_u128(n))
}

// ── Keccak ────────────────────────────────────────────────────────────────────

pub(crate) fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak::v256();
    h.update(data);
    let mut out = [0u8; 32];
    h.finalize(&mut out);
    out
}

// ── Transaction encoding ──────────────────────────────────────────────────────

fn encode_unsigned_rlp_payload(tx: &Eip1559Tx) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(rlp_u64(tx.chain_id));
    payload.extend(rlp_u64(tx.nonce));
    payload.extend(rlp_u64(tx.max_priority_fee_per_gas));
    payload.extend(rlp_u64(tx.max_fee_per_gas));
    payload.extend(rlp_u64(tx.gas_limit));
    payload.extend(rlp_bytes(&tx.to)); // `to` is always 20 bytes
    payload.extend(rlp_u128(tx.value_wei));
    payload.extend(rlp_bytes(&tx.data));
    // Empty access list → rlp_list of empty payload.
    payload.extend(rlp_list(&[]));
    payload
}

/// Compute the EIP-1559 signing hash: keccak256(0x02 || rlp(unsigned fields)).
pub fn sighash(tx: &Eip1559Tx) -> [u8; 32] {
    let payload = encode_unsigned_rlp_payload(tx);
    let rlp_encoded = rlp_list(&payload);
    let mut prefixed = Vec::with_capacity(1 + rlp_encoded.len());
    prefixed.push(0x02);
    prefixed.extend_from_slice(&rlp_encoded);
    keccak256(&prefixed)
}

/// Sign an EIP-1559 transaction with a 32-byte secp256k1 key.
///
/// Returns the fully-encoded signed transaction bytes ready for broadcast:
/// `0x02 || rlp([chain_id, nonce, max_priority_fee, max_fee, gas_limit, to,
///               value, data, access_list, y_parity, r, s])`.
pub fn sign(tx: &Eip1559Tx, key: &[u8; 32]) -> Result<Vec<u8>> {
    let hash = sighash(tx);
    let signing_key = SigningKey::from_bytes(key.as_ref().into())
        .map_err(|e| Error::Chain(format!("invalid signing key: {e}")))?;
    let (sig, recid): (Signature, RecoveryId) = signing_key
        .sign_prehash_recoverable(&hash)
        .map_err(|e| Error::Chain(format!("ecdsa sign: {e}")))?;

    let r = sig.r().to_bytes();
    let s = sig.s().to_bytes();
    let y_parity: u8 = recid.is_y_odd() as u8;

    let mut payload = encode_unsigned_rlp_payload(tx);
    payload.extend(rlp_u64(y_parity as u64));
    payload.extend(rlp_bytes(r.as_ref()));
    payload.extend(rlp_bytes(s.as_ref()));

    let rlp_encoded = rlp_list(&payload);
    let mut out = Vec::with_capacity(1 + rlp_encoded.len());
    out.push(0x02);
    out.extend_from_slice(&rlp_encoded);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::{VerifyingKey, signature::hazmat::PrehashVerifier};

    #[test]
    fn rlp_list_encoding() {
        // [ "dog" ] — classic RLP example: 0xc483646f67
        let dog = rlp_bytes(b"dog");
        let encoded = rlp_list(&dog);
        assert_eq!(encoded, vec![0xc4, 0x83, 0x64, 0x6f, 0x67]);
    }

    #[test]
    fn rlp_single_byte_passthrough() {
        // Single byte in [0x00, 0x7f] encodes as-is (RLP spec §4.1).
        assert_eq!(rlp_bytes(&[0x00]), vec![0x00]);
        assert_eq!(rlp_bytes(&[0x01]), vec![0x01]);
        assert_eq!(rlp_bytes(&[0x7f]), vec![0x7f]);
        // Single byte >= 0x80 needs length prefix.
        assert_eq!(rlp_bytes(&[0x80]), vec![0x81, 0x80]);
    }

    #[test]
    fn rlp_zero_integer() {
        // Zero integer → empty byte string → 0x80.
        assert_eq!(rlp_u64(0), vec![0x80]);
        assert_eq!(rlp_u128(0), vec![0x80]);
    }

    #[test]
    fn sign_and_recover() {
        let tx = Eip1559Tx {
            chain_id: 1,
            nonce: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 20_000_000_000,
            gas_limit: 21_000,
            to: [0u8; 20],
            value_wei: 1_000_000_000_000_000_000,
            data: vec![],
            access_list: vec![],
        };

        // Use a deterministic test key.
        let key: [u8; 32] = [1u8; 32];
        let signed = sign(&tx, &key).unwrap();

        // Must start with 0x02 (EIP-1559 type byte).
        assert_eq!(signed[0], 0x02);

        // Recover and verify the signature round-trips.
        let hash = sighash(&tx);
        let signing_key = SigningKey::from_bytes(key.as_ref().into()).unwrap();
        let (sig, _recid): (Signature, RecoveryId) =
            signing_key.sign_prehash_recoverable(&hash).unwrap();
        let verifying_key = VerifyingKey::from(&signing_key);
        verifying_key.verify_prehash(&hash, &sig).unwrap();
    }
}
