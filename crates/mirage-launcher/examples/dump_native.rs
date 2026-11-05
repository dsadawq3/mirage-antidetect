//! Native reference dump: what a REAL, unspoofed Chrome reports for the
//! surfaces Mirage spoofs. Used to calibrate per-major constants (critic
//! finding B1: GREASE brands) against live truth instead of assumptions.
//!
//! Navigates to the DevTools HTTP endpoint on 127.0.0.1: a localhost origin
//! is a secure context, so `[SecureContext]` attributes like
//! `navigator.userAgentData` are actually exposed (on `data:` URLs they are
//! not — discovered the hard way).
//!
//! ```text
//! cargo run -p mirage-launcher --example dump_native
//! ```

use mirage_launcher::cdp::get_page_target;
use mirage_launcher::{find_chrome, CdpClient};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let chrome = find_chrome().ok_or_else(|| anyhow::anyhow!("Chrome not found"))?;
    let dir = std::env::temp_dir().join("mirage-native-dump");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    // Deliberately NO Mirage flags and NO spoofing CDP commands: we want the
    // native truth of this exact Chrome build.
    let mut child = std::process::Command::new(&chrome)
        .args([
            format!("--user-data-dir={}", dir.display()),
            "--remote-debugging-port=9345".to_string(),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
        ])
        .spawn()?;

    // Wait for DevTools to come up, bounded to 15s.
    let started = Instant::now();
    let page = loop {
        if let Ok(p) = get_page_target(9345).await {
            break p;
        }
        if child.try_wait()?.is_some() {
            anyhow::bail!("Chrome exited before DevTools came up");
        }
        if started.elapsed() > Duration::from_secs(15) {
            child.kill()?;
            let _ = child.wait();
            anyhow::bail!("DevTools did not come up within 15s");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let cdp = CdpClient::attach(&page.ws_url).await?;
    cdp.send("Page.enable", json!({})).await?;
    // Localhost origin => secure context => userAgentData is exposed, and no
    // external network is needed: the DevTools endpoint serves the page.
    cdp.send(
        "Page.navigate",
        json!({ "url": "http://127.0.0.1:9345/json/version" }),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(700)).await;

    // Defensive probe: works with or without userAgentData, records the
    // secure-context flag so the data:-URL behaviour stays measurable.
    let probe = r#"(() => {
        const uad = navigator.userAgentData;
        const base = {
            ua: navigator.userAgent,
            isSecureContext: window.isSecureContext,
            href: String(location.href).slice(0, 60),
            brands: uad ? uad.brands : null,
            mobile: uad ? uad.mobile : null,
            platform: uad ? uad.platform : null,
            ownNamesUAD: uad ? Object.getOwnPropertyNames(uad).join(',') : null,
            stringifyUAD: uad ? JSON.parse(JSON.stringify(uad)) : null,
            plugins: Array.from(navigator.plugins, p => ({ name: p.name, length: p.length })),
            mimeTypesLength: navigator.mimeTypes.length,
            ownNamesPlugins: Object.getOwnPropertyNames(navigator.plugins).join(',')
        };
        if (!uad) { return Promise.resolve(JSON.stringify(base, null, 1)); }
        return uad.getHighEntropyValues([
            "architecture","bitness","model","platformVersion","uaFullVersion",
            "fullVersionList","wow64","formFactors"
        ]).then(v => JSON.stringify(Object.assign(base, { highEntropy: v }), null, 1));
    })()"#;

    let res = cdp
        .send(
            "Runtime.evaluate",
            json!({ "expression": probe, "returnByValue": true, "awaitPromise": true }),
        )
        .await?;

    match res["result"]["value"].as_str() {
        Some(dump) => println!("{dump}"),
        None => println!("raw response:\n{}", serde_json::to_string_pretty(&res)?),
    }

    child.kill()?;
    let _ = child.wait();
    Ok(())
}
