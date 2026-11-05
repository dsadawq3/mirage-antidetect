//! Integration tests for the competitor-profile importers.
//!
//! Fixtures:
//! - `adspower_list.json` — `GET /api/v1/user/list` envelope, fields per
//!   <https://localapi-doc-en.adspower.com/docs/u8m2Ie>, `user_proxy_config`
//!   per <https://localapi-doc-en.adspower.com/docs/Lb8pOg>, `fingerprint_config`
//!   per <https://localapi-doc-en.adspower.com/docs/Awy6Dg>. Verified.
//! - `gologin_export.json` — profile object per
//!   <https://docs-download.gologin.com/openapi.json>. Verified.
//! - `dolphin_export.json` — profile template per
//!   <https://docs.dolphin-anty.com/en/api/api-profile-creation-template-dolphin-anty.md>.
//!   The template itself is verified; the **proxy** object is schema per docs,
//!   unverified against a real export (Dolphin does not document it there).

use mirage_core::{db, validate, Os, ProxyScheme};
use mirage_import::{
    import_ads_power, import_ads_power_all, import_dolphin, import_dolphin_all, import_gologin,
    import_gologin_all, ImportError, ImportResult, ImportWarning,
};

const ADSPOWER: &str = include_str!("fixtures/adspower_list.json");
const DOLPHIN: &str = include_str!("fixtures/dolphin_export.json");
const GOLOGIN: &str = include_str!("fixtures/gologin_export.json");

fn has(result: &ImportResult, predicate: impl Fn(&ImportWarning) -> bool) -> bool {
    result.warnings.iter().any(predicate)
}

fn mapped(result: &ImportResult, field: &str) -> bool {
    has(result, |w| w.is_mapped() && w.field() == field)
}

fn defaulted(result: &ImportResult, field: &str) -> bool {
    has(result, |w| w.is_defaulted() && w.field() == field)
}

fn ignored(result: &ImportResult, field: &str) -> bool {
    has(result, |w| w.is_ignored() && w.field() == field)
}

#[test]
fn adspower_maps_documented_fields() {
    let result = import_ads_power(ADSPOWER).expect("ads power fixture imports");
    let fp = &result.profile.fingerprint;

    assert_eq!(result.profile.name, "Shop-01");
    assert_eq!(result.profile.tags, vec!["work".to_string()]);
    assert_eq!(result.profile.notes, "imported from adspower");

    assert_eq!(fp.os, Os::Windows);
    assert_eq!(fp.platform, "Win32");
    assert_eq!(fp.languages, vec!["en-US".to_string(), "en".to_string()]);
    assert_eq!(fp.timezone, "America/New_York");
    assert_eq!(fp.screen.width, 1920);
    assert_eq!(fp.screen.height, 1080);
    assert_eq!(fp.screen.avail_height, 1080 - 40);
    assert_eq!(fp.web_rtc, "off");
    assert_eq!(fp.hardware_concurrency, 8);
    assert_eq!(fp.device_memory, 8);

    let proxy = result.profile.proxy.as_ref().expect("proxy mapped");
    assert_eq!(proxy.scheme, ProxyScheme::Socks5);
    assert_eq!(proxy.host, "1.2.3.4");
    assert_eq!(proxy.port, 1080);
    assert_eq!(proxy.username.as_deref(), Some("proxyuser"));
    assert_eq!(proxy.password.as_deref(), Some("proxypass"));

    assert!(mapped(&result, "ua"));
    assert!(mapped(&result, "languages"));
    assert!(mapped(&result, "timezone"));
    assert!(mapped(&result, "screen"));
    assert!(mapped(&result, "webRTC"));
    assert!(mapped(&result, "proxy"));
    assert!(mapped(&result, "hardwareConcurrency"));
    assert!(mapped(&result, "deviceMemory"));
    // Source UA is Chrome 156 -> the calibrated major is kept, version not dropped.
    assert!(!ignored(&result, "ua.version"));
    // WebGL metadata is deliberately not mapped.
    assert!(ignored(&result, "webgl"));
    // No dpr in the AdsPower screen string -> generated.
    assert!(defaulted(&result, "screen.dpr"));
}

