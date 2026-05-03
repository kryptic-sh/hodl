//! open-monero-server Light Wallet Server (LWS) protocol client.
//!
//! Endpoints (all POST with JSON body):
//!   /login              — register address + view key with the server.
//!   /get_address_info   — balance and scan height.
//!   /get_address_txs    — transaction list.
//!
//! Privacy note: the LWS server receives the view key in plaintext.
//! No default endpoint is shipped. Users must configure their own
//! self-hosted open-monero-server instance.

use hodl_core::error::{Error, Result};
use hodl_core::tx::TxRef;
use serde::{Deserialize, Serialize};

/// Swappable transport for testing without real HTTP.
pub trait LwsTransport: Send + Sync {
    fn post(&self, path: &str, body: &str) -> Result<String>;
}

struct UreqTransport {
    agent: ureq::Agent,
    base_url: String,
}

impl LwsTransport for UreqTransport {
    fn post(&self, path: &str, body: &str) -> Result<String> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let resp = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(body)
            .map_err(|e| Error::Network(format!("lws post {path}: {e}")))?;
        resp.into_string()
            .map_err(|e| Error::Network(format!("lws read {path}: {e}")))
    }
}

/// LWS client. Constructed with a base URL (no trailing slash) pointing at a
/// self-hosted open-monero-server. No default endpoint — callers must provide
/// an explicit URL.
pub struct LwsClient {
    transport: Box<dyn LwsTransport>,
}

impl LwsClient {
    pub fn new(base_url: String) -> Self {
        let agent = ureq::AgentBuilder::new().build();
        Self {
            transport: Box::new(UreqTransport { agent, base_url }),
        }
    }

    /// Build with a custom transport (for tests).
    pub fn with_transport(transport: Box<dyn LwsTransport>) -> Self {
        Self { transport }
    }

    /// POST /login — register the address + view key with the server.
    ///
    /// `create_account`: if false, only succeed if the account already exists.
    pub fn login(
        &self,
        address: &str,
        view_key_hex: &str,
        create_account: bool,
    ) -> Result<LoginResponse> {
        let body = serde_json::to_string(&LoginRequest {
            address,
            view_key: view_key_hex,
            create_account,
            generated_locally: false,
        })
        .map_err(|e| Error::Chain(format!("serialize login: {e}")))?;
        let raw = self.transport.post("/login", &body)?;
        serde_json::from_str::<LoginResponse>(&raw)
            .map_err(|e| Error::Network(format!("parse login: {e}")))
    }

    /// POST /get_address_info — returns balance and scan height.
    pub fn get_address_info(&self, address: &str, view_key_hex: &str) -> Result<AddressInfo> {
        let body = serde_json::to_string(&AuthRequest {
            address,
            view_key: view_key_hex,
        })
        .map_err(|e| Error::Chain(format!("serialize get_address_info: {e}")))?;
        let raw = self.transport.post("/get_address_info", &body)?;
        serde_json::from_str::<AddressInfo>(&raw)
            .map_err(|e| Error::Network(format!("parse get_address_info: {e}")))
    }

    /// POST /get_address_txs — returns transaction list for the address.
    pub fn get_address_txs(&self, address: &str, view_key_hex: &str) -> Result<AddressTxs> {
        let body = serde_json::to_string(&AuthRequest {
            address,
            view_key: view_key_hex,
        })
        .map_err(|e| Error::Chain(format!("serialize get_address_txs: {e}")))?;
        let raw = self.transport.post("/get_address_txs", &body)?;
        serde_json::from_str::<AddressTxs>(&raw)
            .map_err(|e| Error::Network(format!("parse get_address_txs: {e}")))
    }
}

// --- Request types ---

#[derive(Serialize)]
struct LoginRequest<'a> {
    address: &'a str,
    view_key: &'a str,
    create_account: bool,
    generated_locally: bool,
}

#[derive(Serialize)]
struct AuthRequest<'a> {
    address: &'a str,
    view_key: &'a str,
}

// --- Response types ---

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    pub new_address: bool,
    pub generated_locally: bool,
}

/// Subset of the /get_address_info response fields we decode.
#[derive(Debug, Deserialize)]
pub struct AddressInfo {
    /// Total piconero received (sum of outputs to this address).
    pub total_received: u64,
    /// Total piconero spent (sum of inputs from this address).
    pub total_sent: u64,
    /// The blockchain height the LWS has scanned up to.
    pub scanned_block_height: u64,
    /// Blockchain height at the time of the response.
    pub blockchain_height: u64,
}

