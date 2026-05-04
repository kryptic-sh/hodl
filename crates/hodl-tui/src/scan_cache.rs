//! Per-wallet, encrypted on-disk cache of scan results.
//!
//! Layout: `<data_root>/cache/<wallet_name>/<ticker>.cache`. One file per
//! `ChainId` (named after `ChainId::ticker()` lowercased). Each file is an
//! encrypted blob (see `hodl_wallet::cache`) wrapping a TOML-serialised
//! `WalletScan`.
//!
//! ## Threat model
//!
//! - The cache key is derived from the unlocked seed (see
//!   `UnlockedWallet::cache_key`). Without the seed, the cache is opaque —
//!   so addresses + balances are not leaked to anyone with read access to
//!   the data dir but no password.
//! - The cache is hydrated into memory on unlock and dropped (with the
//!   key zeroized) on lock. Per-call decryption from disk is not on the
//!   hot path; lookups hit the in-memory `BTreeMap`.
//!
//! ## Concurrency
//!
//! All reads/writes happen on the UI thread. Worker threads send the
//! `WalletScan` back via the existing `ScanEvent::Done` channel; the
//! main thread is the only writer to the cache. No `Mutex` needed.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hodl_chain_bitcoin::WalletScan;
use hodl_core::ChainId;
use hodl_wallet::cache as wallet_cache;
use tracing::{debug, warn};
use zeroize::Zeroize;

/// Errors surfaced from cache I/O. The TUI logs and continues — a cache
/// miss is never fatal.
#[derive(Debug)]
pub enum CacheError {
    Io(std::io::Error),
    Crypto(String),
    Serde(String),
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Crypto(s) => write!(f, "crypto: {s}"),
            Self::Serde(s) => write!(f, "serde: {s}"),
        }
    }
}

impl std::error::Error for CacheError {}

impl From<std::io::Error> for CacheError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// In-memory + on-disk cache of `WalletScan` per chain, scoped to one wallet.
pub struct ScanCache {
    mem: HashMap<ChainId, Arc<WalletScan>>,
    dir: PathBuf,
    key: [u8; wallet_cache::KEY_LEN],
}

impl ScanCache {
    /// Open the cache for `wallet_name` under `data_root`. Eagerly loads any
    /// `*.cache` files found into memory (best-effort: corrupted entries
    /// are logged and skipped, never propagated as errors).
    ///
    /// `cache_key` should come from `UnlockedWallet::cache_key()` — the
    /// caller is responsible for producing it from the seed.
    pub fn open(
        data_root: &Path,
        wallet_name: &str,
        cache_key: [u8; wallet_cache::KEY_LEN],
    ) -> Self {
        let dir = data_root.join("cache").join(wallet_name);
        let mut me = Self {
            mem: HashMap::new(),
            dir,
            key: cache_key,
        };
        me.hydrate_from_disk();
        me
    }

    fn hydrate_from_disk(&mut self) {
        if !self.dir.exists() {
            return;
        }
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("scan cache: read_dir {} failed: {e}", self.dir.display());
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("cache") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => continue,
            };
            let chain = match chain_from_ticker(stem) {
                Some(c) => c,
                None => {
                    debug!("scan cache: skipping unknown ticker {stem:?}");
                    continue;
                }
            };
            match self.load_from_disk(chain) {
                Ok(scan) => {
                    self.mem.insert(chain, Arc::new(scan));
                    debug!("scan cache: hydrated {} from disk", chain.ticker());
                }
                Err(e) => {
                    warn!(
                        "scan cache: failed to load {} from disk: {e} ({})",
                        chain.ticker(),
                        path.display()
                    );
                }
            }
        }
    }

    fn path_for(&self, chain: ChainId) -> PathBuf {
        self.dir
            .join(format!("{}.cache", chain.ticker().to_ascii_lowercase()))
    }

    fn load_from_disk(&self, chain: ChainId) -> Result<WalletScan, CacheError> {
        let blob = std::fs::read(self.path_for(chain))?;
        let pt = wallet_cache::decrypt(&blob, &self.key)
            .map_err(|e| CacheError::Crypto(e.to_string()))?;
        let s = std::str::from_utf8(&pt).map_err(|e| CacheError::Serde(e.to_string()))?;
        toml::from_str::<WalletScan>(s).map_err(|e| CacheError::Serde(e.to_string()))
    }

    /// In-memory lookup. Returns a cheap-to-clone `Arc` so callers can hold
    /// a stable snapshot while the next scan runs.
    pub fn get(&self, chain: ChainId) -> Option<Arc<WalletScan>> {
        self.mem.get(&chain).cloned()
    }

    /// Store a fresh scan in memory + write it to disk encrypted. Failures
    /// to write are logged and swallowed — the in-memory copy is still
    /// updated so the UI behaves correctly.
    pub fn put(&mut self, chain: ChainId, scan: WalletScan) {
        let arc = Arc::new(scan);
        self.mem.insert(chain, Arc::clone(&arc));
        if let Err(e) = self.write_to_disk(chain, &arc) {
            warn!("scan cache: failed to persist {}: {e}", chain.ticker());
        }
    }

    fn write_to_disk(&self, chain: ChainId, scan: &WalletScan) -> Result<(), CacheError> {
        std::fs::create_dir_all(&self.dir)?;
        let toml_str = toml::to_string(scan).map_err(|e| CacheError::Serde(e.to_string()))?;
        let blob = wallet_cache::encrypt(toml_str.as_bytes(), &self.key)
            .map_err(|e| CacheError::Crypto(e.to_string()))?;
        let path = self.path_for(chain);
        let tmp = path.with_extension("cache.tmp");
        std::fs::write(&tmp, &blob)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Drop in-memory entries (e.g. on lock). The on-disk blobs remain.
    pub fn clear_memory(&mut self) {
        self.mem.clear();
    }
}

