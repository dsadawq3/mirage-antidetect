//! mirage-e2e: a repeatable, offline end-to-end fingerprint checker.
//!
//! The adversarial runtime smoke in `mirage-launcher/examples/smoke.rs` proves
//! the spoof layer once, by hand, against a throwaway localhost document. This
//! crate turns that protocol into a self-contained artifact:
//!
//! 1. an [`axum`] server on `127.0.0.1:0` serves [`checker_html`] (embedded
//!    with `include_str!`, so the binary carries its own test surface and never
//!    touches a third-party site);
//! 2. [`generate`](mirage_core::generate) builds a deterministic legend from
//!    `(seed, os)`, wrapped in a [`Profile`];
//! 3. [`launch`](mirage_launcher::launch) spawns a masked Chrome on a free
//!    debug port and injects the spoof layer;
//! 4. the page is navigated to the local server; the page runs every probe
//!    (legend values, adversarial invariants, report-only extras) and writes a
//!    single JSON string to `window.__MIRAGE_CHECK__`, which this crate polls
//!    over `Runtime.evaluate` and parses into [`E2eReport`].
//!
//! Everything is local: no external reads, no external writes, no headless
//! mode. The report is `passed` count plus the list of [`FailedCheck`]s.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use mirage_core::{generate, Os, Profile};
use mirage_launcher::{launch, LaunchConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The checker page, embedded so the artifact is self-contained and offline.
/// See `assets/checker.html` for the probe inventory.
pub fn checker_html() -> &'static str {
    include_str!("../assets/checker.html")
}

/// One failing probe: its stable `name` and a human-readable `detail`
/// (the probe's `detail` value re-serialized to compact JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedCheck {
    pub name: String,
    pub detail: String,
}

/// Aggregate result of one [`run_e2e`] run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eReport {
    /// Number of probes that passed.
    pub passed: usize,
    /// Probes that failed, in page-report order.
    pub failed: Vec<FailedCheck>,
}

impl E2eReport {
    /// Total number of probes executed (passed + failed).
    pub fn total(&self) -> usize {
        self.passed + self.failed.len()
    }

    /// `true` when every probe passed.
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Wire shape of the JSON string the page writes to `window.__MIRAGE_CHECK__`.
#[derive(Debug, Clone, Deserialize)]
struct CheckedReport {
    /// Raw observations (including the report-only probes). Unused by the
    /// verdict but kept on the wire for diagnostics.
    #[serde(default)]
    fields: Value,
    verdict: CheckedVerdict,
}

#[derive(Debug, Clone, Deserialize)]
struct CheckedVerdict {
    checks: Vec<CheckedCheck>,
}

#[derive(Debug, Clone, Deserialize)]
struct CheckedCheck {
    name: String,
    pass: bool,
    #[serde(default)]
    detail: Value,
}

/// Parse the page's `window.__MIRAGE_CHECK__` JSON into an [`E2eReport`].
///
/// Exposed (and unit-tested on synthetic input) so the parser can be verified
/// without a browser.
pub fn parse_report(raw: &str) -> Result<E2eReport> {
    let checked: CheckedReport =
        serde_json::from_str(raw).context("failed to deserialize __MIRAGE_CHECK__ JSON")?;
    let mut passed = 0usize;
    let mut failed = Vec::new();
    for check in checked.verdict.checks {
        if check.pass {
            passed += 1;
        } else {
            failed.push(FailedCheck {
                name: check.name,
                // The detail is free-form per probe: render it as compact JSON
                // so arrays/objects survive as readable text in the failure
                // list.
                detail: render_detail(&check.detail),
            });
        }
    }
    // `fields` is intentionally only validated (parsed above), not surfaced:
    // the verdict is the gate, `fields` is diagnostic payload.
    let _ = checked.fields;
    Ok(E2eReport { passed, failed })
}

/// Render a probe `detail` value as text: strings verbatim, everything else as
/// compact JSON. `null` becomes an empty string.
fn render_detail(detail: &Value) -> String {
    match detail {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Start the localhost checker server on an ephemeral port.
///
/// Returns the bound port and the server task handle. The task is aborted by
/// the caller once the run is over; a serve error is swallowed because the only
/// consumer is a one-shot local run.
async fn start_server() -> Result<(u16, JoinHandle<()>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("failed to bind the checker listener on 127.0.0.1:0")?;
    let port = listener
        .local_addr()
        .context("failed to read the checker listener's local address")?
        .port();

    let app = Router::new()
        .route("/", get(checker_handler))
        // Chrome requests /favicon.ico on navigation; answer 204 so the run
        // leaves no 404 noise behind.
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }));

