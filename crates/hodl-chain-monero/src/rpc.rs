//! Monero daemon JSON-RPC client for transaction broadcast.
//!
//! Only /sendrawtransaction is implemented — M7 covers receive + balance.
//! Full send requires ring signatures and bulletproofs (post-v1).

use hodl_core::error::{Error, Result};
use hodl_core::tx::TxId;
use serde::{Deserialize, Serialize};

/// Swappable transport for testing without a real daemon.
pub trait RpcTransport: Send + Sync {
    fn post(&self, path: &str, body: &str) -> Result<String>;
}

struct UreqTransport {
    agent: ureq::Agent,
    base_url: String,
}

impl RpcTransport for UreqTransport {
    fn post(&self, path: &str, body: &str) -> Result<String> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let resp = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(body)
            .map_err(|e| Error::Network(format!("daemon post {path}: {e}")))?;
        resp.into_string()
            .map_err(|e| Error::Network(format!("daemon read {path}: {e}")))
    }
}

/// Monero daemon JSON-RPC client (own-node only — no default endpoint).
pub struct DaemonRpcClient {
    transport: Box<dyn RpcTransport>,
}

impl DaemonRpcClient {
    pub fn new(base_url: String) -> Self {
        let agent = ureq::AgentBuilder::new().build();
        Self {
            transport: Box::new(UreqTransport { agent, base_url }),
        }
    }

    /// Build with a SOCKS5 proxy (e.g., Tor: `socks5://127.0.0.1:9050`).
    pub fn with_socks5(base_url: String, proxy: &str) -> hodl_core::error::Result<Self> {
        use hodl_core::error::Error;
        let proxy =
            ureq::Proxy::new(proxy).map_err(|e| Error::Endpoint(format!("invalid proxy: {e}")))?;
        let agent = ureq::AgentBuilder::new().proxy(proxy).build();
        Ok(Self {
            transport: Box::new(UreqTransport { agent, base_url }),
        })
    }

    /// Build with a custom transport (for tests).
    pub fn with_transport(transport: Box<dyn RpcTransport>) -> Self {
        Self { transport }
    }

    /// Broadcast a raw transaction to the daemon.
    ///
    /// `tx_hex` — the fully signed transaction encoded as lowercase hex.
    /// Returns the transaction hash reported by the daemon on success.
    pub fn send_raw_transaction(&self, tx_hex: &str) -> Result<TxId> {
        let req = SendRawTxRequest {
            tx_as_hex: tx_hex,
            do_not_relay: false,
        };
        let body = serde_json::to_string(&req)
            .map_err(|e| Error::Chain(format!("serialize sendrawtransaction: {e}")))?;
        let raw = self.transport.post("/sendrawtransaction", &body)?;
        let resp: SendRawTxResponse = serde_json::from_str(&raw)
            .map_err(|e| Error::Network(format!("parse sendrawtransaction: {e}")))?;
        if resp.status != "OK" {
            return Err(Error::Chain(format!(
                "sendrawtransaction failed: {}",
                resp.reason.as_deref().unwrap_or("unknown error")
            )));
        }
        // The daemon returns the tx hash in the response; fall back to an
        // echo of the input hex as a best-effort TxId if omitted.
        let txid = resp
            .tx_hash
            .unwrap_or_else(|| format!("broadcast:{tx_hex}"));
        Ok(TxId(txid))
    }
}

#[derive(Serialize)]
struct SendRawTxRequest<'a> {
    tx_as_hex: &'a str,
    do_not_relay: bool,
}

#[derive(Deserialize)]
struct SendRawTxResponse {
    status: String,
    reason: Option<String>,
    tx_hash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTransport {
        response: String,
    }

    impl RpcTransport for MockTransport {
        fn post(&self, _path: &str, _body: &str) -> Result<String> {
            Ok(self.response.clone())
        }
    }

    #[test]
    fn send_raw_transaction_ok() {
        let mock = MockTransport {
            response: r#"{"status":"OK","tx_hash":"deadbeef01234567"}"#.to_string(),
        };
        let client = DaemonRpcClient::with_transport(Box::new(mock));
        let txid = client.send_raw_transaction("cafecafe").unwrap();
        assert_eq!(txid.0, "deadbeef01234567");
    }

    #[test]
    fn send_raw_transaction_error() {
        let mock = MockTransport {
            response: r#"{"status":"Failed","reason":"rejected by pool"}"#.to_string(),
        };
        let client = DaemonRpcClient::with_transport(Box::new(mock));
        let err = client.send_raw_transaction("cafecafe").unwrap_err();
        assert!(
            err.to_string().contains("rejected by pool"),
            "unexpected error: {err}"
        );
    }
}
