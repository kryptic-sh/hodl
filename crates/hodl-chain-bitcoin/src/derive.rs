use bip32::{DerivationPath, XPrv};
use hodl_core::ChainId;
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

/// Validate that `purpose` is supported for the given chain.
///
/// | Chain       | Supported purposes        | Rationale                            |
/// |-------------|---------------------------|--------------------------------------|
/// | Bitcoin     | Bip44, 49, 84, 86         | Full segwit + taproot                |
/// | Litecoin    | Bip44, 49, 84             | MWEB is post-v1; no taproot on LTC   |
/// | Dogecoin    | Bip44 only                | bech32/segwit not deployed on DOGE   |
/// | BitcoinCash | Bip44, 49 (→ CashAddr)    | BIP-84/86 not deployed on BCH        |
/// | BitcoinSv   | Bip44 only                | bech32/segwit not deployed on BSV    |
/// | ECash       | Bip44, 49 (→ CashAddr)    | BIP-84/86 not deployed on XEC        |
/// | Navio       | Bip44, 49, 84             | segwit deployed; no taproot on NAVIO |
/// | others      | all (pass-through)        | Unknown derivative — no restriction  |
fn validate_purpose(purpose: Purpose, params: &NetworkParams) -> Result<()> {
    let chain = params.chain_id;
    let ok = match chain {
        ChainId::Litecoin | ChainId::Navio => {
            matches!(purpose, Purpose::Bip44 | Purpose::Bip49 | Purpose::Bip84)
        }
        ChainId::Dogecoin | ChainId::BitcoinSv => matches!(purpose, Purpose::Bip44),
        ChainId::BitcoinCash | ChainId::ECash => {
            matches!(purpose, Purpose::Bip44 | Purpose::Bip49)
        }
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(Error::Codec(format!(
            "BIP-{} not deployed on {}",
            purpose.value(),
            chain.display_name()
        )))
    }
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
/// compressed public key can be extracted. Validates that `purpose` is
/// supported for the chain described by `params`.
pub fn derive_xprv(
    seed: &[u8; 64],
    purpose: Purpose,
    params: &NetworkParams,
    account: u32,
    change: u32,
    index: u32,
) -> Result<XPrv> {
    validate_purpose(purpose, params)?;
    let coin = params.chain_id.slip44();
    let path_s = path_str(purpose.value(), coin, account, change, index);
    let parsed: DerivationPath = path_s
        .parse()
        .map_err(|e: bip32::Error| Error::Chain(format!("derivation path: {e}")))?;
    XPrv::derive_from_path(seed, &parsed).map_err(|e| Error::Chain(format!("key derivation: {e}")))
}

/// Derive an address from a 64-byte BIP-39 seed for the given purpose and
/// network at the specified account / change / index.
///
/// Validates that `purpose` is supported for the chain described by `params`
/// before performing any derivation. Returns `Error::Codec` for unsupported
/// purpose/chain combinations (e.g. BIP-84 on Dogecoin).
pub fn derive_address(
    seed: &[u8; 64],
    purpose: Purpose,
    params: &NetworkParams,
    account: u32,
    change: u32,
    index: u32,
) -> Result<String> {
    validate_purpose(purpose, params)?;
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

    // --- M6: BTC-derivative chain tests ---

    /// LTC P2PKH — m/44'/2'/0'/0/0 must start with "L".
    #[test]
    fn ltc_p2pkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip44,
            &NetworkParams::LITECOIN_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with('L'),
            "LTC P2PKH must start with 'L', got {addr}"
        );
    }

    /// LTC P2WPKH (BIP-84) — m/84'/2'/0'/0/0 must start with "ltc1q".
    #[test]
    fn ltc_p2wpkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip84,
            &NetworkParams::LITECOIN_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with("ltc1q"),
            "LTC P2WPKH must start with 'ltc1q', got {addr}"
        );
    }

    /// LTC does not support BIP-86 (taproot).
    #[test]
    fn ltc_bip86_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip86,
            &NetworkParams::LITECOIN_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-86 must be rejected on LTC");
    }

    /// DOGE P2PKH — m/44'/3'/0'/0/0 must start with "D".
    #[test]
    fn doge_p2pkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip44,
            &NetworkParams::DOGECOIN_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with('D'),
            "DOGE P2PKH must start with 'D', got {addr}"
        );
    }

    /// DOGE does not support BIP-49.
    #[test]
    fn doge_bip49_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip49,
            &NetworkParams::DOGECOIN_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-49 must be rejected on DOGE");
    }

    /// DOGE does not support BIP-84.
    #[test]
    fn doge_bip84_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip84,
            &NetworkParams::DOGECOIN_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-84 must be rejected on DOGE");
    }

    /// BCH CashAddr P2PKH — m/44'/145'/0'/0/0 must start with "bitcoincash:q".
    #[test]
    fn bch_cashaddr_p2pkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip44,
            &NetworkParams::BITCOIN_CASH_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with("bitcoincash:q"),
            "BCH P2PKH must start with 'bitcoincash:q', got {addr}"
        );
    }

    /// BCH does not support BIP-84.
    #[test]
    fn bch_bip84_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip84,
            &NetworkParams::BITCOIN_CASH_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-84 must be rejected on BCH");
    }

    /// BCH does not support BIP-86.
    #[test]
    fn bch_bip86_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip86,
            &NetworkParams::BITCOIN_CASH_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-86 must be rejected on BCH");
    }

    /// BSV P2PKH — m/44'/236'/0'/0/0 must start with "1" (0x00 prefix, same as BTC).
    #[test]
    fn bsv_p2pkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip44,
            &NetworkParams::BITCOIN_SV_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with('1'),
            "BSV P2PKH must start with '1', got {addr}"
        );
    }

    /// BSV does not support BIP-49.
    #[test]
    fn bsv_bip49_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip49,
            &NetworkParams::BITCOIN_SV_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-49 must be rejected on BSV");
    }

    /// XEC CashAddr — m/44'/1899'/0'/0/0 must start with "ecash:q".
    #[test]
    fn xec_cashaddr_p2pkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip44,
            &NetworkParams::ECASH_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with("ecash:q"),
            "XEC P2PKH must start with 'ecash:q', got {addr}"
        );
    }

    /// XEC does not support BIP-86.
    #[test]
    fn xec_bip86_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip86,
            &NetworkParams::ECASH_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-86 must be rejected on XEC");
    }

    // --- M7.5: Navio (NAVIO) tests ---

    /// NAVIO P2PKH — m/44'/130'/0'/0/0 must start with "N" (0x35 prefix).
    #[test]
    fn navio_p2pkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip44,
            &NetworkParams::NAVIO_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with('N'),
            "NAVIO P2PKH must start with 'N', got {addr}"
        );
    }

    /// NAVIO P2WPKH (BIP-84) — m/84'/130'/0'/0/0 must start with "navio1q".
    #[test]
    fn navio_p2wpkh_prefix() {
        let seed = seed_bytes();
        let addr = derive_address(
            &seed,
            Purpose::Bip84,
            &NetworkParams::NAVIO_MAINNET,
            0,
            0,
            0,
        )
        .unwrap();
        assert!(
            addr.starts_with("navio1q"),
            "NAVIO P2WPKH must start with 'navio1q', got {addr}"
        );
    }

    /// NAVIO does not support BIP-86 (taproot not deployed on Navio).
    #[test]
    fn navio_bip86_rejected() {
        let seed = seed_bytes();
        let result = derive_address(
            &seed,
            Purpose::Bip86,
            &NetworkParams::NAVIO_MAINNET,
            0,
            0,
            0,
        );
        assert!(result.is_err(), "BIP-86 must be rejected on NAVIO");
    }
}
