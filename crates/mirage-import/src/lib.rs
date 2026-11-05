//! `mirage-import` — import browser profiles from competing antidetect
//! browsers (AdsPower, Dolphin {anty}, GoLogin) into Mirage [`Profile`]s.
//!
//! # Design
//!
//! Each source format is parsed into a format-agnostic [`SourceProfile`], and
//! [`build_profile`] turns that into a Mirage profile:
//!
//! 1. A deterministic `seed` is derived from the **source profile id**
//!    (`FNV-1a 64`). It feeds `mirage_core::generate`, so every fingerprint
//!    field the source does not provide is *generated*, never invented by hand,
//!    and re-importing the same record yields a bit-identical fingerprint.
//! 2. Fields the source *does* provide (UA -> os/platform, proxy, timezone,
//!    languages, screen, webRTC) are mapped on top of the generated baseline.
//! 3. Every field decision is recorded as an [`ImportWarning`]: `Mapped`,
//!    `Defaulted` (absent in source, filled by the generator) or `Ignored`
//!    (present in source but not usable).
//! 4. A source screen (and its dpr) is adopted only when `fp-db/screens.json`
//!    backs the exact geometry for the legend os; otherwise the generated
//!    screen is kept. The assembled fingerprint is then checked with
//!    `mirage_core::validate`: any violation is reported and the fingerprint is
//!    rebuilt from the seed, so an imported profile always passes the
//!    consistency validator (wave-2 criterion 3).
//!
//! The parser auto-detects the envelope: a bare profile object, a JSON array,
//! or a wrapper object (`data.list`, `list`, `profiles`, ...) all work.
//!
//! Parsers are tolerant by construction: anything undocumented is simply not
//! mapped (and reported), nothing is guessed. See the per-module doc comments
//! for the documentation each mapping is based on.

mod adspower;
mod dolphin;
mod gologin;
mod util;

pub use adspower::{import_ads_power, import_ads_power_all};
pub use dolphin::{import_dolphin, import_dolphin_all};
pub use gologin::{import_gologin, import_gologin_all};

use mirage_core::{generate, validate, ConsistencyWarning, GeoFp, Os, Profile, ScreenRecord};
use serde_json::Value;
use thiserror::Error;

use util::{fnv1a64, is_iana_timezone, parse_ua, MappedProxy};

/// Values `hardwareConcurrency` may take (CONTRACT consistency rule 4).
const HARDWARE_CONCURRENCY_CHOICES: [u32; 8] = [2, 4, 6, 8, 12, 16, 20, 24];
/// Values `deviceMemory` may take (CONTRACT consistency rule 4).
const DEVICE_MEMORY_CHOICES: [u32; 3] = [2, 4, 8];

/// Mirage has a single calibrated Chrome major; imported source versions are
/// normalized to it (CONTRACT: `CHROME_MAJOR = 156`).
const CHROME_MAJOR: u32 = mirage_core::CHROME_MAJOR;

/// WebRTC policy mapped onto the Mirage vocabulary (`real | relay | off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcMode {
    /// Real local ICE candidates (no masking).
    Real,
    /// Only proxied/relay candidates are exposed.
    Relay,
    /// WebRTC is fully disabled.
    Off,
}

impl WebRtcMode {
    /// Wire value stored in the fingerprint (`fp.webRTC`).
    pub fn as_str(self) -> &'static str {
        match self {
            WebRtcMode::Real => "real",
            WebRtcMode::Relay => "relay",
            WebRtcMode::Off => "off",
        }
    }
}

/// A single field-level outcome produced while importing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportWarning {
    /// The source field was mapped onto the profile (possibly transformed).
    Mapped {
        /// Logical Mirage field name (e.g. `"proxy"`, `"languages"`).
        field: String,
    },
    /// The field was absent in the source and filled by the Mirage generator.
    Defaulted {
        /// Logical Mirage field name.
        field: String,
    },
    /// The field was present in the source but could not be applied.
    Ignored {
        /// Logical Mirage field name (e.g. `"proxy.type"`, `"os"`).
        field: String,
    },
}

