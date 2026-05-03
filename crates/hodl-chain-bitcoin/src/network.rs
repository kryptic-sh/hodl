use hodl_core::ChainId;

/// Per-network constants that parameterize the Bitcoin-family implementation.
///
/// Adding a new Bitcoin-derivative chain (LTC, DOGE, etc.) requires only a new
/// `NetworkParams` constant — no code changes to the address encoders or chain
/// logic.
#[derive(Clone, Debug)]
pub struct NetworkParams {
    pub chain_id: ChainId,
    /// Bech32 human-readable part: "bc" for mainnet, "tb" for testnet.
    pub bech32_hrp: &'static str,
    /// Version byte prepended before the key hash in P2PKH base58check.
    /// 0x00 on mainnet, 0x6f on testnet.
    pub p2pkh_prefix: u8,
    /// Version byte for P2SH base58check.
    /// 0x05 on mainnet, 0xc4 on testnet.
    pub p2sh_prefix: u8,
    pub default_electrum_port: u16,
    pub default_electrum_tls_port: u16,
}

impl NetworkParams {
    pub const BITCOIN_MAINNET: Self = Self {
        chain_id: ChainId::Bitcoin,
        bech32_hrp: "bc",
        p2pkh_prefix: 0x00,
        p2sh_prefix: 0x05,
        default_electrum_port: 50001,
        default_electrum_tls_port: 50002,
    };

    pub const BITCOIN_TESTNET: Self = Self {
        chain_id: ChainId::BitcoinTestnet,
        bech32_hrp: "tb",
        p2pkh_prefix: 0x6f,
        p2sh_prefix: 0xc4,
        default_electrum_port: 60001,
        default_electrum_tls_port: 60002,
    };
}
