//! Fingerprint consistency validation (CONTRACT "Правила консистентности").
//!
//! [`validate`] checks CONTRACT rules 1-7 plus the M13 screen/os rules and
//! the minor-8 version/geometry rules on a single fingerprint and returns
//! every violation as a [`ConsistencyWarning`] (empty vector = fully
//! consistent). Rule 8 (canvas/audio seeds are a deterministic function of
//! the generation seed) is not observable from one instance, so it lives in
//! [`validate_seed_determinism`], which regenerates from the seed and
//! compares the full output.

use std::collections::BTreeSet;
use std::fmt;

use crate::db;
use crate::generator::{
    self, chrome_major, ua_template, CHROME_MAJOR, DEVICE_MEMORY_CHOICES, GREASE_BRAND,
    GREASE_VERSION, HARDWARE_CONCURRENCY_CHOICES,
};
use crate::model::{Fingerprint, Os};

/// A single consistency rule violation. Warnings, not errors: the UI may
/// still show or repair a partially broken fingerprint.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsistencyWarning {
    /// Rule 1-3: `navigator.platform` does not match the os.
    PlatformMismatch {
        os: Os,
        expected: &'static str,
        actual: String,
    },
    /// Rule 1-3: `clientHints.platform` does not match the os.
    ClientHintsPlatformMismatch {
        os: Os,
        expected: &'static str,
        actual: String,
    },
    /// Rule 1-3: the UA does not follow the os template.
    UaTemplateMismatch {
        os: Os,
        expected: String,
        actual: String,
    },
    /// Rule 1-3: the webgl renderer misses the os-specific ANGLE backend marker.
    RendererMismatch { os: Os, actual: String },
    /// Rule 1-3: fonts contain entries outside the fp-db list for the os.
    FontsNotFromDb { os: Os, unknown: Vec<String> },
    /// Rule 1-3: the font list is empty.
    EmptyFonts { os: Os },
    /// Rule 1-3: availHeight is not height minus the os shell height.
    AvailHeightMismatch {
        os: Os,
        height: u32,
        avail_height: u32,
        expected: i64,
    },
    /// Rule 4: deviceMemory outside {2, 4, 8}.
    DeviceMemoryInvalid { actual: u32 },
    /// Rule 4: hardwareConcurrency outside the allowed set.
    HardwareConcurrencyInvalid { actual: u32 },
    /// Rule 5: dpr outside {1, 1.25, 1.5, 2}.
    DprInvalid { actual: f64 },
    /// M13: macOS legends only report the Retina scaling steps 1 and 2.
    MacosDprInvalid { actual: f64 },
    /// Rule 5: the (width, height, dpr) triple is absent from screens.json.
    ScreenNotInDb { width: u32, height: u32, dpr: f64 },
    /// M13: the screens.json record backing this screen is tagged for a
    /// different os than the legend.
    ScreenOsMismatch {
        os: Os,
        screen_os: String,
        width: u32,
        height: u32,
        dpr: f64,
    },
    /// minor 8: osVersion/platformVersion mapping broken on windows.
    WindowsPlatformVersionMismatch { os_version: String, actual: String },
    /// minor 8: availWidth differs from width.
    AvailWidthMismatch { width: u32, avail_width: u32 },
    /// Rule 6: the languages list is empty.
    EmptyLanguages,
    /// Rule 6: the timezone is empty (must be an IANA name).
    EmptyTimezone,
    /// chromeVersion is not a dotted version string (blocks rules 1-3 and 7).
    InvalidChromeVersion { actual: String },
    /// Rule 7: a brand version differs from the chromeVersion major.
    BrandVersionMismatch {
        brand: String,
        expected: String,
        actual: String,
    },
    /// Rule 7: uaFullVersion differs from chromeVersion.
    UaFullVersionMismatch { expected: String, actual: String },
    /// Rule 8: regeneration from the same seed produced different output.
    NondeterministicSeeds,
    /// An fp-db backed check could not run.
    FpDbUnavailable { check: &'static str, error: String },
}

