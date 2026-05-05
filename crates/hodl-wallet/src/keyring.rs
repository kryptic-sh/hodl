//! OS-keyring helpers for vault password storage. Opt-in via the
//! `keyring` Cargo feature. With the feature off, all operations are
//! no-ops returning `Ok(None)` / `Ok(())`.
//!
//! Threat model: stores the user-typed vault password as a bearer
//! credential keyed by wallet name. Argon2id + ChaCha20-Poly1305
//! seed-at-rest crypto is untouched. See hodl#13 for the decision and
//! hodl#17 for the implementation arc.

use thiserror::Error;

#[cfg(feature = "keyring")]
const KEYRING_SERVICE: &str = "hodl-wallet";

/// Keyring backend error. Wraps the backend message as a plain string so
/// consumers don't need to depend on `keyring::Error` directly (that would
/// force the feature onto transitive consumers without the feature enabled).
#[derive(Debug, Error)]
pub enum KeyringError {
    #[error("keyring backend error: {0}")]
    Backend(String),
}

// ── feature ON ───────────────────────────────────────────────────────────────

/// Store the wallet password under `wallet_name`. No-op when the
/// `keyring` feature is off. Errors propagate from the keyring backend
/// (e.g. `PlatformFailure` on a Linux box without D-Bus) — callers
/// should treat any error as "fall back to password prompt".
#[cfg(feature = "keyring")]
pub fn store_password(wallet_name: &str, password: &[u8]) -> Result<(), KeyringError> {
    let pw_str = std::str::from_utf8(password)
        .map_err(|e| KeyringError::Backend(format!("password is not valid UTF-8: {e}")))?;
    let entry = keyring::Entry::new(KEYRING_SERVICE, wallet_name)
        .map_err(|e| KeyringError::Backend(e.to_string()))?;
    entry
        .set_password(pw_str)
        .map_err(|e| KeyringError::Backend(e.to_string()))
}

/// Load the wallet password. Returns `Ok(None)` when:
///   - the feature is off
///   - no entry exists (`NoEntry`)
///   - the platform has no usable keyring (`NoStorageAccess`,
///     `PlatformFailure`)
///
/// Other errors propagate.
#[cfg(feature = "keyring")]
pub fn load_password(wallet_name: &str) -> Result<Option<String>, KeyringError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, wallet_name)
        .map_err(|e| KeyringError::Backend(e.to_string()))?;
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry)
        | Err(keyring::Error::NoStorageAccess(_))
        | Err(keyring::Error::PlatformFailure(_)) => Ok(None),
        Err(e) => Err(KeyringError::Backend(e.to_string())),
    }
}

/// Delete the entry. `NoEntry` is treated as success (idempotent).
/// No-op when feature off.
#[cfg(feature = "keyring")]
pub fn delete_password(wallet_name: &str) -> Result<(), KeyringError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, wallet_name)
        .map_err(|e| KeyringError::Backend(e.to_string()))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(KeyringError::Backend(e.to_string())),
    }
}

// ── feature OFF (no-ops) ─────────────────────────────────────────────────────

#[cfg(not(feature = "keyring"))]
pub fn store_password(_wallet_name: &str, _password: &[u8]) -> Result<(), KeyringError> {
    Ok(())
}

#[cfg(not(feature = "keyring"))]
pub fn load_password(_wallet_name: &str) -> Result<Option<String>, KeyringError> {
    Ok(None)
}

#[cfg(not(feature = "keyring"))]
pub fn delete_password(_wallet_name: &str) -> Result<(), KeyringError> {
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(not(feature = "keyring"))]
mod tests {
    use super::*;

    #[test]
    fn load_password_no_feature_returns_none() {
        assert!(matches!(load_password("any-wallet"), Ok(None)));
    }

    #[test]
    fn store_password_no_feature_returns_ok() {
        assert!(store_password("any-wallet", b"secret").is_ok());
    }

    #[test]
    fn delete_password_no_feature_returns_ok() {
        assert!(delete_password("any-wallet").is_ok());
    }
}
