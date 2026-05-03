//! Amount in atomic units (satoshis, wei, etc.) tagged with a chain.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::chain::ChainId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Amount {
    atoms: u128,
    chain: ChainId,
}

impl Amount {
    pub fn from_atoms(atoms: u128, chain: ChainId) -> Self {
        Amount { atoms, chain }
    }

    pub fn atoms(self) -> u128 {
        self.atoms
    }

    pub fn chain(self) -> ChainId {
        self.chain
    }

    pub fn saturating_add(self, rhs: Amount) -> Amount {
        Amount {
            atoms: self.atoms.saturating_add(rhs.atoms),
            chain: self.chain,
        }
    }

    pub fn saturating_sub(self, rhs: Amount) -> Amount {
        Amount {
            atoms: self.atoms.saturating_sub(rhs.atoms),
            chain: self.chain,
        }
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.atoms, self.chain.ticker())
    }
}
