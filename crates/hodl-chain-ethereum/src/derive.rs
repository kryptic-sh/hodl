use bip32::{DerivationPath, XPrv};
use hodl_core::error::{Error, Result};

use crate::address;

/// BIP-44 derivation path for Ethereum: `m/44'/60'/account'/0/index`.
fn eth_path(account: u32, index: u32) -> String {
    format!("m/44'/60'/{}'/0/{}", account, index)
}

/// Derive the 32-byte secp256k1 secret scalar for a given account/index.
pub fn derive_secret_key(seed: &[u8; 64], account: u32, index: u32) -> Result<[u8; 32]> {
    let path: DerivationPath = eth_path(account, index)
        .parse()
        .map_err(|e: bip32::Error| Error::Chain(format!("derivation path: {e}")))?;
    let xprv = XPrv::derive_from_path(seed, &path)
        .map_err(|e| Error::Chain(format!("key derivation: {e}")))?;
    Ok(xprv.private_key().to_bytes().into())
}

/// Derive the EIP-55 checksummed Ethereum address for a given account/index.
///
/// Path: `m/44'/60'/account'/0/index`.
/// Secret key → k256 public key → uncompressed 64-byte X||Y → keccak256 → last 20 bytes.
pub fn derive_address(seed: &[u8; 64], account: u32, index: u32) -> Result<String> {
    let secret_bytes = derive_secret_key(seed, account, index)?;
    let signing_key = k256::ecdsa::SigningKey::from_bytes(secret_bytes.as_ref().into())
        .map_err(|e| Error::Chain(format!("invalid secret key: {e}")))?;
    let verifying_key = signing_key.verifying_key();
    // Uncompressed point: 0x04 || X || Y (65 bytes). Drop the 0x04 prefix.
    let point = verifying_key.to_encoded_point(false);
    let uncompressed = point.as_bytes();
    debug_assert_eq!(uncompressed.len(), 65, "uncompressed pubkey is 65 bytes");
    let xy: &[u8; 64] = uncompressed[1..].try_into().expect("65 - 1 = 64");
    let addr_bytes = address::from_pubkey(xy);
    Ok(address::to_eip55(&addr_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    // "abandon" × 11 + "about", no passphrase — standard BIP-39 test seed.
    // Seed hex from bip39 spec / trezor reference.
    const ABANDON_SEED_HEX: &str = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";

    fn seed_bytes() -> [u8; 64] {
        hex::decode(ABANDON_SEED_HEX).unwrap().try_into().unwrap()
    }

    #[test]
    fn derive_eth_address_abandon_mnemonic() {
        let seed = seed_bytes();
        // m/44'/60'/0'/0/0 — canonical vector from ethers-rs / MyEtherWallet.
        let addr = derive_address(&seed, 0, 0).unwrap();
        assert_eq!(addr, "0x9858EfFD232B4033E47d90003D41EC34EcaEda94");
    }
}
