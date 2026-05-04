//! CashAddr encoding for Bitcoin Cash (BCH).
//!
//! CashAddr is a base32 encoding scheme defined by the Bitcoin Cash project:
//! <https://github.com/bitcoincashorg/bitcoincash.org/blob/master/spec/cashaddr.md>
//!
//! It is **not** bech32; it uses a different checksum polynomial, a different
//! alphabet, and different version / payload layout. This module implements
//! encode-only (P2PKH and P2SH) for M6 scope. Decode is out of scope.
//!
//! ## Address format
//!
//! ```text
//! <hrp>:<base32-payload><base32-checksum>
//! ```
//!
//! Payload layout (before 5-bit grouping):
//! - 1 version byte: `type_bits | size_bits` (MSB aligned in the byte)
//! - 20 hash bytes (P2PKH or P2SH)
//!
//! Version byte: `(type << 3) | size_code`
//! - type 0 = P2PKH, type 1 = P2SH
//! - size_code 0 = 160-bit hash (20 bytes)
//!
//! The combined 21 bytes are re-packed into 5-bit groups (34 groups for 21
//! bytes × 8 = 168 bits, padded to 170 = 34 × 5).

use hodl_core::error::Result;

/// CashAddr alphabet (not the same as bech32).
const CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// BCH polynomial constants for the CashAddr checksum.
const GENERATOR: [u64; 5] = [
    0x98f2bc8e61,
    0x79b76d99e2,
    0xf33e5fb3c4,
    0xae2eabe2a8,
    0x1e4f43e470,
];

/// Compute the BCH checksum over `data` (5-bit values).
fn polymod(data: &[u8]) -> u64 {
    let mut c: u64 = 1;
    for &d in data {
        let c0 = (c >> 35) as u8;
        c = ((c & 0x0007_ffff_ffff) << 5) ^ (d as u64);
        for (i, &poly) in GENERATOR.iter().enumerate() {
            if (c0 >> i) & 1 != 0 {
                c ^= poly;
            }
        }
    }
    c ^ 1
}

/// Build the data fed to the polymod: HRP-expanded + payload + 8 zero
/// checksum slots.
///
/// HRP expansion: each character contributes its low 5 bits, preceded by a
/// zero separator byte (the spec feeds HRP bytes directly, then a 0).
fn checksum_input(hrp: &str, payload_5bit: &[u8]) -> Vec<u8> {
    let hrp_bytes = hrp.as_bytes();
    let mut v = Vec::with_capacity(hrp_bytes.len() + 1 + payload_5bit.len() + 8);
    // Each HRP byte contributes its low 5 bits.
    for &b in hrp_bytes {
        v.push(b & 0x1f);
    }
    // Separator.
    v.push(0);
    // Payload (already 5-bit values).
    v.extend_from_slice(payload_5bit);
    // 8 checksum placeholder slots.
    v.extend_from_slice(&[0u8; 8]);
    v
}

/// Convert bytes to 5-bit groups (big-endian bit packing).
///
/// `from_bits` = 8, `to_bits` = 5, padding allowed.
fn convert_bits_8_to_5(data: &[u8]) -> Vec<u8> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::new();
    for &b in data {
        acc = (acc << 8) | (b as u32);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(((acc >> bits) & 0x1f) as u8);
        }
    }
    // Pad remaining bits.
    if bits > 0 {
        out.push(((acc << (5 - bits)) & 0x1f) as u8);
    }
    out
}

/// Encode a CashAddr address given a 5-bit payload and HRP.
fn encode_cashaddr(hrp: &str, payload_5bit: &[u8]) -> Result<String> {
    let cs_input = checksum_input(hrp, payload_5bit);
    let checksum = polymod(&cs_input);

    // Build final: payload + 8 checksum characters.
    let mut chars = Vec::with_capacity(payload_5bit.len() + 8);
    for &v in payload_5bit {
        chars.push(CHARSET[v as usize] as char);
    }
    for i in (0..8).rev() {
        let idx = ((checksum >> (5 * i)) & 0x1f) as usize;
        chars.push(CHARSET[idx] as char);
    }

    let payload_str: String = chars.into_iter().collect();
    Ok(format!("{hrp}:{payload_str}"))
}