impl ImportWarning {
    /// The field name this warning refers to, regardless of its kind.
    pub fn field(&self) -> &str {
        match self {
            ImportWarning::Mapped { field }
            | ImportWarning::Defaulted { field }
            | ImportWarning::Ignored { field } => field,
        }
    }

    /// True for [`ImportWarning::Mapped`].
    pub fn is_mapped(&self) -> bool {
        matches!(self, ImportWarning::Mapped { .. })
    }

    /// True for [`ImportWarning::Defaulted`].
    pub fn is_defaulted(&self) -> bool {
        matches!(self, ImportWarning::Defaulted { .. })
    }

    /// True for [`ImportWarning::Ignored`].
    pub fn is_ignored(&self) -> bool {
        matches!(self, ImportWarning::Ignored { .. })
    }
}

/// Result of importing one source profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    /// The resulting Mirage profile (fresh uuid/id and timestamps).
    pub profile: Profile,
    /// Field-level mapping report, in a deterministic order.
    pub warnings: Vec<ImportWarning>,
}

/// Failure modes of an import.
#[derive(Debug, Error)]
pub enum ImportError {
    /// The input is not valid JSON.
    #[error("input is not valid JSON: {0}")]
    NotJson(#[from] serde_json::Error),
    /// The JSON is valid but does not carry a recognizable profile object.
    #[error("unrecognized {format} structure: {detail}")]
    UnknownFormat {
        /// Human-readable source name ("AdsPower" / "Dolphin Anty" / "GoLogin").
        format: &'static str,
        /// What was expected vs. found.
        detail: String,
    },
    /// A required source field is missing or empty.
    #[error("required field `{field}` is missing in the source profile")]
    Missing {
        /// The missing field name.
        field: String,
    },
    /// `mirage_core::generate` failed (embedded fp-db is broken). Never
    /// expected at runtime, surfaced instead of panicking.
    #[error("fingerprint generation failed: {0}")]
    Generate(#[from] mirage_core::GenerateError),
}

/// Screen geometry extracted from a source, before the generator fills `dpr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ScreenSpec {
    pub width: u32,
    pub height: u32,
}

/// Format-agnostic intermediate produced by each parser before mapping.
#[derive(Debug, Clone, Default)]
pub(crate) struct SourceProfile {
    /// Source record id (drives the deterministic fingerprint seed).
    pub source_id: Option<String>,
    pub name: Option<String>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    /// Operating system, if the source states it or its UA implies one.
    pub os: Option<Os>,
    /// True when the source had an `os`/`platform` field (even if unusable).
    pub os_seen: bool,
    /// Raw source user-agent string (Chrome-family only is usable).
    pub ua: Option<String>,
    pub languages: Option<Vec<String>>,
    /// IANA timezone, when the source value is a valid IANA name.
    pub timezone: Option<String>,
    /// True when the source had a timezone value that was not usable.
    pub timezone_seen: bool,
    pub screen: Option<ScreenSpec>,
    /// Device pixel ratio, when the source states it.
    pub dpr: Option<f64>,
    pub webrtc: Option<WebRtcMode>,
    /// True when the source carried WebGL metadata (not mapped: Mirage keeps
    /// the fp-db GPU identity, so this is reported as ignored).
    pub webgl_seen: bool,
    pub hardware_concurrency: Option<u32>,
    pub device_memory: Option<u32>,
    /// Source geolocation (lat, lon), when present.
    pub geo: Option<(f64, f64)>,
    pub proxy: MappedProxy,
}

/// Auto-detects the profile envelope and returns its profile objects.
///
/// Accepts: a bare profile object, a JSON array of profiles, or a wrapper
/// object (`{"data": {"list": [...]}}`, `{"data": [...]}`, `{"list": [...]}`,
/// `{"profiles": [...]}`). `markers` are the keys that identify a bare object
/// as a profile for this specific format.
pub(crate) fn collect_profiles<'a>(
    value: &'a Value,
    format: &'static str,
    markers: &[&str],
) -> Result<Vec<&'a Value>, ImportError> {
    if let Some(items) = value.as_array() {
        let profiles = objects_only(items);
        if profiles.is_empty() {
            return Err(ImportError::UnknownFormat {
                format,
                detail: "JSON array contains no profile objects".to_string(),
            });
        }
        return Ok(profiles);
    }

    let object = value
        .as_object()
        .ok_or_else(|| ImportError::UnknownFormat {
            format,
            detail: "top level is neither an object nor an array".to_string(),
        })?;

    // Wrapper keys carrying a profile array directly.
    for key in ["profiles", "list", "users"] {
        if let Some(Value::Array(items)) = object.get(key) {
            let profiles = objects_only(items);
            if !profiles.is_empty() {
                return Ok(profiles);
            }
        }
    }

    // Nested `data`: array, `data.list`, or a single profile object.
    if let Some(data) = object.get("data") {
        if let Some(items) = data.as_array() {
            let profiles = objects_only(items);
            if !profiles.is_empty() {
                return Ok(profiles);
            }
        }
        if let Some(Value::Array(items)) = data.get("list") {
            let profiles = objects_only(items);
            if !profiles.is_empty() {
                return Ok(profiles);
            }
        }
        if looks_like_profile(data, markers) {
            return Ok(vec![data]);
        }
    }

    if looks_like_profile(value, markers) {
        return Ok(vec![value]);
    }

    Err(ImportError::UnknownFormat {
        format,
        detail: "no profile object found (none of the expected fields are present)".to_string(),
    })
}

