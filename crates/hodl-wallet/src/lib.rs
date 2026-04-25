//! Key management, address derivation, signing.
//!
//! BIP-39 seeds and BIP-32 derivation live here. All sensitive material
//! is wrapped in `Zeroizing` so it scrubs on drop.

pub struct Wallet {
    // TODO: encrypted seed, derived accounts, address cache.
}
