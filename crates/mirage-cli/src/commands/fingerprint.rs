//! `mirage fingerprint show`: generate a fingerprint for legend debugging.

use serde_json::Value;

use mirage_core::{generate, Fingerprint};

use crate::cli::FingerprintShowArgs;
use crate::error::CliError;
use crate::output::{to_json_value, Render};

/// Result of `fingerprint show`.
#[derive(Debug, Clone)]
pub struct FingerprintView {
    /// The generated fingerprint.
    pub fingerprint: Fingerprint,
}

/// Generate a deterministic fingerprint from the seed (and optional os).
pub fn run_fingerprint_show(args: &FingerprintShowArgs) -> Result<FingerprintView, CliError> {
    let os = args.os.map(|arg| arg.to_core());
    let fingerprint = generate(args.seed, os)?;
    Ok(FingerprintView { fingerprint })
}

impl Render for FingerprintView {
    fn human(&self) -> String {
        serde_json::to_string_pretty(&self.fingerprint)
            .unwrap_or_else(|error| format!("<failed to render fingerprint: {error}>"))
    }

    fn json(&self) -> Value {
        to_json_value(&self.fingerprint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{FingerprintShowArgs, OsArg};
    use mirage_core::Os;

    #[test]
    fn generation_is_deterministic_for_a_fixed_seed() {
        let args = FingerprintShowArgs {
            seed: 12345,
            os: Some(OsArg::Windows),
        };
        let first = run_fingerprint_show(&args).expect("first");
        let second = run_fingerprint_show(&args).expect("second");
        assert_eq!(first.fingerprint, second.fingerprint);

        // Must equal the core generator directly.
        let direct = generate(12345, Some(Os::Windows)).expect("direct");
        assert_eq!(first.fingerprint, direct);
    }

    #[test]
    fn os_is_derived_from_seed_when_absent() {
        let args = FingerprintShowArgs { seed: 7, os: None };
        let view = run_fingerprint_show(&args).expect("generated");
        // A concrete os is always produced.
        assert!(matches!(
            view.fingerprint.os,
            Os::Windows | Os::Macos | Os::Linux
        ));
    }
}