impl fmt::Display for ConsistencyWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformMismatch { os, expected, actual } => {
                write!(f, "os `{os}` requires platform `{expected}`, found `{actual}`")
            }
            Self::ClientHintsPlatformMismatch { os, expected, actual } => {
                write!(
                    f,
                    "os `{os}` requires clientHints.platform `{expected}`, found `{actual}`"
                )
            }
            Self::UaTemplateMismatch { os, expected, actual } => {
                write!(f, "ua for os `{os}` must be `{expected}`, found `{actual}`")
            }
            Self::RendererMismatch { os, actual } => write!(
                f,
                "webgl renderer for os `{os}` must contain one of [{}], found `{actual}`",
                renderer_tokens(*os).join(", ")
            ),
            Self::FontsNotFromDb { os, unknown } => write!(
                f,
                "fonts for os `{os}` must come from fp-db; unknown entries: {}",
                unknown.join(", ")
            ),
            Self::EmptyFonts { os } => {
                write!(f, "font list for os `{os}` must not be empty")
            }
            Self::AvailHeightMismatch {
                os,
                height,
                avail_height,
                expected,
            } => write!(
                f,
                "availHeight for os `{os}` must be height({height}) - shell({}) = {expected}, found {avail_height}",
                os.taskbar_height()
            ),
            Self::DeviceMemoryInvalid { actual } => write!(
                f,
                "deviceMemory must be one of {DEVICE_MEMORY_CHOICES:?}, found {actual}"
            ),
            Self::HardwareConcurrencyInvalid { actual } => write!(
                f,
                "hardwareConcurrency must be one of {HARDWARE_CONCURRENCY_CHOICES:?}, found {actual}"
            ),
            Self::DprInvalid { actual } => {
                write!(f, "dpr must be one of 1, 1.25, 1.5, 2, found {actual}")
            }
            Self::MacosDprInvalid { actual } => write!(
                f,
                "macos dpr must be 1 or 2 (real macOS reports no other scaling), found {actual}"
            ),
            Self::ScreenNotInDb { width, height, dpr } => {
                write!(f, "screen {width}x{height}@{dpr} not present in fp-db screens")
            }
            Self::ScreenOsMismatch {
                os,
                screen_os,
                width,
                height,
                dpr,
            } => write!(
                f,
                "screen {width}x{height}@{dpr} is tagged for os `{screen_os}` and cannot back an os `{os}` legend"
            ),
            Self::WindowsPlatformVersionMismatch { os_version, actual } => {
                let expected = if os_version == "11" {
                    "`13.` or `15.`"
                } else {
                    "`10.`"
                };
                write!(
                    f,
                    "windows osVersion {os_version} requires platformVersion starting with {expected}, found `{actual}`"
                )
            }
            Self::AvailWidthMismatch { width, avail_width } => write!(
                f,
                "availWidth must equal width ({width}); the os shell reserves vertical space only, found {avail_width}"
            ),
            Self::EmptyLanguages => write!(f, "languages must not be empty"),
            Self::EmptyTimezone => write!(f, "timezone must be a non-empty IANA name"),
            Self::InvalidChromeVersion { actual } => {
                write!(f, "chromeVersion must be a dotted version, found `{actual}`")
            }
            Self::BrandVersionMismatch {
                brand,
                expected,
                actual,
            } => write!(
                f,
                "brand `{brand}` version must be `{expected}` (chromeVersion major; the GREASE brand carries its own calibrated version), found `{actual}`"
            ),
            Self::UaFullVersionMismatch { expected, actual } => write!(
                f,
                "uaFullVersion must be `{expected}` (chromeVersion), found `{actual}`"
            ),
            Self::NondeterministicSeeds => write!(
                f,
                "canvasSeed/audioSeed must be deterministic for the generation seed"
            ),
            Self::FpDbUnavailable { check, error } => {
                write!(f, "fp-db check `{check}` could not run: {error}")
            }
        }
    }
}

/// Renderer markers per CONTRACT rules 1-3. Linux accepts the proprietary
/// NVIDIA driver marker besides Mesa/llvmpipe.
fn renderer_tokens(os: Os) -> &'static [&'static str] {
    match os {
        Os::Windows => &["Direct3D11", "D3D11"],
        Os::Macos => &["ANGLE Metal"],
        Os::Linux => &["Mesa", "llvmpipe", "OpenGL ES 3.2 NVIDIA"],
    }
}