impl Drop for ScanCache {
    fn drop(&mut self) {
        self.key.zeroize();
        self.mem.clear();
    }
}

/// Reverse of `ChainId::ticker().to_ascii_lowercase()`. Keep in sync with
/// `ChainId` — adding a new variant requires adding an arm here too.
fn chain_from_ticker(s: &str) -> Option<ChainId> {
    match s {
        "btc" => Some(ChainId::Bitcoin),
        "tbtc" => Some(ChainId::BitcoinTestnet),
        "ltc" => Some(ChainId::Litecoin),
        "doge" => Some(ChainId::Dogecoin),
        "bch" => Some(ChainId::BitcoinCash),
        "nav" => Some(ChainId::NavCoin),
        "eth" => Some(ChainId::Ethereum),
        "bnb" => Some(ChainId::BscMainnet),
        "xmr" => Some(ChainId::Monero),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hodl_chain_bitcoin::{BalanceSplit, UsedAddress};
    use tempfile::TempDir;

    fn sample_scan() -> WalletScan {
        WalletScan {
            used: vec![UsedAddress {
                index: 0,
                change: 0,
                address: "bc1qexample".into(),
                balance: BalanceSplit {
                    confirmed: 12_345,
                    pending: 678,
                },
            }],
            total: BalanceSplit {
                confirmed: 12_345,
                pending: 678,
            },
            highest_index_receive: 0,
            highest_index_change: 0,
        }
    }

    #[test]
    fn round_trip_via_disk() {
        let tmp = TempDir::new().unwrap();
        let key = [9u8; 32];

        {
            let mut c = ScanCache::open(tmp.path(), "default", key);
            assert!(c.get(ChainId::Bitcoin).is_none());
            c.put(ChainId::Bitcoin, sample_scan());
            assert!(c.get(ChainId::Bitcoin).is_some());
        }

        // Re-open: should hydrate from disk.
        let c2 = ScanCache::open(tmp.path(), "default", key);
        let got = c2.get(ChainId::Bitcoin).expect("hydrated");
        assert_eq!(got.total.confirmed, 12_345);
        assert_eq!(got.used.len(), 1);
    }

    #[test]
    fn ticker_roundtrip_covers_all_chains() {
        for chain in [
            ChainId::Bitcoin,
            ChainId::BitcoinTestnet,
            ChainId::Litecoin,
            ChainId::Dogecoin,
            ChainId::BitcoinCash,
            ChainId::NavCoin,
            ChainId::Ethereum,
            ChainId::BscMainnet,
            ChainId::Monero,
        ] {
            let s = chain.ticker().to_ascii_lowercase();
            assert_eq!(
                chain_from_ticker(&s),
                Some(chain),
                "ticker {s} did not round-trip"
            );
        }
    }

    #[test]
    fn wrong_key_skips_silently() {
        let tmp = TempDir::new().unwrap();
        {
            let mut c = ScanCache::open(tmp.path(), "w", [1u8; 32]);
            c.put(ChainId::Bitcoin, sample_scan());
        }
        // Open with a different key; corrupted blob should be ignored.
        let c2 = ScanCache::open(tmp.path(), "w", [2u8; 32]);
        assert!(c2.get(ChainId::Bitcoin).is_none());
    }

    #[test]
    fn clear_memory_drops_in_mem_only() {
        let tmp = TempDir::new().unwrap();
        let key = [3u8; 32];
        let mut c = ScanCache::open(tmp.path(), "w", key);
        c.put(ChainId::Bitcoin, sample_scan());
        assert!(c.get(ChainId::Bitcoin).is_some());
        c.clear_memory();
        assert!(c.get(ChainId::Bitcoin).is_none());
        // Disk still intact: re-open re-hydrates.
        let c2 = ScanCache::open(tmp.path(), "w", key);
        assert!(c2.get(ChainId::Bitcoin).is_some());
    }
}
