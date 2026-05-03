use bip32::{DerivationPath, XPrv};
use hodl_core::error::{Error, Result};

use crate::address;
use crate::network::NetworkParams;

/// BIP-purpose selector used to pick derivation path and address type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// BIP-44: legacy P2PKH.
    Bip44,
    /// BIP-49: P2SH-P2WPKH (wrapped segwit).
    Bip49,
    /// BIP-84: native segwit P2WPKH (bech32).
    Bip84,
    /// BIP-86: taproot P2TR (bech32m).
    Bip86,
}

impl Purpose {
    fn value(self) -> u32 {
        match self {
            Purpose::Bip44 => 44,
            Purpose::Bip49 => 49,
            Purpose::Bip84 => 84,
            Purpose::Bip86 => 86,
        }
    }
}

fn path_str(purpose: u32, coin: u32, account: u32, change: u32, index: u32) -> String {
    format!("m/{purpose}'/{coin}'/{account}'/{change}/{index}")
}

pub fn bip44_path(coin: u32, account: u32, change: u32, index: u32) -> String {
    path_str(44, coin, account, change, index)
}

pub fn bip49_path(coin: u32, account: u32, change: u32, index: u32) -> String {
    path_str(49, coin, account, change, index)
}

pub fn bip84_path(coin: u32, account: u32, change: u32, index: u32) -> String {
    path_str(84, coin, account, change, index)
}

pub fn bip86_path(coin: u32, account: u32, change: u32, index: u32) -> String {
    path_str(86, coin, account, change, index)
}

/// Derive an `XPrv` from a 64-byte BIP-39 seed.
///
/// Returns the private extended key, from which both the signing scalar and
/// compressed public key can be extracted.
pub fn derive_xprv(
    seed: &[u8; 64],
    purpose: Purpose,
    params: &NetworkParams,
    account: u32,
    change: u32,
    index: u32,
) -> Result<XPrv> {
    let coin = params.chain_id.slip44();
    let path_s = path_str(purpose.value(), coin, account, change, index);
    let parsed: DerivationPath = path_s
        .parse()
        .map_err(|e: bip32::Error| Error::Chain(format!("derivation path: {e}")))?;
    XPrv::derive_from_path(seed, &parsed).map_err(|e| Error::Chain(format!("key derivation: {e}")))
}

/// Derive an address from a 64-byte BIP-39 seed for the given purpose and
/// network at the specified account / change / index.
pub fn derive_address(
    seed: &[u8; 64],
    purpose: Purpose,
    params: &NetworkParams,
    account: u32,
    change: u32,
    index: u32,
) -> Result<String> {
    let coin = params.chain_id.slip44();
    let path_s = path_str(purpose.value(), coin, account, change, index);
    let parsed: DerivationPath = path_s
        .parse()
        .map_err(|e: bip32::Error| Error::Chain(format!("derivation path: {e}")))?;
    let xprv = XPrv::derive_from_path(seed, &parsed)
        .map_err(|e| Error::Chain(format!("key derivation: {e}")))?;
    let xpub = xprv.public_key();
    match purpose {
        Purpose::Bip44 => address::p2pkh(&xpub, params),
        Purpose::Bip49 => address::p2sh_p2wpkh(&xpub, params),
        Purpose::Bip84 => address::p2wpkh(&xpub, params),
        Purpose::Bip86 => address::p2tr(&xpub, params),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // "abandon" * 11 + "about", no passphrase — standard BIP-39 test seed.
    const ABANDON_SEED: &str = "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4";

    fn seed_bytes() -> [u8; 64] {
        let v = hex::decode(ABANDON_SEED).unwrap();
        v.try_into().unwrap()
    }

    #[test]
    fn path_helpers_format_correctly() {
        assert_eq!(bip44_path(0, 0, 0, 0), "m/44'/0'/0'/0/0");
        assert_eq!(bip49_path(0, 0, 0, 0), "m/49'/0'/0'/0/0");
        assert_eq!(bip84_path(0, 0, 0, 0), "m/84'/0'/0'/0/0");
        assert_eq!(bip86_path(0, 0, 0, 0), "m/86'/0'/0'/0/0");
    }

    #[test]
    fn derive_address_bip84() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip84,
            &NetworkParams::BITCOIN_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert_eq!(addr, "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
    }

    #[test]
    fn derive_address_bip86() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip86,
            &NetworkParams::BITCOIN_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            addr,
            "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr"
        );
    }
}
