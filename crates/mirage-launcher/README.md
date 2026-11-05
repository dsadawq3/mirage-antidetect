# mirage-launcher

Locates Chrome/Chromium, builds the masked launch argument set (CONTRACT.md
"Аргументы Chromium"), spawns the browser, and injects the Mirage JS spoof
layer **into every page target** over the Chrome DevTools Protocol
(CONTRACT.md "CDP-команды" + "Спуф-слой inject/").

## Module layout

| File | Responsibility |
|---|---|
| `src/find.rs` | browser discovery over well-known paths, registry-free |
| `src/args.rs` | `LaunchConfig`, `build_args` (exact flag strings) |
| `src/cdp.rs` | minimal CDP client: raw-TCP HTTP discovery + tokio-tungstenite ws, flat-mode `sessionId`, event subscription |
| `src/launch.rs` | spawn, browser-level attach, flat auto-attach, CDP command set, inject-source assembly |
| `src/session.rs` | `SessionManager`: launch/stop/track profiles by id |
| `tests/` | argument tests + CDP JSON goldens + inject assembly tests + a live auto-attach test |

## Inject layer: embedded, with a dev override

The ten `inject/*.js` modules (order: `rng, navigator, screen, time, plugins,
clienthints, canvas, webgl, audio, webrtc`) are compiled into the binary with
`include_str!`. A relocated or installed binary therefore ships the full spoof
layer — the old source-relative disk read broke the moment the binary left the
source tree (review finding *minor 10*).

Set `MIRAGE_INJECT_DIR=/some/dir` to read the modules from disk at runtime
instead (development / hot-edit mode). A missing module still aborts with
`missing inject module: <name>`: the spoof layer is all-or-nothing, a partial
layer leaks real values.

## Behavior notes

- **Browser-level attach + flat auto-attach.** The CDP session attaches to the
  browser-level websocket (`webSocketDebuggerUrl` from `/json/version`), not to
  a single page. It then sends
  `Target.setAutoAttach { autoAttach: true, waitForDebuggerOnStart: true,
  flatten: true }`, which attaches the initial page *and* every later target
  (Ctrl+T, `window.open`, links in new tabs) as a flat session. This is the fix
  for review finding **B3**: previously only the first page got the spoof, so a
  new tab ran with the real UA/locale/timezone and no inject layer.
- **Pause → spoof → resume.** A new target arrives with
  `waitingForDebugger: true`, so no page script runs before the launcher has
  applied the full spoof command set to that target's session. Then the target
  is released.
  - Resume primitive (verified on a live Chrome 156.0.8078.80): **there is no
    `Target.resume`** — neither `Target.resume { sessionId }` nor a
    session-scoped `Target.resume`; both answer `'Target.resume' wasn't
    found`. The working command is `Runtime.runIfWaitingForDebugger` sent *on*
    the target's session.
  - `Target.setAutoAttachRelated` is not needed: `setAutoAttach{flatten}`
    attaches the page directly (also verified live).
- **First target.** During `launch()` the first page target is spoofed inline
  (same command set, its session id), then the event stream is handed to a
  background task that spoofs every later page target. A failure on one target
  is logged and skipped, never fatal to the others.
