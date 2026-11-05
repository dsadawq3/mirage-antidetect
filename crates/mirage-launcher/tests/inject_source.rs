//! Inject-source assembly tests: contract module order, newline separation,
//! missing-module error, the compile-time embedded layer, and the
//! `MIRAGE_INJECT_DIR` dev override.

mod common;

use common::profile_direct;
use mirage_launcher::launch::{
    assemble_source, build_inject_source_from, build_worker_inject_source_from,
    embedded_inject_modules, embedded_worker_snippet, fingerprint_prefix, inject_dir,
    read_inject_modules, worker_fingerprint_prefix, INJECT_MODULES,
};

#[test]
fn inject_modules_constant_is_the_contract_order() {
    assert_eq!(
        INJECT_MODULES,
        [
            "rng",
            "navigator",
            "screen",
            "time",
            "plugins",
            "clienthints",
            "canvas",
            "webgl",
            "audio",
            "webrtc"
        ]
    );
}

#[test]
fn inject_dir_points_at_the_workspace_root_inject_dir() {
    let dir = inject_dir();
    assert!(
        dir.ends_with("inject"),
        "unexpected inject dir: {}",
        dir.display()
    );
}

#[tokio::test]
async fn reads_modules_in_contract_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (index, name) in INJECT_MODULES.iter().enumerate() {
        std::fs::write(
            dir.path().join(format!("{name}.js")),
            format!("// module {index}\n"),
        )
        .expect("write fake module");
    }
    let modules = read_inject_modules(dir.path()).await.expect("read modules");
    assert_eq!(modules.len(), INJECT_MODULES.len());
    for (index, (name, content)) in modules.iter().enumerate() {
        assert_eq!(*name, INJECT_MODULES[index], "module order broken");
        assert_eq!(content, &format!("// module {index}\n"));
    }
}

#[tokio::test]
async fn missing_module_is_reported_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Only the first module exists; the second one is missing.
    std::fs::write(dir.path().join("rng.js"), "// rng\n").expect("write fake module");
    let error = read_inject_modules(dir.path())
        .await
        .expect_err("reading must fail on a missing module");
    assert!(
        error
            .to_string()
            .contains("missing inject module: navigator"),
        "unexpected error: {error}"
    );
}

#[test]
fn assemble_source_separates_modules_with_newlines() {
    let source = assemble_source(
        "HEAD\n",
        &[("a", String::from("x")), ("b", String::from("y\n"))],
    );
    assert_eq!(source, "HEAD\nx\ny\n");
}

#[test]
fn assemble_source_keeps_prefix_verbatim() {
    let source = assemble_source("window.__MIRAGE_FP__ = {};\n", &[]);
    assert_eq!(source, "window.__MIRAGE_FP__ = {};\n");
}

#[test]
fn embedded_layer_has_every_module_in_contract_order() {
    let modules = embedded_inject_modules();
    assert_eq!(modules.len(), INJECT_MODULES.len());
    for (index, (name, content)) in modules.iter().enumerate() {
        assert_eq!(*name, INJECT_MODULES[index], "embedded order broken");
        assert!(
            !content.trim().is_empty(),
            "embedded module {name} is empty"
        );
    }
}

#[test]
fn embedded_layer_matches_the_on_disk_modules() {
    // Strong guard against `include_str!` pointing at the wrong file: in the
    // source tree the embedded bytes must equal the `inject/*.js` on disk.
    // (Skipped when the inject dir is absent, e.g. a relocated test binary.)
    if !inject_dir().is_dir() {
        eprintln!(
            "skipping: inject dir not present at {}",
            inject_dir().display()
        );
        return;
    }
    for (name, content) in embedded_inject_modules() {
        let on_disk = std::fs::read_to_string(inject_dir().join(format!("{name}.js")))
            .expect("read inject module");
        assert_eq!(content, on_disk, "embedded {name}.js differs from disk");
    }
}

#[tokio::test]
async fn embedded_source_is_the_prefix_plus_modules_and_is_idempotent() {
    let profile = profile_direct();
    let fp = &profile.fingerprint;
    let prefix = fingerprint_prefix(fp).expect("prefix");
    let expected = assemble_source(&prefix, &embedded_inject_modules());

    let first = build_inject_source_from(fp, None).await.expect("build");
    let second = build_inject_source_from(fp, None).await.expect("build");
    assert_eq!(first, expected);
    // Idempotent: repeated builds are byte-identical.
    assert_eq!(first, second);
}

