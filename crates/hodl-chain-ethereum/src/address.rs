use hodl_core::error::{Error, Result};
use tiny_keccak::{Hasher, Keccak};

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak::v256();
    h.update(data);
    let mut out = [0u8; 32];
    h.finalize(&mut out);
    out
}

/// Derive the 20-byte Ethereum address from an uncompressed public key.
///
/// `pubkey_uncompressed` is the 64-byte X || Y form (no 0x04 prefix).
/// keccak256 of the 64 bytes; take the last 20.
pub fn from_pubkey(pubkey_uncompressed: &[u8; 64]) -> [u8; 20] {
    let hash = keccak256(pubkey_uncompressed);
    hash[12..].try_into().expect("32 - 12 = 20")
}

/// EIP-55 checksum-encode a 20-byte address as a `0x`-prefixed string.
pub fn to_eip55(addr: &[u8; 20]) -> String {
    let hex_lower = hex::encode(addr);
    let hash = keccak256(hex_lower.as_bytes());

    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (i, ch) in hex_lower.chars().enumerate() {
        // nibble index i maps to bit (i * 4) in hash → byte i/2, bit (3 - (i%2)*4)
        let byte = hash[i / 2];
        let nibble_high = (i % 2) == 0;
        let hash_bit = if nibble_high { byte >> 4 } else { byte & 0x0f };
        if hash_bit >= 8 {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse a `0x…` hex address, validate EIP-55 checksum if mixed-case.
///
/// All-lowercase and all-uppercase inputs are accepted without checksum check
/// (they are treated as non-checksummed). Mixed-case must pass EIP-55.
pub fn from_str_normalized(s: &str) -> Result<[u8; 20]> {
    let stripped = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .ok_or_else(|| Error::Codec(format!("ethereum address must start with 0x: {s}")))?;

    if stripped.len() != 40 {
        return Err(Error::Codec(format!(
            "ethereum address must be 40 hex chars after 0x, got {}: {s}",
            stripped.len()
        )));
    }

    let bytes: [u8; 20] = hex::decode(stripped)
        .map_err(|e| Error::Codec(format!("invalid hex in address: {e}")))?
        .try_into()
        .expect("length checked above");

    let has_upper = stripped.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = stripped.chars().any(|c| c.is_ascii_lowercase());
    if has_upper && has_lower {
        // Mixed-case: enforce EIP-55 checksum.
        let expected = to_eip55(&bytes);
        if expected != s {
            return Err(Error::Codec(format!("EIP-55 checksum mismatch for {s}")));
        }
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Canonical EIP-55 vectors from EIP-55 spec.
    #[test]
    fn eip55_vectors() {
        let vectors = [
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ];
        for v in vectors {
            let bytes = from_str_normalized(v).expect("parse");
            let encoded = to_eip55(&bytes);
            assert_eq!(encoded, v, "EIP-55 round-trip failed for {v}");
        }
    }

    #[test]
    fn all_lower_accepted() {
        let lower = "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed";
        from_str_normalized(lower).expect("all-lower should be accepted");
    }

    #[test]
    fn all_upper_accepted() {
        let upper = "0x5AAEB6053F3E94C9B9A09F33669435E7EF1BEAED";
        from_str_normalized(upper).expect("all-upper should be accepted");
    }
}
