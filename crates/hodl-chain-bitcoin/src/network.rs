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

    /// Litecoin mainnet. Supports BIP-44/49/84 (MWEB is post-v1, omitted here).
    pub const LITECOIN_MAINNET: Self = Self {
        chain_id: ChainId::Litecoin,
        bech32_hrp: "ltc",
        p2pkh_prefix: 0x30, // "L" addresses
        p2sh_prefix: 0x32,  // "M" addresses (post-soft-fork standard)
        default_electrum_port: 50001,
        default_electrum_tls_port: 50002,
    };

    /// Dogecoin mainnet.
    ///
    /// **Note:** bech32 / segwit is **not deployed** on the DOGE network. The
    /// `bech32_hrp` field is present for record symmetry only. `Purpose::Bip44`
    /// (legacy P2PKH) is the only valid derivation path for DOGE.
    pub const DOGECOIN_MAINNET: Self = Self {
        chain_id: ChainId::Dogecoin,
        bech32_hrp: "doge", // not deployed — field for symmetry only
        p2pkh_prefix: 0x1e, // "D" addresses
        p2sh_prefix: 0x16,  // "9" / "A" addresses
        default_electrum_port: 50001,
        default_electrum_tls_port: 50002,
    };

    /// Bitcoin Cash mainnet. Uses CashAddr encoding (not legacy base58check).
    ///
    /// The `bech32_hrp` field holds the CashAddr HRP (`"bitcoincash"`). The
    /// address codec (see `cashaddr` module) uses this HRP rather than the
    /// standard bech32 segwit encoder. BIP-49/84/86 are not deployed on BCH.
    pub const BITCOIN_CASH_MAINNET: Self = Self {
        chain_id: ChainId::BitcoinCash,
        bech32_hrp: "bitcoincash", // CashAddr HRP
        p2pkh_prefix: 0x00,        // legacy-compatible (rarely used)
        p2sh_prefix: 0x05,         // legacy-compatible
        default_electrum_port: 50001,
        default_electrum_tls_port: 50002,
    };

    /// Bitcoin SV mainnet.
    ///
    /// **Note:** bech32 / segwit is **not deployed** on BSV. The `bech32_hrp`
    /// field is present for record symmetry only. `Purpose::Bip44` (legacy
    /// P2PKH with 0x00 prefix, same as BTC) is the only valid path for BSV.
    pub const BITCOIN_SV_MAINNET: Self = Self {
        chain_id: ChainId::BitcoinSv,
        bech32_hrp: "bsv",  // not deployed — field for symmetry only
        p2pkh_prefix: 0x00, // same as BTC legacy
        p2sh_prefix: 0x05,
        default_electrum_port: 50001,
        default_electrum_tls_port: 50002,
    };

    /// eCash (XEC) mainnet. Uses CashAddr encoding with `"ecash"` HRP.
    ///
    /// The `bech32_hrp` field holds the CashAddr HRP (`"ecash"`). BIP-49/84/86
    /// are not deployed on XEC.
    pub const ECASH_MAINNET: Self = Self {
        chain_id: ChainId::ECash,
        bech32_hrp: "ecash", // CashAddr HRP
        p2pkh_prefix: 0x00,
        p2sh_prefix: 0x05,
        default_electrum_port: 50001,
        default_electrum_tls_port: 50002,
    };

    /// Navio (NAVIO) mainnet. Bitcoin-derivative chain with segwit support.
    ///
    /// Public NAVIO uses standard P2PKH + bech32 P2WPKH. BIP-44/49/84 are
    /// supported. BIP-86 (taproot) is not deployed on Navio. The xNAV blsCT
    /// shielded spend module is explicitly post-v1 and not implemented here.
    ///
    /// Prefix bytes carry over from legacy NavCoin; verify against a live
    /// Electrum-Navio server on first integration — adjust if needed.
    pub const NAVIO_MAINNET: Self = Self {
        chain_id: ChainId::Navio,
        bech32_hrp: "navio",
        p2pkh_prefix: 0x35, // "N" addresses (legacy NavCoin prefix)
        p2sh_prefix: 0x55,  // "X" addresses
        default_electrum_port: 50001,
        default_electrum_tls_port: 50002,
    };
}
