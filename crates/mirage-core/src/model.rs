//! Fingerprint and profile data model (CONTRACT schemas, serde camelCase).
//!
//! These types define the wire format of `window.__MIRAGE_FP__` in the browser
//! and of the encrypted `.mprof` profile files, so field names and shapes are
//! contract-stable and must not drift.

use std::fmt;

use chrono::{SecondsFormat, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Target platform of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

impl Os {
    /// Lowercase wire name: the serde representation, the fp-db `os` tag
    /// vocabulary and the `os` string inside `window.__MIRAGE_FP__`.
    pub fn wire_name(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Macos => "macos",
            Os::Linux => "linux",
        }
    }

    /// `navigator.platform` value for the os.
    pub fn platform(self) -> &'static str {
        match self {
            Os::Windows => "Win32",
            Os::Macos => "MacIntel",
            Os::Linux => "Linux x86_64",
        }
    }

    /// UA-CH `platform` value (`clientHints.platform`).
    pub fn client_hints_platform(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::Macos => "macOS",
            Os::Linux => "Linux",
        }
    }

    /// Vertical screen space reserved by the os shell: windows taskbar 40px,
    /// macos menu bar 25px, linux panel 24px (CONTRACT fp-db note).
    pub fn taskbar_height(self) -> u32 {
        match self {
            Os::Windows => 40,
            Os::Macos => 25,
            Os::Linux => 24,
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.wire_name())
    }
}

/// Proxy transport of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyScheme {
    Http,
    Socks5,
}

/// Approximate geolocation matching the profile locale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoFp {
    pub lat: f64,
    pub lon: f64,
}

/// Spoofed screen geometry. `availHeight` is derived from `height` minus the
/// os shell height (see [`Os::taskbar_height`]); `availWidth` equals `width`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenFp {
    pub width: u32,
    pub height: u32,
    pub avail_width: u32,
    pub avail_height: u32,
    pub dpr: f64,
    pub color_depth: u32,
}

/// Spoofed WebGL parameters (the ANGLE identity of the emulated GPU).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebglFp {
    pub vendor: String,
    pub renderer: String,
    pub max_texture_size: u32,
    pub max_viewport_dims: [u32; 2],
    pub max_renderbuffer_size: u32,
}

/// One UA-CH brand entry (`clientHints.brands[]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brand {
    pub brand: String,
    pub version: String,
}

/// User-Agent Client Hints block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHintsFp {
    pub brands: Vec<Brand>,
    pub mobile: bool,
    pub platform: String,
    pub platform_version: String,
    pub architecture: String,
    pub bitness: String,
    pub ua_full_version: String,
}

/// The complete spoofed fingerprint (CONTRACT: `window.__MIRAGE_FP__`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub os: Os,
    pub os_version: String,
    pub platform: String,
    pub ua: String,
    pub chrome_version: String,
    pub languages: Vec<String>,
    pub timezone: String,
    pub geo: GeoFp,
    pub screen: ScreenFp,
    pub hardware_concurrency: u32,
    pub device_memory: u32,
    pub webgl: WebglFp,
    pub canvas_seed: u32,
    pub audio_seed: u32,
    pub fonts: Vec<String>,
    /// WebRTC policy: "real" | "relay" | "off". Kept as a free-form string
    /// per CONTRACT (the enum surface of the model is `Os`/`ProxyScheme`).
    #[serde(rename = "webRTC")]
    pub web_rtc: String,
    pub client_hints: ClientHintsFp,
}

/// Proxy connection settings (`proxy: null` in a profile = direct connection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// A browser profile: identity + fingerprint + proxy + bookkeeping fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    /// Generation seed the fingerprint was built from. `0` means unknown
    /// (legacy profiles created before the field existed); a non-zero seed
    /// lets `generate(seed, os)` rebuild a bit-identical fingerprint.
    #[serde(default)]
    pub seed: u64,
    pub fingerprint: Fingerprint,
    pub proxy: Option<ProxyConfig>,
    pub notes: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Profile {
    /// Creates a profile with a fresh uuid v4 id, current UTC timestamps
    /// (RFC 3339, `...Z`) and empty tags/notes. The generation `seed` starts
    /// at `0` (unknown); chain [`Profile::with_seed`] to record it.
    pub fn new(
        name: impl Into<String>,
        fingerprint: Fingerprint,
        proxy: Option<ProxyConfig>,
    ) -> Self {
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        Profile {
            id: uuid_v4(),
            name: name.into(),
            tags: Vec::new(),
            seed: 0,
            fingerprint,
            proxy,
            notes: String::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Records the generation seed the fingerprint was built from (chainable).
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
}

/// Lower-case hyphenated uuid v4 from rand bytes (RFC 4122: version nibble
/// `4`, variant `10x`).
fn uuid_v4() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    let mut out = String::with_capacity(36);
    for (index, &byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v4_has_contract_shape() {
        for _ in 0..16 {
            let id = uuid_v4();
            assert_eq!(id.len(), 36);
            assert_eq!(
                id.split('-').map(str::len).collect::<Vec<_>>(),
                vec![8, 4, 4, 4, 12]
            );
            assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            assert!(!id.chars().any(|c| c.is_ascii_uppercase()));
            // Version nibble sits at the start of the third group.
            assert_eq!(id.as_bytes()[14], b'4');
            // Variant bits: first char of the fourth group in 8/9/a/b.
            assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        }
    }

    #[test]
    fn uuid_v4_is_unique_per_call() {
        assert_ne!(uuid_v4(), uuid_v4());
    }

    #[test]
    fn os_helpers_and_display_match_contract() {
        assert_eq!(Os::Windows.platform(), "Win32");
        assert_eq!(Os::Macos.platform(), "MacIntel");
        assert_eq!(Os::Linux.platform(), "Linux x86_64");
        assert_eq!(Os::Windows.client_hints_platform(), "Windows");
        assert_eq!(Os::Macos.client_hints_platform(), "macOS");
        assert_eq!(Os::Linux.client_hints_platform(), "Linux");
        assert_eq!(Os::Windows.taskbar_height(), 40);
        assert_eq!(Os::Macos.taskbar_height(), 25);
        assert_eq!(Os::Linux.taskbar_height(), 24);
        assert_eq!(Os::Windows.wire_name(), "windows");
        assert_eq!(Os::Macos.wire_name(), "macos");
        assert_eq!(Os::Linux.wire_name(), "linux");
        assert_eq!(Os::Windows.to_string(), "windows");
        assert_eq!(Os::Macos.to_string(), "macos");
        assert_eq!(Os::Linux.to_string(), "linux");
    }
}
