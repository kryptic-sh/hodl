//! Fee rate types.

use serde::{Deserialize, Serialize};

use crate::chain::ChainId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FeeRate {
    SatsPerVbyte {
        sats: u64,
        chain: ChainId,
    },
    Gwei {
        max_fee: u64,
        max_priority: u64,
        chain: ChainId,
    },
    Custom {
        value: u64,
        chain: ChainId,
    },
}

impl FeeRate {
    pub fn chain(self) -> ChainId {
        match self {
            FeeRate::SatsPerVbyte { chain, .. } => chain,
            FeeRate::Gwei { chain, .. } => chain,
            FeeRate::Custom { chain, .. } => chain,
        }
    }
}
