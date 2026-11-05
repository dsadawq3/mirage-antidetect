//! Golden tests for the CDP command JSON: exact serialized strings for the
//! contract's command set, built from a fixed CONTRACT.md example profile.
//!
//! Two serialization orders occur, both deterministic:
//! - params built via `serde_json::to_value` (Value with the default BTreeMap
//!   backing) serialize with alphabetically sorted object keys;
//! - the fingerprint prefix serializes the mirage-core struct directly, so its
//!   JSON keeps the struct field order — which CONTRACT.md mandates to be the
//!   schema order ("буква-в-букву: имена полей, типы, пути, порядок").

mod common;

use common::profile_direct;
use mirage_launcher::cdp::{wire_message, wire_message_with_session};
use mirage_launcher::launch::{
    assemble_source, build_add_script_command, build_device_metrics_override,
    build_locale_override, build_timezone_override, build_user_agent_override, fingerprint_prefix,
};

/// The CONTRACT.md "Схема Fingerprint" example as serialized JSON.
///
/// Mirrors mirage-core's field declaration order (= contract schema order) and
/// serde_json's f64 rendering (`1.0`, not `1`). If mirage-core reorders its
/// fields this golden fails loudly — which is exactly its job.
const FP_JSON: &str = r#"{"os":"windows","osVersion":"11","platform":"Win32","ua":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36","chromeVersion":"156.0.8078.80","languages":["en-US","en"],"timezone":"Europe/London","geo":{"lat":51.5074,"lon":-0.1278},"screen":{"width":1920,"height":1080,"availWidth":1920,"availHeight":1040,"dpr":1.0,"colorDepth":24},"hardwareConcurrency":8,"deviceMemory":8,"webgl":{"vendor":"Google Inc. (NVIDIA)","renderer":"ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER Direct3D11 vs_5_0 ps_5_0, D3D11)","maxTextureSize":16384,"maxViewportDims":[32767,32767],"maxRenderbufferSize":16384},"canvasSeed":1234567890,"audioSeed":987654321,"fonts":["Arial","Calibri"],"webRTC":"real","clientHints":{"brands":[{"brand":"Not:A-Brand","version":"8"},{"brand":"Chromium","version":"156"},{"brand":"Google Chrome","version":"156"}],"mobile":false,"platform":"Windows","platformVersion":"15.0.0","architecture":"x86","bitness":"64","uaFullVersion":"156.0.8078.80"}}"#;

/// Exact Network.setUserAgentOverride params (alphabetical key order, see
/// module docs) for the contract example profile.
///
/// `fullVersionList` follows the live-Chrome-156 calibration: the GREASE
/// brand (`Not:A-Brand`) gets the grease-cycle full version `8.0.0.0`, the
/// real brands get `uaFullVersion`.
const EXPECTED_UA_PARAMS: &str = r#"{"acceptLanguage":"en-US,en","platform":"Win32","userAgent":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36","userAgentMetadata":{"architecture":"x86","bitness":"64","brands":[{"brand":"Not:A-Brand","version":"8"},{"brand":"Chromium","version":"156"},{"brand":"Google Chrome","version":"156"}],"fullVersion":"156.0.8078.80","fullVersionList":[{"brand":"Not:A-Brand","version":"8.0.0.0"},{"brand":"Chromium","version":"156.0.8078.80"},{"brand":"Google Chrome","version":"156.0.8078.80"}],"mobile":false,"model":"","platform":"Windows","platformVersion":"15.0.0","wow64":false}}"#;

#[test]
fn grease_constants_match_the_live_calibration() {
    // The goldens below hardcode the values dumped from a live Chrome
    // 156.0.8078.80 (examples/dump_native.rs). If the mirage-core constants
    // drift (e.g. a new Chrome major ships a new grease cycle), the GREASE
    // branch in build_user_agent_override would silently stop matching the
    // fixture brands — this pins the two together.
    assert_eq!(mirage_core::GREASE_BRAND, "Not:A-Brand");
    assert_eq!(mirage_core::GREASE_VERSION, "8");
    assert_eq!(mirage_core::GREASE_FULL_VERSION, "8.0.0.0");
}

#[test]
fn fingerprint_prefix_is_the_contract_json() {
    let profile = profile_direct();
    let prefix = fingerprint_prefix(&profile.fingerprint).expect("serialize prefix");
    assert_eq!(prefix, format!("window.__MIRAGE_FP__ = {FP_JSON};\n"));
}

#[test]
fn user_agent_override_params_match_contract() {
    let profile = profile_direct();
    let params = build_user_agent_override(&profile.fingerprint).expect("build params");
    let actual = serde_json::to_string(&params).expect("serialize params");
    assert_eq!(actual, EXPECTED_UA_PARAMS);
}

#[test]
fn user_agent_override_wire_message_is_exact() {
    let profile = profile_direct();
    let params = build_user_agent_override(&profile.fingerprint).expect("build params");
    let wire =
        wire_message(2, "Network.setUserAgentOverride", params).expect("serialize wire message");
    assert_eq!(
        wire,
        format!(
            r#"{{"id":2,"method":"Network.setUserAgentOverride","params":{EXPECTED_UA_PARAMS}}}"#
        )
    );
}

