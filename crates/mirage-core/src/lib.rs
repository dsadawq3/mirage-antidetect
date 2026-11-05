//! Mirage core: fingerprint data model, deterministic generator, consistency
//! validator and embedded fp-db access (CONTRACT wave 1).
//!
//! Public API surface:
//! - [`model`]: `Fingerprint`, `Profile` and their satellite types.
//! - [`generator`]: [`generate`] plus the Chrome version and GREASE constants.
//! - [`consistency`]: [`validate`] and [`ConsistencyWarning`].
//! - [`db`]: read access to the embedded fp-db (`fp-db/*.json`).

pub mod consistency;
pub mod db;
pub mod generator;
pub mod model;

pub use consistency::{validate, validate_seed_determinism, ConsistencyWarning};
pub use db::{DbError, GpuRecord, LocaleRecord, ScreenRecord};
pub use generator::{
    generate, GenerateError, CHROME_FULL, CHROME_MAJOR, GREASE_BRAND, GREASE_FULL_VERSION,
    GREASE_VERSION,
};
pub use model::{
    Brand, ClientHintsFp, Fingerprint, GeoFp, Os, Profile, ProxyConfig, ProxyScheme, ScreenFp,
    WebglFp,
};
