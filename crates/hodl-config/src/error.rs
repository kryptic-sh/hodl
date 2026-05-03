//! Config error type.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML parse error in {path} at line {line}, col {col}: {message}\n  {snippet}")]
    Parse {
        path: PathBuf,
        line: usize,
        col: usize,
        message: String,
        snippet: String,
    },
    #[error("config error: {0}")]
    Other(String),
}