/// Subset of the /get_address_txs response fields we decode.
#[derive(Debug, Deserialize)]
pub struct AddressTxs {
    pub transactions: Vec<LwsTx>,
    pub scanned_block_height: u64,
    pub blockchain_height: u64,
}

#[derive(Debug, Deserialize)]
pub struct LwsTx {
    pub id: Option<String>,
    pub hash: Option<String>,
    pub height: Option<u64>,
    pub timestamp: Option<i64>,
    pub total_received: u64,
    pub total_sent: u64,
    pub unlock_time: u64,
    pub locked: bool,
}

impl LwsTx {
    /// Convert to the core TxRef type.
    pub fn to_tx_ref(&self) -> TxRef {
        let id = self
            .hash
            .clone()
            .or_else(|| self.id.clone())
            .unwrap_or_default();
        TxRef {
            id: hodl_core::tx::TxId(id),
            height: self.height,
            time: self.timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTransport {
        path: std::sync::Mutex<Option<String>>,
        response: String,
    }

    impl MockTransport {
        fn new(response: &str) -> Self {
            Self {
                path: std::sync::Mutex::new(None),
                response: response.to_string(),
            }
        }

        fn last_path(&self) -> Option<String> {
            self.path.lock().unwrap().clone()
        }
    }

    impl LwsTransport for MockTransport {
        fn post(&self, path: &str, _body: &str) -> Result<String> {
            *self.path.lock().unwrap() = Some(path.to_string());
            Ok(self.response.clone())
        }
    }

    #[test]
    fn login_round_trip() {
        let mock = std::sync::Arc::new(MockTransport::new(
            r#"{"new_address":true,"generated_locally":false}"#,
        ));
        let transport = Box::new(MockTransportRef(mock.clone()));
        let client = LwsClient::with_transport(transport);
        let resp = client.login("4addr", "viewhex", true).unwrap();
        assert!(resp.new_address);
        assert!(!resp.generated_locally);
        assert_eq!(mock.last_path().as_deref(), Some("/login"));
    }

    #[test]
    fn get_address_info_round_trip() {
        let mock = std::sync::Arc::new(MockTransport::new(
            r#"{"total_received":1000000000000,"total_sent":500000000000,"scanned_block_height":3000000,"blockchain_height":3000010}"#,
        ));
        let transport = Box::new(MockTransportRef(mock.clone()));
        let client = LwsClient::with_transport(transport);
        let info = client.get_address_info("4addr", "viewhex").unwrap();
        assert_eq!(info.total_received, 1_000_000_000_000);
        assert_eq!(info.total_sent, 500_000_000_000);
        assert_eq!(info.scanned_block_height, 3_000_000);
        assert_eq!(mock.last_path().as_deref(), Some("/get_address_info"));
    }

    #[test]
    fn get_address_txs_round_trip() {
        let mock = std::sync::Arc::new(MockTransport::new(
            r#"{"transactions":[{"hash":"abc123","height":2999999,"timestamp":1700000000,"total_received":1000000000000,"total_sent":0,"unlock_time":0,"locked":false}],"scanned_block_height":3000000,"blockchain_height":3000010}"#,
        ));
        let transport = Box::new(MockTransportRef(mock.clone()));
        let client = LwsClient::with_transport(transport);
        let txs = client.get_address_txs("4addr", "viewhex").unwrap();
        assert_eq!(txs.transactions.len(), 1);
        let tx = &txs.transactions[0];
        assert_eq!(tx.hash.as_deref(), Some("abc123"));
        assert_eq!(tx.height, Some(2_999_999));
        let tx_ref = tx.to_tx_ref();
        assert_eq!(tx_ref.id.0, "abc123");
        assert_eq!(mock.last_path().as_deref(), Some("/get_address_txs"));
    }

    // Thin Arc wrapper so the mock can check last_path after moving into Box.
    struct MockTransportRef(std::sync::Arc<MockTransport>);

    impl LwsTransport for MockTransportRef {
        fn post(&self, path: &str, body: &str) -> Result<String> {
            self.0.post(path, body)
        }
    }
}
