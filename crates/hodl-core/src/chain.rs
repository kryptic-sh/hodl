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
    BitcoinSv,
    ECash,
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
            ChainId::BitcoinSv => 236,
            ChainId::ECash => 1899,
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
            ChainId::BitcoinSv => "Bitcoin SV",
            ChainId::ECash => "eCash",
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
            ChainId::BitcoinSv => "BSV",
            ChainId::ECash => "XEC",
            ChainId::NavCoin => "NAV",
            ChainId::Ethereum => "ETH",
            ChainId::BscMainnet => "BNB",
            ChainId::Monero => "XMR",
        }
    }

    pub fn is_testnet(self) -> bool {
        matches!(self, ChainId::BitcoinTestnet)
    }
}
