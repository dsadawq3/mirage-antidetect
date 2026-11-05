//! Dolphin {anty} import.
//!
//! Documentation used (verified):
//! - Profile template (fields create the same profile object the app exports):
//!   <https://docs.dolphin-anty.com/en/api/api-profile-creation-template-dolphin-anty.md>
//!   Keys: `name`, `platform` (`windows`/`MacOS`/`Linux`), `browserType`,
//!   `mainWebsite`, `useragent{mode, value}`, `webrtc{mode, ipAddress}`
//!   (`altered`/`real`/...), `canvas{mode}`, `webgl{mode}`,
//!   `webglInfo{mode, vendor, renderer}`, `timezone{mode, value}`,
//!   `locale{mode, value}`, `cpu{mode, value}`, `memory{mode, value}`,
//!   `screen{mode, resolution}`, `doNotTrack`, `osVersion`.
//!
//! The creation template does not include a proxy object: Dolphin stores proxy
//! credentials per profile but does not document them on that page. The proxy
//! reader below is therefore deliberately tolerant (nested object, URL string
//! or top-level fields); it is **schema per docs, unverified against a real
//! export** for the proxy part, and simply maps nothing when the shape differs.

use serde_json::Value;

use crate::util::{
    as_u16, field, field_str, field_u32, is_iana_timezone, normalize_languages, parse_proxy_fields,
    parse_proxy_url, parse_screen, parse_ua, MappedProxy,
};
use crate::{
    build_profile, collect_profiles, first_or_error, seed_material, ImportError, ImportResult,
    ScreenSpec, SourceProfile, WebRtcMode,
};

const FORMAT: &str = "Dolphin Anty";
const MARKERS: &[&str] = &["name", "platform", "useragent", "browserType", "useragent"];

/// Imports a single Dolphin {anty} profile (the first one when a list is given).
pub fn import_dolphin(json: &str) -> Result<ImportResult, ImportError> {
    first_or_error(import_dolphin_all(json)?, FORMAT)
}

/// Imports every Dolphin {anty} profile found in the input.
pub fn import_dolphin_all(json: &str) -> Result<Vec<ImportResult>, ImportError> {
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

/// Maps one Dolphin profile object onto the format-agnostic intermediate.
fn to_source(value: &Value) -> SourceProfile {
    let mut source = SourceProfile {
        source_id: field_str(value, "id").or_else(|| field_str(value, "_id")),
        name: field_str(value, "name"),
        ..SourceProfile::default()
    };

    if let Some(platform) = field_str(value, "platform") {
        source.os_seen = true;
        source.os = dolphin_platform(&platform);
    }

    source.ua = useragent_string(value);

    if let Some(locale) = nested_value(value, "locale") {
        source.languages = normalize_languages(locale);
    }

    if let Some(timezone) = nested_value(value, "timezone").and_then(Value::as_str) {
        source.timezone_seen = true;
        if is_iana_timezone(timezone) {
            source.timezone = Some(timezone.to_string());
        }
    }

    if let Some(resolution) =
        field(value, "screen").and_then(|screen| field_str(screen, "resolution"))
    {
        if let Some((width, height, dpr)) = parse_screen(&resolution) {
            source.screen = Some(ScreenSpec { width, height });
            source.dpr = dpr;
        }
    }

    if let Some(mode) = field(value, "webrtc")
        .and_then(|webrtc| field_str(webrtc, "mode"))
        .as_deref()
        .and_then(dolphin_webrtc)
    {
        source.webrtc = Some(mode);
    }

    source.hardware_concurrency = field(value, "cpu").and_then(|cpu| field_u32(cpu, "value"));
    source.device_memory = field(value, "memory").and_then(|mem| field_u32(mem, "value"));

    if field(value, "webgl").is_some() || field(value, "webglInfo").is_some() {
        source.webgl_seen = true;
    }

    source.proxy = dolphin_proxy(value);

    // OS fallback from the UA when the platform field is absent.
    if source.os.is_none() && !source.os_seen {
        if let Some(ua) = source.ua.as_deref() {
            source.os = parse_ua(ua).os;
        }
    }

    source
}

/// Dolphin `useragent` is `{mode, value}`; tolerate a bare string too.
fn useragent_string(value: &Value) -> Option<String> {
    match field(value, "useragent") {
        Some(Value::String(ua)) => Some(ua.clone()).filter(|ua| !ua.trim().is_empty()),
        Some(object) => field_str(object, "value"),
        None => None,
    }
}

/// Reads a `{mode, value}` object's trimmed `value` (or a bare string).
fn nested_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match field(value, key) {
        Some(Value::Object(_)) => field(value, key).and_then(|object| object.get("value")),
        Some(other) => Some(other),
        None => None,
    }
}

