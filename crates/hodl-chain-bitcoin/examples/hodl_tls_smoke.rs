//! Smoke test for TOFU TLS connect.
//!
//! On first run for each server the leaf cert fingerprint is pinned and
//! printed. On subsequent runs the pinned fingerprint is verified; a mismatch
//! prints the error and exits non-zero.
//!
//! Usage (no TOFU store — fresh pin each run):
//!   cargo run --example hodl_tls_smoke
//!
//! The example intentionally uses `pinned = None` on every invocation so it
//! always acts as a "first connect" and reports the fingerprint. In production
//! code the fingerprint would be loaded from `known_hosts.toml` and passed as
//! `Some(fp)`.

use hodl_chain_bitcoin::electrum::ElectrumClient;

fn main() {
    let mut exit_code = 0;

    for (host, port) in [
        ("electrum3.nav.community", 40002),
        ("electrum2.nav.community", 40002),
        ("electrum.nav.community", 40002),
    ] {
        // Pass `pinned = None` → always acts as first connect (TOFU pin).
        match ElectrumClient::connect_tls(host, port, None) {
            Ok((mut c, new_fp)) => {
                if let Some(fp) = new_fp {
                    println!("PIN  {host}:{port} -> fingerprint: {fp}");
                }
                match c.server_version("hodl", "1.4") {
                    Ok((srv, ver)) => println!("OK   {host}:{port} -> {srv} / {ver}"),
                    Err(e) => println!("VERS {host}:{port} -> {e}"),
                }
            }
            Err(e) => {
                println!("CONN {host}:{port} -> {e}");
                exit_code = 1;
            }
        }
    }

    std::process::exit(exit_code);
}
