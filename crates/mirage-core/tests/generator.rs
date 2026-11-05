//! Integration tests for the mirage-core generator, validator and profile
//! model (CONTRACT wave 1). Covers the five mandatory checks plus serde wire
//! format and negative validator cases.

use chrono::{DateTime, Utc};
use mirage_core::db;
use mirage_core::{
    generate, validate, validate_seed_determinism, ConsistencyWarning, Os, Profile, ProxyConfig,
    ProxyScheme, CHROME_FULL, CHROME_MAJOR, GREASE_BRAND, GREASE_FULL_VERSION, GREASE_VERSION,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const ALL_OS: [Os; 3] = [Os::Windows, Os::Macos, Os::Linux];

/// CONTRACT UA templates for the current Chrome major.
fn expected_ua(os: Os) -> String {
    match os {
        Os::Windows => format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{CHROME_MAJOR}.0.0.0 Safari/537.36"),
        Os::Macos => format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{CHROME_MAJOR}.0.0.0 Safari/537.36"),
        Os::Linux => format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{CHROME_MAJOR}.0.0.0 Safari/537.36"),
    }
}

/// Mandatory test 1: same seed + os -> bitwise identical fingerprint,
/// 10 pairs in a row (plus os=None reproducibility and the rule-8 helper).
#[test]
fn same_seed_and_os_produce_identical_fingerprints() {
    for pair in 0..10u64 {
        let seed = 0x5EED_0000 + pair;
        for os in ALL_OS {
            let first = generate(seed, Some(os)).expect("generate must succeed");
            let second = generate(seed, Some(os)).expect("generate must succeed");
            assert_eq!(first, second, "pair {pair}, os {os}");
            // Bitwise identity must survive a JSON roundtrip too.
            let first_json = serde_json::to_string(&first).expect("serialize first");
            let second_json = serde_json::to_string(&second).expect("serialize second");
            assert_eq!(first_json, second_json, "pair {pair}, os {os}");
        }
        // os=None resolves the os from the seed and must stay reproducible.
        let first = generate(seed, None).expect("generate must succeed");
        let second = generate(seed, None).expect("generate must succeed");
        assert_eq!(first, second, "pair {pair}, os none");
    }
}

/// CONTRACT rule 8 helper: regeneration from the same seed is stable.
#[test]
fn seed_determinism_check_passes_for_generated_fingerprints() {
    for seed in 0..10u64 {
        for os in ALL_OS {
            let warnings = validate_seed_determinism(seed, Some(os));
            assert!(warnings.is_empty(), "seed {seed} / {os}: {warnings:?}");
        }
        let warnings = validate_seed_determinism(seed, None);
        assert!(warnings.is_empty(), "seed {seed} / none: {warnings:?}");
    }
}

/// Mandatory test 2: for 200 random seeds on each of the 3 os values,
/// validate() returns an empty warning vector.
#[test]
fn generated_fingerprints_pass_all_consistency_rules() {
    let mut seed_rng = StdRng::seed_from_u64(0xC0FFEE);
    for os in ALL_OS {
        for _ in 0..200 {
            let seed = seed_rng.gen::<u64>();
            let fp = generate(seed, Some(os)).expect("generate must succeed");
            let warnings = validate(&fp);
            assert!(warnings.is_empty(), "seed {seed} / {os}: {warnings:?}");
        }
    }
}

/// Mandatory test 3: UA follows the os template, brands follow the live-dump
/// GREASE calibration, uaFullVersion == chromeVersion.
#[test]
fn ua_brands_and_full_version_match_contract() {
    for seed in 0..25u64 {
        for os in ALL_OS {
            let fp = generate(seed, Some(os)).expect("generate must succeed");
            assert_eq!(fp.ua, expected_ua(os), "seed {seed} / {os}");
            assert_eq!(fp.chrome_version, CHROME_FULL);
            assert_eq!(fp.client_hints.ua_full_version, fp.chrome_version);
            assert_eq!(fp.client_hints.brands.len(), 3);
            let names: Vec<&str> = fp
                .client_hints
                .brands
                .iter()
                .map(|brand| brand.brand.as_str())
                .collect();
            // Live-dump order: GREASE first, then Chromium, then Google Chrome.
            assert_eq!(names, [GREASE_BRAND, "Chromium", "Google Chrome"]);
            // GREASE carries its own version; the real brands carry the major.
            assert_eq!(fp.client_hints.brands[0].version, GREASE_VERSION);
            assert_eq!(
                fp.client_hints.brands[1].version,
                CHROME_MAJOR.to_string(),
                "seed {seed} / {os}"
            );
            assert_eq!(
                fp.client_hints.brands[2].version,
                CHROME_MAJOR.to_string(),
                "seed {seed} / {os}"
            );
        }
    }
}

/// GREASE constants must match the live Chrome 156.0.8078.80 dump
/// (`crates/mirage-launcher/examples/dump_native.rs`) and must be
/// re-exported from the crate root (the launcher consumes them).
#[test]
fn grease_constants_match_live_dump() {
    assert_eq!(GREASE_BRAND, "Not:A-Brand");
    assert_eq!(GREASE_VERSION, "8");
    assert_eq!(GREASE_FULL_VERSION, "8.0.0.0");
}

/// Mandatory test 4: platform, clientHints.platform, fonts, availHeight and
/// the os-specific value sets follow the CONTRACT rules.
#[test]
fn os_specific_fields_follow_contract() {
    for seed in 0..25u64 {
        for os in ALL_OS {
            let fp = generate(seed, Some(os)).expect("generate must succeed");

            let (platform, ch_platform, shell_height, renderer_markers, os_versions): (
                &str,
                &str,
                u32,
                &[&str],
                &[&str],
            ) = match os {
                Os::Windows => (
                    "Win32",
                    "Windows",
                    40,
                    &["Direct3D11", "D3D11"],
                    &["10", "11"],
                ),
                Os::Macos => (
                    "MacIntel",
                    "macOS",
                    25,
                    &["ANGLE Metal"],
                    &["10.15", "11", "12", "13", "14"],
                ),
                Os::Linux => (
                    "Linux x86_64",
                    "Linux",
                    24,
                    &["Mesa", "llvmpipe", "OpenGL ES 3.2 NVIDIA"],
                    &["5.15", "6.1", "6.5"],
                ),
            };

            assert_eq!(fp.platform, platform, "seed {seed} / {os}");
            assert_eq!(fp.client_hints.platform, ch_platform, "seed {seed} / {os}");
            assert_eq!(fp.screen.avail_width, fp.screen.width);
            assert_eq!(fp.screen.avail_height, fp.screen.height - shell_height);
            assert!(
                renderer_markers
                    .iter()
                    .any(|marker| fp.webgl.renderer.contains(*marker)),
                "seed {seed} / {os}: renderer `{}` misses {renderer_markers:?}",
                fp.webgl.renderer
            );
            assert!(os_versions.contains(&fp.os_version.as_str()));
            assert!(!fp.client_hints.mobile);
            assert_eq!(fp.client_hints.architecture, "x86");
            assert_eq!(fp.client_hints.bitness, "64");
            assert_eq!(fp.web_rtc, "real");
            assert!(matches!(fp.device_memory, 2 | 4 | 8));
            assert!([2, 4, 6, 8, 12, 16, 20, 24].contains(&fp.hardware_concurrency));
            assert!([1.0, 1.25, 1.5, 2.0].contains(&fp.screen.dpr));
            // M13: macOS legends only report the Retina scaling steps.
            if os == Os::Macos {
                assert!(
                    fp.screen.dpr == 1.0 || fp.screen.dpr == 2.0,
                    "seed {seed}: macos legend with dpr {}",
                    fp.screen.dpr
                );
            }
            assert_eq!(fp.screen.color_depth, 24);

            // fonts: the full fp-db list for the os, verbatim.
            let db_fonts = db::fonts(os).expect("fonts db must load");
            assert!(
                !db_fonts.is_empty(),
                "fp-db fonts for {os} must not be empty"
            );
            assert_eq!(fp.fonts, db_fonts, "seed {seed} / {os}");

            // platformVersion mapping per CONTRACT.
            match os {
                Os::Windows => {
                    let allowed: &[&str] = if fp.os_version == "11" {
                        &["13.0.0", "15.0.0"]
                    } else {
                        &["10.0.0"]
                    };
                    assert!(
                        allowed.contains(&fp.client_hints.platform_version.as_str()),
                        "seed {seed}: osVersion {} -> platformVersion {}",
                        fp.os_version,
                        fp.client_hints.platform_version
                    );
                }
                Os::Macos => {
                    // Darwin-style version derived from the osVersion.
                    assert!(fp
                        .client_hints
                        .platform_version
                        .starts_with(format!("{}.", fp.os_version).as_str()));
                }
                Os::Linux => {
                    assert_eq!(
                        fp.client_hints.platform_version,
                        format!("{}.0", fp.os_version)
                    );
                }
            }
        }
    }
}

/// Mandatory test 5: Profile::new builds a valid uuid v4 id and stamps
/// RFC 3339 UTC timestamps.
#[test]
fn profile_new_builds_valid_uuid_v4_and_timestamps() {
    let fp = generate(42, Some(Os::Windows)).expect("generate must succeed");

    let proxy = ProxyConfig {
        scheme: ProxyScheme::Socks5,
        host: "127.0.0.1".to_string(),
        port: 1080,
        username: Some("user".to_string()),
        password: Some("pass".to_string()),
    };
    let profile = Profile::new("Shop-01", fp.clone(), Some(proxy));

    assert_eq!(profile.name, "Shop-01");
    assert_eq!(profile.fingerprint, fp);
    assert!(profile.tags.is_empty());
    assert_eq!(profile.notes, "");
    assert!(matches!(
        profile.proxy,
        Some(ProxyConfig {
            scheme: ProxyScheme::Socks5,
            ..
        })
    ));

    // uuid v4: 8-4-4-4-12 lower-case hex, version nibble 4, variant 8/9/a/b.
    let id = &profile.id;
    assert_eq!(id.len(), 36);
    assert_eq!(
        id.split('-').map(str::len).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12]
    );
    assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    assert!(!id.chars().any(|c| c.is_ascii_uppercase()));
    assert_eq!(id.as_bytes()[14], b'4');
    assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));

    // Timestamps: RFC 3339 UTC, equal at creation, current.
    assert_eq!(profile.created_at, profile.updated_at);
    assert!(profile.created_at.ends_with('Z'));
    let created =
        DateTime::parse_from_rfc3339(&profile.created_at).expect("createdAt must be RFC3339");
    let updated =
        DateTime::parse_from_rfc3339(&profile.updated_at).expect("updatedAt must be RFC3339");
    assert_eq!(created, updated);
    let drift = (created.with_timezone(&Utc) - Utc::now())
        .num_seconds()
        .abs();
    assert!(drift < 300, "timestamps must be current, drift {drift}s");

    // Fresh id per profile; proxy-less profile keeps proxy = null.
    let direct = Profile::new("Shop-02", fp, None);
    assert_ne!(profile.id, direct.id);
    assert!(direct.proxy.is_none());
}

