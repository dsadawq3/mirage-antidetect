//! Live B3 regression: a page target opened *after* launch (a new tab, i.e.
//! Ctrl+T / `window.open`) must receive the full spoof layer through the
//! browser-level flat auto-attach, not just the first target.
//!
//! This spawns a real, GUI Chrome, so it is `#[ignore]`d by default. Run it
//! explicitly with a display available:
//!
//! ```text
//! cargo test -p mirage-launcher --test auto_attach_live -- --ignored --nocapture
//! ```

mod common;

use common::{launch_config, profile_direct};
use mirage_core::Fingerprint;
use mirage_launcher::{find_chrome, launch, CdpClient, LaunchConfig};
use serde_json::json;
use std::time::{Duration, Instant};

/// The data: URL the new tab loads. Its inline script writes the *page-visible*
/// values into the title, so reading the title proves what the tab saw at run
/// time — before any injection point the launcher could add later.
const PROBE_URL: &str =
    "data:text/html,<title>pending</title><script>document.title=navigator.userAgent+'|'+navigator.webdriver+'|'+navigator.platform</script>";

fn cfg_with_chrome(profile: mirage_core::Profile, dir: std::path::PathBuf) -> Option<LaunchConfig> {
    let chrome = find_chrome()?;
    let mut cfg = launch_config(profile, dir, 0);
    cfg.chrome_path = Some(chrome);
    Some(cfg)
}

#[tokio::test]
#[ignore = "spawns a real Chrome window; run manually with --ignored"]
async fn new_tab_is_spoofed() {
    let profile = profile_direct();
    let expected = profile.fingerprint.clone();
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(cfg) = cfg_with_chrome(profile, dir.path().join("profile")) else {
        eprintln!("skipping: Chrome not installed");
        return;
    };

    let browser = launch(&cfg).await.expect("launch");
    let port = browser.debug_port;

    // A second, independent browser-level connection used only to create and
    // probe the new tab; the spoofing itself is done by the launcher's own
    // auto-attach task on its own connection.
    let control = CdpClient::attach(&browser.ws_url)
        .await
        .expect("control attach");
    let created = control
        .send("Target.createTarget", json!({ "url": PROBE_URL }))
        .await
        .expect("createTarget");
    let target_id = created["targetId"].as_str().expect("targetId").to_owned();

    // Wait for the auto-attach task to spoof + resume the new tab, then read
    // what the page actually saw.
    let page_ws = format!("ws://127.0.0.1:{port}/devtools/page/{target_id}");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    let spoofed = loop {
        if let Ok(page) = CdpClient::attach(&page_ws).await {
            if let Ok(result) = page
                .send(
                    "Runtime.evaluate",
                    json!({ "expression": "document.title", "returnByValue": true }),
                )
                .await
            {
                last = result["result"]["value"].as_str().unwrap_or("").to_owned();
                if last != "pending" && !last.is_empty() {
                    break title_matches(&last, &expected);
                }
            }
        }
        if Instant::now() > deadline {
            panic!("new tab never produced a title (last={last:?})");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let mut browser = browser;
    browser.stop().await.expect("stop");

    assert!(
        spoofed,
        "new tab was NOT spoofed: title={last:?}, expected ua={:?} platform={:?}",
        expected.ua, expected.platform
    );
}

/// The title is `<ua>|<webdriver>|<platform>`; the spoof must show the legend's
/// UA/platform and `webdriver === false`.
fn title_matches(title: &str, fp: &Fingerprint) -> bool {
    let parts: Vec<&str> = title.split('|').collect();
    parts.len() == 3 && parts[0] == fp.ua && parts[1] == "false" && parts[2] == fp.platform
}
