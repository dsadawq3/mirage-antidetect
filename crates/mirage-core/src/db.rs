//! Embedded fp-db access (CONTRACT "fp-db — схемы файлов").
//!
//! The JSON databases live in `<repo>/fp-db/` and are embedded into the
//! binary at compile time with `include_str!`. Each file is parsed once on
//! first use; a file that does not match its CONTRACT schema surfaces as a
//! [`DbError`].

use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use thiserror::Error;

use crate::model::Os;

const GPUS_JSON: &str = include_str!("../../../fp-db/gpus.json");
const FONTS_JSON: &str = include_str!("../../../fp-db/fonts.json");
const SCREENS_JSON: &str = include_str!("../../../fp-db/screens.json");
const LOCALES_JSON: &str = include_str!("../../../fp-db/locales.json");

/// Error loading the embedded fp-db: a file is not valid JSON or does not
/// match its CONTRACT schema.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DbError {
    #[error("malformed fp-db file `{file}`: {message}")]
    Malformed { file: &'static str, message: String },
}

/// One `gpus.json` entry (CONTRACT schema).
///
/// `weight` is not part of the CONTRACT schema for GPUs (screens/locales
/// carry it); it is accepted optionally and defaults to 1.0 so selection
/// stays uniform unless the db ships explicit weights.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuRecord {
    pub os: Os,
    pub vendor: String,
    pub renderer: String,
    pub max_texture_size: u32,
    pub max_viewport_dims: [u32; 2],
    pub max_renderbuffer_size: u32,
    #[serde(default = "default_weight")]
    pub weight: f64,
}

fn default_weight() -> f64 {
    1.0
}

/// One `screens.json` entry. `availHeight` is not stored: it is derived
/// per-os by the generator (`height - Os::taskbar_height()`).
///
/// `os` optionally binds the record to a single platform (`"macos"` for
/// Retina-logical sizes, `"windows"` for fractional-scale sizes); `None`
/// means the record is cross-platform. The generator only offers records
/// that are untagged or tagged for the legend os (M13).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenRecord {
    pub width: u32,
    pub height: u32,
    pub dpr: f64,
    pub weight: f64,
    #[serde(default)]
    pub os: Option<String>,
}

/// One `locales.json` entry: languages + timezone + matching geo point.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleRecord {
    pub languages: Vec<String>,
    pub timezone: String,
    pub lat: f64,
    pub lon: f64,
    pub weight: f64,
}

/// Internal shape of `fonts.json` (os -> full font list).
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FontsDb {
    windows: Vec<String>,
    macos: Vec<String>,
    linux: Vec<String>,
}

/// Parses one fp-db file into `T`, tagging failures with the file name.
fn parse_db<T: DeserializeOwned>(file: &'static str, json: &str) -> Result<T, DbError> {
    serde_json::from_str(json).map_err(|error| DbError::Malformed {
        file,
        message: error.to_string(),
    })
}

