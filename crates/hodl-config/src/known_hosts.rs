//! TOFU cert pinning store — `<data_root>/known_hosts.toml`.
//!
//! Maps `host:port` strings to lowercase-hex SHA-256 fingerprints of the
//! server's leaf TLS certificate DER bytes. Loaded once at startup; saved
//! atomically on the first new pin and on every subsequent update.
//!
//! No file is written until the first `save()` call — in keeping with the
//! project-wide policy of never auto-creating default config files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// Pinned SHA-256 fingerprints of Electrum server leaf certs, indexed by
/// `host:port` string. Loaded from `<data_root>/known_hosts.toml` at startup;
/// saved on every modification.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownHosts {
    /// `host:port` → lowercase hex SHA-256 of the leaf cert DER bytes.
    #[serde(default)]
    pub entries: BTreeMap<String, String>,
}

impl KnownHosts {
    /// Load from `<data_root>/known_hosts.toml`.
    ///
    /// Returns `Self::default()` if the file is missing — no auto-creation of
    /// an empty file (write happens only on the first `save()` call).
    pub fn load(data_root: &Path) -> Result<Self, ConfigError> {
        let path = Self::default_path(data_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let src = std::fs::read_to_string(&path).map_err(|e| ConfigError::Io {
            path: path.clone(),
            source: e,
        })?;
        toml::from_str::<Self>(&src).map_err(|e| {
            let span = e.span().unwrap_or(0..0);
            let before = &src[..span.start.min(src.len())];
            let line = before.lines().count().max(1);
            let col = before
                .rfind('\n')
                .map(|p| span.start - p)
                .unwrap_or(span.start + 1);
            let snippet = src
                .lines()
                .nth(line.saturating_sub(1))
                .unwrap_or("")
                .to_string();
            ConfigError::Parse {
                path,
                line,
                col,
                message: e.message().to_string(),
                snippet,
            }
        })
    }

    /// Persist atomically (temp-file + rename) to `<data_root>/known_hosts.toml`.
    pub fn save(&self, data_root: &Path) -> Result<(), ConfigError> {
        let path = Self::default_path(data_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Other(format!("serialize known_hosts: {e}")))?;

        // Atomic write: write to a temp file in the same directory, then rename.
        let tmp_path = path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, &content).map_err(|e| ConfigError::Io {
            path: tmp_path.clone(),
            source: e,
        })?;
        std::fs::rename(&tmp_path, &path).map_err(|e| ConfigError::Io {
            path: path.clone(),
            source: e,
        })?;
        Ok(())
    }

    /// Default path: `<data_root>/known_hosts.toml`.
    pub fn default_path(data_root: &Path) -> PathBuf {
        data_root.join("known_hosts.toml")
    }

    /// Look up a pinned fingerprint for `host:port`.
    pub fn get(&self, host_port: &str) -> Option<&str> {
        self.entries.get(host_port).map(|s| s.as_str())
    }

    /// Insert or update a `host:port → fingerprint` mapping.
    pub fn insert(&mut self, host_port: impl Into<String>, fingerprint: impl Into<String>) {
        self.entries.insert(host_port.into(), fingerprint.into());
    }

    /// Remove an entry, returning the old fingerprint if one existed.
    pub fn remove(&mut self, host_port: &str) -> Option<String> {
        self.entries.remove(host_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let kh = KnownHosts::load(tmp.path()).unwrap();
        assert!(kh.entries.is_empty());
    }

    #[test]
    fn round_trip() {
        let tmp = TempDir::new().unwrap();

        let mut kh = KnownHosts::default();
        kh.insert("electrum.example.com:50002", "aabbcc");
        kh.insert("electrum2.example.com:50002", "ddeeff");

        kh.save(tmp.path()).unwrap();

        let loaded = KnownHosts::load(tmp.path()).unwrap();
        assert_eq!(kh, loaded);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.get("electrum.example.com:50002"), Some("aabbcc"));
        assert_eq!(loaded.get("electrum2.example.com:50002"), Some("ddeeff"));
    }

    #[test]
    fn remove_entry() {
        let mut kh = KnownHosts::default();
        kh.insert("host:50002", "deadbeef");
        assert_eq!(kh.get("host:50002"), Some("deadbeef"));
        let removed = kh.remove("host:50002");
        assert_eq!(removed, Some("deadbeef".to_string()));
        assert!(kh.get("host:50002").is_none());
    }

    #[test]
    fn no_file_written_on_load() {
        let tmp = TempDir::new().unwrap();
        let path = KnownHosts::default_path(tmp.path());
        KnownHosts::load(tmp.path()).unwrap();
        assert!(
            !path.exists(),
            "load must not create the file if it is missing"
        );
    }
}
