//! `MIRAGE_INJECT_DIR` runtime-override plumbing.
//!
//! Kept in its own test binary on purpose: the variable is process-global, and
//! setting it here would race every other test in the same process (they run on
//! parallel threads). A separate binary is a separate process, so the override
//! is invisible to the rest of the suite.

mod common;

use common::profile_direct;
use mirage_launcher::launch::{
    build_inject_source, fingerprint_prefix, INJECT_DIR_ENV, INJECT_MODULES,
};

#[tokio::test]
async fn env_override_switches_the_runtime_source_to_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (index, name) in INJECT_MODULES.iter().enumerate() {
        std::fs::write(
            dir.path().join(format!("{name}.js")),
            format!("// env module {index}\n"),
        )
        .expect("write env module");
    }
    let profile = profile_direct();
    let prefix = fingerprint_prefix(&profile.fingerprint).expect("prefix");

    // Restore the previous value even if the build panics, so a failure cannot
    // leak the override into any later test in this binary.
    let previous = std::env::var_os(INJECT_DIR_ENV);
    std::env::set_var(INJECT_DIR_ENV, dir.path());
    let built = build_inject_source(&profile.fingerprint).await;
    match previous {
        Some(value) => std::env::set_var(INJECT_DIR_ENV, value),
        None => std::env::remove_var(INJECT_DIR_ENV),
    }

    let from_env = built.expect("build");
    assert!(from_env.starts_with(&prefix));
    assert!(
        from_env.contains("// env module 0\n"),
        "env override was not applied"
    );
    // And none of the embedded module bytes leaked in.
    assert!(
        !from_env.contains("__MIRAGE_RNG__"),
        "embedded layer still used despite the override"
    );
}
