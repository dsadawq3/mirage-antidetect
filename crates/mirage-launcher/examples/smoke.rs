//! Adversarial runtime smoke: spawn a masked Chrome from a seed-generated
//! profile, load a fresh localhost document and verify both the legend values
//! AND the anti-detection invariants from the wave-1 review (worker scope,
//! toString masking, descriptor hygiene, blank-canvas rule, media-query/DPR
//! agreement, window geometry, GREASE brands against the live calibration).
//!
//! Run with:
//!
//! ```text
//! cargo run -p mirage-launcher --example smoke
//! ```

use mirage_core::{generate, Os, Profile};
use mirage_launcher::{find_chrome, launch, LaunchConfig};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Compare two JSON values, treating numbers as f64: the page reports DPR as
/// `1` while the legend serializes it as `1.0` — same value, different JSON
/// encoding, and serde_json `Value` equality is encoding-sensitive.
fn veq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < f64::EPSILON,
        _ => a == b,
    }
}

/// One PASS/FAIL line comparing the page-side value against the expectation.
fn check(name: &str, page: &Value, expected: &Value) -> bool {
    let ok = veq(page, expected);
    println!(
        "  [{}] {}\n         page    = {}\n         expected= {}",
        if ok { "PASS" } else { "FAIL" },
        name,
        page,
        expected
    );
    ok
}

