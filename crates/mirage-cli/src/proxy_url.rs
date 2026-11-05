//! `scheme://[user:pass@]host:port` proxy URL parsing.
//!
//! The CLI accepts one compact URL form for `--proxy` and
//! `mirage proxy check`, and converts it into the two crate-local proxy
//! structs: [`mirage_core::ProxyConfig`] (stored in a profile) and
//! [`mirage_proxy::ProxyConfig`] (used to dial the proxy before launch).

use mirage_core::{ProxyConfig as CoreProxyConfig, ProxyScheme as CoreProxyScheme};
use mirage_proxy::{ProxyConfig as ClientProxyConfig, ProxyScheme as ClientProxyScheme};

use crate::error::CliError;

/// Proxy transport accepted by the CLI. `https` is treated as `http` (tunneled
/// the same way); `socks`/`socks5h` are treated as `socks5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyUrlScheme {
    /// Plain HTTP proxy (CONNECT tunneling).
    Http,
    /// SOCKS5 proxy (RFC 1928/1929).
    Socks5,
}

impl ProxyUrlScheme {
    /// The wire name (`"http"` / `"socks5"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyUrlScheme::Http => "http",
            ProxyUrlScheme::Socks5 => "socks5",
        }
    }
}

/// A parsed proxy URL, independent of the crate that consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUrl {
    pub scheme: ProxyUrlScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyUrl {
    /// `scheme://host:port` (credentials omitted) for display.
    pub fn endpoint(&self) -> String {
        format!("{}://{}:{}", self.scheme.as_str(), self.host, self.port)
    }

    /// Convert to the mirage-core config stored inside a profile.
    pub fn to_core(&self) -> CoreProxyConfig {
        CoreProxyConfig {
            scheme: match self.scheme {
                ProxyUrlScheme::Http => CoreProxyScheme::Http,
                ProxyUrlScheme::Socks5 => CoreProxyScheme::Socks5,
            },
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }

    /// Convert to the mirage-proxy config used to dial the proxy.
    pub fn to_client(&self) -> ClientProxyConfig {
        ClientProxyConfig {
            scheme: match self.scheme {
                ProxyUrlScheme::Http => ClientProxyScheme::Http,
                ProxyUrlScheme::Socks5 => ClientProxyScheme::Socks5,
            },
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }
}

/// Parse `scheme://[user:pass@]host:port` into a [`ProxyUrl`].
///
/// Credentials are optional. IPv6 hosts may be bracketed (`[::1]:1080`).
pub fn parse_proxy_url(url: &str) -> Result<ProxyUrl, CliError> {
    let url = url.trim();
    let fail = |reason: &str| CliError::ProxyUrl {
        url: url.to_string(),
        reason: reason.to_string(),
    };

    let (scheme_text, rest) = url
        .split_once("://")
        .ok_or_else(|| fail("missing `://` separator (expected scheme://host:port)"))?;

    let scheme = match scheme_text.trim().to_ascii_lowercase().as_str() {
        "http" | "https" => ProxyUrlScheme::Http,
        "socks5" | "socks" | "socks5h" => ProxyUrlScheme::Socks5,
        other => {
            return Err(fail(&format!(
                "unsupported scheme `{other}` (expected http, https, socks5)"
            )))
        }
    };

    // Split optional credentials at the LAST `@`, so a password containing
    // `@` does not shift the host.
    let (credentials, host_port) = match rest.rsplit_once('@') {
        Some((creds, hp)) => (Some(creds), hp),
        None => (None, rest),
    };

    let (username, password) = match credentials {
        Some(creds) => match creds.split_once(':') {
            Some((user, pass)) => (non_empty(user), non_empty(pass)),
            None => (non_empty(creds), None),
        },
        None => (None, None),
    };

    // Split host:port at the LAST `:` so bracketed IPv6 hosts survive.
    let (host, port_text) = host_port
        .rsplit_once(':')
        .ok_or_else(|| fail("missing `:port`"))?;
    let host = host.trim();
    if host.is_empty() {
        return Err(fail("empty host"));
    }
    let port: u16 = port_text
        .trim()
        .parse()
        .map_err(|_| fail("port must be an integer between 1 and 65535"))?;
    if port == 0 {
        return Err(fail("port must be between 1 and 65535"));
    }

    Ok(ProxyUrl {
        scheme,
        host: host.to_string(),
        port,
        username,
        password,
    })
}

/// Trim a credential part; an empty string becomes `None`.
fn non_empty(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credentials_and_scheme() {
        let url = parse_proxy_url("socks5://user:pass@1.2.3.4:1080").expect("valid");
        assert_eq!(url.scheme, ProxyUrlScheme::Socks5);
        assert_eq!(url.host, "1.2.3.4");
        assert_eq!(url.port, 1080);
        assert_eq!(url.username.as_deref(), Some("user"));
        assert_eq!(url.password.as_deref(), Some("pass"));
        assert_eq!(url.endpoint(), "socks5://1.2.3.4:1080");
    }

    #[test]
    fn parses_without_credentials() {
        let url = parse_proxy_url("http://1.2.3.4:8080").expect("valid");
        assert_eq!(url.scheme, ProxyUrlScheme::Http);
        assert_eq!(url.username, None);
        assert_eq!(url.password, None);
    }

    #[test]
    fn https_and_socks_aliases_map_to_supported_transports() {
        assert_eq!(
            parse_proxy_url("https://h:1").expect("https").scheme,
            ProxyUrlScheme::Http
        );
        assert_eq!(
            parse_proxy_url("socks://h:1").expect("socks").scheme,
            ProxyUrlScheme::Socks5
        );
        assert_eq!(
            parse_proxy_url("socks5h://h:1").expect("socks5h").scheme,
            ProxyUrlScheme::Socks5
        );
    }

    #[test]
    fn password_with_at_sign_survives() {
        let url = parse_proxy_url("http://u:p@ss@host:1").expect("valid");
        assert_eq!(url.host, "host");
        assert_eq!(url.username.as_deref(), Some("u"));
        assert_eq!(url.password.as_deref(), Some("p@ss"));
    }

    #[test]
    fn bracketed_ipv6_host() {
        let url = parse_proxy_url("socks5://[::1]:1080").expect("valid");
        assert_eq!(url.host, "[::1]");
        assert_eq!(url.port, 1080);
    }

    #[test]
    fn rejects_malformed_urls() {
        assert!(parse_proxy_url("not a url").is_err());
        assert!(parse_proxy_url("ftp://h:1").is_err());
        assert!(parse_proxy_url("http://h").is_err());
        assert!(parse_proxy_url("http://:80").is_err());
        assert!(parse_proxy_url("http://h:0").is_err());
        assert!(parse_proxy_url("http://h:70000").is_err());
    }

    #[test]
    fn converts_to_both_configs() {
        let url = parse_proxy_url("socks5://u:p@1.2.3.4:1080").expect("valid");
        let core = url.to_core();
        assert_eq!(core.scheme, CoreProxyScheme::Socks5);
        assert_eq!(core.host, "1.2.3.4");
        assert_eq!(core.port, 1080);
        assert_eq!(core.username.as_deref(), Some("u"));
        let client = url.to_client();
        assert_eq!(client.scheme, ClientProxyScheme::Socks5);
        assert_eq!(client.endpoint(), "1.2.3.4:1080");
    }
}
