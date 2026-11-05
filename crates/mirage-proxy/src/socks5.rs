//! SOCKS5 client (RFC 1928) with username/password auth (RFC 1929).
//!
//! Only the CONNECT command is implemented — that is all a tunnel needs.
//! The target is always resolved locally to an `IpAddr` before CONNECT:
//! IPv4 goes out as ATYP 0x01, IPv6 as ATYP 0x04. A domain (ATYP 0x03)
//! variant is intentionally not sent; add it only if a proxy ever requires
//! remote resolution.

use std::io;
use std::net::IpAddr;
use std::time::Duration;

use tokio::net::{lookup_host, TcpStream};
use tokio::time::timeout;

use crate::error::ProxyError;
use crate::{read_exact_timed, write_all_timed, ProxyConfig};

/// Budget for establishing the TCP connection to the proxy itself.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Open a TCP tunnel through the SOCKS5 proxy `cfg` to `target_host:target_port`.
///
/// On success the returned `TcpStream` is the ready-to-use tunnel: everything
/// written to it is delivered to the target, everything the target sends can
/// be read from it. `target_host` is resolved locally first (IP literals never
/// hit DNS) and always sent as a literal address, never as a domain.
pub async fn socks5_connect(
    cfg: &ProxyConfig,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, ProxyError> {
    // 1. Dial the proxy.
    let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(cfg.endpoint()))
        .await
        .map_err(|_| ProxyError::Timeout)?
        .map_err(ProxyError::Io)?;

    // 2. Resolve the target up front so CONNECT always carries an IP.
    let target_ip = resolve_target(target_host, target_port).await?;

    // 3. Method negotiation (RFC 1928 §3). With credentials we offer both
    //    NO-AUTH (0x00) and USERNAME/PASSWORD (0x02); otherwise only NO-AUTH.
    let greeting: &[u8] = if cfg.has_credentials() {
        &[0x05, 0x02, 0x00, 0x02]
    } else {
        &[0x05, 0x01, 0x00]
    };
    write_all_timed(&mut stream, greeting).await?;

    let mut method_reply = [0u8; 2];
    read_exact_timed(&mut stream, &mut method_reply, "method_selection").await?;
    if method_reply[0] != 0x05 {
        return Err(ProxyError::Handshake {
            step: "method_selection",
            detail: format!("unsupported SOCKS version 0x{:02x}", method_reply[0]),
        });
    }
    match method_reply[1] {
        0x00 => { /* no authentication required */ }
        0x02 => {
            if !cfg.has_credentials() {
                return Err(ProxyError::Handshake {
                    step: "method_selection",
                    detail: "server selected username/password auth which was not offered"
                        .to_owned(),
                });
            }
            authenticate(&mut stream, cfg).await?;
        }
        0xFF => return Err(ProxyError::AuthRejected),
        other => {
            return Err(ProxyError::Handshake {
                step: "method_selection",
                detail: format!("server selected method 0x{other:02x} which was not offered"),
            })
        }
    }

    // 4. CONNECT request: VER, CMD=CONNECT, RSV, ATYP, DST.ADDR, DST.PORT.
    let mut request = Vec::with_capacity(22);
    request.extend_from_slice(&[0x05, 0x01, 0x00]);
    match target_ip {
        IpAddr::V4(v4) => {
            request.push(0x01); // ATYP: IPv4
            request.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            request.push(0x04); // ATYP: IPv6
            request.extend_from_slice(&v6.octets());
        }
    }
    request.extend_from_slice(&target_port.to_be_bytes());
    write_all_timed(&mut stream, &request).await?;

    // 5. Reply: VER REP RSV ATYP BND.ADDR BND.PORT.
    let mut head = [0u8; 4];
    read_exact_timed(&mut stream, &mut head, "connect_reply").await?;
    if head[0] != 0x05 {
        return Err(ProxyError::Handshake {
            step: "connect_reply",
            detail: format!("unsupported SOCKS version 0x{:02x}", head[0]),
        });
    }
    // REP != 0x00: report immediately without draining the rest of the reply —
    // some proxies close the socket right after a failed one. `status` carries
    // the raw REP code (1=general failure, 2=not allowed, 3=network
    // unreachable, 4=host unreachable, 5=connection refused, 6=TTL expired,
    // 7=command not supported, 8=address type not supported).
    if head[1] != 0x00 {
        return Err(ProxyError::ConnectFailed {
            status: head[1] as u16,
        });
    }
    // Drain BND.ADDR (and BND.PORT below) so the tunnel starts at a clean
    // boundary; the bound address itself carries no meaning for the client.
    let addr_len = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0u8; 1];
            read_exact_timed(&mut stream, &mut len, "connect_reply").await?;
            len[0] as usize
        }
        other => {
            return Err(ProxyError::Handshake {
                step: "connect_reply",
                detail: format!("invalid address type 0x{other:02x} in connect reply"),
            })
        }
    };
    let mut bound_addr = vec![0u8; addr_len];
    read_exact_timed(&mut stream, &mut bound_addr, "connect_reply").await?;
    let mut bound_port = [0u8; 2];
    read_exact_timed(&mut stream, &mut bound_port, "connect_reply").await?;

    Ok(stream)
}

/// RFC 1929 username/password subnegotiation.
async fn authenticate(stream: &mut TcpStream, cfg: &ProxyConfig) -> Result<(), ProxyError> {
    let user = cfg.username.as_deref().unwrap_or("");
    let pass = cfg.password.as_deref().unwrap_or("");
    if user.len() > 255 || pass.len() > 255 {
        return Err(ProxyError::Handshake {
            step: "auth",
            detail: "username and password are limited to 255 bytes each (RFC 1929)".to_owned(),
        });
    }

    let mut request = Vec::with_capacity(3 + user.len() + pass.len());
    request.push(0x01); // version of the subnegotiation
    request.push(user.len() as u8);
    request.extend_from_slice(user.as_bytes());
    request.push(pass.len() as u8);
    request.extend_from_slice(pass.as_bytes());
    write_all_timed(stream, &request).await?;

    let mut reply = [0u8; 2];
    read_exact_timed(stream, &mut reply, "auth").await?;
    // reply[0] is 0x01 per RFC 1929, but some servers answer 0x05 here; only
    // the status byte is meaningful in practice, so it is the one enforced.
    if reply[1] != 0x00 {
        return Err(ProxyError::AuthRejected);
    }
    Ok(())
}

/// Resolve `host` to an IP address through tokio's resolver (which offloads
/// to the system resolver on a blocking thread). IP literals never hit DNS.
async fn resolve_target(host: &str, port: u16) -> Result<IpAddr, ProxyError> {
    let mut addrs = lookup_host((host, port)).await.map_err(ProxyError::Io)?;
    addrs.next().map(|addr| addr.ip()).ok_or_else(|| {
        ProxyError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no addresses resolved for {host}"),
        ))
    })
}
