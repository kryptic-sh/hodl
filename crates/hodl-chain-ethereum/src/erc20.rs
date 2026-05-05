//! Minimal ERC-20 ABI helpers — `balanceOf(address)` selector + uint256 decode.
//!
//! ERC-20 is small enough that hand-rolling the two function calls we
//! actually use is cheaper than pulling in `ethabi` and its dependency tree.
//! If we ever need transfer/approve, reconsider.

use std::fmt;

/// keccak256("balanceOf(address)")[..4]
const BALANCE_OF_SELECTOR: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];

/// Errors produced by the ERC-20 ABI helpers.
#[derive(Debug, PartialEq, Eq)]
pub enum ErcError {
    /// The supplied address string was not a valid 40-hex-char `0x…` address.
    InvalidAddress(String),
    /// The `eth_call` response was not the expected 32 bytes.
    BadResponse(usize),
    /// The uint256 value exceeded u128::MAX.
    Overflow,
}

impl fmt::Display for ErcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErcError::InvalidAddress(s) => write!(f, "invalid ERC-20 address: {s}"),
            ErcError::BadResponse(n) => {
                write!(f, "eth_call response was {n} bytes, expected 32")
            }
            ErcError::Overflow => write!(
                f,
                "token balance exceeds u128::MAX (> 340 undecillion raw units)"
            ),
        }
    }
}

impl std::error::Error for ErcError {}

/// Encode `balanceOf(address)` calldata.
///
/// Returns the 36-byte payload (4-byte selector + 32-byte ABI-encoded address)
/// as a hex string with `0x` prefix.
///
/// Accepts the holder address with or without a `0x` prefix.
pub fn encode_balance_of(addr: &str) -> Result<String, ErcError> {
    let trimmed = addr.strip_prefix("0x").unwrap_or(addr);
    if trimmed.len() != 40 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ErcError::InvalidAddress(addr.to_string()));
    }
    let lower = trimmed.to_ascii_lowercase();

    // ABI encoding: 32 bytes, left-padded with zeros, rightmost 20 bytes = address.
    // 24 zero bytes + 20 address bytes = 32 bytes total.
    let mut payload = Vec::with_capacity(36);
    payload.extend_from_slice(&BALANCE_OF_SELECTOR);
    // 24 zero padding bytes.
    payload.extend_from_slice(&[0u8; 12]);
    // 20 address bytes.
    let addr_bytes = hex::decode(&lower).map_err(|_| ErcError::InvalidAddress(addr.to_string()))?;
    payload.extend_from_slice(&addr_bytes);

    Ok(format!("0x{}", hex::encode(&payload)))
}

/// Decode a 32-byte big-endian uint256 from the `eth_call` response bytes.
///
/// Returns `Err(ErcError::BadResponse)` if `bytes.len() != 32`.
/// Returns `Err(ErcError::Overflow)` if the value exceeds `u128::MAX`
/// (i.e. the upper 16 bytes are non-zero).
pub fn decode_uint256_to_u128(bytes: &[u8]) -> Result<u128, ErcError> {
    if bytes.len() != 32 {
        return Err(ErcError::BadResponse(bytes.len()));
    }
    // Upper 16 bytes must be zero for the value to fit in u128.
    if bytes[..16].iter().any(|&b| b != 0) {
        return Err(ErcError::Overflow);
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&bytes[16..32]);
    Ok(u128::from_be_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known fixture: balanceOf(0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045)
    // keccak selector: 70a08231
    // ABI-encoded address (left-padded to 32 bytes):
    //   000000000000000000000000d8dA6BF26964aF9D7eEd9e03E53415D37aA96045
    // Full calldata (hex, no 0x):
    //   70a08231000000000000000000000000d8da6bf26964af9d7eed9e03e53415d37aa96045
    const VITALIK: &str = "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045";
    const VITALIK_CALLDATA: &str =
        "0x70a08231000000000000000000000000d8da6bf26964af9d7eed9e03e53415d37aa96045";

    #[test]
    fn encode_balance_of_matches_known_fixture() {
        let encoded = encode_balance_of(VITALIK).unwrap();
        assert_eq!(encoded, VITALIK_CALLDATA);
    }

    #[test]
    fn encode_balance_of_accepts_no_prefix() {
        let no_prefix = VITALIK.strip_prefix("0x").unwrap();
        let encoded = encode_balance_of(no_prefix).unwrap();
        assert_eq!(encoded, VITALIK_CALLDATA);
    }

    #[test]
    fn encode_balance_of_rejects_short_address() {
        assert!(matches!(
            encode_balance_of("0xabc"),
            Err(ErcError::InvalidAddress(_))
        ));
    }

    #[test]
    fn encode_balance_of_rejects_non_hex_chars() {
        assert!(matches!(
            encode_balance_of("0xGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG"),
            Err(ErcError::InvalidAddress(_))
        ));
    }

    #[test]
    fn decode_uint256_to_u128_one() {
        let mut bytes = [0u8; 32];
        bytes[31] = 1;
        assert_eq!(decode_uint256_to_u128(&bytes).unwrap(), 1u128);
    }

    #[test]
    fn decode_uint256_to_u128_ff() {
        let mut bytes = [0u8; 32];
        bytes[31] = 0xff;
        assert_eq!(decode_uint256_to_u128(&bytes).unwrap(), 255u128);
    }

    #[test]
    fn decode_uint256_to_u128_zero() {
        let bytes = [0u8; 32];
        assert_eq!(decode_uint256_to_u128(&bytes).unwrap(), 0u128);
    }

    #[test]
    fn decode_uint256_to_u128_max() {
        let mut bytes = [0u8; 32];
        bytes[16..32].copy_from_slice(&u128::MAX.to_be_bytes());
        assert_eq!(decode_uint256_to_u128(&bytes).unwrap(), u128::MAX);
    }

    #[test]
    fn decode_uint256_to_u128_overflow() {
        let mut bytes = [0u8; 32];
        bytes[15] = 1; // upper 16 bytes non-zero → overflow
        assert_eq!(decode_uint256_to_u128(&bytes), Err(ErcError::Overflow));
    }

    #[test]
    fn decode_uint256_to_u128_bad_length() {
        assert_eq!(
            decode_uint256_to_u128(&[0u8; 31]),
            Err(ErcError::BadResponse(31))
        );
        assert_eq!(
            decode_uint256_to_u128(&[0u8; 33]),
            Err(ErcError::BadResponse(33))
        );
    }
}
