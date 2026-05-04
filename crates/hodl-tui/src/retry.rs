//! Shared retry primitives for network-bound background threads.
//!
//! Used by the scan worker (`account.rs`) and the send workers (`send.rs`)
//! to classify `hodl_core` errors as retryable or fatal and to drive a
//! uniform retry loop with a consistent attempt ceiling.

use hodl_core::ChainId;

/// Maximum number of attempts for any network operation.
///
/// Each retry calls `ActiveChain::from_chain_id` again, which re-shuffles
/// the endpoint list via `try_endpoints`, so successive attempts contact
/// different servers.
pub const MAX_ATTEMPTS: u32 = 3;

/// Outcome of one attempt in a retry loop.
///
/// Used by the scan worker (streaming via channels). Send workers use the
/// parallel `SendAttempt<T>` enum defined locally in `send.rs` so they can
/// carry a return value on success.
pub enum AttemptResult {
    /// Operation completed successfully — caller should stop retrying.
    Done,
    /// Non-retryable error. Surface to the user and stop.
    Fatal(String),
    /// Retryable error (network/IO). Outer loop should reconnect and retry.
    Retry(String),
    /// TOFU cert mismatch — not retryable, but distinct from Fatal so the
    /// caller can surface an interactive remediation prompt instead of a
    /// plain error message. Carries the host and both fingerprints.
    TofuMismatch {
        host: String,
        pinned: String,
        presented: String,
    },
}

/// Map a `hodl_core::Error` to an [`AttemptResult`] via its retry-ability.
///
/// Classification rules:
/// - `TofuMismatch` → `AttemptResult::TofuMismatch` — security signal; the
///   caller surfaces an interactive remediation overlay instead of a plain
///   error. Retrying would hide a cert-change, so it is never retried.
/// - `Network` / `Io` / `Endpoint` → retryable (transient connectivity issues).
/// - `Codec` / `Chain` / `Config` → fatal (bad data or misconfiguration;
///   reconnecting to a different endpoint won't help).
pub fn classify(chain: ChainId, stage: &str, e: hodl_core::error::Error) -> AttemptResult {
    use hodl_core::error::Error;
    let msg = format!("{}: {stage}: {e}", chain.display_name());
    match e {
        Error::TofuMismatch {
            host,
            pinned,
            presented,
        } => AttemptResult::TofuMismatch {
            host,
            pinned,
            presented,
        },
        Error::Network(_) | Error::Io(_) | Error::Endpoint(_) => AttemptResult::Retry(msg),
        Error::Codec(_) | Error::Chain(_) | Error::Config(_) => AttemptResult::Fatal(msg),
    }
}
