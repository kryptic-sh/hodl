use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hodl_core::error::{Error, Result};
use serde_json::{Value, json};

/// Swappable transport — lets tests inject a mock without touching ureq.
pub trait JsonRpcTransport: Send + Sync {
    fn call(&self, body: &str) -> Result<String>;
}

/// ureq-backed transport.
struct UreqTransport {
    agent: ureq::Agent,
    url: String,
}

impl JsonRpcTransport for UreqTransport {
    fn call(&self, body: &str) -> Result<String> {
        let resp = self
            .agent
            .post(&self.url)
            .set("Content-Type", "application/json")
            .send_string(body)
            .map_err(|e| Error::Network(format!("ureq send: {e}")))?;
        resp.into_string()
            .map_err(|e| Error::Network(format!("ureq read: {e}")))
    }
}

/// JSON-RPC 2.0 client for Ethereum nodes (Infura, Alchemy, Ankr, …).
///
/// Build with `EthRpcClient::new` for plain HTTP, or `EthRpcClient::with_socks5`
/// for Tor / SOCKS5 proxy passthrough.
pub struct EthRpcClient {
    transport: Arc<dyn JsonRpcTransport>,
    seq: Arc<AtomicU64>,
}

impl EthRpcClient {
    pub fn new(url: String) -> Self {
        let agent = ureq::AgentBuilder::new().build();
        Self {
            transport: Arc::new(UreqTransport { agent, url }),
            seq: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Build with a SOCKS5 proxy (e.g., Tor: `socks5://127.0.0.1:9050`).
    pub fn with_socks5(url: String, proxy: &str) -> Result<Self> {
        let proxy =
            ureq::Proxy::new(proxy).map_err(|e| Error::Endpoint(format!("invalid proxy: {e}")))?;
        let agent = ureq::AgentBuilder::new().proxy(proxy).build();
        Ok(Self {
            transport: Arc::new(UreqTransport { agent, url }),
            seq: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Build with a custom transport (for tests).
    pub fn with_transport(transport: Arc<dyn JsonRpcTransport>) -> Self {
        Self {
            transport,
            seq: Arc::new(AtomicU64::new(1)),
        }
    }

    fn next_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id();
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body_str = serde_json::to_string(&body)
            .map_err(|e| Error::Chain(format!("serialize request: {e}")))?;
        let resp_str = self.transport.call(&body_str)?;
        let resp: Value = serde_json::from_str(&resp_str)
            .map_err(|e| Error::Network(format!("parse response: {e}")))?;

        if let Some(err) = resp.get("error") {
            return Err(Error::Network(format!("json-rpc error: {err}")));
        }
        resp.get("result")
            .cloned()
            .ok_or_else(|| Error::Network("missing result field".into()))
    }

    fn parse_hex_u64(v: &Value) -> Result<u64> {
        let s = v
            .as_str()
            .ok_or_else(|| Error::Network(format!("expected hex string, got {v}")))?;
        let stripped = s.strip_prefix("0x").unwrap_or(s);
        u64::from_str_radix(stripped, 16)
            .map_err(|e| Error::Network(format!("parse hex u64 '{s}': {e}")))
    }

    fn parse_hex_u128(v: &Value) -> Result<u128> {
        let s = v
            .as_str()
            .ok_or_else(|| Error::Network(format!("expected hex string, got {v}")))?;
        let stripped = s.strip_prefix("0x").unwrap_or(s);
        u128::from_str_radix(stripped, 16)
            .map_err(|e| Error::Network(format!("parse hex u128 '{s}': {e}")))
    }

    /// `eth_chainId` — sanity check against expected chain id.
    pub fn eth_chain_id(&self) -> Result<u64> {
        let v = self.call("eth_chainId", json!([]))?;
        Self::parse_hex_u64(&v)
    }

    /// `eth_getBalance(address, "latest")` → balance in wei (u128).
    pub fn eth_get_balance(&self, address: &str) -> Result<u128> {
        let v = self.call("eth_getBalance", json!([address, "latest"]))?;
        Self::parse_hex_u128(&v)
    }

    /// `eth_getTransactionCount(address, "pending")` → nonce.
    pub fn eth_get_transaction_count(&self, address: &str) -> Result<u64> {
        let v = self.call("eth_getTransactionCount", json!([address, "pending"]))?;
        Self::parse_hex_u64(&v)
    }

    /// `eth_gasPrice` → gas price in wei.
    pub fn eth_gas_price(&self) -> Result<u64> {
        let v = self.call("eth_gasPrice", json!([]))?;
        Self::parse_hex_u64(&v)
    }

    /// `eth_maxPriorityFeePerGas` → tip in wei (EIP-1559).
    pub fn eth_max_priority_fee_per_gas(&self) -> Result<u64> {
        let v = self.call("eth_maxPriorityFeePerGas", json!([]))?;
        Self::parse_hex_u64(&v)
    }

    /// `eth_estimateGas` → estimated gas units.
    pub fn eth_estimate_gas(
        &self,
        from: &str,
        to: &str,
        value_hex: &str,
        data_hex: &str,
    ) -> Result<u64> {
        let v = self.call(
            "eth_estimateGas",
            json!([{
                "from": from,
                "to": to,
                "value": value_hex,
                "data": data_hex,
            }]),
        )?;
        Self::parse_hex_u64(&v)
    }

    /// `eth_sendRawTransaction(hex_tx)` → transaction hash.
    pub fn eth_send_raw_transaction(&self, raw_hex: &str) -> Result<String> {
        let v = self.call("eth_sendRawTransaction", json!([raw_hex]))?;
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Network("expected string tx hash".into()))
    }

    /// `eth_getTransactionByHash` → raw JSON object (or null if not found).
    pub fn eth_get_transaction_by_hash(&self, hash: &str) -> Result<Value> {
        self.call("eth_getTransactionByHash", json!([hash]))
    }

    /// `eth_getTransactionReceipt` → raw JSON object (or null if pending).
    pub fn eth_get_transaction_receipt(&self, hash: &str) -> Result<Value> {
        self.call("eth_getTransactionReceipt", json!([hash]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTransport {
        response: String,
    }

    impl JsonRpcTransport for MockTransport {
        fn call(&self, _body: &str) -> Result<String> {
            Ok(self.response.clone())
        }
    }

    fn mock_client(response: &str) -> EthRpcClient {
        EthRpcClient::with_transport(Arc::new(MockTransport {
            response: response.to_owned(),
        }))
    }

    #[test]
    fn eth_chain_id_round_trip() {
        let client = mock_client(r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#);
        assert_eq!(client.eth_chain_id().unwrap(), 1u64);
    }

    #[test]
    fn eth_get_balance_round_trip() {
        // 1 ETH = 1_000_000_000_000_000_000 wei = 0xde0b6b3a7640000
        let client = mock_client(r#"{"jsonrpc":"2.0","id":1,"result":"0xde0b6b3a7640000"}"#);
        assert_eq!(
            client.eth_get_balance("0x0").unwrap(),
            1_000_000_000_000_000_000u128
        );
    }

    #[test]
    fn eth_send_raw_transaction_round_trip() {
        let hash = "0xabc123def456";
        let resp = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{hash}"}}"#);
        let client = mock_client(&resp);
        assert_eq!(client.eth_send_raw_transaction("0xdeadbeef").unwrap(), hash);
    }
}
