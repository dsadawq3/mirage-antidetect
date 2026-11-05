//! Deterministic fingerprint generation.
//!
//! [`generate`] derives a complete [`Fingerprint`] from a single `u64` seed.
//! The draw order over the seeded [`StdRng`] is fixed (os, gpu, screen,
//! locale, osVersion/platformVersion, hardwareConcurrency, deviceMemory,
//! canvasSeed, audioSeed), so the same `(seed, os)` pair always produces a
//! bitwise identical fingerprint — this is what CONTRACT consistency rule 8
//! (deterministic canvas/audio seeds) relies on. Screen records tagged for
//! another os are filtered out before the weighted draw (M13); the filter
//! consumes no rng values and therefore does not shift the draw order.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use thiserror::Error;

use crate::db::{self, DbError, GpuRecord, LocaleRecord, ScreenRecord};
use crate::model::{Brand, ClientHintsFp, Fingerprint, GeoFp, Os, ScreenFp, WebglFp};

/// Chrome major version the fp data is calibrated for (UA Reduction era).
pub const CHROME_MAJOR: u32 = 156;
/// Full Chrome version for `chromeVersion` / `clientHints.uaFullVersion`.
pub const CHROME_FULL: &str = "156.0.8078.80";

/// GREASE brand name for this Chrome major (live dump of Chrome
/// 156.0.8078.80 via `crates/mirage-launcher/examples/dump_native.rs`).
/// Chromium rotates the GREASE string with the major: recalibrate the
/// triple below whenever `CHROME_MAJOR` changes.
pub const GREASE_BRAND: &str = "Not:A-Brand";
/// GREASE version inside the low-entropy `brands` list. Deliberately NOT
/// the Chrome major — the live dump shows "8".
pub const GREASE_VERSION: &str = "8";
/// GREASE version inside the CDP `fullVersionList` (CONTRACT CDP section:
/// the GREASE entry gets this, every other brand gets `uaFullVersion`).
pub const GREASE_FULL_VERSION: &str = "8.0.0.0";

/// Screen color depth Chrome reports on every desktop platform.
const COLOR_DEPTH: u32 = 24;

/// CONTRACT osVersion choices; the macos/linux pairs carry the matching
/// platformVersion (Darwin build / kernel `.0` form).
const WINDOWS_OS_VERSIONS: [&str; 2] = ["10", "11"];
const WINDOWS_11_PLATFORM_VERSIONS: [&str; 2] = ["13.0.0", "15.0.0"];
const MACOS_VERSIONS: [(&str, &str); 5] = [
    ("10.15", "10.15.7"),
    ("11", "11.7.10"),
    ("12", "12.7.4"),
    ("13", "13.7.1"),
    ("14", "14.7.2"),
];
const LINUX_VERSIONS: [(&str, &str); 3] = [("5.15", "5.15.0"), ("6.1", "6.1.0"), ("6.5", "6.5.0")];

/// hardwareConcurrency values real desktop Chrome installs report.
pub(crate) const HARDWARE_CONCURRENCY_CHOICES: [u32; 8] = [2, 4, 6, 8, 12, 16, 20, 24];
/// deviceMemory values real Chrome reports (browser cap is 8).
pub(crate) const DEVICE_MEMORY_CHOICES: [u32; 3] = [2, 4, 8];

/// os alternatives used when [`generate`] is called with `os: None`.
const OS_CHOICES: [Os; 3] = [Os::Windows, Os::Macos, Os::Linux];

/// Failure of [`generate`]: the embedded fp-db has no usable data.
#[derive(Debug, Error)]
pub enum GenerateError {
    /// The embedded fp-db failed to parse.
    #[error("fp-db failure: {0}")]
    Db(#[from] DbError),
    /// No GPU entries for the requested os.
    #[error("no usable GPU entries in fp-db for os `{os}`")]
    NoGpus { os: Os },
    /// No screen entries at all.
    #[error("no usable screen entries in fp-db")]
    NoScreens,
    /// No locale entries at all.
    #[error("no usable locale entries in fp-db")]
    NoLocales,
    /// Empty font list for the requested os.
    #[error("empty font list in fp-db for os `{os}`")]
    NoFonts { os: Os },
}

/// Extracts the major from a dotted Chrome version (`"156.0.8078.80"` -> 156).
pub(crate) fn chrome_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// CONTRACT UA template for the os (UA Reduction freezes minor versions in
/// the UA as `.0.0.0`; the macos UA always carries `10_15_7`).
pub(crate) fn ua_template(os: Os, major: u32) -> String {
    match os {
        Os::Windows => format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"),
        Os::Macos => format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"),
        Os::Linux => format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"),
    }
}

/// Weighted choice over `items`: a single `rng.gen::<f64>() * total` draw
/// crosses the cumulative weight sum (CONTRACT generator rule).
/// Returns `None` for an empty slice or a non-positive total weight.
fn weighted_pick<'a, T>(
    rng: &mut StdRng,
    items: &'a [T],
    weight: impl Fn(&T) -> f64,
) -> Option<&'a T> {
    if items.is_empty() {
        return None;
    }
    let total: f64 = items.iter().map(&weight).sum();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let mut point = rng.gen::<f64>() * total;
    for item in items {
        point -= weight(item);
        if point < 0.0 {
            return Some(item);
        }
    }
    // Floating-point rounding can leave the point inside the last item.
    items.last()
}

