//! mirage-proxy — proxy connectivity for Mirage profiles.
//!
//! The browser itself routes traffic through the profile proxy via Chromium's
//! `--proxy-server` flag; this crate exists to validate a proxy *before* a
//! profile is launched (exit-IP check) and to back the future profile API.
//! No HTTP client library is used: SOCKS5 (RFC 1928/1929) and HTTP CONNECT
//! are implemented by hand on top of `tokio`'s `TcpStream`.

pub mod check;
pub mod error;
pub mod http_proxy;
pub mod socks5;

pub use error::ProxyError;

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Budget for a single proxy-side I/O operation during a handshake.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Proxy transport protocol of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProxyScheme {
    /// Plain HTTP proxy (CONNECT tunneling).
    Http,
    /// SOCKS5 proxy (RFC 1928, optional RFC 1929 auth).
    Socks5,
}

/// Proxy endpoint description.
///
/// Field-for-field identical to `Profile.proxy` from the Mirage CONTRACT
/// (`scheme`, `host`, `port`, `username`, `password`). It is deliberately
/// defined locally so this crate does not depend on mirage-core; converting
/// between the two identical structs is trivial (same field names and types,
/// a plain `From` impl if one is ever needed).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProxyConfig {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    /// True when any credential is configured, meaning RFC 1929 / Basic auth
    /// is offered. RFC 1929 permits empty strings, so a lone username or
    /// password still counts as "auth configured" and is sent with the other
    /// field as an empty string.
    pub fn has_credentials(&self) -> bool {
        self.username.is_some() || self.password.is_some()
    }

    /// `host:port` form used to dial the proxy.
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Write the whole buffer with a bounded timeout.
pub(crate) async fn write_all_timed(stream: &mut TcpStream, buf: &[u8]) -> Result<(), ProxyError> {
    match timeout(IO_TIMEOUT, stream.write_all(buf)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(ProxyError::Io(e)),
        Err(_) => Err(ProxyError::Timeout),
    }
}

/// Read exactly `buf.len()` bytes with a bounded timeout. A short read is
/// reported as a handshake failure at `step` — the peer hung up mid-handshake.
pub(crate) async fn read_exact_timed(
    stream: &mut TcpStream,
    buf: &mut [u8],
    step: &'static str,
) -> Result<(), ProxyError> {
    match timeout(IO_TIMEOUT, stream.read_exact(buf)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => Err(ProxyError::Handshake {
            step,
            detail: "peer closed the connection mid-handshake".to_owned(),
        }),
        Ok(Err(e)) => Err(ProxyError::Io(e)),
        Err(_) => Err(ProxyError::Timeout),
    }
}

/// Extract the status code from the first line of an HTTP response head.
/// `head` is the byte section preceding the `\r\n\r\n` terminator and may or
/// may not end with the final CRLF of the last header line.
pub(crate) fn parse_status_line(head: &[u8]) -> Result<u16, ProxyError> {
    let line_end = head.iter().position(|&b| b == b'\n').unwrap_or(head.len());
    let line = std::str::from_utf8(&head[..line_end]).map_err(|_| ProxyError::InvalidResponse)?;
    let line = line.trim_end(); // drop the trailing CR when present

    let mut parts = line.split_whitespace();
    let version = parts.next().ok_or(ProxyError::InvalidResponse)?;
    if !version.starts_with("HTTP/") {
        return Err(ProxyError::InvalidResponse);
    }
    let code = parts.next().ok_or(ProxyError::InvalidResponse)?;
    code.parse::<u16>().map_err(|_| ProxyError::InvalidResponse)
}
