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
}