fn check_bool(name: &str, ok: bool, detail: &Value) -> bool {
    println!(
        "  [{}] {}\n         detail  = {}",
        if ok { "PASS" } else { "FAIL" },
        name,
        detail
    );
    ok
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Fresh profile dir so every run starts from the same state.
    let profile_dir = std::env::temp_dir().join("mirage-smoke-profile");
    let _ = std::fs::remove_dir_all(&profile_dir);

    let chrome =
        find_chrome().ok_or_else(|| anyhow::anyhow!("Chrome not found in standard locations"))?;

    // Deterministic legend: the same seed reproduces the same fingerprint on
    // any machine (CONTRACT.md, reproducibility principle).
    const SEED: u64 = 20_261_103;
    let fingerprint = generate(SEED, Some(Os::Windows))?;
    let legend = serde_json::to_value(&fingerprint)?;
    let profile = Profile::new("smoke-test", fingerprint, None);

    println!("== legend (seed {SEED}) ==");
    println!("{}", serde_json::to_string_pretty(&legend)?);

    let cfg = LaunchConfig {
        profile,
        chrome_path: Some(chrome),
        profile_dir,
        // 0 = launcher picks a free port itself (CONTRACT.md).
        debug_port: 0,
    };

    let mut browser = launch(&cfg).await?;
    println!(
        "\n== browser ==\ntarget {}\nws {}\nport {}",
        browser.target_id, browser.ws_url, browser.debug_port
    );

    // The injection runs on new documents; the initial tab predates it. A
    // localhost origin is a secure context, so [SecureContext] attributes
    // like userAgentData exist (on data: URLs they do not — review M5).
    browser
        .send_command(
            "Page.navigate",
            json!({ "url": format!("http://127.0.0.1:{}/json/version", browser.debug_port) }),
        )
        .await?;

    // The CDP client skips events, so poll readyState instead of waiting for
    // Page.loadEventFired; bounded to 10s, then probe anyway.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let rs = browser
            .send_command(
                "Runtime.evaluate",
                json!({ "expression": "document.readyState", "returnByValue": true }),
            )
            .await?;
        if rs["result"]["value"].as_str() == Some("complete") {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    // Legend values plus every adversarial probe class from the review.
    let probe = r#"(async () => {
        const webgl = (() => {
            try {
                const c = document.createElement('canvas');
                const g = c.getContext('webgl');
                const d = g.getExtension('WEBGL_debug_renderer_info');
                return {
                    vendor: g.getParameter(d.UNMASKED_VENDOR_WEBGL),
                    renderer: g.getParameter(d.UNMASKED_RENDERER_WEBGL)
                };
            } catch (e) { return { vendor: 'ERR:' + e.message, renderer: 'ERR:' + e.message }; }
        })();
        const blank = (() => {
            try {
                const c = document.createElement('canvas'); c.width = 64; c.height = 64;
                const g = c.getContext('2d');
                const d = g.getImageData(0, 0, 64, 64).data;
                let nz = 0; for (let i = 0; i < d.length; i++) if (d[i] !== 0) nz++;
                return nz;
            } catch (e) { return -1; }
        })();
        const stable = (() => {
            try {
                const c = document.createElement('canvas'); c.width = 32; c.height = 32;
                const g = c.getContext('2d');
                g.fillStyle = '#c0ffee'; g.fillRect(0, 0, 32, 32);
                g.fillStyle = '#123456'; g.font = '16px Arial'; g.fillText('mirage', 2, 20);
                const a = Array.from(g.getImageData(0, 0, 32, 32).data);
                const b = Array.from(g.getImageData(0, 0, 32, 32).data);
                return a.length === b.length && a.every((v, i) => v === b[i]);
            } catch (e) { return false; }
        })();
        const workerInfo = await new Promise((resolve) => {
            try {
                const src = 'onmessage = function () { postMessage([navigator.hardwareConcurrency, navigator.deviceMemory]); };';
                const blob = new Blob([src], { type: 'application/javascript' });
                const url = URL.createObjectURL(blob);
                const w = new Worker(url);
                const t = setTimeout(function () { resolve({ timeout: true }); }, 3000);
                w.onmessage = function (e) { clearTimeout(t); resolve({ hw: e.data[0], mem: e.data[1] }); try { w.terminate(); } catch (err) {} };
                w.onerror = function (e) { clearTimeout(t); resolve({ err: String(e.message) }); };
                w.postMessage(1);
            } catch (e) { resolve({ throw: String(e && e.message) }); }
        });
        const dpr = window.devicePixelRatio;
        const mq = (v) => window.matchMedia('(min-resolution: ' + v + 'dppx)').matches;
        const uad = navigator.userAgentData;
        const toStringProbe = (() => {
            try {
                const s = Object.getOwnPropertyDescriptor(Navigator.prototype, 'platform').get.toString();
                return { native: s.indexOf('[native code]') !== -1, leaks: s.indexOf('__MIRAGE') !== -1 || s.indexOf('fp.') !== -1, raw: s.slice(0, 80) };
            } catch (e) { return { err: String(e && e.message) }; }
        })();
        const hev = uad ? await uad.getHighEntropyValues(['architecture']) : null;
        return {
            ua: navigator.userAgent,
            platform: navigator.platform,
            languages: navigator.languages,
            hardwareConcurrency: navigator.hardwareConcurrency,
            deviceMemory: navigator.deviceMemory,
            webdriver: navigator.webdriver,
            screen: [screen.width, screen.height, screen.availWidth, screen.availHeight],
            outer: [window.outerWidth, window.outerHeight],
            devicePixelRatio: dpr,
            mqSelf: mq(dpr),
            mqAbove: mq(dpr + 0.25),
            timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
            tzOffsetMinutes: new Date().getTimezoneOffset(),
            webgl: webgl,
            pluginsLength: navigator.plugins.length,
            pluginLengths: Array.from(navigator.plugins, (p) => p.length),
            mimeTypesLength: navigator.mimeTypes.length,
            ownNamesPlugins: Object.getOwnPropertyNames(navigator.plugins).join(','),
            oscpuIn: ('oscpu' in navigator),
            brands: uad ? uad.brands : null,
            uadPlatform: uad ? uad.platform : null,
            ownNamesUAD: uad ? Object.getOwnPropertyNames(uad).join(',') : null,
            stringifyUAD: uad ? JSON.parse(JSON.stringify(uad)) : null,
            hevKeys: hev ? Object.keys(hev) : null,
            toStringProbe: toStringProbe,
            blankCanvasNonZero: blank,
            canvasStable: stable,
            worker: workerInfo,
            modulesDone: window.__MIRAGE_DONE__ ? Object.keys(window.__MIRAGE_DONE__) : null
        };
    })()"#;

    // send_command returns the CDP `result` object; for Runtime.evaluate the
    // value lives under ["result"]["value"].
    let result = browser
        .send_command(
            "Runtime.evaluate",
            json!({ "expression": probe, "returnByValue": true, "awaitPromise": true }),
        )
        .await?;

    let page = &result["result"]["value"];
    println!("\n== page sees ==");
    println!("{}", serde_json::to_string_pretty(page)?);

    println!("\n== verdict ==");

    // --- legend values -----------------------------------------------------
    let mut ok = true;
    ok &= check("userAgent", &page["ua"], &legend["ua"]);
    ok &= check("platform", &page["platform"], &legend["platform"]);
    ok &= check("languages", &page["languages"], &legend["languages"]);
    ok &= check("timezone", &page["timezone"], &legend["timezone"]);
    ok &= check(
        "hardwareConcurrency",
        &page["hardwareConcurrency"],
        &legend["hardwareConcurrency"],
    );
    ok &= check(
        "deviceMemory",
        &page["deviceMemory"],
        &legend["deviceMemory"],
    );
    ok &= check(
        "webgl.vendor",
        &page["webgl"]["vendor"],
        &legend["webgl"]["vendor"],
    );
    ok &= check(
        "webgl.renderer",
        &page["webgl"]["renderer"],
        &legend["webgl"]["renderer"],
    );
    ok &= check(
        "screen",
        &page["screen"],
        &json!([
            legend["screen"]["width"],
            legend["screen"]["height"],
            legend["screen"]["availWidth"],
            legend["screen"]["availHeight"]
        ]),
    );
    ok &= check(
        "devicePixelRatio",
        &page["devicePixelRatio"],
        &legend["screen"]["dpr"],
    );
    ok &= check(
        "clientHints.platform",
        &page["uadPlatform"],
        &legend["clientHints"]["platform"],
    );
    ok &= check(
        "brands (GREASE calibration)",
        &page["brands"],
        &legend["clientHints"]["brands"],
    );
    ok &= check("webdriver", &page["webdriver"], &json!(false));

    // --- adversarial invariants (review probes as gates) -------------------
    ok &= check("plugins.length", &page["pluginsLength"], &json!(5));
    ok &= check(
        "plugin mime counts (all 2)",
        &page["pluginLengths"],
        &json!([2, 2, 2, 2, 2]),
    );
    ok &= check("mimeTypes.length", &page["mimeTypesLength"], &json!(2));
    let own_names = page["ownNamesPlugins"].as_str().unwrap_or("");
    ok &= check_bool(
        "own props of plugins (no methods)",
        !own_names.contains("item")
            && !own_names.contains("namedItem")
            && !own_names.contains("refresh"),
        &json!(own_names),
    );
    ok &= check_bool(
        "no oscpu property",
        !page["oscpuIn"].as_bool().unwrap_or(true),
        &page["oscpuIn"],
    );
    ok &= check_bool(
        "userAgentData own props empty",
        page["ownNamesUAD"].as_str().unwrap_or("x").is_empty(),
        &json!(page["ownNamesUAD"]),
    );
    ok &= check_bool(
        "JSON.stringify(userAgentData) works",
        page["stringifyUAD"].is_object(),
        &page["stringifyUAD"],
    );
    ok &= check(
        "stringify brands roundtrip",
        &page["stringifyUAD"]["brands"],
        &legend["clientHints"]["brands"],
    );
    let hev_keys: Vec<&str> = page["hevKeys"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    ok &= check_bool(
        "getHighEntropyValues keeps base fields",
        ["architecture", "brands", "mobile", "platform"]
            .iter()
            .all(|k| hev_keys.contains(k)),
        &json!(hev_keys),
    );
    ok &= check_bool(
        "toString of spoofed getter looks native",
        page["toStringProbe"]["native"].as_bool().unwrap_or(false),
        &json!(page["toStringProbe"]),
    );
    ok &= check_bool(
        "toString leaks no Mirage internals",
        !page["toStringProbe"]["leaks"].as_bool().unwrap_or(true),
        &json!(page["toStringProbe"]["raw"]),
    );
    ok &= check(
        "blank canvas reads back zeros",
        &page["blankCanvasNonZero"],
        &json!(0),
    );
    ok &= check_bool(
        "canvas noise is stable across reads",
        page["canvasStable"].as_bool().unwrap_or(false),
        &page["canvasStable"],
    );
    ok &= check_bool(
        "media queries agree with DPR",
        page["mqSelf"].as_bool().unwrap_or(false) && !page["mqAbove"].as_bool().unwrap_or(true),
        &json!({ "mqSelf": page["mqSelf"], "mqAbove": page["mqAbove"], "dpr": page["devicePixelRatio"] }),
    );
    ok &= check(
        "outer geometry matches avail (maximized)",
        &page["outer"],
        &json!([
            legend["screen"]["availWidth"],
            legend["screen"]["availHeight"]
        ]),
    );
    ok &= check(
        "worker sees the legend (hw)",
        &page["worker"]["hw"],
        &legend["hardwareConcurrency"],
    );
    ok &= check(
        "worker sees the legend (mem)",
        &page["worker"]["mem"],
        &legend["deviceMemory"],
    );

    let modules = page["modulesDone"].as_array().map(Vec::len).unwrap_or(0);
    println!(
        "  [{}] inject modules loaded: {} (expected 10)",
        if modules == 10 { "PASS" } else { "FAIL" },
        modules
    );
    ok &= modules == 10;

    browser.stop().await?;

    if ok {
        println!("\nSMOKE OK");
        Ok(())
    } else {
        println!("\nSMOKE FAILED");
        std::process::exit(1);
    }
}
