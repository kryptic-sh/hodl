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
}