/// CONTRACT serde wire format: camelCase keys, lowercase enums, the
/// `webRTC` key, and a lossless profile roundtrip.
#[test]
fn serializes_with_contract_shapes() {
    let fp = generate(7, Some(Os::Macos)).expect("generate must succeed");

    let fp_json = serde_json::to_value(&fp).expect("serialize fingerprint");
    for key in [
        "os",
        "osVersion",
        "platform",
        "ua",
        "chromeVersion",
        "languages",
        "timezone",
        "geo",
        "screen",
        "hardwareConcurrency",
        "deviceMemory",
        "webgl",
        "canvasSeed",
        "audioSeed",
        "fonts",
        "webRTC",
        "clientHints",
    ] {
        assert!(
            fp_json.get(key).is_some(),
            "fingerprint must expose `{key}`"
        );
    }
    assert_eq!(fp_json["os"], serde_json::json!("macos"));
    assert_eq!(fp_json["webRTC"], serde_json::json!("real"));
    assert_eq!(fp_json["chromeVersion"], serde_json::json!(CHROME_FULL));

    let screen = &fp_json["screen"];
    for key in [
        "width",
        "height",
        "availWidth",
        "availHeight",
        "dpr",
        "colorDepth",
    ] {
        assert!(screen.get(key).is_some(), "screen must expose `{key}`");
    }

    let client_hints = &fp_json["clientHints"];
    for key in [
        "brands",
        "mobile",
        "platform",
        "platformVersion",
        "architecture",
        "bitness",
        "uaFullVersion",
    ] {
        assert!(
            client_hints.get(key).is_some(),
            "clientHints must expose `{key}`"
        );
    }

    // Brand wire shape per the live Chrome 156 dump: GREASE brand first with
    // its own version, then Chromium / Google Chrome at the Chrome major.
    assert_eq!(
        client_hints["brands"],
        serde_json::json!([
            { "brand": GREASE_BRAND, "version": GREASE_VERSION },
            { "brand": "Chromium", "version": CHROME_MAJOR.to_string() },
            { "brand": "Google Chrome", "version": CHROME_MAJOR.to_string() },
        ]),
        "brands must follow the GREASE-calibrated contract shape"
    );

    // Profile: camelCase timestamps, proxy object / null.
    let with_proxy = Profile::new(
        "Shop-01",
        fp.clone(),
        Some(ProxyConfig {
            scheme: ProxyScheme::Http,
            host: "10.0.0.1".to_string(),
            port: 8080,
            username: None,
            password: None,
        }),
    );
    let profile_json = serde_json::to_value(&with_proxy).expect("serialize profile");
    for key in [
        "id",
        "name",
        "tags",
        "seed",
        "fingerprint",
        "proxy",
        "notes",
        "createdAt",
        "updatedAt",
    ] {
        assert!(
            profile_json.get(key).is_some(),
            "profile must expose `{key}`"
        );
    }
    assert_eq!(profile_json["proxy"]["scheme"], serde_json::json!("http"));
    // Profile::new leaves the seed unknown (0) until with_seed records it.
    assert_eq!(profile_json["seed"], serde_json::json!(0));

    let seeded = Profile::new("Shop-03", fp.clone(), None).with_seed(20261103);
    assert_eq!(seeded.seed, 20261103);
    let seeded_json = serde_json::to_value(&seeded).expect("serialize seeded profile");
    assert_eq!(seeded_json["seed"], serde_json::json!(20261103));
    let seeded_roundtrip: Profile =
        serde_json::from_value(seeded_json).expect("deserialize seeded profile");
    assert_eq!(seeded_roundtrip, seeded);

    let direct = Profile::new("Shop-02", fp, None);
    let direct_json = serde_json::to_value(&direct).expect("serialize profile");
    assert!(direct_json["proxy"].is_null());

    // Full roundtrip must be lossless.
    let roundtrip: Profile = serde_json::from_value(direct_json).expect("deserialize profile");
    assert_eq!(roundtrip, direct);

    // Enum wire format.
    assert_eq!(
        serde_json::to_value(Os::Windows).expect("os json"),
        serde_json::json!("windows")
    );
    assert_eq!(
        serde_json::to_value(ProxyScheme::Socks5).expect("scheme json"),
        serde_json::json!("socks5")
    );
    let parsed: Os = serde_json::from_value(serde_json::json!("linux")).expect("os parse");
    assert_eq!(parsed, Os::Linux);
}

