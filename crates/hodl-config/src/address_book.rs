//! Address book stored in `address_book.toml` alongside `config.toml`.

use std::path::{Path, PathBuf};

use hodl_core::ChainId;
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// A named recipient address.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub label: String,
    pub address: String,
    pub chain: ChainId,
    pub note: Option<String>,
}

/// The full address book — a flat list of contacts.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddressBook {
    #[serde(default)]
    pub entries: Vec<Contact>,
}

impl AddressBook {
    /// Load from `path`. Returns `Self::default()` if the file does not exist.
    /// Never writes to disk.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let src = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
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
                path: path.to_path_buf(),
                line,
                col,
                message: e.message().to_string(),
                snippet,
            }
        })
    }

    /// Persist to `path` (explicit save only — never called automatically).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Other(format!("serialize address book: {e}")))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        std::fs::write(path, content).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    /// Default path: `hjkl_config::config_dir("hodl")/address_book.toml`.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        hjkl_config::config_dir("hodl")
            .map(|d| d.join("address_book.toml"))
            .map_err(|e| ConfigError::Other(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("address_book.toml");
        let ab = AddressBook::load(&path).unwrap();
        assert!(ab.entries.is_empty());
    }

    #[test]
    fn round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("address_book.toml");

        let mut ab = AddressBook::default();
        ab.entries.push(Contact {
            label: "Alice".into(),
            address: "bc1qalicexyz".into(),
            chain: ChainId::Bitcoin,
            note: Some("test contact".into()),
        });
        ab.entries.push(Contact {
            label: "Bob".into(),
            address: "0xBob".into(),
            chain: ChainId::Ethereum,
            note: None,
        });

        ab.save(&path).unwrap();
        let loaded = AddressBook::load(&path).unwrap();
        assert_eq!(ab, loaded);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].label, "Alice");
        assert_eq!(loaded.entries[1].chain, ChainId::Ethereum);
    }
}