#[tokio::test]
async fn override_dir_reads_from_disk_in_order_and_is_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (index, name) in INJECT_MODULES.iter().enumerate() {
        std::fs::write(
            dir.path().join(format!("{name}.js")),
            format!("// override module {index}\n"),
        )
        .expect("write override module");
    }
    let profile = profile_direct();
    let fp = &profile.fingerprint;
    let prefix = fingerprint_prefix(fp).expect("prefix");

    let first = build_inject_source_from(fp, Some(dir.path()))
        .await
        .expect("build");
    let second = build_inject_source_from(fp, Some(dir.path()))
        .await
        .expect("build");
    assert!(
        first.starts_with(&prefix),
        "source must start with the prefix"
    );
    assert_eq!(first, second);
    // Disk content, in the contract order, one line per module.
    for (index, _name) in INJECT_MODULES.iter().enumerate() {
        assert!(
            first.contains(&format!("// override module {index}\n")),
            "missing module {index} in override source"
        );
    }
}

#[tokio::test]
async fn override_dir_missing_module_aborts() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Only rng present: the override read must fail on the next module.
    std::fs::write(dir.path().join("rng.js"), "// rng\n").expect("write");
    let profile = profile_direct();
    let error = build_inject_source_from(&profile.fingerprint, Some(dir.path()))
        .await
        .expect_err("override must fail on a missing module");
    assert!(
        error
            .to_string()
            .contains("missing inject module: navigator"),
        "unexpected: {error}"
    );
}

// ---- worker snippet (inject/worker.js) -----------------------------------

/// A string that only the worker snippet contains, used to prove it never
/// leaks into the page bundle.
const WORKER_ONLY_MARKER: &str = "installOffscreenCanvas";

#[test]
fn worker_snippet_is_embedded_and_not_a_page_module() {
    assert!(
        !INJECT_MODULES.contains(&"worker"),
        "worker.js must never be listed as a page module"
    );
    let snippet = embedded_worker_snippet();
    assert!(!snippet.trim().is_empty(), "embedded worker.js is empty");
    assert!(
        snippet.contains(WORKER_ONLY_MARKER),
        "embedded worker.js does not look like worker.js"
    );
    assert!(
        !snippet.contains("window."),
        "the worker snippet must be window-free"
    );
}

#[tokio::test]
async fn worker_source_is_worker_prefix_then_the_snippet() {
    let profile = profile_direct();
    let fp = &profile.fingerprint;
    // Golden order: `self.__MIRAGE_FP__ = ...;` first, then worker.js verbatim.
    let prefix = worker_fingerprint_prefix(fp).expect("prefix");
    let expected = assemble_source(&prefix, &[("worker", embedded_worker_snippet().to_owned())]);
    let built = build_worker_inject_source_from(fp, None)
        .await
        .expect("build");

    assert_eq!(built, expected);
    assert!(built.starts_with(&prefix), "worker prefix must come first");
    assert!(built.contains(WORKER_ONLY_MARKER));
    // Idempotent: repeated builds are byte-identical.
    let again = build_worker_inject_source_from(fp, None)
        .await
        .expect("build");
    assert_eq!(built, again);
}

#[tokio::test]
async fn worker_source_reads_worker_js_from_override_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("worker.js"), "// worker override\n").expect("write");
    let profile = profile_direct();
    let fp = &profile.fingerprint;
    let prefix = worker_fingerprint_prefix(fp).expect("prefix");
    let built = build_worker_inject_source_from(fp, Some(dir.path()))
        .await
        .expect("build");
    assert_eq!(built, format!("{prefix}// worker override\n"));
}

#[tokio::test]
async fn worker_source_missing_file_aborts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile = profile_direct();
    let error = build_worker_inject_source_from(&profile.fingerprint, Some(dir.path()))
        .await
        .expect_err("a missing worker.js must abort");
    assert!(
        error
            .to_string()
            .contains("missing worker snippet: worker.js"),
        "unexpected: {error}"
    );
}

#[tokio::test]
async fn page_source_never_carries_the_worker_snippet() {
    let profile = profile_direct();
    let fp = &profile.fingerprint;
    let page = build_inject_source_from(fp, None)
        .await
        .expect("build page");
    // The page bundle is window-only: neither the worker-only marker nor the
    // worker global may appear in it.
    assert!(
        !page.contains(WORKER_ONLY_MARKER),
        "page bundle leaked the worker snippet"
    );
    assert!(
        !page.contains("self.__MIRAGE_FP__"),
        "page bundle leaked the worker config global"
    );
    assert!(page.contains("window.__MIRAGE_FP__"));
}
