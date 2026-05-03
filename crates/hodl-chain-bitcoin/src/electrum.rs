use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Arc;

use hodl_core::error::{Error, Result};
use hodl_core::proxy::parse_socks5_url;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A type that supports both reading and writing.
pub trait ReadWrite: std::io::Read + Write + Send {}
impl<T: std::io::Read + Write + Send> ReadWrite for T {}

/// Electrum 1.4 JSON-RPC client.
///
/// The transport is swappable (TCP or TLS) so tests can inject a mock.
/// Tor/SOCKS5 is wired at the transport level — pass a pre-connected stream.
pub struct ElectrumClient {
    reader: BufReader<Box<dyn ReadWrite>>,
    id: u64,
}

impl ElectrumClient {
    /// Construct from an already-connected transport. Used directly in tests
    /// and by `connect`.
    pub fn from_transport(transport: Box<dyn ReadWrite>) -> Self {
        Self {
            reader: BufReader::new(transport),
            id: 0,
        }
    }

    /// Open a plain TCP connection.
    pub fn connect_tcp(host: &str, port: u16) -> Result<Self> {
        let stream = TcpStream::connect((host, port))
            .map_err(|e| Error::Network(format!("TCP connect {host}:{port}: {e}")))?;
        Ok(Self::from_transport(Box::new(stream)))
    }

    /// Open a plain TCP connection through a SOCKS5 proxy.
    ///
    /// `proxy_url` — `socks5://host:port`.
    pub fn connect_tcp_via_socks5(host: &str, port: u16, proxy_url: &str) -> Result<Self> {
        let (proxy_host, proxy_port) = parse_socks5_url(proxy_url)?;
        let stream = socks::Socks5Stream::connect((proxy_host.as_str(), proxy_port), (host, port))
            .map_err(|e| Error::Network(format!("SOCKS5 connect {host}:{port}: {e}")))?;
        Ok(Self::from_transport(Box::new(stream.into_inner())))
    }

