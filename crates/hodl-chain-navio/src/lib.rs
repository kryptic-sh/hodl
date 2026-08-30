//! Navio (NAV) BLSCT support.
//!
//! Navio is a confidential-by-default chain: the supply is minted into BLSCT
//! outputs (BLS signatures + Confidential Transactions over BLS12-381), and a
//! wallet's receiving identity is a stealth *sub-address* derived from a
//! BLS12-381 key tree rather than a BIP-32 chain.
//!
//! This crate implements that identity layer against nav-io/navio-core:
//!
//! - [`eip2333`] — EIP-2333 BLS key derivation, Navio's variant.
//! - [`keys`] — the Navio key tree and stealth sub-address derivation.
//! - [`bech32_mod`] — the modified bech32 codec BLSCT addresses use.
//! - [`address`] — `nav1…` address encoding.
//!
//! Navio's *transparent* side is an ordinary Bitcoin-derivative script layer
//! and lives in `hodl-chain-bitcoin` alongside the other Electrum chains.
//!
//! Not implemented here: scanning for and spending BLSCT outputs. That needs
//! Bulletproofs+ amount recovery, set-membership proofs and BLS aggregate
//! signing; the crate README sketches what each would take and which Navio
//! ElectrumX methods the scan side would use.

pub mod address;
pub mod bech32_mod;
pub mod eip2333;
pub mod keys;
pub mod scalar;

pub use address::{MAINNET_HRP, TESTNET_HRP, decode_address, encode_address, looks_like_address};
pub use keys::{BlsctKeys, DoublePublicKey};