/// Keeps only object elements of a JSON array.
fn objects_only(items: &[Value]) -> Vec<&Value> {
    items.iter().filter(|item| item.is_object()).collect()
}

/// True when `value` is an object carrying at least one format marker key.
fn looks_like_profile(value: &Value, markers: &[&str]) -> bool {
    value.is_object() && markers.iter().any(|marker| value.get(*marker).is_some())
}

/// `dpr` values a Mirage fingerprint may carry (CONTRACT consistency rule 5).
const DPR_CHOICES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

/// True when a `screens.json` record may back an `os` legend: an untagged
/// record is cross-platform, a tagged record must match the legend os
/// (CONTRACT M13 / the validator's `ScreenOsMismatch` rule).
fn screen_record_fits_os(record: &ScreenRecord, os: Os) -> bool {
    record.os.as_deref().is_none_or(|tag| tag == os.wire_name())
}

/// Outcome of matching a source screen against the embedded fp-db.
enum ScreenMatch {
    /// A `screens.json` record backs this geometry for the legend os.
    Accepted {
        width: u32,
        height: u32,
        /// The dpr actually adopted (always an allowed value).
        dpr: f64,
        /// True when the source's own dpr was adopted, false when the record's
        /// canonical dpr was used instead.
        dpr_from_source: bool,
    },
    /// No record backs the geometry for this os: keep the generated screen.
    Rejected,
}

/// Matches a source screen against `fp-db/screens.json` (CONTRACT rule 5, M13).
///
/// The source geometry is accepted only when a record with the same
/// `(width, height)` exists and can back the legend os (otherwise the
/// validator would flag `ScreenNotInDb` / `ScreenOsMismatch`). The dpr is the
/// source value when it is in the allowed set and such a record exists,
/// otherwise the first compatible record's dpr in file order (deterministic).
/// Anything else rejects the screen, so the caller keeps the generated one.
fn match_source_screen(screen: ScreenSpec, source_dpr: Option<f64>, os: Os) -> ScreenMatch {
    let screens = match mirage_core::db::screens() {
        Ok(screens) => screens,
        // A broken fp-db already fails `generate` upstream; reject defensively.
        Err(_) => return ScreenMatch::Rejected,
    };
    let candidates: Vec<&ScreenRecord> = screens
        .iter()
        .filter(|record| {
            record.width == screen.width
                && record.height == screen.height
                && DPR_CHOICES.contains(&record.dpr)
                && screen_record_fits_os(record, os)
        })
        .collect();
    if candidates.is_empty() {
        return ScreenMatch::Rejected;
    }
    // Prefer the exact source dpr when it is a legal value backed by a record.
    if let Some(source_dpr) = source_dpr.filter(|dpr| DPR_CHOICES.contains(dpr)) {
        if let Some(record) = candidates.iter().find(|record| record.dpr == source_dpr) {
            return ScreenMatch::Accepted {
                width: record.width,
                height: record.height,
                dpr: record.dpr,
                dpr_from_source: true,
            };
        }
    }
    let record = candidates[0];
    ScreenMatch::Accepted {
        width: record.width,
        height: record.height,
        dpr: record.dpr,
        dpr_from_source: false,
    }
}

