//! Integration tests for the HTTP CONNECT client against a scripted mock
//! proxy, plus unit tests for the IP echo response parser.

use std::net::IpAddr;

use mirage_proxy::check::parse_ipify_response;
use mirage_proxy::error::ProxyError;
use mirage_proxy::http_proxy::http_connect;
use mirage_proxy::{ProxyConfig, ProxyScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Read one HTTP request head (up to and including the empty line).
async fn read_request_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await.expect("mock: read");
        head.push(byte[0]);
        assert!(head.len() <= 64 * 1024, "mock: request head too large");
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(head).expect("mock: request must be valid UTF-8")
}

fn config(port: u16, username: Option<&str>, password: Option<&str>) -> ProxyConfig {
    ProxyConfig {
        scheme: ProxyScheme::Http,
        host: "127.0.0.1".to_owned(),
        port,
        username: username.map(str::to_owned),
        password: password.map(str::to_owned),
    }
}

#[tokio::test]
async fn http_connect_sends_connect_line_and_basic_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let request = read_request_head(&mut sock).await;
        sock.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .expect("mock: write 200");
        request
    });

    let cfg = config(port, Some("user"), Some("pass"));
    let _tunnel = http_connect(&cfg, "93.184.216.34", 443)
        .await
        .expect("http_connect must succeed");

    let request = mock.await.expect("mock task");
    assert!(request.starts_with("CONNECT 93.184.216.34:443 HTTP/1.1\r\n"));
    assert!(request.contains("Host: 93.184.216.34:443\r\n"));
    // base64("user:pass") == "dXNlcjpwYXNz" (known answer, exercises the
    // hand-rolled encoder end to end).
    assert!(request.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"));
    assert!(request.ends_with("\r\n\r\n"));
}

#[tokio::test]
async fn http_connect_407_maps_to_connect_failed() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let _ = read_request_head(&mut sock).await;
        sock.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("mock: write 407");
    });

    let cfg = config(port, Some("user"), Some("bad"));
    let err = http_connect(&cfg, "93.184.216.34", 443)
        .await
        .expect_err("407 must fail");
    assert!(matches!(err, ProxyError::ConnectFailed { status: 407 }));

    mock.await.expect("mock task");
}

#[tokio::test]
async fn http_connect_without_credentials_omits_auth_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let request = read_request_head(&mut sock).await;
        sock.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .expect("mock: write 200");
        request
    });

    let cfg = config(port, None, None);
    let _tunnel = http_connect(&cfg, "example.com", 80)
        .await
        .expect("http_connect must succeed");

    let request = mock.await.expect("mock task");
    assert!(!request.contains("Proxy-Authorization"));
    assert!(request.starts_with("CONNECT example.com:80 HTTP/1.1\r\n"));
}

#[tokio::test]
async fn http_connect_pads_base64_and_fills_missing_password() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let request = read_request_head(&mut sock).await;
        sock.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .expect("mock: write 200");
        request
    });

    // A lone username still counts as "auth configured": the missing password
    // is sent as an empty string, so the token is base64("abc:") == "YWJjOg==".
    let cfg = config(port, Some("abc"), None);
    let _tunnel = http_connect(&cfg, "93.184.216.34", 80)
        .await
        .expect("http_connect must succeed");

    let request = mock.await.expect("mock task");
    assert!(request.contains("Proxy-Authorization: Basic YWJjOg==\r\n"));
}

#[tokio::test]
async fn http_connect_brackets_ipv6_literal_target() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    let mock = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let request = read_request_head(&mut sock).await;
        sock.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .expect("mock: write 200");
        request
    });

    let cfg = config(port, None, None);
    let _tunnel = http_connect(&cfg, "2606:2800:220:1:248:1893:25c8:1946", 443)
        .await
        .expect("http_connect must succeed");

    let request = mock.await.expect("mock task");
    assert!(request.starts_with("CONNECT [2606:2800:220:1:248:1893:25c8:1946]:443 HTTP/1.1\r\n"));
    assert!(request.contains("Host: [2606:2800:220:1:248:1893:25c8:1946]:443\r\n"));
}

#[test]
fn parse_ipify_response_extracts_ipv4_body() {
    let raw =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 12\r\n\r\n203.0.113.7\n";
    let ip = parse_ipify_response(raw).expect("parse must succeed");
    assert_eq!(ip, IpAddr::from([203, 0, 113, 7]));
}

#[test]
fn parse_ipify_response_extracts_ipv6_body() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n2001:db8::1\r\n";
    let ip = parse_ipify_response(raw).expect("parse must succeed");
    assert_eq!(ip, "2001:db8::1".parse::<IpAddr>().expect("literal"));
}

#[test]
fn parse_ipify_response_rejects_non_ip_body() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    assert!(matches!(
        parse_ipify_response(raw),
        Err(ProxyError::InvalidResponse)
    ));
}

#[test]
fn parse_ipify_response_rejects_missing_header_separator() {
    assert!(matches!(
        parse_ipify_response(b"203.0.113.7"),
        Err(ProxyError::InvalidResponse)
    ));
}

#[test]
fn parse_ipify_response_rejects_error_status() {
    let raw = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n";
    assert!(matches!(
        parse_ipify_response(raw),
        Err(ProxyError::Handshake { .. })
    ));
}
