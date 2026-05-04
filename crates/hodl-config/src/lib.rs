//! TOML endpoint registry and config loader for hodl.
//!
//! Loading a missing file returns `Config::default()` in memory — no file is
//! ever written automatically.

mod address_book;
mod config;
mod error;
mod known_hosts;

pub use address_book::{AddressBook, Contact};
pub use config::{ChainConfig, Config, Endpoint, KdfPreset, LockConfig, TorConfig};
pub use error::ConfigError;
pub use known_hosts::KnownHosts;
