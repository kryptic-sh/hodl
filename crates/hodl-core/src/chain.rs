//! Chain identifier enum.

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChainId {
    Bitcoin,
    BitcoinTestnet,
    Litecoin,
    Dogecoin,
    BitcoinCash,
    NavCoin,
    Ethereum,
    BscMainnet,
    Monero,
}

impl ChainId {
    pub fn slip44(self) -> u32 {
        match self {
            ChainId::Bitcoin => 0,
            ChainId::BitcoinTestnet => 1,
            ChainId::Litecoin => 2,
            ChainId::Dogecoin => 3,
            ChainId::BitcoinCash => 145,
            // NavCoin: SLIP-44 130 (NAV).
            ChainId::NavCoin => 130,
            ChainId::Ethereum => 60,
            // BSC reuses ETH derivation; coin_type 60 per BEP-44 convention
            ChainId::BscMainnet => 60,
            ChainId::Monero => 128,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ChainId::Bitcoin => "Bitcoin",
            ChainId::BitcoinTestnet => "Bitcoin Testnet",
            ChainId::Litecoin => "Litecoin",
            ChainId::Dogecoin => "Dogecoin",
            ChainId::BitcoinCash => "Bitcoin Cash",
            ChainId::NavCoin => "NavCoin",
            ChainId::Ethereum => "Ethereum",
            ChainId::BscMainnet => "BNB Smart Chain",
            ChainId::Monero => "Monero",
        }
    }

    pub fn ticker(self) -> &'static str {
        match self {
            ChainId::Bitcoin => "BTC",
            ChainId::BitcoinTestnet => "tBTC",
            ChainId::Litecoin => "LTC",
            ChainId::Dogecoin => "DOGE",
            ChainId::BitcoinCash => "BCH",
            ChainId::NavCoin => "NAV",
            ChainId::Ethereum => "ETH",
            ChainId::BscMainnet => "BNB",
            ChainId::Monero => "XMR",
        }
    }

    /// Decimal places for this chain's native coin (satoshis, wei, piconero, etc.).
    pub fn decimals(self) -> u32 {
        match self {
            ChainId::Bitcoin
            | ChainId::BitcoinTestnet
            | ChainId::Litecoin
            | ChainId::Dogecoin
            | ChainId::BitcoinCash
            | ChainId::NavCoin => 8,
            ChainId::Ethereum | ChainId::BscMainnet => 18,
            ChainId::Monero => 12,
        }
    }

    pub fn is_testnet(self) -> bool {
        matches!(self, ChainId::BitcoinTestnet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_btc_family() {
        assert_eq!(ChainId::Bitcoin.decimals(), 8);
        assert_eq!(ChainId::BitcoinTestnet.decimals(), 8);
        assert_eq!(ChainId::Litecoin.decimals(), 8);
        assert_eq!(ChainId::Dogecoin.decimals(), 8);
        assert_eq!(ChainId::BitcoinCash.decimals(), 8);
        assert_eq!(ChainId::NavCoin.decimals(), 8);
    }

    #[test]
    fn decimals_evm() {
        assert_eq!(ChainId::Ethereum.decimals(), 18);
        assert_eq!(ChainId::BscMainnet.decimals(), 18);
    }

    #[test]
    fn decimals_monero() {
        assert_eq!(ChainId::Monero.decimals(), 12);
    }
}
