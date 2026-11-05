//! Shared helpers for competitor profile import.
//!
//! Everything here is format-agnostic: UA parsing, screen-string parsing,
//! language normalization, proxy classification and the deterministic seed
//! hash. The per-format modules (`adspower`, `dolphin`, `gologin`) only pull
//! raw strings out of the source JSON and feed them to these helpers, so the
//! mapping rules stay in one place.

use mirage_core::{Os, ProxyConfig, ProxyScheme};
use serde_json::Value;

/// FNV-1a 64-bit hash.
///
/// This is the seed derivation for the reproducible-import guarantee: the
/// profile id of the source record is hashed into a `u64` that is handed to
/// `mirage_core::generate`. FNV-1a is trivial, dependency-free and stable
/// across runs and platforms, which is exactly what a reproducible seed needs.
pub(crate) fn fnv1a64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV offset basis
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
    }
    hash
}

/// Characteristics of a source user-agent string that matter to a Mirage
/// fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UaInfo {
    /// Operating system inferred from the UA token.
    pub os: Option<Os>,
    /// Major Chrome/Chromium version extracted from the `Chrome/<n>.` token.
    pub chrome_major: Option<u32>,
    /// True when the UA looks like a Chromium-family browser (Chrome,
    /// Chromium or Chrome-on-iOS). Firefox/Safari UAs are not usable by Mirage,
    /// which ships a Chrome engine.
    pub chromium: bool,
}

/// Parses an OS / platform / Chrome-major out of a UA string.
pub(crate) fn parse_ua(ua: &str) -> UaInfo {
    let lower = ua.to_ascii_lowercase();
    let os = if lower.contains("windows") {
        Some(Os::Windows)
    } else if lower.contains("macintosh") || lower.contains("mac os x") {
        Some(Os::Macos)
    } else if lower.contains("linux") && !lower.contains("android") {
        Some(Os::Linux)
    } else {
        None
    };
    let chromium =
        lower.contains("chrome/") || lower.contains("chromium/") || lower.contains("crios/");
    let chrome_major = find_version_major(&lower, "chrome/")
        .or_else(|| find_version_major(&lower, "chromium/"))
        .or_else(|| find_version_major(&lower, "crios/"));
    UaInfo {
        os,
        chrome_major,
        chromium,
    }
}

