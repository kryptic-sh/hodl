use bip32::XPub;
use hodl_core::ChainId;
use hodl_core::error::{Error, Result};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256};

use crate::cashaddr;
use crate::network::NetworkParams;

/// Hash160 = RIPEMD160(SHA256(data)).
fn hash160(data: &[u8]) -> [u8; 20] {
    let sha = Sha256::digest(data);
    let rmd = Ripemd160::digest(sha);
    rmd.into()
}

/// Base58Check encode: prepend version byte, double-SHA256 checksum.
fn base58check(version: u8, payload: &[u8]) -> String {
    let mut data = Vec::with_capacity(1 + payload.len());
    data.push(version);
    data.extend_from_slice(payload);
    bs58::encode(&data).with_check().into_string()
}

/// P2PKH — pay-to-public-key-hash.
/// Script: OP_DUP OP_HASH160 <20-byte hash> OP_EQUALVERIFY OP_CHECKSIG
///
/// For BCH, emits CashAddr instead of legacy base58check.
pub fn p2pkh(xpub: &XPub, params: &NetworkParams) -> Result<String> {
    let pubkey_bytes = xpub.to_bytes();
    let h160 = hash160(&pubkey_bytes);
    match params.chain_id {
        ChainId::BitcoinCash => cashaddr::p2pkh_cashaddr(&h160, params.bech32_hrp),
        _ => Ok(base58check(params.p2pkh_prefix, &h160)),
    }
}

/// P2SH-P2WPKH — BIP-49. Wraps a P2WPKH redeem script inside P2SH.
/// Redeem script: OP_0 <20-byte pubkey hash>
///
/// For BCH, emits CashAddr P2SH instead of legacy base58check.
pub fn p2sh_p2wpkh(xpub: &XPub, params: &NetworkParams) -> Result<String> {
    let pubkey_bytes = xpub.to_bytes();
    let h160 = hash160(&pubkey_bytes);
    // Redeem script: 0x00 0x14 <20 bytes>
    let mut redeem = Vec::with_capacity(22);
    redeem.push(0x00); // OP_0
    redeem.push(0x14); // push 20 bytes
    redeem.extend_from_slice(&h160);
    let script_hash = hash160(&redeem);
    match params.chain_id {
        ChainId::BitcoinCash => cashaddr::p2sh_cashaddr(&script_hash, params.bech32_hrp),
        _ => Ok(base58check(params.p2sh_prefix, &script_hash)),
    }
}

/// P2WPKH — BIP-84. Native segwit bech32 (witness version 0).
pub fn p2wpkh(xpub: &XPub, params: &NetworkParams) -> Result<String> {
    use bech32::segwit;
    let pubkey_bytes = xpub.to_bytes();
    let h160 = hash160(&pubkey_bytes);
    let hrp = bech32::Hrp::parse(params.bech32_hrp)
        .map_err(|e| Error::Codec(format!("invalid HRP: {e}")))?;
    segwit::encode(hrp, segwit::VERSION_0, &h160)
        .map_err(|e| Error::Codec(format!("bech32 encode: {e}")))
}

/// P2TR — BIP-86. Taproot bech32m (witness version 1).
///
/// For a key-path-only taproot output the internal key is tweaked with the
/// tagged hash of itself (BIP-341 §Script validation for taproot):
///   output_key = internal_key + tagged_hash("TapTweak", internal_key) * G
///
/// We use the x-only (32-byte) compressed public key as the internal key and
/// apply the standard BIP-341 key-path tweak so the resulting address matches
/// reference wallets.
pub fn p2tr(xpub: &XPub, params: &NetworkParams) -> Result<String> {
    use bech32::segwit;
    let pubkey_bytes = xpub.to_bytes();
    // pubkey_bytes is 33 bytes (compressed); x-only is the last 32.
    let x_only = &pubkey_bytes[1..33];
    // BIP-341 TapTweak tagged hash for key-path-only (no script tree).
    let tweaked = tap_tweak_key_path(x_only)?;
    let hrp = bech32::Hrp::parse(params.bech32_hrp)
        .map_err(|e| Error::Codec(format!("invalid HRP: {e}")))?;
    segwit::encode(hrp, segwit::VERSION_1, &tweaked)
        .map_err(|e| Error::Codec(format!("bech32m encode: {e}")))
}

