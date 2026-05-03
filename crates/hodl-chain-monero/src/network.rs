//! Per-network constants for Monero. Mainnet only for v1.

use hodl_core::ChainId;

pub struct NetworkParams {
    pub chain_id: ChainId,
    /// Monero standard address network prefix byte.
    /// Mainnet = 18, Stagenet = 24, Testnet = 53.
    pub address_prefix: u8,
    pub display_name: &'static str,
}

impl NetworkParams {
    pub const MAINNET: Self = Self {
        chain_id: ChainId::Monero,
        address_prefix: 18,
        display_name: "Monero",
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_prefix() {
        assert_eq!(NetworkParams::MAINNET.address_prefix, 18);
    }

    #[test]
    fn mainnet_chain_id() {
        assert_eq!(NetworkParams::MAINNET.chain_id, ChainId::Monero);
    }

    #[test]
    fn mainnet_slip44() {
        assert_eq!(NetworkParams::MAINNET.chain_id.slip44(), 128);
    }
}