fn gpus_db() -> Result<&'static [GpuRecord], DbError> {
    static DB: OnceLock<Result<Vec<GpuRecord>, DbError>> = OnceLock::new();
    match DB.get_or_init(|| parse_db("fp-db/gpus.json", GPUS_JSON)) {
        Ok(list) => Ok(list.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

fn fonts_db() -> Result<&'static FontsDb, DbError> {
    static DB: OnceLock<Result<FontsDb, DbError>> = OnceLock::new();
    match DB.get_or_init(|| parse_db("fp-db/fonts.json", FONTS_JSON)) {
        Ok(db) => Ok(db),
        Err(error) => Err(error.clone()),
    }
}

fn screens_db() -> Result<&'static [ScreenRecord], DbError> {
    static DB: OnceLock<Result<Vec<ScreenRecord>, DbError>> = OnceLock::new();
    match DB.get_or_init(|| parse_db("fp-db/screens.json", SCREENS_JSON)) {
        Ok(list) => Ok(list.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

fn locales_db() -> Result<&'static [LocaleRecord], DbError> {
    static DB: OnceLock<Result<Vec<LocaleRecord>, DbError>> = OnceLock::new();
    match DB.get_or_init(|| parse_db("fp-db/locales.json", LOCALES_JSON)) {
        Ok(list) => Ok(list.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

/// All GPU entries known for `os`, in file order.
pub fn gpus(os: Os) -> Result<Vec<GpuRecord>, DbError> {
    Ok(gpus_db()?
        .iter()
        .filter(|gpu| gpu.os == os)
        .cloned()
        .collect())
}

/// The full font list for `os` (the spoof layer uses it as-is).
pub fn fonts(os: Os) -> Result<Vec<String>, DbError> {
    let db = fonts_db()?;
    let list = match os {
        Os::Windows => &db.windows,
        Os::Macos => &db.macos,
        Os::Linux => &db.linux,
    };
    Ok(list.clone())
}

/// All screen records. Most screens are cross-platform; records carrying an
/// `os` tag only fit legends of that os (the generator filters, the
/// validator checks). `availHeight` is still derived per-os by the generator.
pub fn screens() -> Result<Vec<ScreenRecord>, DbError> {
    Ok(screens_db()?.to_vec())
}

/// All locale records (languages, timezone and geo coordinates).
pub fn locales() -> Result<Vec<LocaleRecord>, DbError> {
    Ok(locales_db()?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPU_SAMPLE: &str = r#"[
        {"os":"windows","vendor":"Google Inc. (NVIDIA)","renderer":"ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002504) Direct3D11 vs_5_0 ps_5_0, D3D11)","maxTextureSize":16384,"maxViewportDims":[32767,32767],"maxRenderbufferSize":16384}
    ]"#;

    #[test]
    fn gpu_list_parses_contract_sample_with_default_weight() {
        let list = parse_db::<Vec<GpuRecord>>("fp-db/gpus.json", GPU_SAMPLE).expect("parse");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].os, Os::Windows);
        assert_eq!(list[0].vendor, "Google Inc. (NVIDIA)");
        assert_eq!(list[0].max_texture_size, 16384);
        assert_eq!(list[0].max_viewport_dims, [32767, 32767]);
        assert_eq!(list[0].max_renderbuffer_size, 16384);
        assert_eq!(list[0].weight, 1.0);
    }

    #[test]
    fn gpu_list_accepts_optional_weight() {
        let json = r#"[{"os":"linux","vendor":"Google Inc. (NVIDIA)","renderer":"ANGLE (NVIDIA, NVIDIA GeForce RTX 3060, OpenGL ES 3.2 NVIDIA)","maxTextureSize":16384,"maxViewportDims":[32767,32767],"maxRenderbufferSize":16384,"weight":2.5}]"#;
        let list = parse_db::<Vec<GpuRecord>>("fp-db/gpus.json", json).expect("parse");
        assert_eq!(list[0].weight, 2.5);
        assert_eq!(list[0].os, Os::Linux);
    }

    #[test]
    fn screen_and_locale_lists_parse_contract_samples() {
        let screens = parse_db::<Vec<ScreenRecord>>(
            "fp-db/screens.json",
            r#"[{"width":1920,"height":1080,"dpr":1,"weight":30}]"#,
        )
        .expect("parse screens");
        assert_eq!(screens.len(), 1);
        assert_eq!(screens[0].width, 1920);
        assert_eq!(screens[0].height, 1080);
        assert_eq!(screens[0].dpr, 1.0);
        assert_eq!(screens[0].weight, 30.0);
        assert_eq!(screens[0].os, None);

        let locales = parse_db::<Vec<LocaleRecord>>(
            "fp-db/locales.json",
            r#"[{"languages":["en-US","en"],"timezone":"Europe/London","lat":51.5,"lon":-0.12,"weight":10}]"#,
        )
        .expect("parse locales");
        assert_eq!(locales[0].languages, vec!["en-US", "en"]);
        assert_eq!(locales[0].timezone, "Europe/London");
        assert_eq!(locales[0].lat, 51.5);
        assert_eq!(locales[0].lon, -0.12);
        assert_eq!(locales[0].weight, 10.0);
    }

    #[test]
    fn screen_record_parses_optional_os_tag() {
        let screens = parse_db::<Vec<ScreenRecord>>(
            "fp-db/screens.json",
            r#"[
                {"width":1512,"height":982,"dpr":2,"weight":3,"os":"macos"},
                {"width":1536,"height":864,"dpr":1.25,"weight":8,"os":"windows"},
                {"width":1920,"height":1080,"dpr":1,"weight":30}
            ]"#,
        )
        .expect("parse screens");
        assert_eq!(screens[0].os.as_deref(), Some("macos"));
        assert_eq!(screens[1].os.as_deref(), Some("windows"));
        assert_eq!(screens[2].os, None);
    }

    #[test]
    fn fonts_db_parses_contract_sample() {
        let fonts = parse_db::<FontsDb>(
            "fp-db/fonts.json",
            r#"{"windows":["Arial"],"macos":["Helvetica"],"linux":["DejaVu Sans"]}"#,
        )
        .expect("parse fonts");
        assert_eq!(fonts.windows, vec!["Arial"]);
        assert_eq!(fonts.macos, vec!["Helvetica"]);
        assert_eq!(fonts.linux, vec!["DejaVu Sans"]);
    }

    #[test]
    fn malformed_json_maps_to_db_error() {
        let error =
            parse_db::<Vec<GpuRecord>>("fp-db/gpus.json", "{ not json").expect_err("must fail");
        assert!(matches!(error, DbError::Malformed { .. }));
        assert!(error.to_string().contains("gpus.json"));
    }

    #[test]
    fn schema_violation_maps_to_db_error() {
        // screens.json without the required weight field must be rejected.
        let error = parse_db::<Vec<ScreenRecord>>(
            "fp-db/screens.json",
            r#"[{"width":1920,"height":1080,"dpr":1}]"#,
        )
        .expect_err("must fail");
        assert!(matches!(error, DbError::Malformed { .. }));
    }
}