/// Legacy profiles written before `seed` existed must still deserialize, with
/// the field defaulting to `0` (unknown).
#[test]
fn legacy_profile_without_seed_deserializes_to_zero() {
    let fp = generate(11, Some(Os::Linux)).expect("generate must succeed");
    let profile = Profile::new("Legacy", fp, None);
    let mut json = serde_json::to_value(&profile).expect("serialize profile");
    json.as_object_mut()
        .expect("profile serializes to an object")
        .remove("seed");
    let parsed: Profile = serde_json::from_value(json).expect("legacy profile parses");
    assert_eq!(parsed.seed, 0);
    assert_eq!(parsed.id, profile.id);
    assert_eq!(parsed.fingerprint, profile.fingerprint);
}

/// Negative cases: the validator must catch every rule violation. Guards
/// against a vacuous validate() that always returns an empty vector.
#[test]
fn validator_catches_broken_fingerprints() {
    let base = generate(99, Some(Os::Windows)).expect("generate must succeed");

    // Rule 1: navigator.platform.
    let mut fp = base.clone();
    fp.platform = "MacIntel".to_string();
    assert!(matches!(
        validate(&fp).first(),
        Some(ConsistencyWarning::PlatformMismatch { .. })
    ));

    // Rule 1: clientHints.platform.
    let mut fp = base.clone();
    fp.client_hints.platform = "Linux".to_string();
    assert!(matches!(
        validate(&fp).first(),
        Some(ConsistencyWarning::ClientHintsPlatformMismatch { .. })
    ));

    // Rule 1: UA template.
    let mut fp = base.clone();
    fp.ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36".to_string();
    assert!(matches!(
        validate(&fp).first(),
        Some(ConsistencyWarning::UaTemplateMismatch { .. })
    ));

    // Rule 1: renderer marker.
    let mut fp = base.clone();
    fp.webgl.renderer = "ANGLE (OpenGL)".to_string();
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::RendererMismatch { .. })));

    // Rule 1: fonts outside the db.
    let mut fp = base.clone();
    fp.fonts.push("Totally Made Up Font".to_string());
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::FontsNotFromDb { .. })));

    // Rule 1: empty fonts.
    let mut fp = base.clone();
    fp.fonts.clear();
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::EmptyFonts { .. })));

    // Rule 1: availHeight.
    let mut fp = base.clone();
    fp.screen.avail_height -= 1;
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::AvailHeightMismatch { .. })));

    // Rule 4: memory and cores.
    let mut fp = base.clone();
    fp.device_memory = 16;
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::DeviceMemoryInvalid { .. })));
    let mut fp = base.clone();
    fp.hardware_concurrency = 32;
    assert!(validate(&fp).iter().any(|warning| matches!(
        warning,
        ConsistencyWarning::HardwareConcurrencyInvalid { .. }
    )));

    // Rule 5: dpr and unknown screen size.
    let mut fp = base.clone();
    fp.screen.dpr = 1.75;
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::DprInvalid { .. })));
    let mut fp = base.clone();
    fp.screen.width = 12345;
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::ScreenNotInDb { .. })));

    // Rule 6: languages and timezone.
    let mut fp = base.clone();
    fp.languages.clear();
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::EmptyLanguages)));
    let mut fp = base.clone();
    fp.timezone = "   ".to_string();
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::EmptyTimezone)));

    // Rule 7: brand version and uaFullVersion.
    let mut fp = base.clone();
    fp.client_hints.brands[1].version = "155".to_string();
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::BrandVersionMismatch { .. })));
    // Rule 7: the GREASE brand must keep its calibrated version ("8"),
    // not drift to the chrome major.
    let mut fp = base.clone();
    fp.client_hints.brands[0].version = CHROME_MAJOR.to_string();
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::BrandVersionMismatch { .. })));
    let mut fp = base.clone();
    fp.client_hints.ua_full_version = "156.0.0.0".to_string();
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::UaFullVersionMismatch { .. })));

    // Sanity: the untouched base validates cleanly.
    assert!(validate(&base).is_empty());
}

