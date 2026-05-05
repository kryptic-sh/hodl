//! Shared amount-formatting helpers for the TUI.

use hodl_core::ChainId;

/// Format a chain-tagged atomic-unit amount as a decimal coin string
/// (e.g. `1.23456789 BTC`, `0.001000 ETH`). Decimal width matches
/// `ChainId::decimals()`.
///
/// Note: takes u64. ETH wei requires u128 — when EVM gets multi-row
/// scans this helper will need widening alongside `BalanceSplit`.
pub fn format_amount(atoms: u64, chain: ChainId) -> String {
    let d = chain.decimals();
    let scale = 10u64.pow(d);
    let whole = atoms / scale;
    let frac = atoms % scale;
    let ticker = chain.ticker();
    format!("{whole}.{frac:0width$} {ticker}", width = d as usize)
}

/// Format an ERC-20 / BEP-20 token balance as a human-readable decimal string.
///
/// `atoms` is the raw `balanceOf` uint256 value (capped at u128).
/// `decimals` is the token's decimal places (e.g. 6 for USDC, 18 for DAI).
/// `symbol` is the display ticker appended to the result.
///
/// Examples:
/// - `format_token(1_000_000, 6, "USDC")` → `"1.000000 USDC"`
/// - `format_token(1_500_000_000_000_000_000, 18, "DAI")` → `"1.500000000000000000 DAI"`
pub fn format_token(atoms: u128, decimals: u8, symbol: &str) -> String {
    let scale = 10u128.pow(decimals as u32);
    let whole = atoms / scale;
    let frac = atoms % scale;
    format!("{whole}.{frac:0width$} {symbol}", width = decimals as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btc_standard() {
        assert_eq!(
            format_amount(123_456_789, ChainId::Bitcoin),
            "1.23456789 BTC"
        );
    }

    #[test]
    fn btc_zero_frac() {
        assert_eq!(
            format_amount(100_000_000, ChainId::Bitcoin),
            "1.00000000 BTC"
        );
    }

    #[test]
    fn eth_partial_wei() {
        assert_eq!(
            format_amount(1_000_000_000_000_000, ChainId::Ethereum),
            "0.001000000000000000 ETH"
        );
    }

    #[test]
    fn xmr_standard() {
        assert_eq!(
            format_amount(1_500_000_000_000, ChainId::Monero),
            "1.500000000000 XMR"
        );
    }

    #[test]
    fn zero_btc() {
        assert_eq!(format_amount(0, ChainId::Bitcoin), "0.00000000 BTC");
    }

    #[test]
    fn token_usdc_standard() {
        assert_eq!(format_token(1_000_000, 6, "USDC"), "1.000000 USDC");
    }

    #[test]
    fn token_dai_standard() {
        assert_eq!(
            format_token(1_500_000_000_000_000_000, 18, "DAI"),
            "1.500000000000000000 DAI"
        );
    }

    #[test]
    fn token_zero() {
        assert_eq!(format_token(0, 6, "USDC"), "0.000000 USDC");
    }

    #[test]
    fn token_large_whole() {
        // 1_234_000_000 atoms with 6 decimals = 1234.000000
        assert_eq!(format_token(1_234_000_000, 6, "USDC"), "1234.000000 USDC");
    }
}