/// BIP-341 tagged hash helper: SHA256(SHA256(tag) || SHA256(tag) || msg).
fn tagged_hash(tag: &[u8], msg: &[u8]) -> [u8; 32] {
    let tag_hash: [u8; 32] = Sha256::digest(tag).into();
    let mut hasher = Sha256::new();
    hasher.update(tag_hash);
    hasher.update(tag_hash);
    hasher.update(msg);
    hasher.finalize().into()
}

/// BIP-341 key-path tweak: output_key_x = lift_x(x) + t*G where t =
/// tagged_hash("TapTweak", x_only). Returns the x-only output key (32 bytes).
///
/// We use k256 which is already in the dependency tree via bip32.
fn tap_tweak_key_path(x_only: &[u8]) -> Result<[u8; 32]> {
    use k256::elliptic_curve::PrimeField;
    use k256::elliptic_curve::sec1::ToEncodedPoint;

    // Reconstruct the full compressed point from x-only (assume even Y, parity 0x02).
    let mut compressed = [0u8; 33];
    compressed[0] = 0x02;
    compressed[1..].copy_from_slice(x_only);

    let internal_key = k256::PublicKey::from_sec1_bytes(&compressed)
        .map_err(|e| Error::Codec(format!("invalid pubkey for taproot: {e}")))?;

    let tweak_bytes = tagged_hash(b"TapTweak", x_only);
    let tweak_scalar = k256::Scalar::from_repr(tweak_bytes.into())
        .into_option()
        .ok_or_else(|| Error::Codec("taproot tweak scalar out of range".into()))?;

    // output = internal_key_point + tweak_scalar * G
    let generator = k256::ProjectivePoint::GENERATOR;
    let internal_proj = internal_key.to_projective();
    let output_proj = internal_proj + generator * tweak_scalar;
    let output_key = k256::PublicKey::from_affine(output_proj.into())
        .map_err(|_| Error::Codec("taproot output key is identity".into()))?;

    // x-only: strip the parity byte.
    let encoded = output_key.to_encoded_point(true);
    let bytes = encoded.as_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes[1..33]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bip32::XPrv;
    use std::str::FromStr;

    fn xpub_at(seed_hex: &str, path: &str) -> XPub {
        let seed = hex::decode(seed_hex).unwrap();
        let path = bip32::DerivationPath::from_str(path).unwrap();
        let xprv = XPrv::derive_from_path(&seed, &path).unwrap();
        xprv.public_key()
    }

    // BIP-39 "abandon * 11 + about", passphrase = ""
    // Trezor vector seed hex (64 bytes, no passphrase):
    const ABANDON_SEED: &str = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";

    /// BIP-84 reference: m/84'/0'/0'/0/0 → bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu
    /// Source: BIP-84 test vectors.
    #[test]
    fn bip84_mainnet_address_vector() {
        let xpub = xpub_at(ABANDON_SEED, "m/84'/0'/0'/0/0");
        let addr = p2wpkh(&xpub, &crate::network::NetworkParams::BITCOIN_MAINNET).unwrap();
        assert_eq!(addr, "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
    }

    /// BIP-86 reference: m/86'/0'/0'/0/0 →
    /// bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr
    #[test]
    fn bip86_mainnet_address_vector() {
        let xpub = xpub_at(ABANDON_SEED, "m/86'/0'/0'/0/0");
        let addr = p2tr(&xpub, &crate::network::NetworkParams::BITCOIN_MAINNET).unwrap();
        assert_eq!(
            addr,
            "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr"
        );
    }

    #[test]
    fn p2pkh_mainnet_smoke() {
        let xpub = xpub_at(ABANDON_SEED, "m/44'/0'/0'/0/0");
        let addr = p2pkh(&xpub, &crate::network::NetworkParams::BITCOIN_MAINNET).unwrap();
        assert!(addr.starts_with('1'), "P2PKH mainnet must start with '1'");
    }

    #[test]
    fn p2sh_p2wpkh_mainnet_smoke() {
        let xpub = xpub_at(ABANDON_SEED, "m/49'/0'/0'/0/0");
        let addr = p2sh_p2wpkh(&xpub, &crate::network::NetworkParams::BITCOIN_MAINNET).unwrap();
        assert!(addr.starts_with('3'), "P2SH mainnet must start with '3'");
    }

    #[test]
    fn p2wpkh_testnet_prefix() {
        let xpub = xpub_at(ABANDON_SEED, "m/84'/1'/0'/0/0");
        let addr = p2wpkh(&xpub, &crate::network::NetworkParams::BITCOIN_TESTNET).unwrap();
        assert!(
            addr.starts_with("tb1q"),
            "testnet bech32 must start with tb1q"
        );
    }
}
