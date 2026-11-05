//! Launch argument assembly, byte-for-byte per CONTRACT.md ("Аргументы
//! Chromium"): the mandatory flag set, `--proxy-server` when the profile has a
//! proxy, the WebRTC relay policy, and never any of the forbidden flags.

use mirage_core::Profile;
use serde::Serialize;
use std::path::PathBuf;

/// Everything needed to spawn one masked Chromium instance.
pub struct LaunchConfig {
    /// Profile whose fingerprint/proxy drives both the arguments and the CDP
    /// spoofing applied right after spawn.
    pub profile: Profile,
    /// Explicit browser executable. `None` triggers [`crate::find::find_chrome`].
    pub chrome_path: Option<PathBuf>,
    /// Chromium `--user-data-dir` (created on launch if missing).
    pub profile_dir: PathBuf,
    /// Local DevTools TCP port the launcher talks CDP over.
    pub debug_port: u16,
}

/// Flags that must never be passed (CONTRACT.md forbidden list):
/// they either break the masking or the spoofed WebGL renderer.
const FORBIDDEN_FLAGS: [&str; 4] = [
    "--headless",
    "--enable-automation",
    "--disable-gpu",
    "--no-sandbox",
];

/// Build the Chromium command line for `cfg`, strictly per CONTRACT.md.
///
/// Returns exact `--flag=value` strings; the caller passes them to the
/// process command unmodified. The mandatory flags come first, in contract
/// order, followed by the conditional proxy and WebRTC flags.
///
/// `cfg.debug_port` is used verbatim: [`crate::launch::launch`] resolves the
/// port-0 "pick a free one" semantics to a concrete port before calling the
/// internal variant, so the public entry point only ever sees the port the
/// caller configured.
pub fn build_args(cfg: &LaunchConfig) -> Vec<String> {
    build_args_with_port(cfg, cfg.debug_port)
}

/// `build_args` with an explicit DevTools port, used by [`crate::launch::launch`]
/// after it has resolved a port-0 config to a concrete free port.
pub(crate) fn build_args_with_port(cfg: &LaunchConfig, debug_port: u16) -> Vec<String> {
    let fp = &cfg.profile.fingerprint;

    // Mandatory flags, in contract order.
    let mut args = vec![
        format!("--user-data-dir={}", cfg.profile_dir.display()),
        format!("--remote-debugging-port={debug_port}"),
        String::from("--no-first-run"),
        String::from("--no-default-browser-check"),
        String::from("--disable-blink-features=AutomationControlled"),
        String::from("--password-store=basic"),
        String::from("--hide-crash-restore-bubble"),
        // The core validator guarantees non-empty languages; the fallback only
        // guards against hand-built profiles.
        format!(
            "--lang={}",
            fp.languages.first().map(String::as_str).unwrap_or("en-US")
        ),
        // Maximized window geometry must match the screen legend: only a
        // maximized window reports outerWidth == availWidth and
        // outerHeight == availHeight on a default taskbar layout.
        String::from("--start-maximized"),
    ];

    // Optional proxy: --proxy-server=<scheme>://<host>:<port>.
    if let Some(proxy) = &cfg.profile.proxy {
        args.push(format!(
            "--proxy-server={}://{}:{}",
            json_str(&proxy.scheme),
            proxy.host,
            proxy.port
        ));
    }

    // WebRTC IP handling policy only in relay mode. "real" stays untouched and
    // "off" is enforced by the JS layer (inject/webrtc.js), not by a flag.
    // `disable_non_proxied_udp` is the value Chrome 156 actually applies;
    // the older `relay_only` value is silently ignored by it (verified on a
    // live 156.0.8078.80 run), which would leak host/mDNS candidates.
    if json_str(&fp.web_rtc) == "relay" {
        args.push(String::from(
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
        ));
    }

    // Belt and suspenders: the forbidden flags must never leak out of here.
    debug_assert!(
        !args.iter().any(|arg| is_forbidden(arg)),
        "forbidden flag emitted: {args:?}"
    );
    args
}

/// `true` when `arg` matches a forbidden flag exactly or with a value suffix
/// (so `--headless=new` is caught as well).
fn is_forbidden(arg: &str) -> bool {
    FORBIDDEN_FLAGS
        .iter()
        .any(|flag| arg == *flag || arg.starts_with(&format!("{flag}=")))
}

/// Stringify a closed-set mirage-core field through serde.
///
/// Used for `webRTC` and the proxy scheme: the contract fixes their JSON
/// values ("real" | "relay" | "off", "http" | "socks5"), but the Rust-side
/// representation (plain `String` vs a serde enum) belongs to mirage-core.
/// Going through serde keeps this crate correct for either representation.
fn json_str<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_matcher_catches_exact_and_valued_forms() {
        assert!(is_forbidden("--headless"));
        assert!(is_forbidden("--headless=new"));
        assert!(is_forbidden("--enable-automation"));
        assert!(is_forbidden("--no-sandbox"));
        assert!(is_forbidden("--disable-gpu"));
        // Near misses that must NOT be treated as forbidden.
        assert!(!is_forbidden(
            "--disable-blink-features=AutomationControlled"
        ));
        assert!(!is_forbidden("--disable-gpu-something-else"));
        assert!(!is_forbidden("--no-first-run"));
    }
}
