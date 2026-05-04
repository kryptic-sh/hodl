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

    /// NavCoin (NAV) mainnet. Bitcoin-derivative chain — legacy P2PKH + P2SH
    /// only.
    ///
    /// Bech32/segwit is **NOT deployed** in navcoin-core (verified against
    /// 7.0.3 and master 2026-05-04: `CChainParams` has no `Bech32HRP()` and
    /// no `bech32_hrp` field; the WitnessV0KeyHash encoder in `key_io.cpp`
    /// references `m_params.Bech32HRP()` but the method has no
    /// implementation in upstream chainparams.h). Only Bip44 (legacy P2PKH
    /// with prefix 0x35 → "N…" addresses) is supported. NavCoin's blsCT-
    /// based xNAV shielded spends are explicitly post-v1.
    ///
    /// Endpoints: Electrum-NavCoin servers (default ports 40001 / 40002 per
    /// upstream electrumx-NAV; verify on first integration).
    pub const NAVCOIN_MAINNET: Self = Self {
        chain_id: ChainId::NavCoin,
        // No bech32 in upstream — kept as empty for type compatibility, never
        // produces a valid encoding because Bip84/86 are gated off in
        // derive::validate_purpose.
        bech32_hrp: "",
        p2pkh_prefix: 0x35, // "N" addresses
        p2sh_prefix: 0x55,  // "X" addresses (matches navcoin-core SCRIPT_ADDRESS = 85)
        default_electrum_port: 40001,
        default_electrum_tls_port: 40002,
    };
}
