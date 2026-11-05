//! Error type for proxy dialing, handshakes and exit-IP checks.

use std::io;

use tokio::time::error::Elapsed;

/// Everything that can go wrong while establishing or checking a proxy connection.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// Underlying transport or DNS failure.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Protocol-level failure at a named handshake stage.
    #[error("proxy handshake failed at '{step}': {detail}")]
    Handshake { step: &'static str, detail: String },

    /// The proxy refused the offered methods or the credentials
    /// (SOCKS5 method 0xFF, or an RFC 1929 status other than 0x00).
    #[error("proxy rejected the authentication")]
    AuthRejected,

    /// The proxy refused the CONNECT request. `status` carries the HTTP
    /// status code for HTTP proxies, or the raw RFC 1928 REP code for SOCKS5.
    #[error("proxy CONNECT failed with status {status}")]
    ConnectFailed { status: u16 },

    /// A bounded wait elapsed before the operation completed.
    #[error("proxy operation timed out")]
    Timeout,

    /// The peer sent bytes that cannot be parsed as the expected protocol message.
    #[error("invalid or malformed response")]
    InvalidResponse,
}

/// Lets callers use `?` directly on `tokio::time::timeout` results.
impl From<Elapsed> for ProxyError {
    fn from(_: Elapsed) -> Self {
        ProxyError::Timeout
    }
}