/// M13: a screen record may back an `os` legend when it carries no os tag
/// (cross-platform record) or a tag matching the os wire name. Tagged
/// mismatches are impossible combinations in the wild (macOS with a 125%
/// scale, Windows with a Retina-logical size).
fn screen_fits_os(record: &ScreenRecord, os: Os) -> bool {
    record.os.as_deref().is_none_or(|tag| tag == os.wire_name())
}

/// CONTRACT brand list, calibrated against the live Chrome 156 dump:
/// GREASE brand first carrying its own version, then Chromium and
/// Google Chrome at `CHROME_MAJOR` (order is fingerprint-relevant).
fn contract_brands() -> Vec<Brand> {
    let version = CHROME_MAJOR.to_string();
    vec![
        Brand {
            brand: GREASE_BRAND.to_string(),
            version: GREASE_VERSION.to_string(),
        },
        Brand {
            brand: "Chromium".to_string(),
            version: version.clone(),
        },
        Brand {
            brand: "Google Chrome".to_string(),
            version,
        },
    ]
}

/// Generates a complete fingerprint deterministically from `seed`.
///
/// `os` selects the target platform; `None` derives it from the seed. All
/// fp-db driven values are picked by weight; `webRTC` is always `"real"`
/// (the relay/off modes are a launch-time policy, not a fingerprint trait).
pub fn generate(seed: u64, os: Option<Os>) -> Result<Fingerprint, GenerateError> {
    let mut rng = StdRng::seed_from_u64(seed);

    let os = match os {
        Some(os) => os,
        None => OS_CHOICES[rng.gen_range(0..OS_CHOICES.len())],
    };

    // --- fp-db driven picks (weighted) --------------------------------------
    let gpus = db::gpus(os)?;
    let gpu = weighted_pick(&mut rng, &gpus, |gpu: &GpuRecord| gpu.weight)
        .ok_or(GenerateError::NoGpus { os })?;

    let screens = db::screens()?;
    // M13: drop records tagged for another os before the weighted draw.
    // Filtering consumes no rng values, so the draw order stays fixed.
    let screens: Vec<ScreenRecord> = screens
        .into_iter()
        .filter(|screen| screen_fits_os(screen, os))
        .collect();
    let screen = weighted_pick(&mut rng, &screens, |screen: &ScreenRecord| screen.weight)
        .ok_or(GenerateError::NoScreens)?;

    let locales = db::locales()?;
    let locale = weighted_pick(&mut rng, &locales, |locale: &LocaleRecord| locale.weight)
        .ok_or(GenerateError::NoLocales)?;

    let fonts = db::fonts(os)?;
    if fonts.is_empty() {
        return Err(GenerateError::NoFonts { os });
    }

    // --- os / platform versions ----------------------------------------------
    let (os_version, platform_version) = match os {
        Os::Windows => {
            let version = WINDOWS_OS_VERSIONS[rng.gen_range(0..WINDOWS_OS_VERSIONS.len())];
            let platform = if version == "11" {
                WINDOWS_11_PLATFORM_VERSIONS[rng.gen_range(0..WINDOWS_11_PLATFORM_VERSIONS.len())]
            } else {
                "10.0.0"
            };
            (version.to_string(), platform.to_string())
        }
        Os::Macos => {
            let (version, platform) = MACOS_VERSIONS[rng.gen_range(0..MACOS_VERSIONS.len())];
            (version.to_string(), platform.to_string())
        }
        Os::Linux => {
            let (version, platform) = LINUX_VERSIONS[rng.gen_range(0..LINUX_VERSIONS.len())];
            (version.to_string(), platform.to_string())
        }
    };

    // --- hardware -------------------------------------------------------------
    let hardware_concurrency =
        HARDWARE_CONCURRENCY_CHOICES[rng.gen_range(0..HARDWARE_CONCURRENCY_CHOICES.len())];
    let device_memory = DEVICE_MEMORY_CHOICES[rng.gen_range(0..DEVICE_MEMORY_CHOICES.len())];

    // --- spoofing seeds (CONTRACT rule 8: pure functions of the rng state) ----
    let canvas_seed: u32 = rng.gen();
    let audio_seed: u32 = rng.gen();

    Ok(Fingerprint {
        os,
        os_version,
        platform: os.platform().to_string(),
        ua: ua_template(os, CHROME_MAJOR),
        chrome_version: CHROME_FULL.to_string(),
        languages: locale.languages.clone(),
        timezone: locale.timezone.clone(),
        geo: GeoFp {
            lat: locale.lat,
            lon: locale.lon,
        },
        screen: ScreenFp {
            width: screen.width,
            height: screen.height,
            avail_width: screen.width,
            avail_height: screen.height.saturating_sub(os.taskbar_height()),
            dpr: screen.dpr,
            color_depth: COLOR_DEPTH,
        },
        hardware_concurrency,
        device_memory,
        webgl: WebglFp {
            vendor: gpu.vendor.clone(),
            renderer: gpu.renderer.clone(),
            max_texture_size: gpu.max_texture_size,
            max_viewport_dims: gpu.max_viewport_dims,
            max_renderbuffer_size: gpu.max_renderbuffer_size,
        },
        canvas_seed,
        audio_seed,
        fonts,
        web_rtc: "real".to_string(),
        client_hints: ClientHintsFp {
            brands: contract_brands(),
            mobile: false,
            platform: os.client_hints_platform().to_string(),
            platform_version,
            architecture: "x86".to_string(),
            bitness: "64".to_string(),
            ua_full_version: CHROME_FULL.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn chrome_major_parses_and_rejects() {
        assert_eq!(chrome_major("156.0.8078.80"), Some(156));
        assert_eq!(chrome_major("156"), Some(156));
        assert_eq!(chrome_major(""), None);
        assert_eq!(chrome_major("abc.1"), None);
    }

    #[test]
    fn weighted_pick_is_deterministic_and_covers_all_items() {
        let items = [("a", 1.0), ("b", 3.0), ("c", 0.5)];
        let mut seen = BTreeSet::new();
        for seed in 0..100u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let choice = weighted_pick(&mut rng, &items, |item: &(&str, f64)| item.1)
                .expect("non-empty with positive weights");
            seen.insert(choice.0);
        }
        assert_eq!(seen.len(), 3, "every item must be reachable");
        let mut rng_a = StdRng::seed_from_u64(7);
        let mut rng_b = StdRng::seed_from_u64(7);
        assert_eq!(
            weighted_pick(&mut rng_a, &items, |item: &(&str, f64)| item.1),
            weighted_pick(&mut rng_b, &items, |item: &(&str, f64)| item.1),
            "same seed must pick the same item"
        );
    }

    #[test]
    fn weighted_pick_rejects_empty_and_degenerate_weights() {
        let mut rng = StdRng::seed_from_u64(1);
        let empty: [u32; 0] = [];
        assert!(weighted_pick(&mut rng, &empty, |_| 1.0).is_none());
        let zeroed = [("a", 0.0), ("b", 0.0)];
        assert!(
            weighted_pick(&mut rng, &zeroed, |item: &(&str, f64)| item.1).is_none(),
            "zero total weight must not be selectable"
        );
    }

    #[test]
    fn ua_templates_match_contract() {
        assert_eq!(
            ua_template(Os::Windows, 156),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
        );
        assert_eq!(
            ua_template(Os::Macos, 156),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
        );
        assert_eq!(
            ua_template(Os::Linux, 156),
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"
        );
    }

    #[test]
    fn grease_constants_match_live_dump() {
        // Live dump of Chrome 156.0.8078.80 (dump_native example).
        assert_eq!(GREASE_BRAND, "Not:A-Brand");
        assert_eq!(GREASE_VERSION, "8");
        assert_eq!(GREASE_FULL_VERSION, "8.0.0.0");
    }

    #[test]
    fn contract_brands_follow_live_dump_order() {
        let brands = contract_brands();
        let names: Vec<&str> = brands.iter().map(|brand| brand.brand.as_str()).collect();
        assert_eq!(names, [GREASE_BRAND, "Chromium", "Google Chrome"]);
        assert_eq!(brands[0].version, GREASE_VERSION);
        assert_eq!(brands[1].version, CHROME_MAJOR.to_string());
        assert_eq!(brands[2].version, CHROME_MAJOR.to_string());
    }

    #[test]
    fn screen_fits_os_honors_tags() {
        let untagged = ScreenRecord {
            width: 1920,
            height: 1080,
            dpr: 1.0,
            weight: 30.0,
            os: None,
        };
        let macos_tagged = ScreenRecord {
            width: 1512,
            height: 982,
            dpr: 2.0,
            weight: 3.0,
            os: Some("macos".to_string()),
        };
        let windows_tagged = ScreenRecord {
            width: 1536,
            height: 864,
            dpr: 1.25,
            weight: 8.0,
            os: Some("windows".to_string()),
        };
        for os in OS_CHOICES {
            assert!(screen_fits_os(&untagged, os), "untagged fits {os}");
        }
        assert!(screen_fits_os(&macos_tagged, Os::Macos));
        assert!(!screen_fits_os(&macos_tagged, Os::Windows));
        assert!(!screen_fits_os(&macos_tagged, Os::Linux));
        assert!(screen_fits_os(&windows_tagged, Os::Windows));
        assert!(!screen_fits_os(&windows_tagged, Os::Macos));
        assert!(!screen_fits_os(&windows_tagged, Os::Linux));
    }
}
