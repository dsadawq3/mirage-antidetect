//! Integration tests for the SOCKS5 client against a scripted in-process mock
//! server. Targets are passed as IP literals so nothing ever touches DNS.

use std::net::IpAddr;

use mirage_proxy::error::ProxyError;
use mirage_proxy::socks5::socks5_connect;
use mirage_proxy::{ProxyConfig, ProxyScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Read exactly `n` bytes from the mock connection.
async fn read_exact(stream: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf).await.expect("mock: read_exact");
    buf
}

/// Read a client greeting (VER NMETHODS METHODS...) and return the method list.
async fn read_greeting(stream: &mut TcpStream) -> Vec<u8> {
    let head = read_exact(stream, 2).await;
    assert_eq!(head[0], 0x05, "greeting VER must be 0x05");
    read_exact(stream, head[1] as usize).await
}

/// Read a full CONNECT request, returning (atyp, address bytes, port).
async fn read_connect_request(stream: &mut TcpStream) -> (u8, Vec<u8>, u16) {
    let head = read_exact(stream, 4).await;
    assert_eq!(
        &head[..3],
        &[0x05, 0x01, 0x00],
        "CONNECT must be VER=5 CMD=1 RSV=0"
    );
    let atyp = head[3];
    let addr = match atyp {
        0x01 => read_exact(stream, 4).await,
        0x04 => read_exact(stream, 16).await,
        0x03 => {
            let len = read_exact(stream, 1).await[0] as usize;
            read_exact(stream, len).await
        }
        other => panic!("mock: unexpected ATYP 0x{other:02x}"),
    };
    let port_bytes = read_exact(stream, 2).await;
    (
        atyp,
        addr,
        u16::from_be_bytes([port_bytes[0], port_bytes[1]]),
    )
}

/// Read an RFC 1929 auth request, returning (username, password).
async fn read_auth_request(stream: &mut TcpStream) -> (Vec<u8>, Vec<u8>) {
    let ver = read_exact(stream, 1).await;
    assert_eq!(ver[0], 0x01, "RFC 1929 VER must be 0x01");
    let ulen = read_exact(stream, 1).await[0] as usize;
    let user = read_exact(stream, ulen).await;
    let plen = read_exact(stream, 1).await[0] as usize;
    let pass = read_exact(stream, plen).await;
    (user, pass)
}

/// Send a successful SOCKS5 CONNECT reply (BND.ADDR = 127.0.0.1, BND.PORT = 0).
async fn write_success_reply(stream: &mut TcpStream) {
    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0x00, 0x00])
        .await
        .expect("mock: write success reply");
}

fn config(port: u16, username: Option<&str>, password: Option<&str>) -> ProxyConfig {
    ProxyConfig {
        scheme: ProxyScheme::Socks5,
        host: "127.0.0.1".to_owned(),
        port,
        username: username.map(str::to_owned),
        password: password.map(str::to_owned),
    }
}

#[tokio::test]
async fn socks5_connect_no_auth_sends_resolved_ipv4_target() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let methods = read_greeting(&mut sock).await;
        assert_eq!(methods, vec![0x00], "no-auth config must offer only 0x00");
        sock.write_all(&[0x05, 0x00])
            .await
            .expect("write method reply");

        let (atyp, addr, port) = read_connect_request(&mut sock).await;
        write_success_reply(&mut sock).await;
        (atyp, addr, port)
    });

    let cfg = config(port, None, None);
    let _tunnel = socks5_connect(&cfg, "93.184.216.34", 443)
        .await
        .expect("socks5_connect must succeed");

    let (atyp, addr, target_port) = mock.await.expect("mock task");
    assert_eq!(atyp, 0x01, "IPv4 target must use ATYP 0x01");
    assert_eq!(addr, vec![93, 184, 216, 34]);
    assert_eq!(target_port, 443);
}

#[tokio::test]
async fn socks5_connect_rejects_when_no_acceptable_method() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let methods = read_greeting(&mut sock).await;
        assert_eq!(methods, vec![0x00]);
        // No acceptable methods: none of the offered ones is allowed here.
        sock.write_all(&[0x05, 0xFF]).await.expect("write 0xFF");
    });

    let cfg = config(port, None, None);
    let err = socks5_connect(&cfg, "93.184.216.34", 80)
        .await
        .expect_err("0xFF method selection must fail");
    assert!(matches!(err, ProxyError::AuthRejected));

    mock.await.expect("mock task");
}

#[tokio::test]
async fn socks5_connect_username_password_flow() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let methods = read_greeting(&mut sock).await;
        assert_eq!(
            methods,
            vec![0x00, 0x02],
            "auth config must offer 0x00 and 0x02"
        );
        // The server demands username/password.
        sock.write_all(&[0x05, 0x02])
            .await
            .expect("write method reply");

        let (user, pass) = read_auth_request(&mut sock).await;
        // Authentication succeeds.
        sock.write_all(&[0x01, 0x00])
            .await
            .expect("write auth status");

        let (atyp, addr, port) = read_connect_request(&mut sock).await;
        write_success_reply(&mut sock).await;
        (user, pass, atyp, addr, port)
    });

    let cfg = config(port, Some("mirage-user"), Some("s3cret-pass"));
    let _tunnel = socks5_connect(&cfg, "198.51.100.7", 8080)
        .await
        .expect("socks5_connect must succeed");

    let (user, pass, atyp, addr, target_port) = mock.await.expect("mock task");
    assert_eq!(user, b"mirage-user".to_vec());
    assert_eq!(pass, b"s3cret-pass".to_vec());
    assert_eq!(atyp, 0x01);
    assert_eq!(addr, vec![198, 51, 100, 7]);
    assert_eq!(target_port, 8080);
}

#[tokio::test]
async fn socks5_connect_bad_credentials_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let methods = read_greeting(&mut sock).await;
        assert_eq!(methods, vec![0x00, 0x02]);
        sock.write_all(&[0x05, 0x02])
            .await
            .expect("write method reply");

        let _ = read_auth_request(&mut sock).await;
        // Authentication fails.
        sock.write_all(&[0x01, 0x01])
            .await
            .expect("write auth status");
    });

    let cfg = config(port, Some("user"), Some("wrong"));
    let err = socks5_connect(&cfg, "93.184.216.34", 80)
        .await
        .expect_err("auth failure must surface");
    assert!(matches!(err, ProxyError::AuthRejected));

    mock.await.expect("mock task");
}

#[tokio::test]
async fn socks5_connect_ipv6_target_uses_atyp4() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let expected: IpAddr = "2606:2800:220:1:248:1893:25c8:1946"
        .parse()
        .expect("parse IPv6 literal");
    let expected_octets = match expected {
        IpAddr::V6(v6) => v6.octets(),
        _ => panic!("expected an IPv6 literal"),
    };

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let methods = read_greeting(&mut sock).await;
        assert_eq!(methods, vec![0x00]);
        sock.write_all(&[0x05, 0x00])
            .await
            .expect("write method reply");

        let (atyp, addr, port) = read_connect_request(&mut sock).await;
        write_success_reply(&mut sock).await;
        (atyp, addr, port)
    });

    let cfg = config(port, None, None);
    let _tunnel = socks5_connect(&cfg, "2606:2800:220:1:248:1893:25c8:1946", 443)
        .await
        .expect("socks5_connect must succeed");

    let (atyp, addr, target_port) = mock.await.expect("mock task");
    assert_eq!(atyp, 0x04, "IPv6 target must use ATYP 0x04");
    assert_eq!(addr, expected_octets.to_vec());
    assert_eq!(target_port, 443);
}