/// M13 negative cases: macOS with a fractional dpr and legends backed by a
/// screen tagged for another os must warn.
#[test]
fn validator_catches_screen_os_and_macos_dpr_violations() {
    // macOS + dpr 1.25 (a Windows 125%-scale artifact).
    let base = generate(5, Some(Os::Macos)).expect("generate must succeed");
    let mut fp = base.clone();
    fp.screen.width = 1536;
    fp.screen.height = 864;
    fp.screen.avail_width = 1536;
    fp.screen.avail_height = 864 - Os::Macos.taskbar_height();
    fp.screen.dpr = 1.25;
    let warnings = validate(&fp);
    assert!(
        warnings
            .iter()
            .any(|warning| matches!(warning, ConsistencyWarning::MacosDprInvalid { .. })),
        "macos with dpr 1.25 must warn: {warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| matches!(warning, ConsistencyWarning::ScreenOsMismatch { .. })),
        "macos legend on the windows-tagged 1536x864 screen must warn: {warnings:?}"
    );

    // macOS + dpr 1.5 (kept out of the macos pool by the windows tag).
    let mut fp = base.clone();
    fp.screen.width = 2560;
    fp.screen.height = 1440;
    fp.screen.avail_width = 2560;
    fp.screen.avail_height = 1440 - Os::Macos.taskbar_height();
    fp.screen.dpr = 1.5;
    let warnings = validate(&fp);
    assert!(
        warnings
            .iter()
            .any(|warning| matches!(warning, ConsistencyWarning::MacosDprInvalid { .. })),
        "macos with dpr 1.5 must warn: {warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| matches!(warning, ConsistencyWarning::ScreenOsMismatch { .. })),
        "macos legend on the windows-tagged 2560x1440@1.5 screen must warn: {warnings:?}"
    );

    // Windows legend on a macos-tagged Retina-logical screen.
    let base = generate(6, Some(Os::Windows)).expect("generate must succeed");
    let mut fp = base.clone();
    fp.screen.width = 1512;
    fp.screen.height = 982;
    fp.screen.avail_width = 1512;
    fp.screen.avail_height = 982 - Os::Windows.taskbar_height();
    fp.screen.dpr = 2.0;
    let warnings = validate(&fp);
    assert!(
        warnings
            .iter()
            .any(|warning| matches!(warning, ConsistencyWarning::ScreenOsMismatch { .. })),
        "windows legend on the macos-tagged 1512x982@2 screen must warn: {warnings:?}"
    );

    // Cross-platform screens stay legal for every os.
    for os in ALL_OS {
        let mut fp = generate(7, Some(os)).expect("generate must succeed");
        fp.screen.width = 1920;
        fp.screen.height = 1080;
        fp.screen.avail_width = 1920;
        fp.screen.avail_height = 1080 - os.taskbar_height();
        fp.screen.dpr = 1.0;
        assert!(
            validate(&fp)
                .iter()
                .all(|warning| !matches!(warning, ConsistencyWarning::ScreenOsMismatch { .. })),
            "untagged 1920x1080@1 must fit every os ({os})"
        );
    }
}

