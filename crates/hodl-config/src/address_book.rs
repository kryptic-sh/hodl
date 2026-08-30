//! Address book stored in `address_book.toml` alongside `config.toml`.

use std::path::{Path, PathBuf};

use hodl_core::ChainId;
use serde::{Deserialize, Serialize};

use crate::config::LEGACY_CHAIN_KEYS;
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
    #[serde(default, deserialize_with = "deserialize_entries")]
    pub entries: Vec<Contact>,
}

/// A contact as it appears on disk, before its chain name is resolved.
#[derive(Deserialize)]
struct RawContact {
    label: String,
    address: String,
    chain: ContactChain,
    note: Option<String>,
}

/// A contact's `chain` field: either a live chain or one hodl has retired.
enum ContactChain {
    Live(ChainId),
    Retired(String),
}

impl<'de> Deserialize<'de> for ContactChain {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error as _, IntoDeserializer};

        let name = String::deserialize(de)?;
        let parsed: Result<ChainId, serde::de::value::Error> =
            ChainId::deserialize(name.as_str().into_deserializer());
        match parsed {
            Ok(id) => Ok(ContactChain::Live(id)),
            Err(_) if LEGACY_CHAIN_KEYS.contains(&name.as_str()) => Ok(ContactChain::Retired(name)),
            Err(e) => Err(D::Error::custom(e)),
        }
    }
}

/// Drop contacts naming a retired chain, keeping the rest.
///
/// The stakes here are higher than in `config.rs`: `AddressBook::load` is
/// consumed with `unwrap_or_default()`, and the address-book screen persists
/// with `toml::to_string_pretty(self)` over the whole struct. So one contact
/// carrying a retired chain name would fail the parse, hand the screen an
/// empty book, and the next "add contact" would overwrite the file — taking
/// every unrelated contact with it. Dropping the one stale entry keeps the
/// rest recoverable.
fn deserialize_entries<'de, D>(de: D) -> Result<Vec<Contact>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<RawContact> = Vec::deserialize(de)?;
    let mut out = Vec::with_capacity(raw.len());
    for c in raw {
        match c.chain {
            ContactChain::Live(chain) => out.push(Contact {
                label: c.label,
                address: c.address,
                chain,
                note: c.note,
            }),
            ContactChain::Retired(name) => tracing::warn!(
                "address book: dropping contact {:?} — chain {name} is no longer supported",
                c.label
            ),
        }
    }
    Ok(out)
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
    /// A contact left behind by the NavCoin era must not take the rest of the
    /// book with it. `AddressBook::load` is consumed with
    /// `unwrap_or_default()` and the screen saves the whole struct back, so a
    /// hard parse error here means the next edit silently overwrites every
    /// surviving contact.
    #[test]
    fn retired_chain_contact_is_dropped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("address_book.toml");
        std::fs::write(
            &path,
            r#"
[[entries]]
label = "old nav friend"
address = "NZ1Wp9Q6yTFhBd2nBSSbDA8vX3xLmYyGkE"
chain = "nav-coin"

[[entries]]
label = "btc friend"
address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"
chain = "bitcoin"

[[entries]]
label = "xmr friend"
address = "4anything"
chain = "monero"
"#,
        )
        .unwrap();

        let book = AddressBook::load(&path).expect("a retired chain must not fail the load");
        let labels: Vec<&str> = book.entries.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["btc friend", "xmr friend"],
            "the retired contact should be dropped and every other kept"
        );
    }

    /// A genuinely unknown chain is still an error — a typo must not quietly
    /// delete a contact.
    #[test]
    fn unknown_chain_contact_still_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("address_book.toml");
        std::fs::write(
            &path,
            "[[entries]]\nlabel = \"x\"\naddress = \"y\"\nchain = \"not-a-chain\"\n",
        )
        .unwrap();
        assert!(AddressBook::load(&path).is_err());
    }

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