/// Maps a [`ConsistencyWarning`] onto the logical Mirage field it concerns, so
/// a validator failure can be reported through the [`ImportWarning`] channel.
fn consistency_field(warning: &ConsistencyWarning) -> &'static str {
    match warning {
        ConsistencyWarning::PlatformMismatch { .. } => "platform",
        ConsistencyWarning::ClientHintsPlatformMismatch { .. } => "clientHints.platform",
        ConsistencyWarning::UaTemplateMismatch { .. } => "ua",
        ConsistencyWarning::RendererMismatch { .. } => "webgl",
        ConsistencyWarning::FontsNotFromDb { .. } => "fonts",
        ConsistencyWarning::EmptyFonts { .. } => "fonts",
        ConsistencyWarning::AvailHeightMismatch { .. } => "screen.availHeight",
        ConsistencyWarning::DeviceMemoryInvalid { .. } => "deviceMemory",
        ConsistencyWarning::HardwareConcurrencyInvalid { .. } => "hardwareConcurrency",
        ConsistencyWarning::DprInvalid { .. } => "screen.dpr",
        ConsistencyWarning::MacosDprInvalid { .. } => "screen.dpr",
        ConsistencyWarning::ScreenNotInDb { .. } => "screen",
        ConsistencyWarning::ScreenOsMismatch { .. } => "screen",
        ConsistencyWarning::WindowsPlatformVersionMismatch { .. } => "osVersion",
        ConsistencyWarning::AvailWidthMismatch { .. } => "screen.availWidth",
        ConsistencyWarning::EmptyLanguages => "languages",
        ConsistencyWarning::EmptyTimezone => "timezone",
        ConsistencyWarning::InvalidChromeVersion { .. } => "chromeVersion",
        ConsistencyWarning::BrandVersionMismatch { .. } => "clientHints.brands",
        ConsistencyWarning::UaFullVersionMismatch { .. } => "clientHints.uaFullVersion",
        ConsistencyWarning::NondeterministicSeeds => "canvasSeed",
        ConsistencyWarning::FpDbUnavailable { .. } => "fpDb",
    }
}

