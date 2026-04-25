//! TOML config loading for hodl.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub rpc: RpcConfig,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub bitcoin: Option<String>,
    pub ethereum: Option<String>,
}