- **The exact command set** applied per target (CONTRACT.md "CDP-команды", 6
  commands, in this order, each carrying the target's `sessionId`):
  1. `Page.enable`, then `Network.enable`
  2. `Network.setUserAgentOverride` — `userAgent`, `acceptLanguage`
     (`languages.join(",")`), `platform`, and `userAgentMetadata` where
     `fullVersionList` = the fingerprint's brands with full versions: the
     GREASE brand gets `mirage_core::GREASE_FULL_VERSION` (`"8.0.0.0"` for the
     Chrome-156 grease cycle), every other brand gets `uaFullVersion`
  3. `Emulation.setTimezoneOverride` — `{ "timezoneId": ... }`
  4. `Emulation.setLocaleOverride` — `{ "locale": languages[0] }`
  5. `Emulation.setDeviceMetricsOverride` —
     `{ width: 0, height: 0, deviceScaleFactor: screen.dpr, mobile: false }`
  6. `Page.addScriptToEvaluateOnNewDocument` — the assembled inject source
- **CDP client internals.** `CdpClient::send` waits for the response to *its*
  id, exactly as before; a background pump owns the websocket and routes
  command responses to their awaiters and protocol events to the channel from
  `CdpClient::events()`. `send_with_session` adds the flat `sessionId` field.
- **Readiness polling:** `/json/version` every 200 ms up to 15 s after spawn,
  then the first page attach up to 10 s.
- **`LaunchConfig::debug_port = 0`** means "pick a free port" (ephemeral bind,
  read back and hand the concrete port to Chrome). The chosen port is reported
  on `LaunchedBrowser::debug_port`.
- **Cross-contamination guard:** after attaching, the launcher sends
  `SystemInfo.getProcessInfo` on the browser session and verifies the spawned
  child's pid is among the browser's processes — otherwise it aborts with
  `attached to a foreign browser on port X`. Best-effort: if the command is not
  supported, the check is skipped.
- **Lifetime:** graceful shutdown is `LaunchedBrowser::stop()` (abort the
  auto-attach task, kill + reap). `Drop` is only a safety net (abort + a
  synchronous `start_kill()`); it never panics.

## Session manager

`SessionManager` owns one `LaunchedBrowser` per `profile_id`:

```rust
let manager = SessionManager::new();
let handle = manager.launch_profile(profile, profile_dir).await?; // debug port auto
manager.running().await;      // Vec<RunningSession> (launch order)
manager.stop(&profile_id).await?;   // idempotent
manager.stop_all().await?;          // best effort, first error returned
```

A second `launch_profile` with a live `profile_id` fails with the typed
`AlreadyRunning` error (recoverable from `anyhow::Error` via `downcast_ref`),
so one profile's fingerprint is never served by two browsers. Dropping the
manager drops every browser, whose `Drop` fires `start_kill()` — no zombies.

## Launch flags (CONTRACT.md "Аргументы Chromium")

The mandatory set (`--user-data-dir`, `--remote-debugging-port`,
`--no-first-run`, `--no-default-browser-check`,
`--disable-blink-features=AutomationControlled`, `--password-store=basic`,
`--hide-crash-restore-bubble`, `--lang=<languages[0]>`, `--start-maximized` —
maximized window geometry must match the screen legend), `--proxy-server` when
the profile has a proxy, and
`--force-webrtc-ip-handling-policy=disable_non_proxied_udp` in WebRTC `"relay"`
mode (`relay_only` is silently ignored by Chrome 156 and would leak host
candidates). Forbidden: `--headless`, `--enable-automation`, `--disable-gpu`,
`--no-sandbox`.

## Consumed mirage-core API (verified against `crates/mirage-core/src/model.rs`)

```rust
// re-exported from the crate root
Os            // enum: Windows | Macos | Linux (serde lowercase)
ProxyScheme   // enum: Http | Socks5 (serde lowercase)
// GREASE calibration constants (live Chrome 156.0.8078.80 dump; they rotate
// with the Chrome major — see CONTRACT.md "GREASE-калибровка")
GREASE_BRAND        // "Not:A-Brand"
GREASE_VERSION      // "8"
GREASE_FULL_VERSION // "8.0.0.0"
Brand { brand: String, version: String }
GeoFp { lat: f64, lon: f64 }
ScreenFp { width, height, avail_width, avail_height: u32, dpr: f64, color_depth: u32 }
WebglFp { vendor: String, renderer: String, max_texture_size: u32,
          max_viewport_dims: [u32; 2], max_renderbuffer_size: u32 }
ClientHintsFp { brands: Vec<Brand>, mobile: bool, platform: String,
                platform_version: String, architecture: String,
                bitness: String, ua_full_version: String }
Fingerprint { os: Os, os_version, platform, ua, chrome_version: String,
              languages: Vec<String>, timezone: String,
              geo: GeoFp, screen: ScreenFp,
              hardware_concurrency: u32, device_memory: u32,
              webgl: WebglFp, canvas_seed: u32, audio_seed: u32,
              fonts: Vec<String>, web_rtc: String,   // serde rename "webRTC"
              client_hints: ClientHintsFp }
ProxyConfig { scheme: ProxyScheme, host: String, port: u16,
              username: Option<String>, password: Option<String> }
Profile { id, name, notes, created_at, updated_at: String,
          tags: Vec<String>, fingerprint: Fingerprint,
          proxy: Option<ProxyConfig> }
```

Notes:

- `Fingerprint`'s field order equals the CONTRACT.md schema order, which the
  `window.__MIRAGE_FP__` golden test enforces.
- `webRTC` stays a free-form `String`; the launcher compares it through
  `args::json_str` (serde stringification), which also covers enum-shaped
  fields like `ProxyScheme` for `--proxy-server`.

## Live tests

`tests/auto_attach_live.rs` spawns a real Chrome and verifies the B3 fix (a new
tab is spoofed). It is `#[ignore]`d so the default `cargo test` stays headless:

```text
cargo test -p mirage-launcher --test auto_attach_live -- --ignored --nocapture
```
