//! AdsPower import.
//!
//! Documentation used (verified):
//! - `GET /api/v1/user/list` response envelope and per-profile keys:
//!   <https://localapi-doc-en.adspower.com/docs/u8m2Ie> — top level
//!   `{code, data:{list:[...], page, page_size}, msg}`; profile keys
//!   `user_id`, `name`, `group_id`, `group_name`, `remark`, `serial_number`,
//!   `created_time`, ... The documented `list` *sample* omits
//!   `user_proxy_config`/`fingerprint_config`, but those are the documented
//!   parameter objects used by the same profile and are present in real
//!   responses, so they are read here when available.
//! - `user_proxy_config`:
//!   <https://localapi-doc-en.adspower.com/docs/Lb8pOg> — `proxy_soft`,
//!   `proxy_type` (`http`/`https`/`socks5`; `no_proxy` = no proxy),
//!   `proxy_host`, `proxy_port`, `proxy_user`, `proxy_password`, `proxy_url`.
//! - `fingerprint_config`:
//!   <https://localapi-doc-en.adspower.com/docs/Awy6Dg> — `ua`, `language`
//!   (list), `timezone` (IANA), `screen_resolution` (`"width_height"`, plus
//!   `none`/`random` placeholders), `webrtc`
//!   (`forward`/`proxy`/`local`/`disabled`), `hardware_concurrency`,
//!   `device_memory`, `latitude`/`longitude`, `webgl_config`.

use serde_json::Value;

use crate::util::{
    as_f64, as_u16, field, field_str, field_u32, is_iana_timezone, normalize_languages,
    parse_proxy_fields, parse_screen, parse_ua, MappedProxy,
};
use crate::{
    build_profile, collect_profiles, first_or_error, seed_material, ImportError, ImportResult,
    ScreenSpec, SourceProfile, WebRtcMode,
};

const FORMAT: &str = "AdsPower";
const MARKERS: &[&str] = &["user_id", "name", "fingerprint_config", "user_proxy_config"];

/// Imports a single AdsPower profile (the first one when a list is given).
pub fn import_ads_power(json: &str) -> Result<ImportResult, ImportError> {
    first_or_error(import_ads_power_all(json)?, FORMAT)
}

/// Imports every AdsPower profile found in the input.
pub fn import_ads_power_all(json: &str) -> Result<Vec<ImportResult>, ImportError> {
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

/// Maps one AdsPower profile object onto the format-agnostic intermediate.
fn to_source(value: &Value) -> SourceProfile {
    let mut source = SourceProfile {
        source_id: field_str(value, "user_id").or_else(|| field_str(value, "id")),
        name: field_str(value, "name"),
        notes: field_str(value, "remark"),
        ..SourceProfile::default()
    };

    if let Some(group) = field_str(value, "group_name") {
        source.tags.push(group);
    }

    if let Some(config) = field(value, "fingerprint_config") {
        source.ua = field_str(config, "ua");
        source.languages = field(config, "language").and_then(normalize_languages);

        if let Some(timezone) = field_str(config, "timezone") {
            source.timezone_seen = true;
            if is_iana_timezone(&timezone) {
                source.timezone = Some(timezone);
            }
        }

        if let Some(resolution) = field_str(config, "screen_resolution") {
            if let Some((width, height, dpr)) = parse_screen(&resolution) {
                source.screen = Some(ScreenSpec { width, height });
                source.dpr = dpr;
            }
        }

        if let Some(mode) = field_str(config, "webrtc").as_deref().and_then(ads_webrtc) {
            source.webrtc = Some(mode);
        }

        source.hardware_concurrency = field_u32(config, "hardware_concurrency");
        source.device_memory = field_u32(config, "device_memory");

        if let (Some(lat), Some(lon)) = (
            as_f64_opt(config, "latitude"),
            as_f64_opt(config, "longitude"),
        ) {
            source.geo = Some((lat, lon));
        }

        // WebGL metadata is not mapped (Mirage uses its fp-db GPU identity).
        if field(config, "webgl_config").is_some() {
            source.webgl_seen = true;
        }
    }

    if let Some(proxy) = field(value, "user_proxy_config") {
        source.proxy = ads_proxy(proxy);
    }

    // OS falls back to the UA when AdsPower exposes no explicit platform field.
    if source.os.is_none() {
        if let Some(ua) = source.ua.as_deref() {
            source.os = parse_ua(ua).os;
        }
    }

    source
}

/// Reads `latitude`/`longitude` style numeric fields.
fn as_f64_opt(value: &Value, key: &str) -> Option<f64> {
    field(value, key).and_then(as_f64)
}

/// AdsPower `webrtc` -> Mirage policy. `forward`/`proxy` cover the real IP with
/// the proxy IP (relay candidates); `local` exposes the real IP; `disabled`
/// disables WebRTC.
fn ads_webrtc(mode: &str) -> Option<WebRtcMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "local" | "real" => Some(WebRtcMode::Real),
        "forward" | "proxy" => Some(WebRtcMode::Relay),
        "disabled" | "off" => Some(WebRtcMode::Off),
        _ => None,
    }
}

/// AdsPower `user_proxy_config` -> [`MappedProxy`].
fn ads_proxy(proxy: &Value) -> MappedProxy {
    // `proxy_soft == "no_proxy"` is AdsPower's explicit direct-connection marker.
    if let Some(soft) = field_str(proxy, "proxy_soft") {
        if soft.eq_ignore_ascii_case("no_proxy") || soft.eq_ignore_ascii_case("no proxy") {
            return MappedProxy::Absent;
        }
    }
    let kind = field_str(proxy, "proxy_type");
    let host = field_str(proxy, "proxy_host");
    let port = field(proxy, "proxy_port").and_then(as_u16);
    let username = field_str(proxy, "proxy_user");
    let password = field_str(proxy, "proxy_password");
    parse_proxy_fields(kind.as_deref(), host, port, username, password)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::as_string;

    #[test]
    fn webrtc_mapping() {
        assert_eq!(ads_webrtc("disabled"), Some(WebRtcMode::Off));
        assert_eq!(ads_webrtc("local"), Some(WebRtcMode::Real));
        assert_eq!(ads_webrtc("forward"), Some(WebRtcMode::Relay));
        assert_eq!(ads_webrtc("proxy"), Some(WebRtcMode::Relay));
        assert_eq!(ads_webrtc("weird"), None);
    }

    #[test]
    fn proxy_no_proxy_is_absent() {
        let value = serde_json::json!({"proxy_soft": "no_proxy"});
        assert_eq!(ads_proxy(&value), MappedProxy::Absent);
    }

    #[test]
    fn proxy_socks5_parsed() {
        let value = serde_json::json!({
            "proxy_soft": "other",
            "proxy_type": "socks5",
            "proxy_host": "1.2.3.4",
            "proxy_port": "1080",
            "proxy_user": "u",
            "proxy_password": "p"
        });
        match ads_proxy(&value) {
            MappedProxy::Parsed(config) => {
                assert_eq!(config.host, "1.2.3.4");
                assert_eq!(config.port, 1080);
            }
            other => panic!("expected parsed proxy, got {other:?}"),
        }
    }

    #[test]
    fn field_helpers_are_tolerant() {
        let value = serde_json::json!({"a": " x ", "b": 5, "c": ""});
        assert_eq!(field_str(&value, "a").as_deref(), Some("x"));
        assert_eq!(field_u32(&value, "b"), Some(5));
        assert_eq!(as_string(&value["c"]), None);
    }
}
