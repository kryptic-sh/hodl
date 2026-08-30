//! BLSCT address encoding.
//!
//! A Navio confidential address is the bech32m-mod encoding of a
//! [`DoublePublicKey`] under a per-network HRP: `nav` on mainnet, `tnv` on
//! testnet, `rnv` on regtest (`kernel/chainparams.cpp`, `bech32_mod_hrp`).

use crate::bech32_mod::{
    self, DOUBLE_PUBKEY_DATA_ENC_SIZE, Encoding, convert_bits_5_to_8, convert_bits_8_to_5,
};
use crate::keys::{DOUBLE_PUBKEY_SIZE, DoublePublicKey};

/// Mainnet BLSCT address prefix.
pub const MAINNET_HRP: &str = "nav";
/// Testnet BLSCT address prefix.
pub const TESTNET_HRP: &str = "tnv";
/// Regtest BLSCT address prefix.
pub const REGTEST_HRP: &str = "rnv";

/// Encode a BLSCT destination as an address string.
pub fn encode_address(hrp: &str, dpk: &DoublePublicKey) -> Option<String> {
    let values = convert_bits_8_to_5(&dpk.to_bytes());
    debug_assert_eq!(values.len(), DOUBLE_PUBKEY_DATA_ENC_SIZE);
    bech32_mod::encode(Encoding::Bech32m, hrp, &values)
}

/// Decode a BLSCT address, requiring the given `hrp`.
///
/// Both bech32 and bech32m checksums are accepted, matching upstream's
/// `blsct::DecodeDoublePublicKey`; the encoder only ever emits bech32m.
pub fn decode_address(hrp: &str, s: &str) -> Option<DoublePublicKey> {
    if s.len() != address_len(hrp) {
        return None;
    }
    let decoded = bech32_mod::decode(s)?;
    if decoded.hrp != hrp || decoded.data.len() != DOUBLE_PUBKEY_DATA_ENC_SIZE {
        return None;
    }
    let bytes = convert_bits_5_to_8(&decoded.data)?;
    let arr: [u8; DOUBLE_PUBKEY_SIZE] = bytes.try_into().ok()?;
    DoublePublicKey::from_bytes(&arr)
}

/// Length of an encoded address: HRP, separator, payload, 8-symbol checksum.
pub const fn address_len(hrp: &str) -> usize {
    hrp.len() + 1 + DOUBLE_PUBKEY_DATA_ENC_SIZE + 8
}

/// Cheap syntactic check: does `s` look like a BLSCT address for `hrp`?
///
/// Used by UI address validation before the (comparatively expensive) point
/// decompression in [`decode_address`].
pub fn looks_like_address(hrp: &str, s: &str) -> bool {
    s.len() == address_len(hrp) && s.to_ascii_lowercase().starts_with(&format!("{hrp}1"))
}