#[test]
fn timezone_override_params_and_wire_are_exact() {
    let profile = profile_direct();
    let params = build_timezone_override(&profile.fingerprint);
    assert_eq!(
        serde_json::to_string(&params).expect("serialize params"),
        r#"{"timezoneId":"Europe/London"}"#
    );
    let wire = wire_message(3, "Emulation.setTimezoneOverride", params).expect("serialize");
    assert_eq!(
        wire,
        r#"{"id":3,"method":"Emulation.setTimezoneOverride","params":{"timezoneId":"Europe/London"}}"#
    );
}

#[test]
fn locale_override_params_and_wire_are_exact() {
    let profile = profile_direct();
    let params = build_locale_override(&profile.fingerprint);
    assert_eq!(
        serde_json::to_string(&params).expect("serialize params"),
        r#"{"locale":"en-US"}"#
    );
    let wire = wire_message(4, "Emulation.setLocaleOverride", params).expect("serialize");
    assert_eq!(
        wire,
        r#"{"id":4,"method":"Emulation.setLocaleOverride","params":{"locale":"en-US"}}"#
    );
}

#[test]
fn device_metrics_override_params_and_wire_are_exact() {
    let profile = profile_direct();
    let params = build_device_metrics_override(&profile.fingerprint);
    // CONTRACT.md item 5: screen geometry from the legend (1920x1080 for the
    // fixture) for BOTH the viewport (width/height) and the emulated screen
    // (screenWidth/screenHeight), the legend's dpr, desktop mode. Keys come out
    // alphabetically.
    assert_eq!(
        serde_json::to_string(&params).expect("serialize params"),
        r#"{"deviceScaleFactor":1.0,"height":1080,"mobile":false,"screenHeight":1080,"screenWidth":1920,"width":1920}"#
    );
    let wire = wire_message(5, "Emulation.setDeviceMetricsOverride", params).expect("serialize");
    assert_eq!(
        wire,
        r#"{"id":5,"method":"Emulation.setDeviceMetricsOverride","params":{"deviceScaleFactor":1.0,"height":1080,"mobile":false,"screenHeight":1080,"screenWidth":1920,"width":1920}}"#
    );
}

#[test]
fn add_script_command_carries_the_full_source() {
    let profile = profile_direct();
    let prefix = fingerprint_prefix(&profile.fingerprint).expect("serialize prefix");

    // Two fake modules: one without a trailing newline (the assembler must
    // add the separator), one with it (no double newline).
    let source = assemble_source(
        &prefix,
        &[
            ("rng", String::from("(function(){/*rng*/})();")),
            (
                "navigator",
                String::from("(function(){/*navigator*/})();\n"),
            ),
        ],
    );
    assert_eq!(
        source,
        format!("{prefix}(function(){{/*rng*/}})();\n(function(){{/*navigator*/}})();\n")
    );

    // The command wraps the source verbatim under the single "source" key.
    let params = build_add_script_command(&source);
    let object = params.as_object().expect("params must be an object");
    assert_eq!(
        object.len(),
        1,
        "params must carry exactly one key: {params}"
    );
    assert_eq!(
        object.get("source"),
        Some(&serde_json::Value::String(source)),
        "source must be the assembled string verbatim: {params}"
    );
}

#[test]
fn add_script_wire_message_is_exact() {
    let params = build_add_script_command("window.__MIRAGE_FP__ = 1;\n");
    let wire = wire_message(9, "Page.addScriptToEvaluateOnNewDocument", params).expect("serialize");
    assert_eq!(
        wire,
        r#"{"id":9,"method":"Page.addScriptToEvaluateOnNewDocument","params":{"source":"window.__MIRAGE_FP__ = 1;\n"}}"#
    );
}

/// Sessions observed on a live Chrome 156 flat auto-attach are 32 hex chars;
/// the exact value is irrelevant, only that it round-trips verbatim.
const SESSION: &str = "27072A4EF8125A3DC6AADE03310F911C";

#[test]
fn session_scoped_timezone_wire_is_exact() {
    let profile = profile_direct();
    let params = build_timezone_override(&profile.fingerprint);
    let wire = wire_message_with_session(3, Some(SESSION), "Emulation.setTimezoneOverride", params)
        .expect("serialize");
    assert_eq!(
        wire,
        r#"{"id":3,"method":"Emulation.setTimezoneOverride","params":{"timezoneId":"Europe/London"},"sessionId":"27072A4EF8125A3DC6AADE03310F911C"}"#
    );
}

#[test]
fn session_scoped_add_script_wire_is_exact() {
    // The shape the auto-attach task emits for every new page target: the full
    // inject source wrapped for one flat session.
    let params = build_add_script_command("window.__MIRAGE_FP__ = 1;\n");
    let wire = wire_message_with_session(
        12,
        Some(SESSION),
        "Page.addScriptToEvaluateOnNewDocument",
        params,
    )
    .expect("serialize");
    assert_eq!(
        wire,
        r#"{"id":12,"method":"Page.addScriptToEvaluateOnNewDocument","params":{"source":"window.__MIRAGE_FP__ = 1;\n"},"sessionId":"27072A4EF8125A3DC6AADE03310F911C"}"#
    );
}

#[test]
fn session_field_is_absent_without_a_session() {
    // Byte-for-byte equality with the session-less builder: no stray key.
    let params = build_add_script_command("x");
    assert_eq!(
        wire_message_with_session(1, None, "Page.enable", params.clone()).expect("serialize"),
        wire_message(1, "Page.enable", params).expect("serialize")
    );
}