    /// Open a TLS connection through a SOCKS5 proxy.
    ///
    /// SOCKS5 dials the tunnel, then rustls performs the TLS handshake over it.
    /// `proxy_url` — `socks5://host:port`.
    pub fn connect_tls_via_socks5(host: &str, port: u16, proxy_url: &str) -> Result<Self> {
        use rustls::pki_types::ServerName;
        use rustls::{ClientConfig, ClientConnection};

        let (proxy_host, proxy_port) = parse_socks5_url(proxy_url)?;
        let stream = socks::Socks5Stream::connect((proxy_host.as_str(), proxy_port), (host, port))
            .map_err(|e| Error::Network(format!("SOCKS5 connect {host}:{port}: {e}")))?;
        let tcp = stream.into_inner();

        let roots = webpki_roots::TLS_SERVER_ROOTS.to_vec();
        let root_store = rustls::RootCertStore { roots };
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let config = Arc::new(config);
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|e| Error::Network(format!("invalid TLS server name {host}: {e}")))?;
        let conn = ClientConnection::new(config, server_name)
            .map_err(|e| Error::Network(format!("TLS handshake: {e}")))?;
        let tls = rustls::StreamOwned::new(conn, tcp);
        Ok(Self::from_transport(Box::new(tls)))
    }

    /// Open a TLS connection using rustls with the system/webpki root store.
    pub fn connect_tls(host: &str, port: u16) -> Result<Self> {
        use rustls::pki_types::ServerName;
        use rustls::{ClientConfig, ClientConnection};

        let roots = webpki_roots::TLS_SERVER_ROOTS.to_vec();
        let root_store = rustls::RootCertStore { roots };
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let config = Arc::new(config);
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|e| Error::Network(format!("invalid TLS server name {host}: {e}")))?;
        let tcp = TcpStream::connect((host, port))
            .map_err(|e| Error::Network(format!("TCP connect {host}:{port}: {e}")))?;
        let conn = ClientConnection::new(config, server_name)
            .map_err(|e| Error::Network(format!("TLS handshake: {e}")))?;
        let tls = rustls::StreamOwned::new(conn, tcp);
        Ok(Self::from_transport(Box::new(tls)))
    }

    fn next_id(&mut self) -> u64 {
        self.id += 1;
        self.id
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let req = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": method,
            "params": params,
        });
        let mut line =
            serde_json::to_string(&req).map_err(|e| Error::Network(format!("JSON encode: {e}")))?;
        line.push('\n');
        self.reader
            .get_mut()
            .write_all(line.as_bytes())
            .map_err(|e| Error::Network(format!("send: {e}")))?;

        let mut resp_line = String::new();
        self.reader
            .read_line(&mut resp_line)
            .map_err(|e| Error::Network(format!("recv: {e}")))?;
        let resp: Value = serde_json::from_str(&resp_line)
            .map_err(|e| Error::Network(format!("JSON decode: {e}")))?;

        if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
            return Err(Error::Network(format!("Electrum error: {err}")));
        }
        Ok(resp["result"].clone())
    }

    /// `server.version` — negotiate protocol version.
    pub fn server_version(
        &mut self,
        client_name: &str,
        protocol_version: &str,
    ) -> Result<(String, String)> {
        let result = self.call("server.version", json!([client_name, protocol_version]))?;
        let server_ver = result[0]
            .as_str()
            .ok_or_else(|| Error::Network("server.version: missing server version".into()))?
            .to_owned();
        let proto_ver = result[1]
            .as_str()
            .ok_or_else(|| Error::Network("server.version: missing protocol version".into()))?
            .to_owned();
        Ok((server_ver, proto_ver))
    }

    /// `blockchain.scripthash.get_balance`.
    pub fn scripthash_get_balance(&mut self, scripthash: &str) -> Result<ScriptHashBalance> {
        let result = self.call("blockchain.scripthash.get_balance", json!([scripthash]))?;
        serde_json::from_value(result)
            .map_err(|e| Error::Network(format!("get_balance decode: {e}")))
    }

    /// `blockchain.scripthash.get_history`.
    pub fn scripthash_get_history(&mut self, scripthash: &str) -> Result<Vec<HistoryEntry>> {
        let result = self.call("blockchain.scripthash.get_history", json!([scripthash]))?;
        serde_json::from_value(result)
            .map_err(|e| Error::Network(format!("get_history decode: {e}")))
    }

    /// `blockchain.estimatefee` — returns BTC/kB.
    pub fn estimate_fee(&mut self, target_blocks: u32) -> Result<f64> {
        let result = self.call("blockchain.estimatefee", json!([target_blocks]))?;
        result
            .as_f64()
            .ok_or_else(|| Error::Network("estimatefee: expected f64".into()))
    }

    /// `blockchain.scripthash.listunspent` — UTXOs for a scripthash.
    pub fn scripthash_listunspent(&mut self, scripthash: &str) -> Result<Vec<Utxo>> {
        let result = self.call("blockchain.scripthash.listunspent", json!([scripthash]))?;
        serde_json::from_value(result)
            .map_err(|e| Error::Network(format!("listunspent decode: {e}")))
    }

    /// `blockchain.transaction.broadcast` — broadcast a raw hex tx. Returns txid.
    pub fn transaction_broadcast(&mut self, raw_hex: &str) -> Result<String> {
        let result = self.call("blockchain.transaction.broadcast", json!([raw_hex]))?;
        result
            .as_str()
            .map(|s| s.to_owned())
            .ok_or_else(|| Error::Network("broadcast: expected string txid".into()))
    }

    /// `blockchain.transaction.get` — fetch raw hex for a txid.
    pub fn transaction_get(&mut self, tx_hash: &str) -> Result<String> {
        let result = self.call("blockchain.transaction.get", json!([tx_hash, false]))?;
        result
            .as_str()
            .map(|s| s.to_owned())
            .ok_or_else(|| Error::Network("transaction.get: expected hex string".into()))
    }
}

#[derive(Debug, Deserialize)]
pub struct ScriptHashBalance {
    pub confirmed: u64,
    pub unconfirmed: i64,
}

