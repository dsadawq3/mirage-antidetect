//! Shared fixtures for the integration tests: the exact CONTRACT.md example
//! profile (Chrome 156, Windows 11) assembled by hand from mirage-core types.

#![allow(dead_code)]

use mirage_core::{
    Brand, ClientHintsFp, Fingerprint, GeoFp, Os, Profile, ProxyConfig, ProxyScheme, ScreenFp,
    WebglFp,
};
use mirage_launcher::LaunchConfig;
use std::path::PathBuf;

/// The fingerprint from CONTRACT.md ("Схема Fingerprint"), with the
/// `webRTC` mode parameterized so tests can exercise "real" vs "relay".
pub fn contract_fingerprint(web_rtc: &str) -> Fingerprint {
    Fingerprint {
        os: Os::Windows,
        os_version: String::from("11"),
        platform: String::from("Win32"),
        ua: String::from(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36",
        ),
        chrome_version: String::from("156.0.8078.80"),
        languages: vec![String::from("en-US"), String::from("en")],
        timezone: String::from("Europe/London"),
        geo: GeoFp {
            lat: 51.5074,
            lon: -0.1278,
        },
        screen: ScreenFp {
            width: 1920,
            height: 1080,
            avail_width: 1920,
            avail_height: 1040,
            dpr: 1.0,
            color_depth: 24,
        },
        hardware_concurrency: 8,
        device_memory: 8,
        webgl: WebglFp {
            vendor: String::from("Google Inc. (NVIDIA)"),
            renderer: String::from(
                "ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER Direct3D11 \
                 vs_5_0 ps_5_0, D3D11)",
            ),
            max_texture_size: 16384,
            max_viewport_dims: [32767, 32767],
            max_renderbuffer_size: 16384,
        },
        canvas_seed: 1234567890,
        audio_seed: 987654321,
        fonts: vec![String::from("Arial"), String::from("Calibri")],
        web_rtc: String::from(web_rtc),
        client_hints: ClientHintsFp {
            // Live-Chrome-156 calibration (CONTRACT.md "GREASE-калибровка"):
            // order [GREASE, Chromium, Google Chrome]; the GREASE entry
            // carries the grease-cycle versions, not the Chrome ones.
            brands: vec![
                Brand {
                    brand: String::from("Not:A-Brand"),
                    version: String::from("8"),
                },
                Brand {
                    brand: String::from("Chromium"),
                    version: String::from("156"),
                },
                Brand {
                    brand: String::from("Google Chrome"),
                    version: String::from("156"),
                },
            ],
            mobile: false,
            platform: String::from("Windows"),
            platform_version: String::from("15.0.0"),
            architecture: String::from("x86"),
            bitness: String::from("64"),
            ua_full_version: String::from("156.0.8078.80"),
        },
    }
}

/// CONTRACT.md ("Схема Profile"): no proxy, direct connection, webRTC "real".
pub fn profile_direct() -> Profile {
    profile(None)
}

/// CONTRACT.md ("Схема Profile"): socks5 proxy 1.2.3.4:1080, no credentials.
pub fn profile_socks5() -> Profile {
    profile(Some(ProxyConfig {
        scheme: ProxyScheme::Socks5,
        host: String::from("1.2.3.4"),
        port: 1080,
        username: None,
        password: None,
    }))
}

/// Direct-connection profile whose fingerprint uses webRTC = "relay".
pub fn profile_relay() -> Profile {
    let mut profile = profile_direct();
    profile.fingerprint = contract_fingerprint("relay");
    profile
}

fn profile(proxy: Option<ProxyConfig>) -> Profile {
    Profile {
        id: String::from("6f9619ff-8b86-d011-b42d-00cf4fc964ff"),
        name: String::from("Shop-01"),
        tags: vec![String::from("work")],
        seed: 0,
        fingerprint: contract_fingerprint("real"),
        proxy,
        notes: String::from(""),
        created_at: String::from("2026-11-03T12:00:00Z"),
        updated_at: String::from("2026-11-03T12:00:00Z"),
    }
}

/// A LaunchConfig around a given profile, directory and port.
pub fn launch_config(profile: Profile, profile_dir: PathBuf, debug_port: u16) -> LaunchConfig {
    LaunchConfig {
        profile,
        chrome_path: None,
        profile_dir,
        debug_port,
    }
}