/// Folds a [`SourceProfile`] into a complete Mirage [`Profile`].
///
/// `seed_material` is used for the deterministic seed only when the source has
/// no id (it is the serialized source record, so the seed stays reproducible).
pub(crate) fn build_profile(
    mut source: SourceProfile,
    seed_material: &str,
) -> Result<ImportResult, ImportError> {
    let mut warnings: Vec<ImportWarning> = Vec::new();
    // Records a `Mapped` note for `field` on the local `warnings` vector.
    macro_rules! mapped {
        ($field:expr) => {
            warnings.push(ImportWarning::Mapped {
                field: $field.into(),
            })
        };
    }

    let has_id = source
        .source_id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty());
    let has_name = source
        .name
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty());
    if !has_id && !has_name {
        return Err(ImportError::Missing {
            field: "name".to_string(),
        });
    }

    // --- deterministic baseline -------------------------------------------------
    let seed_key = if has_id {
        source.source_id.as_deref().unwrap_or(seed_material)
    } else {
        seed_material
    };
    let seed = fnv1a64(seed_key);
    let mut fingerprint = generate(seed, source.os)?;

    // --- identity ---------------------------------------------------------------
    let name = if let Some(name) = source.name.clone().filter(|n| !n.trim().is_empty()) {
        mapped!("name");
        name
    } else {
        warnings.push(ImportWarning::Defaulted {
            field: "name".into(),
        });
        match source.source_id.clone() {
            Some(id) if !id.trim().is_empty() => format!("imported {id}"),
            _ => "imported profile".to_string(),
        }
    };

    // --- os / platform ----------------------------------------------------------
    if let Some(os) = source.os {
        fingerprint.os = os;
        fingerprint.platform = os.platform().to_string();
        mapped!("os");
        mapped!("platform");
    } else if source.os_seen {
        warnings.push(ImportWarning::Ignored { field: "os".into() });
        warnings.push(ImportWarning::Defaulted { field: "os".into() });
        warnings.push(ImportWarning::Defaulted {
            field: "platform".into(),
        });
    } else {
        warnings.push(ImportWarning::Defaulted { field: "os".into() });
        warnings.push(ImportWarning::Defaulted {
            field: "platform".into(),
        });
    }

    // --- user-agent -------------------------------------------------------------
    // Mirage ships Chrome 156; the source UA is only used to confirm the OS and
    // report the source's Chrome major. The profile UA stays the calibrated
    // Mirage template (so it round-trips exactly when the source is Chrome 156).
    match source.ua.as_deref() {
        Some(ua) => {
            let info = parse_ua(ua);
            if info.chromium && info.os.is_some() {
                mapped!("ua");
                if info.chrome_major != Some(CHROME_MAJOR) {
                    warnings.push(ImportWarning::Ignored {
                        field: "ua.version".into(),
                    });
                }
            } else {
                warnings.push(ImportWarning::Ignored { field: "ua".into() });
                warnings.push(ImportWarning::Defaulted { field: "ua".into() });
            }
        }
        None => warnings.push(ImportWarning::Defaulted { field: "ua".into() }),
    }

    // --- languages --------------------------------------------------------------
    if let Some(languages) = source.languages.clone().filter(|l| !l.is_empty()) {
        fingerprint.languages = languages;
        mapped!("languages");
    } else {
        warnings.push(ImportWarning::Defaulted {
            field: "languages".into(),
        });
    }

    // --- timezone ---------------------------------------------------------------
    match source.timezone.clone() {
        Some(timezone) if is_iana_timezone(&timezone) => {
            fingerprint.timezone = timezone;
            mapped!("timezone");
        }
        Some(_) => {
            warnings.push(ImportWarning::Ignored {
                field: "timezone".into(),
            });
            warnings.push(ImportWarning::Defaulted {
                field: "timezone".into(),
            });
        }
        None if source.timezone_seen => {
            warnings.push(ImportWarning::Ignored {
                field: "timezone".into(),
            });
            warnings.push(ImportWarning::Defaulted {
                field: "timezone".into(),
            });
        }
        None => warnings.push(ImportWarning::Defaulted {
            field: "timezone".into(),
        }),
    }

    // --- screen -----------------------------------------------------------------
    // A source screen is adopted only when `fp-db/screens.json` backs the exact
    // geometry for the legend os (CONTRACT rule 5 / M13); otherwise the
    // generated screen wins, so the imported profile never fails the validator
    // on the screen rules.
    match source.screen {
        Some(screen) => match match_source_screen(screen, source.dpr, fingerprint.os) {
            ScreenMatch::Accepted {
                width,
                height,
                dpr,
                dpr_from_source,
            } => {
                fingerprint.screen.width = width;
                fingerprint.screen.height = height;
                fingerprint.screen.avail_width = width;
                fingerprint.screen.avail_height =
                    height.saturating_sub(fingerprint.os.taskbar_height());
                fingerprint.screen.dpr = dpr;
                mapped!("screen");
                if dpr_from_source {
                    mapped!("screen.dpr");
                } else {
                    // The source dpr (when present) was unusable: either it was
                    // outside {1, 1.25, 1.5, 2} or no record carried it.
                    if source.dpr.is_some() {
                        warnings.push(ImportWarning::Ignored {
                            field: "screen.dpr".into(),
                        });
                    }
                    warnings.push(ImportWarning::Defaulted {
                        field: "screen.dpr".into(),
                    });
                }
            }
            ScreenMatch::Rejected => {
                // Missing from fp-db, or tagged for another os: keep the
                // generated screen (always a valid, os-compatible db record).
                warnings.push(ImportWarning::Ignored {
                    field: "screen".into(),
                });
                warnings.push(ImportWarning::Defaulted {
                    field: "screen".into(),
                });
                if source.dpr.is_some() {
                    warnings.push(ImportWarning::Ignored {
                        field: "screen.dpr".into(),
                    });
                }
                warnings.push(ImportWarning::Defaulted {
                    field: "screen.dpr".into(),
                });
            }
        },
        None => {
            warnings.push(ImportWarning::Defaulted {
                field: "screen".into(),
            });
            warnings.push(ImportWarning::Defaulted {
                field: "screen.dpr".into(),
            });
        }
    }

    // --- WebRTC -----------------------------------------------------------------
    match source.webrtc {
        Some(mode) => {
            fingerprint.web_rtc = mode.as_str().to_string();
            mapped!("webRTC");
        }
        None => warnings.push(ImportWarning::Defaulted {
            field: "webRTC".into(),
        }),
    }

    // --- hardware (validated against the CONTRACT allowed sets) -----------------
    match source.hardware_concurrency {
        Some(value) if HARDWARE_CONCURRENCY_CHOICES.contains(&value) => {
            fingerprint.hardware_concurrency = value;
            mapped!("hardwareConcurrency");
        }
        Some(_) => warnings.push(ImportWarning::Ignored {
            field: "hardwareConcurrency".into(),
        }),
        None => warnings.push(ImportWarning::Defaulted {
            field: "hardwareConcurrency".into(),
        }),
    }
    match source.device_memory {
        Some(value) if DEVICE_MEMORY_CHOICES.contains(&value) => {
            fingerprint.device_memory = value;
            mapped!("deviceMemory");
        }
        Some(_) => warnings.push(ImportWarning::Ignored {
            field: "deviceMemory".into(),
        }),
        None => warnings.push(ImportWarning::Defaulted {
            field: "deviceMemory".into(),
        }),
    }

    // --- geolocation ------------------------------------------------------------
    match source.geo {
        Some((lat, lon)) => {
            fingerprint.geo = GeoFp { lat, lon };
            mapped!("geo");
        }
        None => warnings.push(ImportWarning::Defaulted {
            field: "geo".into(),
        }),
    }

    // --- fields always taken from the generator ---------------------------------
    for field in [
        "fonts",
        "canvasSeed",
        "audioSeed",
        "clientHints",
        "chromeVersion",
        "osVersion",
    ] {
        warnings.push(ImportWarning::Defaulted {
            field: field.into(),
        });
    }
    if source.webgl_seen {
        warnings.push(ImportWarning::Ignored {
            field: "webgl".into(),
        });
    }
    warnings.push(ImportWarning::Defaulted {
        field: "webgl".into(),
    });

    // --- proxy ------------------------------------------------------------------
    let proxy = match source.proxy {
        MappedProxy::Parsed(config) => {
            mapped!("proxy");
            Some(config)
        }
        MappedProxy::Approximated(config) => {
            mapped!("proxy");
            warnings.push(ImportWarning::Ignored {
                field: "proxy.type".into(),
            });
            Some(config)
        }
        MappedProxy::Unsupported => {
            warnings.push(ImportWarning::Ignored {
                field: "proxy".into(),
            });
            warnings.push(ImportWarning::Defaulted {
                field: "proxy".into(),
            });
            None
        }
        MappedProxy::Absent => {
            warnings.push(ImportWarning::Defaulted {
                field: "proxy".into(),
            });
            None
        }
    };

    // --- consistency guarantee --------------------------------------------------
    // Every value mapped above is sanitized (CONTRACT allowed sets and an
    // fp-db backed, os-compatible screen), so the assembled fingerprint already
    // passes `mirage_core::validate`. Verify it anyway: a mapped value the
    // validator rejects would make the profile unusable downstream (the API
    // refuses such a fingerprint on `POST /profiles`). If anything is off,
    // report each violation and rebuild a clean fingerprint from the same seed,
    // so `build_profile` never returns a profile in a state `validate` rejects.
    let violations = validate(&fingerprint);
    if !violations.is_empty() {
        for violation in &violations {
            warnings.push(ImportWarning::Ignored {
                field: consistency_field(violation).to_string(),
            });
        }
        fingerprint = generate(seed, source.os)?;
    }

    // --- assemble ---------------------------------------------------------------
    // Record the deterministic seed so the profile can rebuild its exact
    // fingerprint later (`Profile::with_seed`), not just this import run.
    let mut profile = Profile::new(name, fingerprint, proxy).with_seed(seed);
    profile.tags = std::mem::take(&mut source.tags);
    if let Some(notes) = source.notes.filter(|n| !n.trim().is_empty()) {
        profile.notes = notes;
        mapped!("notes");
    }
    if !profile.tags.is_empty() {
        mapped!("tags");
    }

    Ok(ImportResult { profile, warnings })
}

