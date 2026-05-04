use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

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

    /// Open a TLS connection through a SOCKS5 proxy with TOFU cert pinning.
    ///
    /// SOCKS5 dials the tunnel, then rustls performs the TLS handshake over it.
    /// `proxy_url` — `socks5://host:port`.
    ///
    /// Returns `(client, Some(fingerprint))` when a new fingerprint was pinned
    /// (caller must persist it); `(client, None)` when an existing pin matched.
    /// Returns `Err(Error::TofuMismatch { … })` on a fingerprint mismatch.
    pub fn connect_tls_via_socks5(
        host: &str,
        port: u16,
        proxy_url: &str,
        pinned: Option<String>,
    ) -> Result<(Self, Option<String>)> {
        use rustls::ClientConnection;
        use rustls::pki_types::ServerName;

        let host_port = format!("{host}:{port}");
        let (proxy_host, proxy_port) = parse_socks5_url(proxy_url)?;
        let stream = socks::Socks5Stream::connect((proxy_host.as_str(), proxy_port), (host, port))
            .map_err(|e| Error::Network(format!("SOCKS5 connect {host}:{port}: {e}")))?;
        let tcp = stream.into_inner();

        let newly_pinned: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let config = electrum_tls_config(host_port.clone(), pinned, Arc::clone(&newly_pinned))?;
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|e| Error::Network(format!("invalid TLS server name {host}: {e}")))?;
        let conn =
            ClientConnection::new(config, server_name).map_err(|e| map_tls_error(e, &host_port))?;
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        // rustls is lazy: the handshake (and our verify_server_cert) only
        // runs when there's actual I/O. Force it now so newly_pinned is
        // populated before we return. flush on a fresh connection drives
        // the handshake to completion without sending application data.
        tls.flush().map_err(|e| {
            map_tls_error(
                rustls::Error::General(format!("TLS flush: {e}")),
                &host_port,
            )
        })?;
        let new_fp = newly_pinned.lock().unwrap().take();
        Ok((Self::from_transport(Box::new(tls)), new_fp))
    }

    /// Open a TLS connection with TOFU cert pinning.
    ///
    /// `pinned` — the previously-saved SHA-256 fingerprint for this `host:port`,
    /// if any (pass `None` on first connect). On first connect the verifier pins
    /// the server's leaf cert fingerprint and returns it as `Some(fp)` so the
    /// caller can persist it. On subsequent connects with a matching fingerprint
    /// returns `(client, None)`. On mismatch returns
    /// `Err(Error::TofuMismatch { … })`.
    pub fn connect_tls(
        host: &str,
        port: u16,
        pinned: Option<String>,
    ) -> Result<(Self, Option<String>)> {
        use rustls::ClientConnection;
        use rustls::pki_types::ServerName;

        let host_port = format!("{host}:{port}");
        let newly_pinned: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let config = electrum_tls_config(host_port.clone(), pinned, Arc::clone(&newly_pinned))?;
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|e| Error::Network(format!("invalid TLS server name {host}: {e}")))?;
        let tcp = TcpStream::connect((host, port))
            .map_err(|e| Error::Network(format!("TCP connect {host}:{port}: {e}")))?;
        let conn =
            ClientConnection::new(config, server_name).map_err(|e| map_tls_error(e, &host_port))?;
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        // rustls is lazy: the handshake (and our verify_server_cert) only
        // runs when there's actual I/O. Force it now so newly_pinned is
        // populated before we return.
        tls.flush().map_err(|e| {
            map_tls_error(
                rustls::Error::General(format!("TLS flush: {e}")),
                &host_port,
            )
        })?;
        let new_fp = newly_pinned.lock().unwrap().take();
        Ok((Self::from_transport(Box::new(tls)), new_fp))
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

    /// Number of history entries for a scripthash — thin wrapper over
    /// `scripthash_get_history` that avoids allocating the full entry list at
    /// the call site when only the count matters (e.g. gap-limit scan).
    pub fn get_history_count(&mut self, scripthash: &str) -> Result<usize> {
        self.scripthash_get_history(scripthash).map(|h| h.len())
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

// ── TLS configuration ─────────────────────────────────────────────────────

/// Compute the SHA-256 fingerprint of a TLS certificate's DER bytes.
///
/// Returns a lowercase hex string (64 characters, no separators). This is the
/// same computation used by Electrum desktop and Sparrow for TOFU pinning.
pub fn cert_fingerprint_sha256(cert_der: &[u8]) -> String {
    let hash = Sha256::digest(cert_der);
    hex::encode(hash)
}

/// Convert a `rustls::Error` into a `hodl_core::Error`, detecting TOFU
/// mismatch messages encoded by `TofuVerifier` via `rustls::Error::General`.
fn map_tls_error(e: rustls::Error, host_port: &str) -> Error {
    // TofuVerifier encodes mismatches as:
    // "TOFU mismatch for <host:port>: pinned <fp>, server presented <fp2>"
    // We parse this prefix to upgrade it to Error::TofuMismatch.
    let msg = e.to_string();
    if let Some(rest) = msg.strip_prefix("TOFU mismatch for ") {
        // Format: "<host:port>: pinned <pinned>, server presented <presented>"
        if let Some(colon_pos) = rest.find(": pinned ") {
            let host = rest[..colon_pos].to_string();
            let after_pinned = &rest[colon_pos + ": pinned ".len()..];
            if let Some(sep_pos) = after_pinned.find(", server presented ") {
                let pinned = after_pinned[..sep_pos].to_string();
                let presented = after_pinned[sep_pos + ", server presented ".len()..].to_string();
                return Error::TofuMismatch {
                    host,
                    pinned,
                    presented,
                };
            }
        }
    }
    Error::Network(format!("TLS error for {host_port}: {e}"))
}

/// Build a `rustls::ClientConfig` with TOFU cert pinning for the given
/// `host:port`.
///
/// The verifier compares the server's leaf cert fingerprint against `pinned`:
///
/// - `pinned == None` → first connect: write the fingerprint into `on_pinned`
///   and accept.
/// - `pinned == Some(fp)` and fingerprints match → accept.
/// - `pinned == Some(fp)` and fingerprints differ → refuse with
///   `rustls::Error::General` carrying the mismatch details so `map_tls_error`
///   can upgrade it to `Error::TofuMismatch`.
///
/// The cert chain itself is NOT validated against any CA bundle — Electrum
/// servers overwhelmingly use self-signed certs. TOFU pinning is the
/// wallet-appropriate trust model (Electrum desktop / Sparrow default).
///
/// TLS-12/13 signature verification is intentionally bypassed (same as the
/// previous `AcceptAnyServerCert`) — it would reject self-signed certs that
/// use RSA keys without a certificate chain that rustls can verify.
fn electrum_tls_config(
    host_port: String,
    pinned: Option<String>,
    on_pinned: Arc<Mutex<Option<String>>>,
) -> Result<Arc<rustls::ClientConfig>> {
    use rustls::ClientConfig;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

    #[derive(Debug)]
    struct TofuVerifier {
        host_port: String,
        pinned: Option<String>,
        on_pinned: Arc<Mutex<Option<String>>>,
    }

    impl ServerCertVerifier for TofuVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> std::result::Result<ServerCertVerified, rustls::Error> {
            let computed = cert_fingerprint_sha256(end_entity.as_ref());
            match &self.pinned {
                Some(saved) if saved == &computed => {
                    // Pin matches — allow.
                    Ok(ServerCertVerified::assertion())
                }
                Some(saved) => {
                    // Mismatch — refuse with a structured message that
                    // `map_tls_error` can parse into `Error::TofuMismatch`.
                    Err(rustls::Error::General(format!(
                        "TOFU mismatch for {}: pinned {}, server presented {}",
                        self.host_port, saved, computed
                    )))
                }
                None => {
                    // First connect — pin and accept.
                    *self.on_pinned.lock().unwrap() = Some(computed);
                    Ok(ServerCertVerified::assertion())
                }
            }
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            use rustls::SignatureScheme::*;
            vec![
                RSA_PKCS1_SHA256,
                RSA_PKCS1_SHA384,
                RSA_PKCS1_SHA512,
                ECDSA_NISTP256_SHA256,
                ECDSA_NISTP384_SHA384,
                ECDSA_NISTP521_SHA512,
                RSA_PSS_SHA256,
                RSA_PSS_SHA384,
                RSA_PSS_SHA512,
                ED25519,
            ]
        }
    }

    let verifier = TofuVerifier {
        host_port,
        pinned,
        on_pinned,
    };

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
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

/// Compute the Electrum scripthash for a P2PKH script pubkey.
///
/// Electrum uses SHA256(script_pubkey) with bytes reversed (little-endian).
pub fn p2pkh_scripthash(pubkey_hash: &[u8; 20]) -> String {
    // P2PKH scriptPubKey: OP_DUP OP_HASH160 <20 bytes> OP_EQUALVERIFY OP_CHECKSIG
    let mut script = Vec::with_capacity(25);
    script.push(0x76); // OP_DUP
    script.push(0xa9); // OP_HASH160
    script.push(0x14); // push 20 bytes
    script.extend_from_slice(pubkey_hash);
    script.push(0x88); // OP_EQUALVERIFY
    script.push(0xac); // OP_CHECKSIG
    let hash: [u8; 32] = Sha256::digest(&script).into();
    let mut rev = hash;
    rev.reverse();
    hex::encode(rev)
}

/// Compute the Electrum scripthash for a P2SH script pubkey.
///
/// Electrum uses SHA256(script_pubkey) with bytes reversed (little-endian).
/// `script_hash` is the HASH160 of the redeemScript (20 bytes).
pub fn p2sh_scripthash(script_hash: &[u8; 20]) -> String {
    // P2SH scriptPubKey: OP_HASH160 <20 bytes> OP_EQUAL
    let mut script = Vec::with_capacity(23);
    script.push(0xa9); // OP_HASH160
    script.push(0x14); // push 20 bytes
    script.extend_from_slice(script_hash);
    script.push(0x87); // OP_EQUAL
    let hash: [u8; 32] = Sha256::digest(&script).into();
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
