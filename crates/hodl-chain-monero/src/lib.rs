//! Monero chain implementation for hodl.
//!
//! Covers M7 scope: Ledger-compatible BIP-39 key derivation, view-key sync
//! via open-monero-server LWS protocol, and broadcast via daemon JSON-RPC.
//!
//! WARNING: Monero key derivation here uses the Ledger-compatible BIP-39
//! scheme, not Monero's native 25-word Electrum-style mnemonic.
//! Restoring this wallet on a non-Ledger Monero wallet will produce a
//! DIFFERENT address. Match: Cake Wallet, Monerujo (Ledger-seed mode),
//! Ledger Live. Do NOT match: monero-wallet-cli, GUI wallet, MyMonero
//! (those use the 25-word seed format directly).
//!
//! Ring signatures, bulletproofs, stealth addresses, and subaddresses are
//! explicitly out of scope for M7. build_tx / sign return clear errors.
//! Native 25-word Monero seed import is post-v1.

pub mod address;
pub mod chain;
pub mod derive;
pub mod lws;
pub mod network;
pub mod rpc;

pub use chain::MoneroChain;
pub use derive::{MoneroKeys, derive_keys, pubkey_from_secret, standard_address};
pub use lws::LwsClient;
pub use network::NetworkParams;
pub use rpc::DaemonRpcClient;
