//! The `Chain` trait and opaque key type that chain crates implement.

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::address::Address;
use crate::amount::Amount;
use crate::chain::ChainId;
use crate::error::Result;
use crate::fee::FeeRate;
use crate::tx::{SignedTx, TxId, TxRef, UnsignedTx};

/// Opaque 32-byte private key. Zeroized on drop.
///
/// Chain crates interpret the bytes as appropriate for their key format
/// (secp256k1 scalar, ed25519 scalar, etc.).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PrivateKeyBytes(pub [u8; 32]);

impl std::fmt::Debug for PrivateKeyBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateKeyBytes").finish_non_exhaustive()
    }
}

/// Implemented by each chain family crate.
///
/// `derive` accepts `&[u8; 64]` — the raw BIP-39 seed bytes — so `hodl-core`
/// carries no dep on `hodl-wallet`. Call `seed.as_bytes()` at the call site to
/// cross the boundary.
pub trait Chain {
    fn id(&self) -> ChainId;
    fn slip44(&self) -> u32;

    /// Derive an address from the 64-byte BIP-39 seed at the given account/index.
    fn derive(&self, seed: &[u8; 64], account: u32, index: u32) -> Result<Address>;

    fn balance(&self, addr: &Address) -> Result<Amount>;
    fn history(&self, addr: &Address) -> Result<Vec<TxRef>>;
    fn estimate_fee(&self, target_blocks: u32) -> Result<FeeRate>;
    fn build_tx(&self, params: crate::tx::SendParams) -> Result<UnsignedTx>;
    fn sign(&self, tx: UnsignedTx, key: &PrivateKeyBytes) -> Result<SignedTx>;
    fn broadcast(&self, tx: SignedTx) -> Result<TxId>;
}