    let handle = tokio::spawn(async move {
        // A bind/serve failure here only matters to a live run; there is no
        // caller to report it to inside the task.
        let _ = axum::serve(listener, app).await;
    });

    Ok((port, handle))
}

/// Serve the embedded checker page as `text/html; charset=utf-8`.
async fn checker_handler() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        checker_html(),
    )
}

/// The `Runtime.evaluate` probe: report the document readiness and the checker
/// payload once the page has written it.
const POLL_EXPRESSION: &str = "({ ready: document.readyState, \
     check: (typeof window.__MIRAGE_CHECK__ === 'string') ? window.__MIRAGE_CHECK__ : null })";

/// Budget for the page to finish loading and run every probe.
const CHECK_DEADLINE: Duration = Duration::from_secs(10);
/// Poll interval while waiting for the page.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Run one full end-to-end check: serve the page, spawn a masked Chrome from a
/// `(seed, os)` legend, navigate to the page, harvest and parse the report, and
/// tear the browser down.
///
/// The browser is stopped on every path, including errors.
pub async fn run_e2e(seed: u64, os: Os) -> Result<E2eReport> {
    // A fresh profile dir per run so every launch starts from the same state.
    // The seed and pid keep concurrent runs from colliding.
    let profile_dir = unique_profile_dir(seed);

    let (port, server) = start_server().await?;

    // Deterministic legend: the same (seed, os) reproduces the same
    // fingerprint, which is what makes this a repeatable artifact.
    let fingerprint = generate(seed, Some(os))?;
    let profile = Profile::new("mirage-e2e", fingerprint, None);

    let cfg = LaunchConfig {
        profile,
        // None: the launcher resolves Chrome itself (find_chrome).
        chrome_path: None,
        profile_dir,
        // 0 = the launcher picks a free debug port (CONTRACT.md).
        debug_port: 0,
    };

    let mut browser = launch(&cfg).await?;

    // Harvest inside an inner scope so `stop()` runs on both success and error.
    let harvested = harvest(&mut browser, port).await;

    let stop_result = browser.stop().await;
    server.abort();

    // The page report is the primary result; a failed teardown must not mask a
    // failing harvest, but is not swallowed either.
    match harvested {
        Ok(report) => {
            stop_result.context("failed to stop the browser after a successful run")?;
            Ok(report)
        }
        Err(error) => Err(error),
    }
}