/// minor 8 negative cases: windows osVersion <-> platformVersion mapping and
/// availWidth == width.
#[test]
fn validator_catches_windows_version_and_avail_width_violations() {
    let base = generate(8, Some(Os::Windows)).expect("generate must succeed");

    // Windows 11 must not report a Windows 10 platformVersion.
    let mut fp = base.clone();
    fp.os_version = "11".to_string();
    fp.client_hints.platform_version = "10.0.0".to_string();
    assert!(validate(&fp).iter().any(|warning| matches!(
        warning,
        ConsistencyWarning::WindowsPlatformVersionMismatch { .. }
    )));

    // Windows 10 must not report a Windows 11 platformVersion.
    let mut fp = base.clone();
    fp.os_version = "10".to_string();
    fp.client_hints.platform_version = "13.0.0".to_string();
    assert!(validate(&fp).iter().any(|warning| matches!(
        warning,
        ConsistencyWarning::WindowsPlatformVersionMismatch { .. }
    )));

    // Legal mappings stay silent for both osVersion values.
    for (os_version, platform_version) in [("11", "13.0.0"), ("11", "15.0.0"), ("10", "10.0.0")] {
        let mut fp = base.clone();
        fp.os_version = os_version.to_string();
        fp.client_hints.platform_version = platform_version.to_string();
        assert!(
            validate(&fp).iter().all(|warning| !matches!(
                warning,
                ConsistencyWarning::WindowsPlatformVersionMismatch { .. }
            )),
            "osVersion {os_version} -> platformVersion {platform_version} is legal"
        );
    }

    // availWidth must equal width.
    let mut fp = base.clone();
    fp.screen.avail_width = fp.screen.width - 1;
    assert!(validate(&fp)
        .iter()
        .any(|warning| matches!(warning, ConsistencyWarning::AvailWidthMismatch { .. })));
}

/// M13 positive case: the generator only picks screens that are untagged or
/// tagged for the legend os, and keeps macOS on the Retina dpr steps.
#[test]
fn screen_os_tags_are_respected_by_generation() {
    let db_screens = db::screens().expect("screens db must load");
    assert!(
        db_screens.iter().any(|screen| screen.os.is_some()),
        "fp-db must carry os-tagged screen records for this test to mean anything"
    );
    for seed in 0..50u64 {
        for os in ALL_OS {
            let fp = generate(seed, Some(os)).expect("generate must succeed");
            let record = db_screens
                .iter()
                .find(|screen| {
                    screen.width == fp.screen.width
                        && screen.height == fp.screen.height
                        && screen.dpr == fp.screen.dpr
                })
                .expect("generated screen must exist in fp-db");
            let tag_fits = record.os.as_deref().is_none_or(|tag| tag == os.wire_name());
            assert!(
                tag_fits,
                "seed {seed} / {os}: picked screen tagged `{:?}`",
                record.os
            );
            if os == Os::Macos {
                assert!(
                    fp.screen.dpr == 1.0 || fp.screen.dpr == 2.0,
                    "seed {seed}: macos legend generated with dpr {}",
                    fp.screen.dpr
                );
            }
        }
    }
}