/// Shared helper: import the first profile and note when more were present.
pub(crate) fn first_or_error(
    mut all: Vec<ImportResult>,
    format: &'static str,
) -> Result<ImportResult, ImportError> {
    if all.is_empty() {
        return Err(ImportError::UnknownFormat {
            format,
            detail: "profile list is empty".to_string(),
        });
    }
    let total = all.len();
    let mut first = all.remove(0);
    if total > 1 {
        first.warnings.push(ImportWarning::Ignored {
            field: "profiles".to_string(),
        });
    }
    Ok(first)
}

/// Serializes a source `Value` into the seed-fallback material (never fails in
/// practice; an empty string fallback keeps the function total).
pub(crate) fn seed_material(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_helpers() {
        let warn = ImportWarning::Mapped {
            field: "proxy".into(),
        };
        assert_eq!(warn.field(), "proxy");
        assert!(warn.is_mapped());
        assert!(!warn.is_ignored());
        assert!(ImportWarning::Ignored { field: "x".into() }.is_ignored());
        assert!(ImportWarning::Defaulted { field: "x".into() }.is_defaulted());
    }

    #[test]
    fn web_rtc_wire_values() {
        assert_eq!(WebRtcMode::Real.as_str(), "real");
        assert_eq!(WebRtcMode::Relay.as_str(), "relay");
        assert_eq!(WebRtcMode::Off.as_str(), "off");
    }

    #[test]
    fn collect_profiles_autodetects_shapes() {
        let single = serde_json::json!({"name": "a", "user_id": "1"});
        assert_eq!(
            collect_profiles(&single, "T", &["name"])
                .expect("single")
                .len(),
            1
        );
        let array = serde_json::json!([{"name": "a"}, {"name": "b"}]);
        assert_eq!(
            collect_profiles(&array, "T", &["name"])
                .expect("array")
                .len(),
            2
        );
        let wrapped = serde_json::json!({"data": {"list": [{"name": "a"}]}});
        assert_eq!(
            collect_profiles(&wrapped, "T", &["name"])
                .expect("wrapped")
                .len(),
            1
        );
        let data_single = serde_json::json!({"data": {"name": "a"}});
        assert_eq!(
            collect_profiles(&data_single, "T", &["name"])
                .expect("data single")
                .len(),
            1
        );
        assert!(collect_profiles(&serde_json::json!({"foo": 1}), "T", &["name"]).is_err());
        assert!(collect_profiles(&serde_json::json!(42), "T", &["name"]).is_err());
    }

    #[test]
    fn missing_identity_is_an_error() {
        let source = SourceProfile::default();
        assert!(matches!(
            build_profile(source, "{}"),
            Err(ImportError::Missing { .. })
        ));
    }

    #[test]
    fn match_source_screen_accepts_db_backed_geometry() {
        // 1920x1080@1 is an untagged fp-db record: usable on any os.
        let matched = match_source_screen(
            ScreenSpec {
                width: 1920,
                height: 1080,
            },
            None,
            Os::Windows,
        );
        assert!(matches!(
            matched,
            ScreenMatch::Accepted {
                width: 1920,
                height: 1080,
                dpr_from_source: false,
                ..
            }
        ));
        if let ScreenMatch::Accepted { dpr, .. } = matched {
            assert_eq!(dpr, 1.0);
        }
    }

    #[test]
    fn match_source_screen_accepts_source_dpr_when_backed() {
        // 1536x864@1.25 is a windows-tagged record: the source dpr is honored.
        let matched = match_source_screen(
            ScreenSpec {
                width: 1536,
                height: 864,
            },
            Some(1.25),
            Os::Windows,
        );
        match matched {
            ScreenMatch::Accepted {
                dpr,
                dpr_from_source,
                ..
            } => {
                assert_eq!(dpr, 1.25);
                assert!(dpr_from_source);
            }
            ScreenMatch::Rejected => panic!("1536x864@1.25 must be accepted on windows"),
        }
    }

    #[test]
    fn match_source_screen_rejects_unknown_and_cross_os_geometry() {
        // 1024x768 is absent from fp-db.
        assert!(matches!(
            match_source_screen(
                ScreenSpec {
                    width: 1024,
                    height: 768
                },
                None,
                Os::Windows
            ),
            ScreenMatch::Rejected
        ));
        // 1512x982 is macos-tagged and cannot back a windows legend.
        assert!(matches!(
            match_source_screen(
                ScreenSpec {
                    width: 1512,
                    height: 982
                },
                None,
                Os::Windows
            ),
            ScreenMatch::Rejected
        ));
    }

    #[test]
    fn match_source_screen_defaults_invalid_source_dpr() {
        // dpr 3 is outside the allowed set: keep the record's canonical dpr.
        let matched = match_source_screen(
            ScreenSpec {
                width: 1920,
                height: 1080,
            },
            Some(3.0),
            Os::Windows,
        );
        match matched {
            ScreenMatch::Accepted {
                dpr,
                dpr_from_source,
                ..
            } => {
                assert_eq!(dpr, 1.0);
                assert!(!dpr_from_source);
            }
            ScreenMatch::Rejected => panic!("1920x1080 must be accepted"),
        }
    }

    #[test]
    fn consistency_field_maps_violations_to_fields() {
        use mirage_core::ConsistencyWarning;
        assert_eq!(
            consistency_field(&ConsistencyWarning::ScreenNotInDb {
                width: 1,
                height: 1,
                dpr: 1.0,
            }),
            "screen"
        );
        assert_eq!(
            consistency_field(&ConsistencyWarning::DprInvalid { actual: 3.0 }),
            "screen.dpr"
        );
        assert_eq!(
            consistency_field(&ConsistencyWarning::EmptyLanguages),
            "languages"
        );
        assert_eq!(
            consistency_field(&ConsistencyWarning::DeviceMemoryInvalid { actual: 3 }),
            "deviceMemory"
        );
    }

    #[test]
    fn build_profile_falls_back_when_source_screen_is_unknown() {
        let source = SourceProfile {
            source_id: Some("unit-screen".into()),
            name: Some("Unit".into()),
            os: Some(Os::Windows),
            screen: Some(ScreenSpec {
                width: 1024,
                height: 768,
            }),
            ..SourceProfile::default()
        };
        let result = build_profile(source, "{}").expect("build");
        assert!(result
            .warnings
            .iter()
            .any(|w| w.is_ignored() && w.field() == "screen"));
        assert!(result
            .warnings
            .iter()
            .any(|w| w.is_defaulted() && w.field() == "screen"));
        // The generated screen is a valid fp-db record, so validate passes.
        assert!(validate(&result.profile.fingerprint).is_empty());
        assert_ne!(result.profile.fingerprint.screen.width, 1024);
    }
}