#[test]
fn dolphin_maps_documented_fields() {
    let result = import_dolphin(DOLPHIN).expect("dolphin fixture imports");
    let fp = &result.profile.fingerprint;

    assert_eq!(result.profile.name, "dolphin-01");
    assert_eq!(fp.os, Os::Windows);
    assert_eq!(fp.languages, vec!["de-DE".to_string(), "de".to_string()]);
    assert_eq!(fp.timezone, "Europe/Berlin");
    assert_eq!(fp.screen.width, 1920);
    assert_eq!(fp.screen.height, 1080);
    assert_eq!(fp.screen.dpr, 1.0);
    assert_eq!(fp.web_rtc, "relay");
    assert_eq!(fp.hardware_concurrency, 8);
    assert_eq!(fp.device_memory, 8);

    let proxy = result.profile.proxy.as_ref().expect("proxy mapped");
    assert_eq!(proxy.scheme, ProxyScheme::Http);
    assert_eq!(proxy.host, "5.6.7.8");
    assert_eq!(proxy.port, 8080);

    assert!(mapped(&result, "os"));
    assert!(mapped(&result, "ua"));
    assert!(mapped(&result, "screen.dpr"));
    assert!(mapped(&result, "webRTC"));
    assert!(ignored(&result, "webgl"));
}

#[test]
fn gologin_maps_documented_fields() {
    let result = import_gologin(GOLOGIN).expect("gologin fixture imports");
    let fp = &result.profile.fingerprint;

    assert_eq!(result.profile.name, "gologin-01");
    assert_eq!(result.profile.notes, "imported from gologin");
    assert_eq!(result.profile.tags, vec!["work".to_string()]);
    assert_eq!(fp.os, Os::Windows);
    assert_eq!(fp.languages, vec!["en-US".to_string(), "en".to_string()]);
    assert_eq!(fp.timezone, "Europe/London");
    assert_eq!(fp.screen.width, 1920);
    assert_eq!(fp.screen.height, 1080);
    assert_eq!(fp.screen.dpr, 1.0);
    assert_eq!(fp.web_rtc, "real");

    let proxy = result.profile.proxy.as_ref().expect("proxy mapped");
    assert_eq!(proxy.scheme, ProxyScheme::Socks5);
    assert_eq!(proxy.host, "9.9.9.9");
    assert_eq!(proxy.port, 1080);
    assert_eq!(proxy.username.as_deref(), Some("gluser"));

    assert!(mapped(&result, "proxy"));
    assert!(ignored(&result, "webgl"));
}

#[test]
fn roundtrip_key_fields() {
    // Proxy, UA and languages must survive the import verbatim.
    let ads = import_ads_power(ADSPOWER).expect("ads").profile;
    assert_eq!(
        ads.fingerprint.ua,
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
    );
    assert_eq!(ads.fingerprint.languages, vec!["en-US", "en"]);
    let ads_proxy = ads.proxy.expect("ads proxy");
    assert_eq!((ads_proxy.host.as_str(), ads_proxy.port), ("1.2.3.4", 1080));

    let dolphin = import_dolphin(DOLPHIN).expect("dolphin").profile;
    assert_eq!(
        dolphin.fingerprint.ua,
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
    );
    assert_eq!(dolphin.fingerprint.languages, vec!["de-DE", "de"]);

    let gologin = import_gologin(GOLOGIN).expect("gologin").profile;
    assert_eq!(
        gologin.fingerprint.ua,
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
    );
    assert_eq!(gologin.fingerprint.languages, vec!["en-US", "en"]);
    assert_eq!(gologin.proxy.expect("gologin proxy").port, 1080);
}

#[test]
fn seed_is_deterministic_across_runs() {
    // Same source => identical fingerprint (including canvas/audio seeds).
    for (first, second) in [
        (import_ads_power(ADSPOWER), import_ads_power(ADSPOWER)),
        (import_dolphin(DOLPHIN), import_dolphin(DOLPHIN)),
        (import_gologin(GOLOGIN), import_gologin(GOLOGIN)),
    ] {
        let first = first.expect("first import");
        let second = second.expect("second import");
        assert_eq!(first.profile.fingerprint, second.profile.fingerprint);
        assert_eq!(
            first.profile.fingerprint.canvas_seed,
            second.profile.fingerprint.canvas_seed
        );
        assert_eq!(
            first.profile.fingerprint.audio_seed,
            second.profile.fingerprint.audio_seed
        );
        assert_eq!(first.warnings, second.warnings);
        // The derived seed is recorded on the profile and is reproducible.
        assert_ne!(first.profile.seed, 0, "import must record its seed");
        assert_eq!(first.profile.seed, second.profile.seed);
    }
}

