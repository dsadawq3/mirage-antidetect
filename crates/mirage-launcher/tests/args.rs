//! build_args tests against CONTRACT.md ("Аргументы Chromium"):
//! mandatory flags, conditional proxy/WebRTC flags, forbidden flags.

mod common;

use common::{launch_config, profile_direct, profile_relay, profile_socks5};
use mirage_core::Profile;
use mirage_launcher::args::build_args;
use std::path::PathBuf;

/// The forbidden list from CONTRACT.md.
const FORBIDDEN: [&str; 4] = [
    "--headless",
    "--enable-automation",
    "--disable-gpu",
    "--no-sandbox",
];

fn args_for(profile: Profile, dir: PathBuf, port: u16) -> Vec<String> {
    build_args(&launch_config(profile, dir, port))
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn has_prefix(args: &[String], prefix: &str) -> bool {
    args.iter().any(|arg| arg.starts_with(prefix))
}

#[test]
fn mandatory_flags_present_without_proxy() {
    let dir = PathBuf::from("profiles").join("shop-01");
    let args = args_for(profile_direct(), dir.clone(), 9222);

    assert!(
        args.contains(&format!("--user-data-dir={}", dir.display())),
        "user-data-dir must be the configured profile dir: {args:?}"
    );
    assert!(
        has(&args, "--remote-debugging-port=9222"),
        "missing debug port: {args:?}"
    );
    for flag in [
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-blink-features=AutomationControlled",
        "--password-store=basic",
        "--hide-crash-restore-bubble",
        "--lang=en-US",
        // Maximized window geometry is part of the screen legend.
        "--start-maximized",
    ] {
        assert!(has(&args, flag), "missing mandatory flag {flag}: {args:?}");
    }

    // No proxy, webRTC "real": neither optional flag may appear.
    assert!(
        !has_prefix(&args, "--proxy-server"),
        "unexpected proxy flag: {args:?}"
    );
    assert!(
        !has_prefix(&args, "--force-webrtc-ip-handling-policy"),
        "unexpected webrtc policy flag: {args:?}"
    );
}

#[test]
fn proxy_flag_matches_contract_format() {
    let args = args_for(profile_socks5(), PathBuf::from("p"), 9222);
    assert!(
        has(&args, "--proxy-server=socks5://1.2.3.4:1080"),
        "proxy flag must be scheme://host:port: {args:?}"
    );
}

#[test]
fn webrtc_relay_adds_the_policy_flag() {
    let args = args_for(profile_relay(), PathBuf::from("p"), 9222);
    assert!(
        has(
            &args,
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp"
        ),
        "relay mode must force the non-proxied-UDP policy: {args:?}"
    );
    // The pre-156 value is silently ignored by Chrome 156 (live-run finding):
    // emitting it would leak host candidates while pretending to be relay-only.
    assert!(
        !args.iter().any(|arg| arg.contains("relay_only")),
        "the dead relay_only value must never be emitted: {args:?}"
    );
}

#[test]
fn webrtc_real_adds_no_policy_flag() {
    let args = args_for(profile_direct(), PathBuf::from("p"), 9222);
    assert!(
        !has_prefix(&args, "--force-webrtc-ip-handling-policy"),
        "real mode must not touch the policy: {args:?}"
    );
}

#[test]
fn forbidden_flags_never_emitted() {
    for profile in [profile_direct(), profile_socks5(), profile_relay()] {
        let args = args_for(profile, PathBuf::from("p"), 9222);
        for forbidden in FORBIDDEN {
            let leaked = args
                .iter()
                .any(|arg| arg == forbidden || arg.starts_with(&format!("{forbidden}=")));
            assert!(!leaked, "forbidden flag {forbidden} emitted: {args:?}");
        }
    }
}

#[test]
fn user_data_dir_is_the_configured_dir_verbatim() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("nested").join("profile");
    let args = args_for(profile_direct(), dir.clone(), 1234);
    assert!(
        args.contains(&format!("--user-data-dir={}", dir.display())),
        "user-data-dir must contain the profile dir: {args:?}"
    );
    assert!(
        has(&args, "--remote-debugging-port=1234"),
        "port mismatch: {args:?}"
    );
}
