//! GoLogin import.
//!
//! Documentation used (verified):
//! - OpenAPI spec:
//!   <https://docs-download.gologin.com/openapi.json>
//!   The profile object (`CreateCustomBrowserValidation` / `OBrowser`) carries:
//!   `name`, `notes`, `os` (`lin`/`mac`/`win`/`android`/`android-cloud`/`ios`),
//!   `osSpec` (`M1..M5`, `win11`), `devicePixelRatio`,
//!   `navigator{userAgent, resolution ("1920x1080"), language, platform,
//!   hardwareConcurrency, deviceMemory, maxTouchPoints}`,
//!   `proxy{mode, host, port, username, password, changeIpUrl}` with
//!   `mode` in `http`/`socks4`/`socks5`/`possh`/`geolocation`/`none`/`gologin`/`tor`,
//!   `timezone{enabled, fillBasedOnIp, timezone}`, `webRTC{mode}` with `mode`
//!   in `alerted`/`disabled`/`real`/`public`/`fake`, `folders[]`,
//!   `webGLMetadata{mode, vendor, renderer}`, `canvas`.
//! - The list endpoint also flattens the proxy to top-level `host`/`port`/
//!   `proxyType`/`proxyEnabled` (`OBrowserListDTO`); that fallback is honored.

use serde_json::Value;

use crate::util::{
    as_u16, field, field_f64, field_str, field_u32, is_iana_timezone, normalize_languages,
    parse_proxy_fields, parse_screen, parse_ua, MappedProxy,
};
use crate::{
    build_profile, collect_profiles, first_or_error, seed_material, ImportError, ImportResult,
    ScreenSpec, SourceProfile, WebRtcMode,
};

const FORMAT: &str = "GoLogin";
const MARKERS: &[&str] = &["name", "id", "navigator", "proxy", "os", "timezone"];

/// Imports a single GoLogin profile (the first one when a list is given).
pub fn import_gologin(json: &str) -> Result<ImportResult, ImportError> {
    first_or_error(import_gologin_all(json)?, FORMAT)
}

/// Imports every GoLogin profile found in the input.
pub fn import_gologin_all(json: &str) -> Result<Vec<ImportResult>, ImportError> {
    let value: Value = serde_json::from_str(json)?;
    let profiles = collect_profiles(&value, FORMAT, MARKERS)?;
    profiles
        .into_iter()
        .map(|profile| {
            let source = to_source(profile);
            build_profile(source, &seed_material(profile))
        })
        .collect()
}

/// Maps one GoLogin profile object onto the format-agnostic intermediate.
fn to_source(value: &Value) -> SourceProfile {
    let mut source = SourceProfile {
        source_id: field_str(value, "id"),
        name: field_str(value, "name"),
        notes: field_str(value, "notes"),
        ..SourceProfile::default()
    };

    if let Some(Value::Array(folders)) = field(value, "folders") {
        for folder in folders {
            if let Some(tag) = folder.as_str().map(str::trim).filter(|t| !t.is_empty()) {
                source.tags.push(tag.to_string());
            }
        }
    }

    if let Some(os) = field_str(value, "os") {
        source.os_seen = true;
        source.os = gologin_os(&os);
    }

    let navigator = field(value, "navigator");
    if let Some(navigator) = navigator {
        source.ua = field_str(navigator, "userAgent");
        source.languages = field(navigator, "language").and_then(normalize_languages);
        if let Some(resolution) = field_str(navigator, "resolution") {
            if let Some((width, height, dpr)) = parse_screen(&resolution) {
                source.screen = Some(ScreenSpec { width, height });
                source.dpr = dpr;
            }
        }
        source.hardware_concurrency = field_u32(navigator, "hardwareConcurrency");
        source.device_memory = field_u32(navigator, "deviceMemory");
    }

    // Explicit devicePixelRatio overrides a value parsed out of `resolution`.
    if source.dpr.is_none() {
        source.dpr = field_f64(value, "devicePixelRatio");
    }

    if let Some(timezone) = field(value, "timezone") {
        let name = field_str(timezone, "timezone").or_else(|| field_str(timezone, "id"));
        if let Some(name) = name {
            source.timezone_seen = true;
            if is_iana_timezone(&name) {
                source.timezone = Some(name);
            }
        }
    }

    if let Some(mode) = field(value, "webRTC")
        .and_then(|webrtc| field_str(webrtc, "mode"))
        .as_deref()
        .and_then(gologin_webrtc)
    {
        source.webrtc = Some(mode);
    }

    if field(value, "webGLMetadata").is_some() || field(value, "webGL").is_some() {
        source.webgl_seen = true;
    }

    if let Some(geo) = field(value, "geolocation") {
        if let (Some(lat), Some(lon)) = (field_f64(geo, "latitude"), field_f64(geo, "longitude")) {
            source.geo = Some((lat, lon));
        }
    }

    source.proxy = gologin_proxy(value);

    if source.os.is_none() && !source.os_seen {
        if let Some(ua) = source.ua.as_deref() {
            source.os = parse_ua(ua).os;
        }
    }

    source
}