/// Navigate the page session to the local checker and poll for its payload.
async fn harvest(browser: &mut mirage_launcher::LaunchedBrowser, port: u16) -> Result<E2eReport> {
    let url = format!("http://127.0.0.1:{port}/");
    browser
        .send_command("Page.navigate", json!({ "url": url }))
        .await
        .context("Page.navigate to the checker page failed")?;

    let deadline = Instant::now() + CHECK_DEADLINE;
    let raw = loop {
        // The CDP client skips events, so readiness is polled: readyState must
        // be `complete` AND the page must have written __MIRAGE_CHECK__.
        let result = browser
            .send_command(
                "Runtime.evaluate",
                json!({ "expression": POLL_EXPRESSION, "returnByValue": true }),
            )
            .await
            .context("Runtime.evaluate while polling __MIRAGE_CHECK__ failed")?;

        let value = &result["result"]["value"];
        let ready = value["ready"].as_str().unwrap_or("");
        if let Some(check) = value["check"].as_str() {
            if ready == "complete" {
                break check.to_owned();
            }
        }

        if Instant::now() > deadline {
            return Err(anyhow!(
                "__MIRAGE_CHECK__ did not appear within {CHECK_DEADLINE:?} \
                 (readyState={ready:?})"
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    parse_report(&raw)
}

/// A unique, cleaned profile directory for a run.
fn unique_profile_dir(seed: u64) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("mirage-e2e")
        .join(format!("{}-{seed}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probes that must be present in the embedded page. These are the load
    /// bearing pieces of the checker; a regressed page (probe deletion,
    /// renamed globals, an accidental external dependency) trips here without
    /// launching a browser.
    const REQUIRED_PROBES: [&str; 13] = [
        "__MIRAGE_CHECK__",
        "__MIRAGE_FP__",
        "userAgentData",
        "getHighEntropyValues",
        "WEBGL_debug_renderer_info",
        "matchMedia",
        "OfflineAudioContext",
        "navigator.plugins",
        "devicePixelRatio",
        "new Worker",
        "screen.availWidth",
        "Intl.DateTimeFormat",
        "enumerateDevices",
    ];

    #[test]
    fn checker_page_embeds_and_parses() {
        let html = checker_html();

        // The page is served and must look like a document.
        assert!(
            html.contains("<!DOCTYPE html>"),
            "checker.html is not a document"
        );
        assert!(
            html.contains("text/html") || html.contains("<html"),
            "checker.html is missing its root element"
        );

        // Every load-bearing probe is wired in.
        for probe in REQUIRED_PROBES {
            assert!(html.contains(probe), "checker.html lost the probe: {probe}");
        }

        // No external requests: no remote script/style/font/image references.
        // (A localhost run must never reach out.)
        for bad in ["http://", "https://", "<script src", "<link ", "@import"] {
            assert!(
                !html.contains(bad),
                "checker.html must be self-contained, found: {bad}"
            );
        }

        // The page assigns the exact JSON envelope the harness parses.
        assert!(
            html.contains("window.__MIRAGE_CHECK__ = JSON.stringify("),
            "checker.html must assign window.__MIRAGE_CHECK__"
        );

        // Unit-test the parser on a synthetic envelope: one pass, two failures,
        // and a non-string detail to exercise `render_detail`.
        let synthetic = r#"{
            "fields": { "ua": "x", "fonts": { "detected": ["Arial"] } },
            "verdict": { "checks": [
                { "name": "navigator.platform", "pass": true, "detail": { "got": "Win32" } },
                { "name": "webgl.renderer", "pass": false, "detail": { "got": "a", "want": "b" } },
                { "name": "canvas.blankIsZero", "pass": false, "detail": "nonZero=3" }
            ] }
        }"#;
        let report = parse_report(synthetic).expect("synthetic envelope must parse");
        assert_eq!(report.passed, 1);
        assert_eq!(report.total(), 3);
        assert!(!report.is_clean());
        assert_eq!(report.failed.len(), 2);
        assert_eq!(report.failed[0].name, "webgl.renderer");
        assert_eq!(report.failed[0].detail, r#"{"got":"a","want":"b"}"#);
        assert_eq!(report.failed[1].name, "canvas.blankIsZero");
        assert_eq!(report.failed[1].detail, "nonZero=3");

        // A `null` detail renders empty, and a fully-passing report is clean.
        let all_pass = r#"{ "verdict": { "checks": [ { "name": "a", "pass": true } ] } }"#;
        let report = parse_report(all_pass).expect("passing envelope must parse");
        assert_eq!(report.passed, 1);
        assert!(report.is_clean());
        assert_eq!(report.total(), 1);

        // A malformed payload is an error, not a silent empty report.
        assert!(parse_report("not json").is_err());
        assert!(parse_report(r#"{"verdict":{}}"#).is_err());
        assert!(parse_report(r#"{"fields":{}}"#).is_err());
    }

    #[tokio::test]
    #[ignore = "launches a real Chrome window; set MIRAGE_E2E=1 and run with --ignored"]
    async fn e2e_windows_legend() {
        // The test is #[ignore]d so the default `cargo test` never opens a
        // browser; even under `--ignored` it only runs when explicitly armed.
        // Without the env var it explains itself and returns (no silent skip).
        if std::env::var("MIRAGE_E2E").ok().as_deref() != Some("1") {
            eprintln!(
                "MIRAGE_E2E != 1: skipping the live browser e2e. \
                 Run: MIRAGE_E2E=1 cargo test -p mirage-e2e -- --ignored --nocapture"
            );
            return;
        }

        // The seed used by the launcher smoke; fixed so a run is reproducible.
        const SEED: u64 = 20_261_103;
        let report = run_e2e(SEED, Os::Windows)
            .await
            .expect("run_e2e must complete (Chrome present, page reachable)");

        println!(
            "e2e: {} checks, {} failed",
            report.total(),
            report.failed.len()
        );
        for failed in &report.failed {
            println!("  [FAIL] {}: {}", failed.name, failed.detail);
        }

        assert!(
            report.is_clean(),
            "{} of {} probes failed: {:?}",
            report.failed.len(),
            report.total(),
            report.failed
        );
    }
}