/// Encode a P2PKH CashAddr address.
///
/// Type bits = 0 (P2PKH), size bits = 0 (160-bit hash).
/// Version byte = `(0 << 3) | 0` = 0x00.
pub fn p2pkh_cashaddr(hash160: &[u8; 20], hrp: &str) -> Result<String> {
    // Version byte: type=0 (P2PKH), size=0 (160-bit)
    let version: u8 = 0x00;
    let mut payload_bytes = Vec::with_capacity(21);
    payload_bytes.push(version);
    payload_bytes.extend_from_slice(hash160);

    let payload_5bit = convert_bits_8_to_5(&payload_bytes);
    encode_cashaddr(hrp, &payload_5bit)
}

/// Convert 5-bit groups back to bytes (big-endian bit unpacking).
///
/// `from_bits` = 5, `to_bits` = 8, strict (no padding bits allowed).
fn convert_bits_5_to_8(data: &[u8]) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::new();
    for &b in data {
        acc = (acc << 5) | (b as u32);
        bits += 5;
        while bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    // Remaining bits must be zero padding (< 5 bits).
    if bits >= 5 || (acc & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

/// Decode a CashAddr P2PKH address and return the 20-byte pubkey hash.
///
/// Verifies the checksum. Returns `Err` for invalid addresses or non-P2PKH types.
pub fn decode_p2pkh_cashaddr(addr: &str) -> Result<[u8; 20]> {
    use hodl_core::error::Error;
    let colon = addr
        .rfind(':')
        .ok_or_else(|| Error::Codec("missing ':' in CashAddr".into()))?;
    let hrp = &addr[..colon];
    let payload_str = &addr[colon + 1..];

    let mut data_5bit = Vec::with_capacity(payload_str.len());
    for ch in payload_str.chars() {
        let pos = CHARSET
            .iter()
            .position(|&c| c == ch as u8)
            .ok_or_else(|| Error::Codec(format!("invalid CashAddr char '{ch}'")))?;
        data_5bit.push(pos as u8);
    }

    // Verify checksum.
    let hrp_bytes = hrp.as_bytes();
    let mut check_input = Vec::with_capacity(hrp_bytes.len() + 1 + data_5bit.len());
    for &b in hrp_bytes {
        check_input.push(b & 0x1f);
    }
    check_input.push(0);
    check_input.extend_from_slice(&data_5bit);
    let result = polymod(&check_input);
    if result != 0 {
        return Err(Error::Codec(format!(
            "CashAddr checksum invalid (polymod={result:#x})"
        )));
    }

    // Strip 8 checksum characters from end to get payload 5-bit groups.
    let payload_5bit = &data_5bit[..data_5bit.len().saturating_sub(8)];
    let payload_bytes = convert_bits_5_to_8(payload_5bit)
        .ok_or_else(|| Error::Codec("CashAddr 5-to-8 bit conversion failed".into()))?;

    // payload_bytes[0] = version byte; [0] & 0xf8 >> 3 = type, [0] & 0x07 = size.
    if payload_bytes.len() != 21 {
        return Err(Error::Codec("CashAddr payload length mismatch".into()));
    }
    let type_bits = (payload_bytes[0] >> 3) & 0x1f;
    if type_bits != 0 {
        return Err(Error::Codec(
            "CashAddr address is not P2PKH (type != 0)".into(),
        ));
    }
    let mut h160 = [0u8; 20];
    h160.copy_from_slice(&payload_bytes[1..21]);
    Ok(h160)
}

/// Encode a P2SH CashAddr address.
///
/// Type bits = 1 (P2SH), size bits = 0 (160-bit hash).
/// Version byte = `(1 << 3) | 0` = 0x08.
pub fn p2sh_cashaddr(hash160: &[u8; 20], hrp: &str) -> Result<String> {
    // Version byte: type=1 (P2SH), size=0 (160-bit)
    let version: u8 = 0x08;
    let mut payload_bytes = Vec::with_capacity(21);
    payload_bytes.push(version);
    payload_bytes.extend_from_slice(hash160);

    let payload_5bit = convert_bits_8_to_5(&payload_bytes);
    encode_cashaddr(hrp, &payload_5bit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hodl_core::error::{Error, Result};

    /// Verify a CashAddr address by re-computing its polymod checksum.
    ///
    /// Returns `Ok(())` if valid, `Err` if the checksum does not match.
    /// Decode is out of scope for M6; this helper is test-only.
    fn verify_cashaddr(addr: &str) -> Result<()> {
        let colon = addr
            .rfind(':')
            .ok_or_else(|| Error::Codec("missing ':' in CashAddr".into()))?;
        let hrp = &addr[..colon];
        let payload_str = &addr[colon + 1..];

        // Decode base32 characters back to 5-bit values.
        let mut data_5bit = Vec::with_capacity(payload_str.len());
        for ch in payload_str.chars() {
            let pos = CHARSET
                .iter()
                .position(|&c| c == ch as u8)
                .ok_or_else(|| Error::Codec(format!("invalid CashAddr char '{ch}'")))?;
            data_5bit.push(pos as u8);
        }

        // Build polymod input: HRP low-5-bits + separator + full payload (including
        // the 8 checksum characters already appended by encode_cashaddr).
        let hrp_bytes = hrp.as_bytes();
        let mut v = Vec::with_capacity(hrp_bytes.len() + 1 + data_5bit.len());
        for &b in hrp_bytes {
            v.push(b & 0x1f);
        }
        v.push(0);
        v.extend_from_slice(&data_5bit);

        let result = polymod(&v);
        if result == 0 {
            Ok(())
        } else {
            Err(Error::Codec(format!(
                "CashAddr checksum mismatch (polymod={result:#x})"
            )))
        }
    }

    /// All-zeros hash → well-known BCH test vector.
    /// bitcoincash:qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq
    #[test]
    fn p2pkh_all_zeros_bch() {
        let hash = [0u8; 20];
        let addr = p2pkh_cashaddr(&hash, "bitcoincash").unwrap();
        assert!(
            addr.starts_with("bitcoincash:q"),
            "expected bitcoincash:q prefix, got {addr}"
        );
        // Verify the checksum is self-consistent.
        verify_cashaddr(&addr).expect("checksum should be valid");
    }

    /// P2SH all-zeros hash (type bit flipped → different address).
    #[test]
    fn p2sh_all_zeros_bch() {
        let hash = [0u8; 20];
        let addr = p2sh_cashaddr(&hash, "bitcoincash").unwrap();
        assert!(
            addr.starts_with("bitcoincash:"),
            "expected bitcoincash: prefix, got {addr}"
        );
        verify_cashaddr(&addr).expect("checksum should be valid");
    }

    /// Round-trip: encode then verify checksum is zero (polymod property).
    #[test]
    fn checksum_polymod_roundtrip() {
        let hash = [
            0xdeu8, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
            0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ];
        let addr = p2pkh_cashaddr(&hash, "bitcoincash").unwrap();
        verify_cashaddr(&addr).expect("round-trip checksum must validate");
    }

    /// P2PKH and P2SH of the same hash must produce different addresses.
    #[test]
    fn p2pkh_vs_p2sh_differ() {
        let hash = [0x42u8; 20];
        let p2pkh = p2pkh_cashaddr(&hash, "bitcoincash").unwrap();
        let p2sh = p2sh_cashaddr(&hash, "bitcoincash").unwrap();
        assert_ne!(p2pkh, p2sh);
    }
}
