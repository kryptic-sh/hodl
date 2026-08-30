//! Navio's BLSCT key tree and stealth sub-address derivation.
//!
//! Ports `blsct::KeyMan::SetHDSeed`, `blsct::SubAddress` and
//! `blsct/wallet/helpers.cpp` from nav-io/navio-core.
//!
//! ```text
//! master ──child(130) ──┬── transaction(0) ──┬── view(0)
//!                       │                    └── spend(1)
//!                       ├── blinding(1)
//!                       └── token(2)
//! ```
//!
//! A receive address is a *sub-address*: a stealth pair derived from the view
//! key, the spend public key, and an `(account, index)` identifier.

use bls12_381::{G1Affine, G1Projective, Scalar};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::eip2333::{derive_child_sk, derive_master_sk};
use crate::scalar::{scalar_from_be_bytes, scalar_to_be_bytes};

/// Child index of the Navio branch under the BLS master key. Matches NAV's
/// SLIP-44 coin type, though this is EIP-2333 derivation, not BIP-32.
const NAVIO_CHILD_INDEX: u32 = 130;

/// Domain separator hashed into every sub-address.
///
/// Upstream writes `std::string subAddressHeader = "SubAddress\0"`, whose
/// `const char*` constructor stops at the NUL — so the ten bytes below are
/// what actually reaches the hash, despite the literal's trailing escape.
const SUBADDRESS_HEADER: &[u8] = b"SubAddress";

/// Serialized length of a `DoublePublicKey`: two compressed G1 points.
pub const DOUBLE_PUBKEY_SIZE: usize = 96;

/// A BLSCT destination: a view key point and a spend key point.
///
/// Named after upstream's `blsct::DoublePublicKey`. Ordering matters — the
/// wire encoding is `view || spend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoublePublicKey {
    pub view: G1Affine,
    pub spend: G1Affine,
}

impl DoublePublicKey {
    pub fn to_bytes(self) -> [u8; DOUBLE_PUBKEY_SIZE] {
        let mut out = [0u8; DOUBLE_PUBKEY_SIZE];
        out[..48].copy_from_slice(&self.view.to_compressed());
        out[48..].copy_from_slice(&self.spend.to_compressed());
        out
    }

    /// Parse the 96-byte `view || spend` encoding.
    ///
    /// Rejects the identity in either half. Upstream's `IsValid()` only checks
    /// that both points deserialize, but an output built for an identity key
    /// has publicly derivable ownership keys — i.e. anyone can spend it — so
    /// upstream added `HasNonIdentityKeys()` for every path that turns a
    /// user-supplied destination into an output. Parsing an address is exactly
    /// such a path, so the check lives here.
    pub fn from_bytes(bytes: &[u8; DOUBLE_PUBKEY_SIZE]) -> Option<Self> {
        let view = point_from_compressed(&bytes[..48])?;
        let spend = point_from_compressed(&bytes[48..])?;
        if bool::from(view.is_identity()) || bool::from(spend.is_identity()) {
            return None;
        }
        Some(Self { view, spend })
    }
}

fn point_from_compressed(bytes: &[u8]) -> Option<G1Affine> {
    let arr: [u8; 48] = bytes.try_into().ok()?;
    // `from_compressed` enforces both curve membership and prime-order
    // subgroup membership, which is what `MclG1Point::SetVch` checks.
    Option::from(G1Affine::from_compressed(&arr))
}

/// The secret half of a Navio BLSCT wallet.
///
/// Zeroized on drop. `spend_public` is kept alongside so sub-address
/// derivation — which needs only the view key and the spend *public* key —
/// does not re-derive the point each time.
pub struct BlsctKeys {
    view: Scalar,
    spend: Scalar,
    blinding: Scalar,
    token: Scalar,
    spend_public: G1Affine,
}

/// `bls12_381::Scalar` is `Copy` and implements no `Zeroize`, so wipe its
/// limbs with a volatile write the optimizer is not allowed to elide.
fn wipe_scalar(s: &mut Scalar) {
    unsafe { std::ptr::write_volatile(s, Scalar::from(0u64)) };
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

impl Drop for BlsctKeys {
    fn drop(&mut self) {
        wipe_scalar(&mut self.view);
        wipe_scalar(&mut self.spend);
        wipe_scalar(&mut self.blinding);
        wipe_scalar(&mut self.token);
    }
}

impl std::fmt::Debug for BlsctKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlsctKeys").finish_non_exhaustive()
    }
}