#[test]
fn autodetects_single_object_and_array() {
    let single = r#"{
        "user_id": "single-1",
        "name": "Single",
        "fingerprint_config": { "timezone": "Europe/Paris", "language": ["fr-FR", "fr"] }
    }"#;
    let one = import_ads_power(single).expect("single object");
    assert_eq!(one.profile.name, "Single");
    assert_eq!(one.profile.fingerprint.languages, vec!["fr-FR", "fr"]);
    assert!(!ignored(&one, "profiles"));

    let array = format!("[{single}, {single}]");
    let first = import_ads_power(&array).expect("array");
    assert_eq!(first.profile.name, "Single");
    assert!(
        ignored(&first, "profiles"),
        "extra profiles must be reported"
    );

    let all = import_ads_power_all(&array).expect("array batch");
    assert_eq!(all.len(), 2);
}

#[test]
fn missing_fields_become_defaulted_warnings() {
    // A minimal AdsPower profile without fingerprint_config / proxy.
    let json = r#"{ "user_id": "min-1", "name": "Minimal" }"#;
    let result = import_ads_power(json).expect("minimal import");
    for field in [
        "languages",
        "timezone",
        "screen",
        "screen.dpr",
        "webRTC",
        "proxy",
        "ua",
        "geo",
        "webgl",
    ] {
        assert!(
            defaulted(&result, field),
            "`{field}` should be defaulted, warnings: {:?}",
            result.warnings
        );
    }
    // Nothing from a missing UA: os is generated too.
    assert!(defaulted(&result, "os"));
    // A direct connection is a valid outcome.
    assert!(result.profile.proxy.is_none());
}

#[test]
fn unsupported_proxy_transport_is_ignored_not_defaulted() {
    let json = r#"{
        "user_id": "tor-1",
        "name": "TorProfile",
        "user_proxy_config": { "proxy_soft": "other", "proxy_type": "gologin", "proxy_host": "h", "proxy_port": "1" }
    }"#;
    let result = import_ads_power(json).expect("import");
    assert!(ignored(&result, "proxy"));
    assert!(result.profile.proxy.is_none());
}

#[test]
fn non_iana_timezone_is_ignored() {
    let json = r#"{
        "user_id": "tz-1",
        "name": "Tz",
        "fingerprint_config": { "timezone": "UTC+03:00" }
    }"#;
    let result = import_ads_power(json).expect("import");
    assert!(ignored(&result, "timezone"));
}

