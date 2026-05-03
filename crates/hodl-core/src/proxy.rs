//! SOCKS5 proxy URL parsing utilities.

use crate::error::{Error, Result};

/// Parse a `socks5://host:port` URL into `(host, port)`.
///
/// Accepts only the `socks5` scheme; anything else is an error.
pub fn parse_socks5_url(url: &str) -> Result<(String, u16)> {
    let rest = url
        .strip_prefix("socks5://")
        .ok_or_else(|| Error::Endpoint(format!("proxy URL must start with socks5://: {url}")))?;

    // Strip optional trailing slash.
    let rest = rest.trim_end_matches('/');

    // Split off userinfo if present (user:pass@host:port).
    let hostport = if let Some(at_pos) = rest.rfind('@') {
        &rest[at_pos + 1..]
    } else {
        rest
    };

    let (host, port_str) = hostport
        .rsplit_once(':')
        .ok_or_else(|| Error::Endpoint(format!("proxy URL missing port: {url}")))?;

    if host.is_empty() {
        return Err(Error::Endpoint(format!("proxy URL missing host: {url}")));
    }

    let port = port_str
        .parse::<u16>()
        .map_err(|_| Error::Endpoint(format!("proxy URL invalid port: {url}")))?;

    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_localhost() {
        let (host, port) = parse_socks5_url("socks5://127.0.0.1:9050").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9050);
    }

    #[test]
    fn happy_path_hostname() {
        let (host, port) = parse_socks5_url("socks5://proxy.example.com:1080").unwrap();
        assert_eq!(host, "proxy.example.com");
        assert_eq!(port, 1080);
    }

    #[test]
    fn bad_scheme_rejected() {
        assert!(parse_socks5_url("http://127.0.0.1:9050").is_err());
        assert!(parse_socks5_url("socks4://127.0.0.1:9050").is_err());
    }

    #[test]
    fn missing_port_rejected() {
        assert!(parse_socks5_url("socks5://127.0.0.1").is_err());
    }

    #[test]
    fn invalid_port_rejected() {
        assert!(parse_socks5_url("socks5://127.0.0.1:notaport").is_err());
        assert!(parse_socks5_url("socks5://127.0.0.1:99999").is_err());
    }

    #[test]
    fn empty_host_rejected() {
        assert!(parse_socks5_url("socks5://:9050").is_err());
    }
}
