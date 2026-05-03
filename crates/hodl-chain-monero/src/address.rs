//! Monero standard address encoding (Monero base58 variant).
//!
//! Format: [prefix(1) | spend_pub(32) | view_pub(32) | checksum(4)]
//! Checksum = first 4 bytes of keccak256([prefix | spend_pub | view_pub]).
//! Encoded with Monero's base58 variant: 8-byte blocks → 11-char blocks,
//! final partial block uses a shorter encoded width.

use tiny_keccak::{Hasher, Keccak};

fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut k = Keccak::v256();
    k.update(input);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

/// Encode a standard (non-integrated, non-subaddress) Monero address.
///
/// Panics only if base58-monero rejects a 69-byte input, which is not
/// possible by the library's implementation — the panic guard is purely
/// defensive.
pub fn encode(spend_pub: &[u8; 32], view_pub: &[u8; 32], prefix: u8) -> String {
    // Build the raw payload before checksum.
    let mut payload = Vec::with_capacity(69);
    payload.push(prefix);
    payload.extend_from_slice(spend_pub);
    payload.extend_from_slice(view_pub);

    // Checksum: first 4 bytes of keccak256(payload).
    let checksum = keccak256(&payload);
    payload.extend_from_slice(&checksum[..4]);

    // payload is now 69 bytes: 1 + 32 + 32 + 4.
    // A 69-byte input always produces 95 base58 chars (8*11 + 7) per lib docs.
    base58_monero::encode(&payload).expect("base58-monero encode of 69-byte payload is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_shape() {
        let spend = [1u8; 32];
        let view = [2u8; 32];
        let addr = encode(&spend, &view, 18);
        assert_eq!(addr.len(), 95);
        assert!(addr.starts_with('4'));
    }
}