/// Dolphin `platform` -> Mirage [`Os`].
fn dolphin_platform(platform: &str) -> Option<mirage_core::Os> {
    match platform.trim().to_ascii_lowercase().as_str() {
        "windows" | "win" => Some(mirage_core::Os::Windows),
        "macos" | "mac" | "mac os" => Some(mirage_core::Os::Macos),
        "linux" | "lin" => Some(mirage_core::Os::Linux),
        _ => None,
    }
}

/// Dolphin `webrtc.mode` -> Mirage policy. `altered` replaces the public IP with
/// the proxy IP (relay candidates); `real` exposes the local IP.
fn dolphin_webrtc(mode: &str) -> Option<WebRtcMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "real" => Some(WebRtcMode::Real),
        "altered" | "alter" | "fake" | "public" => Some(WebRtcMode::Relay),
        "disabled" | "off" => Some(WebRtcMode::Off),
        _ => None,
    }
}

/// Tolerant proxy reader (see the module note: proxy shape is not documented on
/// the creation template). Accepts a `proxy` object, a `proxy` URL string, or
/// top-level `host`/`port` fields.
fn dolphin_proxy(value: &Value) -> MappedProxy {
    if let Some(proxy) = field(value, "proxy") {
        if let Some(url) = proxy.as_str() {
            return match parse_proxy_url(url) {
                Some(config) => MappedProxy::Parsed(config),
                None => MappedProxy::Unsupported,
            };
        }
        let kind = field_str(proxy, "type")
            .or_else(|| field_str(proxy, "mode"))
            .or_else(|| field_str(proxy, "scheme"));
        let host = field_str(proxy, "host").or_else(|| field_str(proxy, "ip"));
        let port = field(proxy, "port").and_then(as_u16);
        let username = field_str(proxy, "login")
            .or_else(|| field_str(proxy, "username"))
            .or_else(|| field_str(proxy, "user"));
        let password = field_str(proxy, "password").or_else(|| field_str(proxy, "pass"));
        if kind.is_none() && host.is_none() {
            return MappedProxy::Absent;
        }
        return parse_proxy_fields(kind.as_deref(), host, port, username, password);
    }

    if let Some(host) = field_str(value, "proxyHost").or_else(|| field_str(value, "proxy_host")) {
        let port = field(value, "proxyPort")
            .or_else(|| field(value, "proxy_port"))
            .and_then(as_u16);
        let kind = field_str(value, "proxyType").or_else(|| field_str(value, "proxy_type"));
        return parse_proxy_fields(kind.as_deref(), Some(host), port, None, None);
    }

    MappedProxy::Absent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_and_webrtc_mapping() {
        assert_eq!(dolphin_platform("windows"), Some(mirage_core::Os::Windows));
        assert_eq!(dolphin_platform("MacOS"), Some(mirage_core::Os::Macos));
        assert_eq!(dolphin_platform("Linux"), Some(mirage_core::Os::Linux));
        assert_eq!(dolphin_platform("android"), None);
        assert_eq!(dolphin_webrtc("real"), Some(WebRtcMode::Real));
        assert_eq!(dolphin_webrtc("altered"), Some(WebRtcMode::Relay));
        assert_eq!(dolphin_webrtc("disabled"), Some(WebRtcMode::Off));
    }

    #[test]
    fn useragent_reads_object_and_string() {
        let object = serde_json::json!({"useragent": {"mode": "manual", "value": "UA"}});
        assert_eq!(useragent_string(&object).as_deref(), Some("UA"));
        let string = serde_json::json!({"useragent": "UA2"});
        assert_eq!(useragent_string(&string).as_deref(), Some("UA2"));
    }

    #[test]
    fn proxy_reads_object_and_url() {
        let object =
            serde_json::json!({"proxy": {"type": "http", "host": "1.2.3.4", "port": 8080}});
        assert!(matches!(dolphin_proxy(&object), MappedProxy::Parsed(_)));
        let url = serde_json::json!({"proxy": "socks5://h:1080"});
        assert!(matches!(dolphin_proxy(&url), MappedProxy::Parsed(_)));
        let none = serde_json::json!({"name": "x"});
        assert_eq!(dolphin_proxy(&none), MappedProxy::Absent);
    }
}