/// Returns the leading integer right after `needle` in `haystack`.
fn find_version_major(haystack: &str, needle: &str) -> Option<u32> {
    let index = haystack.find(needle)? + needle.len();
    let digits: String = haystack[index..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Normalizes a screen string into `(width, height, dpr?)`.
///
/// Accepts the separators used across the competitors: `x`/`X` (GoLogin
/// `navigator.resolution`, Dolphin `screen.resolution`), `_` (AdsPower
/// `screen_resolution`, e.g. `"1024_600"`) and `*`. An optional third component
/// carries the device pixel ratio (`"1920x1080x1.5"`). AdsPower placeholders
/// (`"none"`, `"random"`) and malformed values yield `None`.
pub(crate) fn parse_screen(input: &str) -> Option<(u32, u32, Option<f64>)> {
    let cleaned = input.trim();
    if cleaned.is_empty() {
        return None;
    }
    let lowered = cleaned.to_ascii_lowercase();
    if matches!(lowered.as_str(), "none" | "random" | "default" | "auto") {
        return None;
    }
    let parts: Vec<&str> = cleaned
        .split(['x', 'X', '_', '*', '×'])
        .filter(|part| !part.trim().is_empty())
        .collect();
    if parts.len() < 2 {
        return None;
    }
    let width: u32 = parts[0].trim().parse().ok()?;
    let height: u32 = parts[1].trim().parse().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let dpr = parts.get(2).and_then(|p| p.trim().parse::<f64>().ok());
    Some((width, height, dpr))
}

/// Normalizes a language value (array of strings, or a single `"en-US"` string)
/// into a non-empty language list. A single string also yields its base code
/// (`"en-US"` -> `["en-US", "en"]`) to match how Mirage lists accept-languages.
pub(crate) fn normalize_languages(value: &Value) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    match value {
        Value::Array(items) => {
            for item in items {
                if let Some(entry) = as_string(item) {
                    out.push(entry);
                }
            }
        }
        Value::String(_) => {
            if let Some(entry) = as_string(value) {
                if let Some(base) = entry.split(['-', '_']).next() {
                    if !base.is_empty() && base != entry {
                        let with_base = vec![entry.clone(), base.to_string()];
                        return Some(with_base);
                    }
                }
                out.push(entry);
            }
        }
        _ => {}
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// True when `timezone` looks like an IANA zone name (`Area/Location`).
pub(crate) fn is_iana_timezone(timezone: &str) -> bool {
    let value = timezone.trim();
    if !value.contains('/') || value.contains(' ') || value.starts_with('/') {
        return false;
    }
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
}

/// Outcome of classifying a source proxy block against Mirage's supported
/// transports (`http` and `socks5` only, per CONTRACT).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum MappedProxy {
    /// No proxy in the source (or an explicit no-proxy marker).
    #[default]
    Absent,
    /// A proxy carrying a transport Mirage supports natively.
    Parsed(ProxyConfig),
    /// A proxy forced onto the closest supported transport (socks4 -> socks5).
    /// The original transport is reported as ignored so the caller can warn.
    Approximated(ProxyConfig),
    /// A proxy block was present but cannot be represented (gologin/tor/... or
    /// missing host/port).
    Unsupported,
}

/// Builds a [`MappedProxy`] from the raw proxy fields shared by all formats.
pub(crate) fn parse_proxy_fields(
    kind: Option<&str>,
    host: Option<String>,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
) -> MappedProxy {
    // A missing transport defaults to http, the most common proxy kind.
    let kind = kind.unwrap_or("http").trim().to_ascii_lowercase();
    if matches!(
        kind.as_str(),
        "no_proxy" | "none" | "direct" | "without" | "off" | ""
    ) {
        return MappedProxy::Absent;
    }
    let scheme = match kind.as_str() {
        "http" | "https" => ProxyScheme::Http,
        "socks5" | "socks" | "socks5h" => ProxyScheme::Socks5,
        "socks4" | "socks4a" => ProxyScheme::Socks5,
        _ => return MappedProxy::Unsupported,
    };
    let approximate = kind.starts_with("socks4");
    match (host, port) {
        (Some(host), Some(port)) if !host.trim().is_empty() && port != 0 => {
            let config = ProxyConfig {
                scheme,
                host: host.trim().to_string(),
                port,
                username: username.and_then(|u| non_empty(&u)),
                password: password.and_then(|p| non_empty(&p)),
            };
            if approximate {
                MappedProxy::Approximated(config)
            } else {
                MappedProxy::Parsed(config)
            }
        }
        _ => MappedProxy::Unsupported,
    }
}

/// Parses a `scheme://user:pass@host:port` proxy URL into a [`ProxyConfig`].
pub(crate) fn parse_proxy_url(url: &str) -> Option<ProxyConfig> {
    let (scheme_str, rest) = url.split_once("://")?;
    let (credentials, host_port) = match rest.rsplit_once('@') {
        Some((creds, hp)) => (Some(creds), hp),
        None => (None, rest),
    };
    let (host, port) = host_port.rsplit_once(':')?;
    if host.trim().is_empty() {
        return None;
    }
    let port: u16 = port.trim().parse().ok()?;
    let (username, password) =
        match credentials.and_then(|c| c.split_once(':').map(|(u, p)| (u, Some(p)))) {
            Some((u, p)) => (non_empty(u), p.and_then(non_empty)),
            None => (credentials.and_then(non_empty), None),
        };
    let kind = Some(scheme_str.to_string());
    match parse_proxy_fields(
        kind.as_deref(),
        Some(host.to_string()),
        Some(port),
        username,
        password,
    ) {
        MappedProxy::Parsed(config) | MappedProxy::Approximated(config) => Some(config),
        _ => None,
    }
}

/// Trims a string and returns `None` when the result is empty.
pub(crate) fn non_empty(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Reads a value as a trimmed non-empty string (numbers coerced too).
pub(crate) fn as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => non_empty(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Reads a value as `u32` (JSON number or numeric string).
pub(crate) fn as_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(n) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Reads a value as `u16` (JSON number or numeric string).
pub(crate) fn as_u16(value: &Value) -> Option<u16> {
    match value {
        Value::Number(n) => n.as_u64().and_then(|v| u16::try_from(v).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Reads a value as `f64` (JSON number or numeric string).
pub(crate) fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Object field access: `field(value, "name")`.
pub(crate) fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|object| object.get(key))
}

/// Convenience: object field as a trimmed string.
pub(crate) fn field_str(value: &Value, key: &str) -> Option<String> {
    field(value, key).and_then(as_string)
}

/// Convenience: object field as `u32`.
pub(crate) fn field_u32(value: &Value, key: &str) -> Option<u32> {
    field(value, key).and_then(as_u32)
}

/// Convenience: object field as `f64`.
pub(crate) fn field_f64(value: &Value, key: &str) -> Option<f64> {
    field(value, key).and_then(as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_is_stable_and_distinct() {
        assert_eq!(fnv1a64("abc"), fnv1a64("abc"));
        assert_ne!(fnv1a64("abc"), fnv1a64("abd"));
        // Known FNV-1a 64 test vector.
        assert_eq!(fnv1a64("a"), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn parse_ua_detects_os_and_major() {
        let win = parse_ua("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36");
        assert_eq!(win.os, Some(Os::Windows));
        assert_eq!(win.chrome_major, Some(156));
        assert!(win.chromium);

        let mac = parse_ua("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.6998.36 Safari/537.36");
        assert_eq!(mac.os, Some(Os::Macos));

        let linux = parse_ua("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36");
        assert_eq!(linux.os, Some(Os::Linux));

        let firefox = parse_ua(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:120.0) Gecko/20100101 Firefox/120.0",
        );
        assert!(!firefox.chromium);
        assert_eq!(firefox.os, Some(Os::Windows));
    }

    #[test]
    fn parse_screen_handles_all_separators() {
        assert_eq!(parse_screen("1920x1080"), Some((1920, 1080, None)));
        assert_eq!(parse_screen("1920X1080"), Some((1920, 1080, None)));
        assert_eq!(parse_screen("1024_600"), Some((1024, 600, None)));
        assert_eq!(parse_screen("1920x1080x1.5"), Some((1920, 1080, Some(1.5))));
        assert_eq!(parse_screen("1920_1080_2"), Some((1920, 1080, Some(2.0))));
        assert_eq!(parse_screen("none"), None);
        assert_eq!(parse_screen("random"), None);
        assert_eq!(parse_screen("garbage"), None);
        assert_eq!(parse_screen("0x1080"), None);
    }

    #[test]
    fn normalize_languages_array_and_string() {
        let arr = serde_json::json!(["en-US", "en", ""]);
        assert_eq!(
            normalize_languages(&arr),
            Some(vec!["en-US".to_string(), "en".to_string()])
        );
        let single = serde_json::json!("de-DE");
        assert_eq!(
            normalize_languages(&single),
            Some(vec!["de-DE".to_string(), "de".to_string()])
        );
        let empty = serde_json::json!([]);
        assert_eq!(normalize_languages(&empty), None);
    }

    #[test]
    fn iana_timezone_validation() {
        assert!(is_iana_timezone("Europe/London"));
        assert!(is_iana_timezone("America/New_York"));
        assert!(!is_iana_timezone("UTC+3"));
        assert!(!is_iana_timezone(""));
        assert!(!is_iana_timezone("Europe London"));
    }

    #[test]
    fn proxy_classification() {
        let http = parse_proxy_fields(Some("http"), Some("1.2.3.4".into()), Some(8080), None, None);
        assert!(matches!(http, MappedProxy::Parsed(_)));
        let socks = parse_proxy_fields(
            Some("socks5"),
            Some("1.2.3.4".into()),
            Some(1080),
            Some("u".into()),
            Some("p".into()),
        );
        match socks {
            MappedProxy::Parsed(cfg) => assert_eq!(cfg.scheme, ProxyScheme::Socks5),
            other => panic!("expected parsed socks5, got {other:?}"),
        }
        let socks4 = parse_proxy_fields(
            Some("socks4"),
            Some("1.2.3.4".into()),
            Some(1080),
            None,
            None,
        );
        assert!(matches!(socks4, MappedProxy::Approximated(_)));
        let none = parse_proxy_fields(Some("no_proxy"), None, None, None, None);
        assert_eq!(none, MappedProxy::Absent);
        let tor = parse_proxy_fields(Some("tor"), Some("h".into()), Some(9050), None, None);
        assert_eq!(tor, MappedProxy::Unsupported);
        let nohost = parse_proxy_fields(Some("http"), None, Some(80), None, None);
        assert_eq!(nohost, MappedProxy::Unsupported);
    }

    #[test]
    fn proxy_url_parsing() {
        let cfg = parse_proxy_url("http://user:pass@1.2.3.4:8080").expect("valid url");
        assert_eq!(cfg.host, "1.2.3.4");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.scheme, ProxyScheme::Http);
        assert_eq!(cfg.username.as_deref(), Some("user"));
        assert_eq!(cfg.password.as_deref(), Some("pass"));
        assert!(parse_proxy_url("not a url").is_none());
    }
}