#[test]
fn errors_are_reported() {
    // Invalid JSON.
    assert!(matches!(
        import_ads_power("{ not json"),
        Err(ImportError::NotJson(_))
    ));

    // Valid JSON, wrong shape.
    assert!(matches!(
        import_gologin(r#"{ "foo": 1 }"#),
        Err(ImportError::UnknownFormat { .. })
    ));

    // Structure present, but no identity.
    assert!(matches!(
        import_ads_power(r#"{ "name": "", "fingerprint_config": {} }"#),
        Err(ImportError::Missing { .. })
    ));
}

#[test]
fn batch_import_returns_every_profile() {
    let dolphin_batch = format!("[{DOLPHIN}, {DOLPHIN}]");
    let all = import_dolphin_all(&dolphin_batch).expect("dolphin batch");
    assert_eq!(all.len(), 2);

    let wrapped = format!(r#"{{ "profiles": [{GOLOGIN}, {GOLOGIN}] }}"#);
    let all = import_gologin_all(&wrapped).expect("gologin wrapped batch");
    assert_eq!(all.len(), 2);
}

// --- consistency guarantee (wave-2 criterion: imported profiles validate) ----

/// Wave-2 criterion 3: an imported profile must pass the CONTRACT consistency
/// validator, exactly like a profile created through the API.
#[test]
fn imported_fixtures_pass_consistency_validation() {
    for result in [
        import_ads_power(ADSPOWER).expect("ads"),
        import_dolphin(DOLPHIN).expect("dolphin"),
        import_gologin(GOLOGIN).expect("gologin"),
    ] {
        let warnings = validate(&result.profile.fingerprint);
        assert!(
            warnings.is_empty(),
            "`{}` imported fingerprint fails validate: {warnings:?}",
            result.profile.name
        );
    }
}

/// A screen absent from fp-db (1024x768) must not survive: the generated,
/// db-backed screen is kept and a warning is reported.
#[test]
fn screen_outside_fp_db_falls_back_to_a_valid_record() {
    let json = r#"{
        "user_id": "screen-1024",
        "name": "OffDbScreen",
        "fingerprint_config": {
            "screen_resolution": "1024_768",
            "ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
        }
    }"#;
    let result = import_ads_power(json).expect("import");
    assert!(
        ignored(&result, "screen"),
        "screen outside fp-db must be ignored"
    );
    assert!(
        defaulted(&result, "screen"),
        "the generated screen must be used"
    );

    let fp = &result.profile.fingerprint;
    assert_ne!(
        fp.screen.width, 1024,
        "the off-db geometry must not survive"
    );
    let screens = db::screens().expect("screens db");
    assert!(
        screens.iter().any(|record| record.width == fp.screen.width
            && record.height == fp.screen.height
            && record.dpr == fp.screen.dpr),
        "final screen {}x{}@{} must be an fp-db record",
        fp.screen.width,
        fp.screen.height,
        fp.screen.dpr
    );
    assert!(validate(fp).is_empty(), "validate: {:?}", validate(fp));
}

/// macOS only reports the Retina steps 1 and 2; a fractional source dpr is
/// dropped and the record's dpr is used instead.
#[test]
fn macos_fractional_dpr_is_corrected_to_a_retina_step() {
    let json = r#"{
        "name": "mac-retina",
        "platform": "MacOS",
        "screen": { "mode": "manual", "resolution": "1920x1080x1.25" },
        "useragent": { "mode": "manual", "value": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36" }
    }"#;
    let result = import_dolphin(json).expect("import");
    let fp = &result.profile.fingerprint;
    assert_eq!(fp.os, Os::Macos);
    assert!(
        ignored(&result, "screen.dpr"),
        "macos dpr 1.25 must be rejected"
    );
    assert!(defaulted(&result, "screen.dpr"));
    assert_eq!(fp.screen.dpr, 1.0, "macos must keep a Retina-native dpr");
    assert!(validate(fp).is_empty(), "validate: {:?}", validate(fp));
}

/// A dpr outside {1, 1.25, 1.5, 2} is ignored and replaced by a legal value.
#[test]
fn dpr_outside_the_allowed_set_is_ignored() {
    let json = r#"{
        "id": "dpr-three",
        "name": "BadDpr",
        "os": "win",
        "navigator": {
            "userAgent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36",
            "resolution": "1920x1080"
        },
        "devicePixelRatio": 3
    }"#;
    let result = import_gologin(json).expect("import");
    let fp = &result.profile.fingerprint;
    assert!(
        ignored(&result, "screen.dpr"),
        "dpr 3 is outside {{1, 1.25, 1.5, 2}}"
    );
    assert!(defaulted(&result, "screen.dpr"));
    assert_eq!((fp.screen.width, fp.screen.height), (1920, 1080));
    assert_eq!(fp.screen.dpr, 1.0);
    assert!(validate(fp).is_empty(), "validate: {:?}", validate(fp));
}

/// A screen tagged for another os (Retina-logical size on windows) cannot back
/// the legend and must fall back to the generated screen.
#[test]
fn screen_tagged_for_another_os_falls_back() {
    let json = r#"{
        "name": "wrong-os-screen",
        "platform": "windows",
        "screen": { "mode": "manual", "resolution": "1512x982" },
        "useragent": { "mode": "manual", "value": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36" }
    }"#;
    let result = import_dolphin(json).expect("import");
    assert!(ignored(&result, "screen"));
    assert!(defaulted(&result, "screen"));
    assert!(validate(&result.profile.fingerprint).is_empty());
}

/// The fallback path stays reproducible: re-importing yields an identical
/// fingerprint and an identical warning report.
#[test]
fn fallback_import_stays_deterministic() {
    let json = r#"{
        "user_id": "det-screen",
        "name": "Det",
        "fingerprint_config": {
            "screen_resolution": "1024_768",
            "ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
        }
    }"#;
    let first = import_ads_power(json).expect("first");
    let second = import_ads_power(json).expect("second");
    assert_eq!(first.profile.fingerprint, second.profile.fingerprint);
    assert_eq!(first.profile.seed, second.profile.seed);
    assert_eq!(first.warnings, second.warnings);
    assert_ne!(first.profile.seed, 0);
}