/// Validates a fingerprint against CONTRACT rules 1-7 and returns every
/// violation found. An empty vector means the fingerprint is consistent.
pub fn validate(fp: &Fingerprint) -> Vec<ConsistencyWarning> {
    let mut warnings = Vec::new();
    let os = fp.os;

    // Rules 1-3: navigator.platform and clientHints.platform.
    let expected_platform = os.platform();
    if fp.platform != expected_platform {
        warnings.push(ConsistencyWarning::PlatformMismatch {
            os,
            expected: expected_platform,
            actual: fp.platform.clone(),
        });
    }
    let expected_ch_platform = os.client_hints_platform();
    if fp.client_hints.platform != expected_ch_platform {
        warnings.push(ConsistencyWarning::ClientHintsPlatformMismatch {
            os,
            expected: expected_ch_platform,
            actual: fp.client_hints.platform.clone(),
        });
    }

    // minor 8: osVersion <-> platformVersion mapping on windows. Windows 11
    // builds report UA-CH platformVersion 13.x/15.x; Windows 10 stays on 10.x.
    if os == Os::Windows {
        let expected_prefixes: &[&str] = match fp.os_version.as_str() {
            "11" => &["13.", "15."],
            "10" => &["10."],
            // Unknown windows osVersion: CONTRACT defines no mapping.
            _ => &[],
        };
        if !expected_prefixes.is_empty()
            && !expected_prefixes
                .iter()
                .any(|prefix| fp.client_hints.platform_version.starts_with(prefix))
        {
            warnings.push(ConsistencyWarning::WindowsPlatformVersionMismatch {
                os_version: fp.os_version.clone(),
                actual: fp.client_hints.platform_version.clone(),
            });
        }
    }

    // Rules 1-3: UA rebuilt from the fingerprint's own chrome major.
    let major = chrome_major(&fp.chrome_version);
    if let Some(major) = major {
        let expected_ua = ua_template(os, major);
        if fp.ua != expected_ua {
            warnings.push(ConsistencyWarning::UaTemplateMismatch {
                os,
                expected: expected_ua,
                actual: fp.ua.clone(),
            });
        }
    } else {
        warnings.push(ConsistencyWarning::InvalidChromeVersion {
            actual: fp.chrome_version.clone(),
        });
    }

    // Rules 1-3: renderer must carry the os ANGLE backend marker.
    if !renderer_tokens(os)
        .iter()
        .any(|token| fp.webgl.renderer.contains(*token))
    {
        warnings.push(ConsistencyWarning::RendererMismatch {
            os,
            actual: fp.webgl.renderer.clone(),
        });
    }

    // Rules 1-3: fonts must come from the fp-db list for the os.
    match db::fonts(os) {
        Ok(db_fonts) => {
            if fp.fonts.is_empty() {
                warnings.push(ConsistencyWarning::EmptyFonts { os });
            } else {
                let known: BTreeSet<&str> = db_fonts.iter().map(String::as_str).collect();
                let unknown: Vec<String> = fp
                    .fonts
                    .iter()
                    .filter(|font| !known.contains(font.as_str()))
                    .cloned()
                    .collect();
                if !unknown.is_empty() {
                    warnings.push(ConsistencyWarning::FontsNotFromDb { os, unknown });
                }
            }
        }
        Err(error) => warnings.push(ConsistencyWarning::FpDbUnavailable {
            check: "fonts",
            error: error.to_string(),
        }),
    }

    // Rules 1-3: availHeight = height - os shell height.
    // i64 math: a (broken) height below the shell height must not underflow.
    let expected_avail = fp.screen.height as i64 - os.taskbar_height() as i64;
    if fp.screen.avail_height as i64 != expected_avail {
        warnings.push(ConsistencyWarning::AvailHeightMismatch {
            os,
            height: fp.screen.height,
            avail_height: fp.screen.avail_height,
            expected: expected_avail,
        });
    }

    // minor 8: the os shell reserves vertical space only, so availWidth
    // must equal width.
    if fp.screen.avail_width != fp.screen.width {
        warnings.push(ConsistencyWarning::AvailWidthMismatch {
            width: fp.screen.width,
            avail_width: fp.screen.avail_width,
        });
    }

    // Rule 4: memory and cores.
    if !DEVICE_MEMORY_CHOICES.contains(&fp.device_memory) {
        warnings.push(ConsistencyWarning::DeviceMemoryInvalid {
            actual: fp.device_memory,
        });
    }
    if !HARDWARE_CONCURRENCY_CHOICES.contains(&fp.hardware_concurrency) {
        warnings.push(ConsistencyWarning::HardwareConcurrencyInvalid {
            actual: fp.hardware_concurrency,
        });
    }

    // Rule 5: dpr and screen presence in the db.
    let dpr = fp.screen.dpr;
    if !(dpr == 1.0 || dpr == 1.25 || dpr == 1.5 || dpr == 2.0) {
        warnings.push(ConsistencyWarning::DprInvalid { actual: dpr });
    } else if os == Os::Macos && !(dpr == 1.0 || dpr == 2.0) {
        // M13: fractional dpr values (1.25/1.5) are OS scaling artifacts;
        // real macOS only exposes the Retina steps 1 and 2.
        warnings.push(ConsistencyWarning::MacosDprInvalid { actual: dpr });
    }
    match db::screens() {
        Ok(screens) => {
            let record = screens.iter().find(|screen| {
                screen.width == fp.screen.width
                    && screen.height == fp.screen.height
                    && screen.dpr == fp.screen.dpr
            });
            match record {
                None => warnings.push(ConsistencyWarning::ScreenNotInDb {
                    width: fp.screen.width,
                    height: fp.screen.height,
                    dpr: fp.screen.dpr,
                }),
                Some(record) => {
                    // M13: a record tagged for another os cannot back this
                    // legend (Retina-logical size on windows, 125% scale on
                    // macos, ...).
                    if let Some(tag) = record.os.as_deref() {
                        if tag != os.wire_name() {
                            warnings.push(ConsistencyWarning::ScreenOsMismatch {
                                os,
                                screen_os: tag.to_string(),
                                width: fp.screen.width,
                                height: fp.screen.height,
                                dpr: fp.screen.dpr,
                            });
                        }
                    }
                }
            }
        }
        Err(error) => warnings.push(ConsistencyWarning::FpDbUnavailable {
            check: "screens",
            error: error.to_string(),
        }),
    }

    // Rule 6: languages and timezone.
    if fp.languages.is_empty() {
        warnings.push(ConsistencyWarning::EmptyLanguages);
    }
    if fp.timezone.trim().is_empty() {
        warnings.push(ConsistencyWarning::EmptyTimezone);
    }

    // Rule 7: brand versions and uaFullVersion (skipped when the major is
    // unparseable — InvalidChromeVersion is already reported above).
    if let Some(major) = major {
        let expected_major = major.to_string();
        for brand in &fp.client_hints.brands {
            // The GREASE brand carries its own calibrated version instead of
            // the chromeVersion major (CONTRACT GREASE calibration). The
            // exact GREASE value is only assertable while the fingerprint
            // matches the Chrome major this build was calibrated for.
            let expected: &str = if brand.brand == GREASE_BRAND {
                if major == CHROME_MAJOR {
                    GREASE_VERSION
                } else {
                    continue;
                }
            } else {
                expected_major.as_str()
            };
            if brand.version != expected {
                warnings.push(ConsistencyWarning::BrandVersionMismatch {
                    brand: brand.brand.clone(),
                    expected: expected.to_string(),
                    actual: brand.version.clone(),
                });
            }
        }
        if fp.client_hints.ua_full_version != fp.chrome_version {
            warnings.push(ConsistencyWarning::UaFullVersionMismatch {
                expected: fp.chrome_version.clone(),
                actual: fp.client_hints.ua_full_version.clone(),
            });
        }
    }

    warnings
}

/// CONTRACT rule 8: canvasSeed/audioSeed must be a deterministic function of
/// the generation seed. Regenerates from `seed` twice and compares the full
/// fingerprints (full equality subsumes the seed comparison).
pub fn validate_seed_determinism(seed: u64, os: Option<Os>) -> Vec<ConsistencyWarning> {
    let first = match generator::generate(seed, os) {
        Ok(fp) => fp,
        Err(error) => {
            return vec![ConsistencyWarning::FpDbUnavailable {
                check: "generator",
                error: error.to_string(),
            }]
        }
    };
    let second = match generator::generate(seed, os) {
        Ok(fp) => fp,
        Err(error) => {
            return vec![ConsistencyWarning::FpDbUnavailable {
                check: "generator",
                error: error.to_string(),
            }]
        }
    };
    if first != second {
        return vec![ConsistencyWarning::NondeterministicSeeds];
    }
    Vec::new()
}
