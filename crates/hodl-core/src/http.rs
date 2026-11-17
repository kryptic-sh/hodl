//! The HTTP agent every JSON-RPC client shares.

use std::time::Duration;

use crate::error::{Error, Result};

/// How long to wait for a TCP connection before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How many redirects to follow before failing the request.
const MAX_REDIRECTS: u32 = 5;

/// Build an HTTP agent, routed through `proxy` when one is given.
///
/// The agent never takes a proxy from the environment (`ALL_PROXY`,
/// `HTTPS_PROXY`, ...): traffic goes through `proxy` or straight to the
/// endpoint.
pub fn agent(proxy: Option<ureq::Proxy>) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .proxy(proxy)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .max_redirects(MAX_REDIRECTS)
        .build();
    ureq::Agent::new_with_config(config)
}

/// Parse a proxy URL such as `socks5://127.0.0.1:9050` (Tor).
///
/// A `socks5://` proxy resolves host names on the proxy's side, as
/// `socks5h://` does. ureq resolves them locally for `socks5://`, which over
/// Tor would send every endpoint's name to the local resolver in the clear.
pub fn socks_proxy(url: &str) -> Result<ureq::Proxy> {
    let url = match url.strip_prefix("socks5://") {
        Some(rest) => format!("socks5h://{rest}"),
        None => url.to_string(),
    };
    ureq::Proxy::new(&url).map_err(|e| Error::Endpoint(format!("invalid proxy: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socks5_resolves_names_on_the_proxy() {
        let proxy = socks_proxy("socks5://127.0.0.1:9050").unwrap();
        assert!(!proxy.resolve_target());
        assert_eq!(proxy.host(), "127.0.0.1");
        assert_eq!(proxy.port(), 9050);
    }

    #[test]
    fn socks5h_is_kept() {
        let proxy = socks_proxy("socks5h://127.0.0.1:9050").unwrap();
        assert!(!proxy.resolve_target());
    }

    #[test]
    fn plain_agent_has_no_proxy() {
        let agent = agent(None);
        assert!(agent.config().proxy().is_none());
    }

    #[test]
    fn agent_keeps_the_given_proxy() {
        let agent = agent(Some(socks_proxy("socks5://127.0.0.1:9050").unwrap()));
        let proxy = agent.config().proxy().expect("proxy set");
        assert!(!proxy.resolve_target());
        assert_eq!(agent.config().max_redirects(), MAX_REDIRECTS);
    }

    #[test]
    fn bad_proxy_is_an_endpoint_error() {
        assert!(matches!(socks_proxy("not a url"), Err(Error::Endpoint(_))));
    }
}
