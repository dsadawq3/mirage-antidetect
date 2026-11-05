//! HTTP CONNECT tunneling for plain HTTP proxies.
//!
//! The request is written by hand and the response head is read one byte at a
//! time: the function returns a raw `TcpStream`, so not a single byte past the
//! header terminator may ever be consumed — that byte would be tunneled data
//! with no way to push it back into the stream.

use std::net::Ipv6Addr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::ProxyError;
use crate::{parse_status_line, read_exact_timed, write_all_timed, ProxyConfig};

/// Budget for establishing the TCP connection to the proxy itself.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Total budget for reading the CONNECT response head. A per-byte timeout
/// alone would let a byte-dribbling peer stretch the read unboundedly, so the
/// whole head must arrive within this window.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard cap for the CONNECT response head, guarding against a broken proxy
/// that never terminates its header section.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// Open a CONNECT tunnel through the HTTP proxy `cfg` to `target_host:target_port`.
///
/// The target is sent as-is (host or IP literal — HTTP proxies resolve names
/// themselves). On success the returned `TcpStream` is the ready-to-use tunnel.
pub async fn http_connect(
    cfg: &ProxyConfig,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, ProxyError> {
    // 1. Dial the proxy.
    let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(cfg.endpoint()))
        .await
        .map_err(|_| ProxyError::Timeout)?
        .map_err(ProxyError::Io)?;

    // 2. Build the CONNECT request by hand. An IPv6 literal must be bracketed
    //    in the authority form (RFC 3986 §3.2.2): without brackets the colons
    //    inside the address cannot be told apart from the host:port separator.
    let authority = if target_host.starts_with('[') {
        format!("{target_host}:{target_port}")
    } else if target_host.parse::<Ipv6Addr>().is_ok() {
        format!("[{target_host}]:{target_port}")
    } else {
        format!("{target_host}:{target_port}")
    };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if cfg.has_credentials() {
        let user = cfg.username.as_deref().unwrap_or("");
        let pass = cfg.password.as_deref().unwrap_or("");
        request.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64_encode(format!("{user}:{pass}").as_bytes())
        ));
    }
    request.push_str("\r\n");
    write_all_timed(&mut stream, request.as_bytes()).await?;

    // 3. Read the full response head (status line plus every header line,
    //    up to and including the empty line) and check the status code.
    //    The whole head read is bounded by one overall deadline.
    let head = timeout(HEAD_TIMEOUT, read_head(&mut stream))
        .await
        .map_err(|_| ProxyError::Timeout)??;
    let status = parse_status_line(&head)?;
    if status == 200 {
        Ok(stream)
    } else {
        Err(ProxyError::ConnectFailed { status })
    }
}

/// Read the response head byte by byte until the `\r\n\r\n` terminator.
///
/// Byte granularity is deliberate: everything after the terminator belongs to
/// the tunnel, and a raw `TcpStream` cannot unread bytes. Header sections are
/// small and this runs once per connection, so the cost is irrelevant.
async fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>, ProxyError> {
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        read_exact_timed(stream, &mut byte, "connect_response").await?;
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            return Ok(head);
        }
        if head.len() >= MAX_HEAD_BYTES {
            return Err(ProxyError::Handshake {
                step: "connect_response",
                detail: format!("response head exceeds {MAX_HEAD_BYTES} bytes"),
            });
        }
    }
}

/// Standard base64 (RFC 4648, padded), hand-rolled to keep the dependency
/// surface at tokio/thiserror/anyhow.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |&b| u32::from(b));
        let b2 = chunk.get(2).map_or(0, |&b| u32::from(b));
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) as usize) & 0x3f] as char);
        out.push(ALPHABET[((triple >> 12) as usize) & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) as usize) & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple as usize) & 0x3f] as char
        } else {
            '='
        });
    }
    out
}
