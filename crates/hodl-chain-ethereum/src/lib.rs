//! Ethereum chain implementation for hodl.
//!
//! Covers M4 scope: JSON-RPC client (ureq), EIP-55 addresses,
//! BIP-44 derivation (`m/44'/60'/account'/0/index`), hand-rolled RLP,
//! EIP-1559 (type 0x02) transaction build + sign + broadcast.
//!
//! ERC-20 / ERC-721 are explicitly out of scope for v1.

pub mod address;
pub mod chain;
pub mod derive;
pub mod network;
pub mod rpc;
pub mod tx;

pub use chain::EthereumChain;
pub use network::NetworkParams;
pub use rpc::EthRpcClient;
