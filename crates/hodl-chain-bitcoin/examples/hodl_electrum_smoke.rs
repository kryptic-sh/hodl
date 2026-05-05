//! CI smoke test: probe every curated default Electrum endpoint.
//!
//! Walks `Config::default().chains`, parses each `Endpoint::Electrum` URL
//! into `(host, port)`, and calls `connect_tls` + `server.version` against
//! each one.  Each probe runs in a dedicated thread with a 10-second
//! timeout via `mpsc`.
//!
//! Exit codes:
//!   0 — all endpoints responded OK
//!   1 — one or more endpoints failed
//!
//! Usage:
//!   cargo run --release --example hodl_electrum_smoke

use std::sync::mpsc;
use std::time::Duration;

use hodl_chain_bitcoin::electrum::ElectrumClient;
use hodl_config::{Config, Endpoint};

/// Per-probe result sent back from the worker thread.
struct ProbeResult {
    chain: String,
    host: String,
    port: u16,
    outcome: Result<(String, String), String>,
}

/// Run a single TLS probe and send the result on `tx`.
fn probe(chain: String, host: String, port: u16, tx: mpsc::SyncSender<ProbeResult>) {
    let outcome = match ElectrumClient::connect_tls(&host, port, None) {
        Ok((mut client, _fp)) => client
            .server_version("hodl-smoke", "1.4")
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    // Ignore send error — main thread may have already timed out.
    let _ = tx.send(ProbeResult {
        chain,
        host,
        port,
        outcome,
    });
}

fn main() {
    let config = Config::default();

    // Collect (chain_label, host, port) in a stable order.
    let mut probes: Vec<(String, String, u16)> = Vec::new();

    let mut chain_ids: Vec<_> = config.chains.keys().collect();
    // Sort by debug repr for deterministic output.
    chain_ids.sort_by_key(|c| format!("{c:?}"));

    for chain_id in chain_ids {
        let chain_cfg = &config.chains[chain_id];
        let label = format!("{chain_id:?}");
        for ep in &chain_cfg.endpoints {
            if let Endpoint::Electrum { url, .. } = ep {
                // url is "ssl://host:port"
                let addr = url.trim_start_matches("ssl://");
                if let Some(colon) = addr.rfind(':') {
                    let host = addr[..colon].to_string();
                    let port: u16 = match addr[colon + 1..].parse() {
                        Ok(p) => p,
                        Err(e) => {
                            println!("FAIL {label} {addr} -> bad port: {e}");
                            probes.push((label.clone(), host, 0));
                            continue;
                        }
                    };
                    probes.push((label.clone(), host, port));
                } else {
                    println!("FAIL {label} {url} -> malformed URL (no colon)");
                }
            }
        }
    }

    let timeout = Duration::from_secs(10);
    let mut failures = 0usize;

    for (chain, host, port) in probes {
        if port == 0 {
            // Already printed above.
            failures += 1;
            continue;
        }

        let (tx, rx) = mpsc::sync_channel::<ProbeResult>(1);
        let chain_c = chain.clone();
        let host_c = host.clone();
        std::thread::spawn(move || probe(chain_c, host_c, port, tx));

        match rx.recv_timeout(timeout) {
            Ok(result) => match result.outcome {
                Ok((srv, ver)) => {
                    println!(
                        "OK   {} {}:{} -> {}/{}",
                        result.chain, result.host, result.port, srv, ver
                    );
                }
                Err(e) => {
                    println!(
                        "FAIL {} {}:{} -> {}",
                        result.chain, result.host, result.port, e
                    );
                    failures += 1;
                }
            },
            Err(_) => {
                println!(
                    "FAIL {chain} {host}:{port} -> timeout after {}s",
                    timeout.as_secs()
                );
                failures += 1;
            }
        }
    }

    if failures > 0 {
        eprintln!("{failures} endpoint(s) failed");
        std::process::exit(1);
    }
}
