use hodl_core::ChainId;

/// Per-network constants for Ethereum-family chains.
///
/// BNB Smart Chain (chain_id 56) will add its constant here in M5.
#[derive(Clone, Debug)]
pub struct NetworkParams {
    pub chain_id: ChainId,
    /// EIP-155 chain id used in transaction signing.
    pub eip155_chain_id: u64,
    pub display_name: &'static str,
}

impl NetworkParams {
    pub const ETHEREUM_MAINNET: Self = Self {
        chain_id: ChainId::Ethereum,
        eip155_chain_id: 1,
        display_name: "Ethereum",
    };

    /// BNB Smart Chain mainnet (EIP-155 chain id 56).
    ///
    /// Reuses the BIP-44 coin_type 60 (`m/44'/60'/account'/0/index`) per
    /// BEP-44 convention — identical key + address derivation as Ethereum.
    /// No default RPC endpoints: configure via `Config.chains`.
    pub const BSC_MAINNET: Self = Self {
        chain_id: ChainId::BscMainnet,
        eip155_chain_id: 56,
        display_name: "BNB Smart Chain",
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bsc_mainnet_chain_id() {
        assert_eq!(NetworkParams::BSC_MAINNET.chain_id, ChainId::BscMainnet);
    }

    #[test]
    fn bsc_mainnet_eip155_chain_id() {
        assert_eq!(NetworkParams::BSC_MAINNET.eip155_chain_id, 56);
    }

    #[test]
    fn bsc_mainnet_slip44_is_60() {
        // BEP-44: BSC reuses ETH coin_type 60.
        assert_eq!(NetworkParams::BSC_MAINNET.chain_id.slip44(), 60);
    }
}