/// GoLogin `os` -> Mirage [`Os`] (desktop only; mobile yields `None`).
fn gologin_os(os: &str) -> Option<mirage_core::Os> {
    match os.trim().to_ascii_lowercase().as_str() {
        "win" | "win10" | "win11" | "windows" => Some(mirage_core::Os::Windows),
        "mac" | "macos" | "macm1" => Some(mirage_core::Os::Macos),
        "lin" | "linux" => Some(mirage_core::Os::Linux),
        _ => None,
    }
}

/// GoLogin `webRTC.mode` -> Mirage policy. `real` exposes the local IP;
/// `disabled` turns WebRTC off; `alerted`/`public`/`fake` route through proxy
/// candidates (relay).
fn gologin_webrtc(mode: &str) -> Option<WebRtcMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "real" => Some(WebRtcMode::Real),
        "disabled" | "off" => Some(WebRtcMode::Off),
        "alerted" | "public" | "fake" | "altered" => Some(WebRtcMode::Relay),
        _ => None,
    }
}

/// GoLogin `proxy` -> [`MappedProxy`]. Prefers the nested proxy object; falls
/// back to the flattened top-level `host`/`port`/`proxyType` of the list DTO.
fn gologin_proxy(value: &Value) -> MappedProxy {
    if let Some(proxy) = field(value, "proxy") {
        let kind = field_str(proxy, "mode");
        let host = field_str(proxy, "host");
        if kind.is_none() && host.is_none() {
            return MappedProxy::Absent;
        }
        let port = field(proxy, "port").and_then(as_u16);
        let username = field_str(proxy, "username");
        let password = field_str(proxy, "password");
        return parse_proxy_fields(kind.as_deref(), host, port, username, password);
    }

    if let Some(host) = field_str(value, "host") {
        let port = field(value, "port").and_then(as_u16);
        let kind = field_str(value, "proxyType");
        return parse_proxy_fields(kind.as_deref(), Some(host), port, None, None);
    }

    MappedProxy::Absent
}

#[cfg(test)]
mod tests {
    use super::*;
    use mirage_core::{Os, ProxyScheme};

    #[test]
    fn os_mapping() {
        assert_eq!(gologin_os("win"), Some(Os::Windows));
        assert_eq!(gologin_os("macM1"), Some(Os::Macos));
        assert_eq!(gologin_os("lin"), Some(Os::Linux));
        assert_eq!(gologin_os("android"), None);
    }

    #[test]
    fn webrtc_mapping() {
        assert_eq!(gologin_webrtc("real"), Some(WebRtcMode::Real));
        assert_eq!(gologin_webrtc("disabled"), Some(WebRtcMode::Off));
        assert_eq!(gologin_webrtc("alerted"), Some(WebRtcMode::Relay));
        assert_eq!(gologin_webrtc("nonsense"), None);
    }

    #[test]
    fn proxy_nested_and_flat() {
        let nested =
            serde_json::json!({"proxy": {"mode": "socks5", "host": "1.2.3.4", "port": 1080}});
        match gologin_proxy(&nested) {
            MappedProxy::Parsed(config) => assert_eq!(config.scheme, ProxyScheme::Socks5),
            other => panic!("expected parsed, got {other:?}"),
        }
        let unsupported = serde_json::json!({"proxy": {"mode": "gologin", "host": "x", "port": 1}});
        assert_eq!(gologin_proxy(&unsupported), MappedProxy::Unsupported);
        let flat = serde_json::json!({"host": "5.6.7.8", "port": 8080, "proxyType": "http"});
        assert!(matches!(gologin_proxy(&flat), MappedProxy::Parsed(_)));
        let none = serde_json::json!({"name": "x"});
        assert_eq!(gologin_proxy(&none), MappedProxy::Absent);
    }
}
