//! Exit-IP check: dial an IP echo endpoint through the proxy with a
//! hand-written HTTP/1.1 exchange and parse the address it reports.

use std::net::IpAddr;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::error::ProxyError;
use crate::http_proxy::http_connect;
use crate::socks5::socks5_connect;
use crate::{parse_status_line, write_all_timed, ProxyConfig, ProxyScheme};

/// The echo endpoint used for the check. It answers with the caller's IPv4
/// address in plain text; the parser also accepts IPv6 text so the same code
/// works for v6 endpoints later.
const IP_ECHO_HOST: &str = "api.ipify.org";

/// Hard cap on the HTTP response, guarding against a proxy that streams forever.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Determine the exit IP address the outside world sees when traffic goes
/// through `cfg`.
///
/// The whole check — DNS, proxy handshake, HTTP request and response — must
/// complete within `timeout`.
pub async fn check_exit_ip(cfg: &ProxyConfig, timeout: Duration) -> Result<IpAddr, ProxyError> {
    // Fully qualified call: the `timeout` parameter shadows the helper name.
    tokio::time::timeout(timeout, check_once(cfg))
        .await
        .map_err(|_| ProxyError::Timeout)?
}

async fn check_once(cfg: &ProxyConfig) -> Result<IpAddr, ProxyError> {
    // 1. Tunnel to the echo endpoint through the proxy.
    let mut stream = match cfg.scheme {
        ProxyScheme::Socks5 => socks5_connect(cfg, IP_ECHO_HOST, 80).await?,
        ProxyScheme::Http => http_connect(cfg, IP_ECHO_HOST, 80).await?,
    };

    // 2. Hand-written HTTP/1.1 GET; `Connection: close` makes EOF the end marker.
    let request = format!("GET / HTTP/1.1\r\nHost: {IP_ECHO_HOST}\r\nConnection: close\r\n\r\n");
    write_all_timed(&mut stream, request.as_bytes()).await?;

    // 3. Read the full response to EOF and parse the body as an IP.
    let response = read_to_eof(&mut stream).await?;
    parse_ipify_response(&response)
}

/// Read everything the peer sends until it closes the connection.
async fn read_to_eof(stream: &mut TcpStream) -> Result<Vec<u8>, ProxyError> {
    let mut response = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.map_err(ProxyError::Io)?;
        if n == 0 {
            return Ok(response);
        }
        if response.len() + n > MAX_RESPONSE_BYTES {
            return Err(ProxyError::Handshake {
                step: "exit_ip_response",
                detail: format!("response exceeds {MAX_RESPONSE_BYTES} bytes"),
            });
        }
        response.extend_from_slice(&chunk[..n]);
    }
}

/// Parse a raw HTTP response from the IP echo endpoint and return the exit IP.
///
/// Headers are split from the body at the first `\r\n\r\n`; the body is
/// trimmed and parsed as an `IpAddr` (both IPv4 and IPv6 text are accepted).
/// A non-200 status or a body that is not an IP address is an error.
pub fn parse_ipify_response(response: &[u8]) -> Result<IpAddr, ProxyError> {
    let sep = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(ProxyError::InvalidResponse)?;
    let (head, rest) = response.split_at(sep);
    let body = &rest[4..];

    let status = parse_status_line(head)?;
    if status != 200 {
        return Err(ProxyError::Handshake {
            step: "exit_ip_response",
            detail: format!("unexpected HTTP status {status}, expected 200"),
        });
    }

    let text = std::str::from_utf8(body).map_err(|_| ProxyError::InvalidResponse)?;
    text.trim()
        .parse::<IpAddr>()
        .map_err(|_| ProxyError::InvalidResponse)
}
