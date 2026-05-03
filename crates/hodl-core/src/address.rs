//! Address newtype — a tagged string. Validation is the chain crate's job.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::chain::ChainId;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address(String, ChainId);

impl Address {
    pub fn new(text: impl Into<String>, chain: ChainId) -> Self {
        Address(text.into(), chain)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn chain(&self) -> ChainId {
        self.1
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
