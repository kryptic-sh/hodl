//! Transaction types. Raw payloads stay opaque — chain crates own serialization.

use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::amount::Amount;
use crate::chain::ChainId;
use crate::fee::FeeRate;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TxId(pub String);

impl std::fmt::Display for TxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxRef {
    pub id: TxId,
    pub height: Option<u64>,
    pub time: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendParams {
    pub from: Address,
    pub to: Address,
    pub amount: Amount,
    pub fee: FeeRate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnsignedTx {
    pub chain: ChainId,
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedTx {
    pub chain: ChainId,
    pub raw: Vec<u8>,
}