impl BlsctKeys {
    /// Build the key tree from an already-derived master scalar.
    pub fn from_master_scalar(master: &Scalar) -> Self {
        let mut child = derive_child_sk(master, NAVIO_CHILD_INDEX);
        let mut transaction = derive_child_sk(&child, 0);
        let blinding = derive_child_sk(&child, 1);
        let token = derive_child_sk(&child, 2);
        let view = derive_child_sk(&transaction, 0);
        let spend = derive_child_sk(&transaction, 1);
        // The two interior nodes are as sensitive as the leaves they produced.
        wipe_scalar(&mut child);
        wipe_scalar(&mut transaction);
        let spend_public = G1Affine::from(G1Projective::generator() * spend);
        Self {
            view,
            spend,
            blinding,
            token,
            spend_public,
        }
    }

    /// Build the key tree from the 64-byte BIP-39 seed.
    ///
    /// This is navio-core's mnemonic-**with-passphrase** path
    /// (`SetupMnemonicFromEntropy` → `derive_master_SK(MnemonicToSeed(...))`).
    /// A navio-core or navio-electrum wallet restored from the same mnemonic
    /// with an *empty* passphrase derives from the raw 32-byte BIP-39 entropy
    /// instead and will show different addresses — see
    /// [`from_bip39_entropy`](Self::from_bip39_entropy). hodl's vault stores
    /// only the stretched seed, so this is the path it can offer.
    pub fn from_bip39_seed(seed: &[u8; 64]) -> Self {
        let mut master = derive_master_sk(seed).expect("64-byte seed is long enough");
        let keys = Self::from_master_scalar(&master);
        wipe_scalar(&mut master);
        keys
    }

    /// Build the key tree from raw BIP-39 entropy — navio-core's
    /// empty-passphrase path.
    ///
    /// Returns `None` for entropy shorter than 32 bytes, which EIP-2333
    /// rejects.
    pub fn from_bip39_entropy(entropy: &[u8]) -> Option<Self> {
        let mut master = derive_master_sk(entropy)?;
        let keys = Self::from_master_scalar(&master);
        wipe_scalar(&mut master);
        Some(keys)
    }

    pub fn view_key(&self) -> &Scalar {
        &self.view
    }

    pub fn spend_key(&self) -> &Scalar {
        &self.spend
    }

    pub fn blinding_key(&self) -> &Scalar {
        &self.blinding
    }

    pub fn token_key(&self) -> &Scalar {
        &self.token
    }

    pub fn spend_public_key(&self) -> &G1Affine {
        &self.spend_public
    }

    /// Derive the sub-address at `(account, index)`.
    pub fn sub_address(&self, account: i64, index: u64) -> DoublePublicKey {
        sub_address(&self.view, &self.spend_public, account, index)
    }
}

/// SHA-256d, Bitcoin Core's `HashWriter::GetHash`.
fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    Sha256::digest(first).into()
}

/// Derive a stealth sub-address from the view key and spend public key.
///
/// ```text
/// m = Hs(header || a || account || index)
/// M = m * G
/// D = B + M          (the spend half)
/// C = a * D          (the view half)
/// ```
///
/// The preimage is Bitcoin Core's serialization: a `std::vector<unsigned char>`
/// carries a CompactSize length prefix, a `PrivateKey` writes its 32 raw
/// big-endian bytes, and the two integers are little-endian.
pub fn sub_address(
    view_key: &Scalar,
    spend_public_key: &G1Affine,
    account: i64,
    index: u64,
) -> DoublePublicKey {
    let mut preimage = Vec::with_capacity(1 + SUBADDRESS_HEADER.len() + 32 + 8 + 8);
    // CompactSize for a length below 253 is a single byte.
    preimage.push(SUBADDRESS_HEADER.len() as u8);
    preimage.extend_from_slice(SUBADDRESS_HEADER);
    let mut view_be = scalar_to_be_bytes(view_key);
    preimage.extend_from_slice(&view_be);
    preimage.extend_from_slice(&account.to_le_bytes());
    preimage.extend_from_slice(&index.to_le_bytes());

    let m = scalar_from_be_bytes(&sha256d(&preimage));
    preimage.zeroize();
    view_be.zeroize();

    let d = G1Projective::generator() * m + G1Projective::from(spend_public_key);
    let c = d * view_key;
    DoublePublicKey {
        view: G1Affine::from(c),
        spend: G1Affine::from(d),
    }
}
