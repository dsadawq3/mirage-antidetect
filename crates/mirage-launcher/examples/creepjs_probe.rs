//! Manual live probe: launch a masked profile, open creepjs and print its
//! report, so a worker/screen regression can be seen with real numbers.
//!
//! Not part of the automated suite (needs network + a display), like
//! `examples/smoke.rs`. Run with:
//!
//! ```text
//! cargo run -p mirage-launcher --example creepjs_probe [SEED]
//! ```
//!
//! The report is printed between the `=====CREEPJS_BEGIN=====` /
//! `=====CREEPJS_END=====` markers as the page's own `document.body.innerText`,
//! so the caller can grep the `Worker` / `Screen` sections out of it.

use mirage_core::{generate, Os, Profile};
use mirage_launcher::{launch, LaunchConfig};
use serde_json::json;
use std::time::{Duration, Instant};

const CREEPJS: &str = "https://abrahamjuliot.github.io/creepjs/";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_261_103);

    let fingerprint = generate(seed, Some(Os::Windows))?;
    let legend = serde_json::to_value(&fingerprint)?;
    println!("== legend (seed {seed}) ==");
    println!("{}", serde_json::to_string_pretty(&legend)?);

    let profile_dir = std::env::temp_dir().join(format!("mirage-creepjs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile_dir);

    let profile = Profile::new("creepjs-probe", fingerprint, None);
    let cfg = LaunchConfig {
        profile,
        chrome_path: None,
        profile_dir,
        debug_port: 0,
    };

    let mut browser = launch(&cfg).await?;
    println!("MIRAGE_DEBUG_PORT={}", browser.debug_port);

    browser
        .send_command("Page.navigate", json!({ "url": CREEPJS }))
        .await?;

    // creepjs runs its probes asynchronously; poll for the report body until it
    // mentions the two sections we care about, or the deadline passes.
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut last = String::new();
    loop {
        if let Ok(result) = browser
            .send_command(
                "Runtime.evaluate",
                json!({
                    "expression": "document.body ? document.body.innerText : ''",
                    "returnByValue": true,
                }),
            )
            .await
        {
            if let Some(text) = result["result"]["value"].as_str() {
                last = text.to_owned();
                if last.contains("cores:") && last.contains("Screen") && last.contains("Worker") {
                    break;
                }
            }
        }
        if Instant::now() > deadline {
            eprintln!("creepjs did not report every section within the deadline");
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    println!("=====CREEPJS_BEGIN=====");
    println!("{last}");
    println!("=====CREEPJS_END=====");

    browser.stop().await?;
    Ok(())
}
