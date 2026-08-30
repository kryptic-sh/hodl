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

    /// Navio (NAV) mainnet — the chain that succeeded NavCoin.
    ///
    /// Navio is a fresh Bitcoin-derivative launched in 2026 (genesis
    /// `0af3c23a…`, `kernel/chainparams.cpp`) with BIP-141/143/147 segwit
    /// active from height 0 and taproot `ALWAYS_ACTIVE`, so unlike NavCoin the
    /// whole BIP-44/49/84/86 range is valid on the transparent side.
    ///
    /// **Address encoding follows the light-wallet stack, not
    /// `navio-core`'s `chainparams.cpp`.** Upstream's `CMainParams` still
    /// carries Bitcoin's inherited values (`PUBKEY_ADDRESS = 0`,
    /// `SCRIPT_ADDRESS = 5`, `bech32_hrp = "bc"`), which would make Navio
    /// addresses indistinguishable from Bitcoin's. The servers and wallets
    /// hodl actually talks to agree on a distinct set instead:
    /// nav-io/electrumx's `Navio` coin class (`P2PKH_VERBYTE = 0x35`,
    /// `P2SH_VERBYTES = 0x55`) and nav-io/navio-electrum's `BitcoinMainnet`
    /// (the same two, plus `SEGWIT_HRP = "nv"`) — and navio-core's own
    /// `delegation_tests.cpp` uses an `nv1…` placeholder. We follow those.
    ///
    /// Endpoints: Navio ElectrumX. The ports below are the documented
    /// defaults (and navio-electrum's `DEFAULT_PORTS`), but the one public
    /// mainnet server its `servers.json` lists answers on 50002 -- which is
    /// what the curated config endpoint uses. These two fields have no
    /// readers today; endpoints carry explicit ports.
    ///
    /// Note this covers Navio's *transparent* outputs only. Confidential
    /// (BLSCT) addresses are a different encoding entirely and live in
    /// `hodl-chain-navio`.
    pub const NAVIO_MAINNET: Self = Self {
        chain_id: ChainId::Navio,
        bech32_hrp: "nv",
        p2pkh_prefix: 0x35, // "N" addresses
        p2sh_prefix: 0x55,  // "b" addresses
        default_electrum_port: 40001,
        default_electrum_tls_port: 40002,
    };
}