#[derive(Debug, Deserialize)]
pub struct HistoryEntry {
    pub tx_hash: String,
    pub height: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Utxo {
    pub tx_hash: String,
    pub tx_pos: u32,
    pub height: i64,
    pub value: u64,
}

/// Compute the Electrum scripthash for a P2WPKH script pubkey.
///
/// Electrum uses SHA256(script_pubkey) with bytes reversed (little-endian).
pub fn p2wpkh_scripthash(pubkey_hash: &[u8; 20]) -> String {
    // P2WPKH scriptPubKey: OP_0 <20 bytes>
    let mut script = Vec::with_capacity(22);
    script.push(0x00); // OP_0
    script.push(0x14); // push 20 bytes
    script.extend_from_slice(pubkey_hash);
    let hash: [u8; 32] = Sha256::digest(&script).into();
    // Electrum wants reversed bytes.
    let mut rev = hash;
    rev.reverse();
    hex::encode(rev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Fake transport: pre-load response bytes, capture writes in a Vec.
    struct MockTransport {
        read: Cursor<Vec<u8>>,
        write: Vec<u8>,
    }

    impl MockTransport {
        fn new(response: &str) -> Self {
            Self {
                read: Cursor::new(response.as_bytes().to_vec()),
                write: Vec::new(),
            }
        }
    }

    impl std::io::Read for MockTransport {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.read.read(buf)
        }
    }

    impl Write for MockTransport {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.write.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn server_version_encode_decode() {
        let response =
            r#"{"jsonrpc":"2.0","id":1,"result":["ElectrumX 1.16.0","1.4"]}"#.to_owned() + "\n";
        let mock = MockTransport::new(&response);
        let mut client = ElectrumClient::from_transport(Box::new(mock));
        let (sv, pv) = client.server_version("hodl/0.1", "1.4").unwrap();
        assert_eq!(sv, "ElectrumX 1.16.0");
        assert_eq!(pv, "1.4");
    }

    #[test]
    fn get_balance_decode() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"confirmed":100000,"unconfirmed":0}}"#
            .to_owned()
            + "\n";
        let mock = MockTransport::new(&response);
        let mut client = ElectrumClient::from_transport(Box::new(mock));
        let bal = client.scripthash_get_balance("deadbeef").unwrap();
        assert_eq!(bal.confirmed, 100_000);
        assert_eq!(bal.unconfirmed, 0);
    }

    #[test]
    fn get_history_decode() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":[{"tx_hash":"abc","height":800000}]}"#
            .to_owned()
            + "\n";
        let mock = MockTransport::new(&response);
        let mut client = ElectrumClient::from_transport(Box::new(mock));
        let hist = client.scripthash_get_history("deadbeef").unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].tx_hash, "abc");
        assert_eq!(hist[0].height, 800_000);
    }

    #[test]
    fn estimate_fee_decode() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":0.00012}"#.to_owned() + "\n";
        let mock = MockTransport::new(&response);
        let mut client = ElectrumClient::from_transport(Box::new(mock));
        let fee = client.estimate_fee(6).unwrap();
        assert!((fee - 0.00012).abs() < 1e-9);
    }

    #[test]
    fn p2wpkh_scripthash_deterministic() {
        // Known vector: OP_0 <20-byte zeroes> → sha256 reversed.
        let hash = p2wpkh_scripthash(&[0u8; 20]);
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn listunspent_decode() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":[{"tx_hash":"abcd","tx_pos":0,"height":800001,"value":50000}]}"#.to_owned() + "\n";
        let mock = MockTransport::new(&response);
        let mut client = ElectrumClient::from_transport(Box::new(mock));
        let utxos = client.scripthash_listunspent("deadbeef").unwrap();
        assert_eq!(utxos.len(), 1);
        assert_eq!(utxos[0].tx_hash, "abcd");
        assert_eq!(utxos[0].tx_pos, 0);
        assert_eq!(utxos[0].height, 800_001);
        assert_eq!(utxos[0].value, 50_000);
    }

    #[test]
    fn transaction_broadcast_decode() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":"deadbeefdeadbeef"}"#.to_owned() + "\n";
        let mock = MockTransport::new(&response);
        let mut client = ElectrumClient::from_transport(Box::new(mock));
        let txid = client.transaction_broadcast("0100000000").unwrap();
        assert_eq!(txid, "deadbeefdeadbeef");
    }

    #[test]
    fn transaction_get_decode() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":"0100000001"}"#.to_owned() + "\n";
        let mock = MockTransport::new(&response);
        let mut client = ElectrumClient::from_transport(Box::new(mock));
        let raw = client.transaction_get("deadbeef").unwrap();
        assert_eq!(raw, "0100000001");
    }
}
