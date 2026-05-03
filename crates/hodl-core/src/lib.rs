//! Shared types and errors for hodl chain implementations.

pub mod address;
pub mod amount;
pub mod chain;
pub mod chain_trait;
pub mod error;
pub mod fee;
pub mod proxy;
pub mod tx;

pub use address::Address;
pub use amount::Amount;
pub use chain::ChainId;
pub use chain_trait::{Chain, PrivateKeyBytes};
pub use error::{Error, Result};
pub use fee::FeeRate;
pub use tx::{SendParams, SignedTx, TxId, TxRef, UnsignedTx};
