//! Browser spawn + spoofing-layer injection over CDP.
//!
//! Flow of [`launch`]: resolve the binary → create the profile dir → spawn
//! Chrome with the masked arguments → poll the DevTools endpoint until it
//! answers → attach to the *browser-level* websocket → enable flat
//! auto-attach → spoof the first page target → spawn a background task that
//! spoofs every later page target (Ctrl+T, `window.open`, links in new tabs).
//!
//! ## Why browser-level (not page-level) attach
//!
//! A page-level socket only controls the one tab it is attached to: new tabs
//! opened by the user ran with the real UA/locale/timezone and no inject layer
//! (review finding B3). Attaching to the browser socket and using
//! `Target.setAutoAttach { autoAttach, waitForDebuggerOnStart, flatten }`
//! surfaces every tab as a flat CDP session; each one is held paused until its
//! spoof commands have run, then released.
//!
//! ## Resume primitive (verified against Chrome 156.0.8078.80)
//!
//! The task brief suggested `Target.resume { sessionId }`, but that command
//! does not exist on Chrome 156 — neither as a browser-level command with a
//! `sessionId` parameter nor as a session-scoped command; both answer
//! `'Target.resume' wasn't found`. The correct primitive is
//! `Runtime.runIfWaitingForDebugger` sent **on** the paused target's session.
//!
//! ## Worker / service-worker scoping
//!
//! Fingerprint pages compare the main thread against workers, so a worker that
//! reads the real `hardwareConcurrency` / `deviceMemory` / `userAgentData` /
//! WebGL strings is a strong main-vs-worker tell. Verified live on Chrome 156:
//! the browser-level flat auto-attach surfaces `service_worker` targets but
//! NEVER the page's dedicated `worker` targets - those appear only once the
//! *page* session turns on flat auto-attach. [`apply_spoof`] therefore enables
//! `Target.setAutoAttach` (worker-type filter) on every page session, and each
//! attached `worker` / `shared_worker` from a web origin is fed
//! [`EMBEDDED_WORKER_SNIPPET`] via `Runtime.evaluate` BEFORE it is released -
//! while `waitForDebuggerOnStart` still holds it, i.e. before the worker's own
//! script runs. A `service_worker` is the exception: a paused one never answers
//! Runtime commands (verified live), so it is released first and patched as a
//! best effort. Spoofing is idempotent per session (see [`SpoofedSessions`]).

use crate::args::{build_args_with_port, LaunchConfig};
use crate::cdp::{self, CdpClient};
use crate::find::find_chrome;
use anyhow::{anyhow, Context, Result};
use mirage_core::{Fingerprint, GREASE_BRAND, GREASE_FULL_VERSION};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::fs;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Inject module order, byte-for-byte per CONTRACT.md ("Спуф-слой inject/").
pub const INJECT_MODULES: [&str; 10] = [
    "rng",
    "navigator",
    "screen",
    "time",
    "plugins",
    "clienthints",
    "canvas",
    "webgl",
    "audio",
    "webrtc",
];

/// The inject layer compiled into the binary, in [`INJECT_MODULES`] order.
///
/// Embedding (rather than reading at runtime) is what makes a relocated or
/// installed binary ship the full spoof layer: the old source-relative disk
/// read breaks the moment the binary leaves the source tree (review finding
/// minor 10). [`INJECT_DIR_ENV`] restores disk reading for development.
const EMBEDDED_INJECT: [&str; INJECT_MODULES.len()] = [
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../inject/rng.js")),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/navigator.js"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/screen.js"
    )),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../inject/time.js")),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/plugins.js"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/clienthints.js"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/canvas.js"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/webgl.js"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/audio.js"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../inject/webrtc.js"
    )),
];

/// Environment variable that, when set to a non-empty path, makes the launcher
/// read the inject layer from that directory at runtime instead of using the
/// embedded copy (development / hot-edit mode).
pub const INJECT_DIR_ENV: &str = "MIRAGE_INJECT_DIR";

/// The worker-scope snippet compiled into the binary.
///
/// Deliberately NOT part of [`INJECT_MODULES`]: the page bundle never contains
/// it, and it never touches `window`. The launcher injects it only into
/// worker-scope CDP targets (`worker` / `service_worker` / `shared_worker`),
/// as the source of a `Runtime.evaluate` that runs while the target is held
/// paused by `waitForDebuggerOnStart` — i.e. before the worker's own script,
/// which is the only way to reach a service worker's global scope at all.
const EMBEDDED_WORKER_SNIPPET: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../inject/worker.js"
));

/// CDP target types whose global scope gets the worker snippet. `worker` =
/// dedicated, `service_worker` = service worker, `shared_worker` = shared.
pub const WORKER_TARGET_TYPES: [&str; 3] = ["worker", "service_worker", "shared_worker"];

/// Session ids already spoofed, shared between the first-page setup path and
/// the background auto-attach task. `Target.attachedToTarget` can be delivered
/// more than once for the same session (browser-level and page-level flat
/// auto-attach both surface page-derived workers), and Chrome then answers a
/// re-applied `Emulation.setLocaleOverride` with "Another locale override is
/// already in effect". Tracking sessions makes spoofing idempotent instead of
/// relying on swallowing that error.
type SpoofedSessions = Arc<StdMutex<HashSet<String>>>;

/// Budget for the DevTools HTTP endpoint to come up after spawn.
const DEVTOOLS_WAIT: Duration = Duration::from_secs(15);
/// Budget for the first page target to be attached once auto-attach is on.
const PAGE_WAIT: Duration = Duration::from_secs(10);
/// Fixed 200 ms poll interval between attempts (task spec; CONTRACT.md does
/// not pin a retry interval).
const RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// Per-command budget for worker-scope targets.
///
/// A freshly-attached service worker can occasionally leave a Runtime command
/// unanswered (verified live against a real page). Worker commands are bounded
/// by this so one stuck instance cannot stall the serialized auto-attach loop
/// and every target behind it — the dedicated worker a fingerprint page reads
/// is reached well within its own probe deadline even when a service worker
/// on the same page goes quiet.
const WORKER_COMMAND_TIMEOUT: Duration = Duration::from_millis(2500);

/// A spawned, masked browser whose CDP session is attached at browser level
/// with every page target spoofed.
///
/// Lifetime contract: the graceful shutdown path is [`LaunchedBrowser::stop`]
/// (abort the auto-attach task, kill + reap). [`Drop`] is only a safety net: it
/// aborts the task and fires a synchronous `start_kill()` so a handle dropped
/// without `stop()` cannot leave a zombie Chrome behind; it does not wait for
/// the process to exit.
pub struct LaunchedBrowser {
    /// The spawned Chromium process.
    pub child: Child,
    /// The DevTools TCP port actually in use. When the config asked for port
    /// 0 ("pick a free one"), this is the concrete port that was chosen.
    pub debug_port: u16,
    /// DevTools target id of the *first* page the spoofing layer was injected
    /// into (later tabs are handled by the auto-attach task).
    pub target_id: String,
    /// Browser-level websocket debugger URL the CDP session is attached to.
    pub ws_url: String,
    /// Flat-mode session id of the first page target; [`Self::send_command`]
    /// routes page commands through it.
    pub page_session_id: String,
    /// Browser-level CDP client, shared with the auto-attach task.
    ///
    /// `Arc<CdpClient>` (not `Arc<Mutex<..>>`): [`CdpClient`] commands take
    /// `&self` and are internally synchronized, so caller commands and the
    /// auto-attach task never starve each other.
    cdp: Arc<CdpClient>,
    /// Background auto-attach task; aborted on stop/drop.
    auto_attach: Option<JoinHandle<()>>,
}

impl LaunchedBrowser {
    /// Stop the auto-attach task, kill the browser and wait until it is reaped.
    ///
    /// Killing an already-exited child is not an error — what matters is that
    /// `wait()` reaps the process and reports real failures.
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(task) = self.auto_attach.take() {
            task.abort();
        }
        // Chrome may already be gone (user closed the window): kill() would
        // fail on a dead child, wait() below still reaps it.
        let _ = self.child.kill().await;
        self.child
            .wait()
            .await
            .map(|_| ())
            .context("failed to wait for Chrome process after kill")
    }

    /// Send a CDP command to the *first page* session
    /// (e.g. `Page.navigate`, `Page.captureScreenshot`).
    pub async fn send_command(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp
            .send_with_session(Some(&self.page_session_id), method, params)
            .await
    }

    /// Send a browser-level CDP command (no session), e.g. `Target.*`.
    pub async fn send_browser_command(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp.send(method, params).await
    }
}

impl Drop for LaunchedBrowser {
    /// Safety net against zombie Chrome processes, not the shutdown path.
    ///
    /// The graceful path is [`LaunchedBrowser::stop`], which aborts the
    /// auto-attach task, kills *and* reaps the child. This Drop only signals
    /// the process (`start_kill`: synchronous, no await, no reap) for handles
    /// dropped without `stop()`. Re-killing after `stop()` (or an already-
    /// exited child) is harmless, and errors are deliberately ignored: `Drop`
    /// must never panic.
    fn drop(&mut self) {
        if let Some(task) = self.auto_attach.take() {
            task.abort();
        }
        let _ = self.child.start_kill();
    }
}

/// Spawn Chromium with masking arguments and inject the spoofing layer.
pub async fn launch(cfg: &LaunchConfig) -> Result<LaunchedBrowser> {
    // Port 0 means "pick a free port yourself": bind an ephemeral listener
    // on 127.0.0.1, read back the OS-assigned port, drop the listener and
    // hand the concrete port to Chrome. (Chrome's own port-0 mode is not
    // usable here: the launcher must know the port to reach DevTools.)
    let port = if cfg.debug_port == 0 {
        free_debug_port()?
    } else {
        cfg.debug_port
    };

    // 1. Resolve the browser binary.
    let chrome_path = cfg
        .chrome_path
        .clone()
        .or_else(find_chrome)
        .ok_or_else(|| {
            anyhow!(
                "Chrome/Chromium not found in any default location; \
                 set LaunchConfig::chrome_path explicitly"
            )
        })?;

    // 2. Ensure the profile dir exists (Chromium requires it for a clean,
    //    non-default user-data-dir — which remote debugging also mandates).
    fs::create_dir_all(&cfg.profile_dir)
        .await
        .with_context(|| format!("failed to create profile dir {}", cfg.profile_dir.display()))?;

    // 3. Spawn with the masked argument set (the resolved port, never a
    //    literal 0, goes into --remote-debugging-port).
    let args = build_args_with_port(cfg, port);
    let mut child = Command::new(&chrome_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to spawn {}", chrome_path.display()))?;

    // 4-6. Wait for DevTools, attach at browser level, spoof the first target
    // and start the auto-attach task. Every failure on this path must not leak
    // an orphaned browser process: kill and reap the child before propagating.
    // tokio's `id()` is None only once the child has been reaped; nothing
    // polls it before this point, so None means the process is already gone
    // and no launch can succeed (or the endpoint answering is a stranger's).
    let child_id = child
        .id()
        .ok_or_else(|| anyhow!("spawned Chrome exited before the CDP session was set up"))?;
    let session = match setup_session(cfg, port, child_id).await {
        Ok(session) => session,
        Err(error) => {
            // The setup error is the primary failure; a failing kill/wait on
            // an already-dying child must not mask it.
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
    };

    Ok(LaunchedBrowser {
        child,
        debug_port: port,
        target_id: session.target_id,
        ws_url: session.ws_url,
        page_session_id: session.session_id,
        cdp: session.cdp,
        auto_attach: Some(session.auto_attach),
    })
}

/// The CDP-side result of a successful setup: the browser client, the first
/// spoofed page session and the running auto-attach task.
struct SpoofedSession {
    target_id: String,
    ws_url: String,
    session_id: String,
    cdp: Arc<CdpClient>,
    auto_attach: JoinHandle<()>,
}

/// One `Target.attachedToTarget` event, reduced to what the launcher needs.
struct AttachedTarget {
    target_id: String,
    session_id: String,
    target_type: String,
    /// Target URL, used to skip the browser's own components (extensions,
    /// chrome:// pages) when deciding whether a worker-like target is a page's
    /// worker (spoofable) or a browser-internal one (not ours).
    url: String,
    waiting_for_debugger: bool,
}

impl AttachedTarget {
    /// `true` for a dedicated / service / shared worker target.
    fn is_worker(&self) -> bool {
        WORKER_TARGET_TYPES.contains(&self.target_type.as_str())
    }

    /// `true` when the target belongs to web content (http/https/blob/data)
    /// rather than a browser component (chrome-extension://, chrome://,
    /// devtools://). Browser-internal workers must be released but never
    /// spoofed.
    fn is_web_worker(&self) -> bool {
        self.is_worker() && is_spoofable_worker_url(&self.url)
    }
}

/// `true` when `url` is a web-content worker origin we should spoof.
///
/// Chrome's own component extensions and internals (`chrome-extension://`,
/// `chrome://`, `devtools://`) are released but never injected into.
fn is_spoofable_worker_url(url: &str) -> bool {
    !(url.starts_with("chrome-extension://")
        || url.starts_with("chrome://")
        || url.starts_with("devtools://"))
}

/// Phases 4-6 of [`launch`]: wait for the DevTools HTTP endpoint, attach to
/// the browser-level websocket, turn on flat auto-attach, spoof the first page
/// target and hand the event stream to a background task for later tabs.
async fn setup_session(cfg: &LaunchConfig, port: u16, child_id: u32) -> Result<SpoofedSession> {
    // Phase 4: readiness probe; resolves to the browser-level websocket URL,
    // which is what everything below attaches to.
    let browser_ws = retry_until_ok(
        "DevTools endpoint",
        DEVTOOLS_WAIT,
        RETRY_INTERVAL,
        || async move { cdp::get_ws_url(port).await },
    )
    .await?;

    let fp = &cfg.profile.fingerprint;
    // Both sources are identical for every target; build them once. The page
    // source goes through Page.addScriptToEvaluateOnNewDocument; the worker
    // source (worker.js, NOT in INJECT_MODULES) is instead the payload of a
    // Runtime.evaluate sent to each worker-scope target.
    let source = build_inject_source(fp).await?;
    let worker_source = build_worker_inject_source(fp).await?;

    // Phase 5: attach at browser level and subscribe to events before issuing
    // the auto-attach command, so no attach event can be missed.
    let mut client = CdpClient::attach(&browser_ws).await?;
    let mut events = client.events();
    // Cross-contamination guard, before anything is spoofed: verify the
    // socket really belongs to the child we spawned.
    verify_ownership(&client, port, child_id).await?;

    client
        .send(
            "Target.setAutoAttach",
            json!({
                "autoAttach": true,
                "waitForDebuggerOnStart": true,
                "flatten": true,
            }),
        )
        .await?;

    // Idempotency ledger shared with the auto-attach task: a session is
    // spoofed exactly once, no matter how many attach events name it.
    let done: SpoofedSessions = Arc::new(StdMutex::new(HashSet::new()));

    // The first page target (the initial about:blank / newtab) is attached
    // retroactively; grab it, spoof it, then release it.
    let first = wait_for_first_page(&client, &mut events, PAGE_WAIT).await?;
    spoof_page(&client, &first.session_id, fp, &source, &done).await?;
    if first.waiting_for_debugger {
        resume_target(&client, &first.session_id).await?;
    }

    let cdp = Arc::new(client);
    let auto_attach = tokio::spawn(auto_attach_loop(
        cdp.clone(),
        events,
        fp.clone(),
        source,
        worker_source,
        done,
    ));

    Ok(SpoofedSession {
        target_id: first.target_id,
        ws_url: browser_ws,
        session_id: first.session_id,
        cdp,
        auto_attach,
    })
}

/// Consume attach events until the first `page` target shows up (or `total`
/// elapses). Non-page targets that are paused (a new worker/extension) are
/// released immediately so startup is never stalled by them.
async fn wait_for_first_page(
    cdp: &CdpClient,
    events: &mut mpsc::UnboundedReceiver<Value>,
    total: Duration,
) -> Result<AttachedTarget> {
    let deadline = tokio::time::Instant::now() + total;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!("no page target attached within {total:?}"));
        }
        let event = match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => {
                return Err(anyhow!(
                    "CDP event stream closed before a page target was attached"
                ))
            }
            Err(_elapsed) => return Err(anyhow!("no page target attached within {total:?}")),
        };
        let Some(target) = attached_target(&event) else {
            continue;
        };
        if target.target_type == "page" {
            return Ok(target);
        }
        // Not a page: release it if it is waiting, then keep looking.
        if target.waiting_for_debugger {
            let _ = resume_target(cdp, &target.session_id).await;
        }
    }
}

/// Background task: spoof every target attached after startup.
///
/// Each iteration handles one event independently; a failure on one target is
/// logged and skipped so a single dead target cannot take the rest (or the
/// task) down. Page targets get the full page layer. Dedicated and shared
/// workers get the worker snippet injected BEFORE they are released (they
/// answer Runtime commands while paused); a service worker is released first
/// and patched best-effort, because a paused service worker does not answer
/// Runtime commands at all. Every paused target is then resumed — a target
/// that vanished mid-flight is not an error.
async fn auto_attach_loop(
    cdp: Arc<CdpClient>,
    mut events: mpsc::UnboundedReceiver<Value>,
    fp: Fingerprint,
    page_source: String,
    worker_source: String,
    done: SpoofedSessions,
) {
    while let Some(event) = events.recv().await {
        let Some(target) = attached_target(&event) else {
            continue;
        };

        // Page targets: full page layer, then release.
        if target.target_type == "page" {
            if let Err(error) = spoof_page(&cdp, &target.session_id, &fp, &page_source, &done).await
            {
                report_spoof_failure("spoof page", &target, &error);
            }
            if target.waiting_for_debugger {
                if let Err(error) = resume_target(&cdp, &target.session_id).await {
                    report_spoof_failure("resume", &target, &error);
                }
            }
            continue;
        }

        // Worker-scope targets from web origins (not browser components).
        if target.is_web_worker() {
            if target.target_type == "service_worker" {
                // A service worker held by `waitForDebuggerOnStart` does not
                // answer Runtime commands (verified live: both Runtime.enable
                // and Runtime.evaluate never respond, and even resuming can
                // time out). Release it FIRST so the page's service worker is
                // never stalled, then patch it as a best effort for the reads
                // that happen after start-up. This path is expected to be
                // silent: a service worker we cannot reach is a known residual
                // limitation, not a failure worth logging.
                if target.waiting_for_debugger {
                    let _ = resume_target_bounded(&cdp, &target.session_id).await;
                }
                let _ = spoof_worker(&cdp, &target.session_id, &worker_source, &done).await;
                continue;
            }
            // Dedicated / shared worker: it answers while paused, so inject the
            // snippet BEFORE releasing it (the worker's own script, which reads
            // navigator first thing, must never run unpatched).
            if let Err(error) = spoof_worker(&cdp, &target.session_id, &worker_source, &done).await
            {
                report_spoof_failure("spoof worker", &target, &error);
            }
            if target.waiting_for_debugger {
                if let Err(error) = resume_target_bounded(&cdp, &target.session_id).await {
                    report_spoof_failure("resume", &target, &error);
                }
            }
            continue;
        }

        // Everything else (extension service workers, background pages, ...):
        // never spoofed, but a paused one must still be released.
        if target.waiting_for_debugger {
            if let Err(error) = resume_target_bounded(&cdp, &target.session_id).await {
                report_spoof_failure("resume", &target, &error);
            }
        }
    }
}

/// Print a spoof/resume failure unless it is just a target that vanished.
fn report_spoof_failure(kind: &str, target: &AttachedTarget, error: &anyhow::Error) {
    if !is_gone(error) {
        eprintln!(
            "mirage: could not {kind} target {} ({}): {error:#}",
            target.target_id, target.session_id
        );
    }
}

/// Spoof one page target exactly once.
///
/// The session ledger is checked/updated before any command runs. A genuine
/// failure clears the marker so a later attach event may retry; benign
/// idempotency errors never reach here (they are swallowed per command in
/// [`apply_spoof`]).
async fn spoof_page(
    cdp: &CdpClient,
    session_id: &str,
    fp: &Fingerprint,
    source: &str,
    done: &SpoofedSessions,
) -> Result<()> {
    if !mark_session(done, session_id) {
        return Ok(()); // already spoofed
    }
    match apply_spoof(cdp, Some(session_id), fp, source).await {
        Ok(()) => Ok(()),
        Err(error) => {
            unmark_session(done, session_id);
            Err(error)
        }
    }
}

/// Spoof one worker-scope target exactly once.
///
/// Injects the worker snippet while the target is held paused by
/// `waitForDebuggerOnStart` (see [`EMBEDDED_WORKER_SNIPPET`]), then — best
/// effort — turns on the target's own flat auto-attach so a worker nested
/// inside this worker is reached too. Every command is bounded by
/// [`WORKER_COMMAND_TIMEOUT`]: a stuck worker must never stall the loop.
async fn spoof_worker(
    cdp: &CdpClient,
    session_id: &str,
    source: &str,
    done: &SpoofedSessions,
) -> Result<()> {
    if !mark_session(done, session_id) {
        return Ok(());
    }
    match evaluate_in_worker(cdp, session_id, source).await {
        Ok(()) => {
            // Best effort: nested-worker coverage. A rejection (or timeout on
            // an agent without the Target domain) is not fatal.
            let _ = cdp
                .send_with_session_timeout(
                    Some(session_id),
                    "Target.setAutoAttach",
                    worker_auto_attach_params(),
                    WORKER_COMMAND_TIMEOUT,
                )
                .await;
            Ok(())
        }
        Err(error) => {
            if !is_benign(&error) {
                unmark_session(done, session_id);
            }
            Err(error)
        }
    }
}

/// Run `source` in the (paused) worker's global scope.
///
/// `Runtime.enable` goes first and is best effort: live testing showed a
/// freshly-attached SERVICE WORKER can leave a bare first `Runtime.evaluate`
/// unanswered, while the same target answers immediately once its Runtime
/// agent has been initialized. Both commands are bounded so a target that
/// stays silent cannot block the loop.
///
/// `allowUnsafeEvalBlockedByCSP` lets the evaluate through even on a worker
/// whose response carries a restrictive CSP; a DevTools evaluate is not page
/// script, so this is not a sandbox escape, only a bypass of the page's own
/// script-src for our one-shot setup.
async fn evaluate_in_worker(cdp: &CdpClient, session_id: &str, source: &str) -> Result<()> {
    // `Runtime.enable` first, best effort and bounded: a service worker that
    // was released may still be slow to answer, and enabling the Runtime agent
    // is the cheapest way to wake it. A failure here must not skip the
    // evaluate.
    let _ = cdp
        .send_with_session_timeout(
            Some(session_id),
            "Runtime.enable",
            json!({}),
            WORKER_COMMAND_TIMEOUT,
        )
        .await;
    cdp.send_with_session_timeout(
        Some(session_id),
        "Runtime.evaluate",
        json!({
            "expression": source,
            "returnByValue": false,
            "awaitPromise": false,
            "includeCommandLineAPI": false,
            "allowUnsafeEvalBlockedByCSP": true,
        }),
        WORKER_COMMAND_TIMEOUT,
    )
    .await
    .map(|_| ())
}

/// Paused-by-default flat auto-attach params, reused for the browser-level
/// command and the page-level (worker-scoping) command.
fn worker_auto_attach_params() -> Value {
    json!({
        "autoAttach": true,
        "waitForDebuggerOnStart": true,
        "flatten": true,
        // Restrict the page-level attach to worker-scope targets: dedicated
        // workers appear only when the *page* session auto-attaches (the
        // browser-level attach surfaces pages and service workers, never
        // dedicated ones), and the filter keeps OOPIFs/other page children out.
        "filter": [
            { "type": "worker" },
            { "type": "service_worker" },
            { "type": "shared_worker" },
        ],
    })
}

/// Record `session_id` as spoofed; `false` when it was already present.
fn mark_session(done: &SpoofedSessions, session_id: &str) -> bool {
    let mut set = done.lock().unwrap_or_else(|poison| poison.into_inner());
    set.insert(session_id.to_owned())
}

/// Drop a session marker so a retry can run.
fn unmark_session(done: &SpoofedSessions, session_id: &str) {
    let mut set = done.lock().unwrap_or_else(|poison| poison.into_inner());
    set.remove(session_id);
}

/// `true` when the CDP error is Chrome's idempotency answer, not a failure.
///
/// Chrome 156 rejects a second `Emulation.setLocaleOverride` on the same
/// target with "Another locale override is already in effect"; that means the
/// desired state is already in place.
fn is_benign(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("already in effect")
        || text.contains("Session with given id not found")
        || text.contains("No session with given id")
}

/// `true` when the failure is only a target/session that vanished mid-flight.
fn is_gone(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("Session with given id not found") || text.contains("No session with given id")
}

/// Reduce a CDP event to an [`AttachedTarget`] when it is a
/// `Target.attachedToTarget`.
fn attached_target(event: &Value) -> Option<AttachedTarget> {
    if event.get("method").and_then(Value::as_str) != Some("Target.attachedToTarget") {
        return None;
    }
    let params = event.get("params")?;
    let session_id = params.get("sessionId").and_then(Value::as_str)?.to_owned();
    let info = params.get("targetInfo")?;
    Some(AttachedTarget {
        target_id: info.get("targetId").and_then(Value::as_str)?.to_owned(),
        session_id,
        target_type: info
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        url: info
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        waiting_for_debugger: params
            .get("waitingForDebugger")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Apply the exact command set of CONTRACT.md ("CDP-команды") to one target.
///
/// `session_id` is `None` on a page-level socket and `Some` on the browser
/// socket (flat mode). Order is contractual: `Page.enable` → `Network.enable`
/// → `Network.setUserAgentOverride` → `Emulation.setTimezoneOverride` →
/// `Emulation.setLocaleOverride` → `Emulation.setDeviceMetricsOverride` →
/// `Page.addScriptToEvaluateOnNewDocument`.
async fn apply_spoof(
    cdp: &CdpClient,
    session_id: Option<&str>,
    fp: &Fingerprint,
    source: &str,
) -> Result<()> {
    send_tolerant(cdp, session_id, "Page.enable", json!({})).await?;
    send_tolerant(cdp, session_id, "Network.enable", json!({})).await?;
    send_tolerant(
        cdp,
        session_id,
        "Network.setUserAgentOverride",
        build_user_agent_override(fp)?,
    )
    .await?;
    send_tolerant(
        cdp,
        session_id,
        "Emulation.setTimezoneOverride",
        build_timezone_override(fp),
    )
    .await?;
    send_tolerant(
        cdp,
        session_id,
        "Emulation.setLocaleOverride",
        build_locale_override(fp),
    )
    .await?;
    send_tolerant(
        cdp,
        session_id,
        "Emulation.setDeviceMetricsOverride",
        build_device_metrics_override(fp),
    )
    .await?;
    send_tolerant(
        cdp,
        session_id,
        "Page.addScriptToEvaluateOnNewDocument",
        build_add_script_command(source),
    )
    .await?;
    // Page-level flat auto-attach: without it Chrome never surfaces the page's
    // DEDICATED workers (verified live: the browser-level attach delivers pages
    // and service workers, never dedicated workers). The worker-type filter
    // keeps other page children (OOPIFs) out. Idempotent across re-attaches.
    send_tolerant(
        cdp,
        session_id,
        "Target.setAutoAttach",
        worker_auto_attach_params(),
    )
    .await?;
    Ok(())
}

/// Send a command, treating Chrome's idempotency answer as success.
///
/// A repeated `Emulation.setLocaleOverride` on the same target answers
/// "Another locale override is already in effect" - the desired state holds,
/// so it is not a failure.
async fn send_tolerant(
    cdp: &CdpClient,
    session_id: Option<&str>,
    method: &str,
    params: Value,
) -> Result<()> {
    match cdp.send_with_session(session_id, method, params).await {
        Ok(_) => Ok(()),
        Err(error) if is_benign(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Release a target paused by `waitForDebuggerOnStart`.
///
/// Chrome 156 has no `Target.resume`; the working command is
/// `Runtime.runIfWaitingForDebugger` on the target's own session (verified on
/// a live 156.0.8078.80 run).
async fn resume_target(cdp: &CdpClient, session_id: &str) -> Result<()> {
    cdp.send_with_session(
        Some(session_id),
        "Runtime.runIfWaitingForDebugger",
        json!({}),
    )
    .await
    .map(|_| ())
}

/// [`resume_target`] with the worker command budget.
///
/// Used for worker-scope targets so a paused worker that never answers the
/// Runtime domain cannot hold the auto-attach loop for the full command
/// timeout (and, with it, delay every worker behind it).
async fn resume_target_bounded(cdp: &CdpClient, session_id: &str) -> Result<()> {
    cdp.send_with_session_timeout(
        Some(session_id),
        "Runtime.runIfWaitingForDebugger",
        json!({}),
        WORKER_COMMAND_TIMEOUT,
    )
    .await
    .map(|_| ())
}

/// Best-effort ownership guard against cross-contamination.
///
/// `SystemInfo.getProcessInfo` answers with the process list of the browser
/// the websocket belongs to. If the command succeeds and the spawned child's
/// pid is not among the ids, we attached to a foreign browser sitting on that
/// port (a leftover instance, or the winner of the free-port race) — spoofing
/// it would inject our fingerprint into a stranger's session, so the launch
/// aborts loudly.
///
/// Best-effort on purpose: an older Chrome may legitimately reject the probe;
/// any error from it is swallowed and the launch continues.
async fn verify_ownership(cdp: &CdpClient, port: u16, child_id: u32) -> Result<()> {
    let info = match cdp.send("SystemInfo.getProcessInfo", json!({})).await {
        Ok(info) => info,
        // Not supported / command rejected: skip the check (best-effort).
        Err(_not_supported) => return Ok(()),
    };
    let ours = info
        .get("processInfo")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.get("id").and_then(Value::as_u64) == Some(u64::from(child_id)))
        });
    if !ours {
        return Err(anyhow!(
            "attached to a foreign browser on port {port}: \
             spawned child pid {child_id} is not among its processes"
        ));
    }
    Ok(())
}

/// Pick a free TCP port on 127.0.0.1 for the DevTools endpoint.
///
/// Binds an ephemeral listener (`127.0.0.1:0`), reads back the OS-assigned
/// port and drops the listener. Micro-race: between the drop and Chrome
/// binding the port another process could theoretically grab it — accepted
/// here, because retrying would not close the window either; a stolen port
/// simply surfaces as the DevTools endpoint never becoming ready (launch
/// error, child killed) or as the ownership guard firing.
pub(crate) fn free_debug_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .context("failed to bind an ephemeral listener while picking a free debug port")?;
    let port = listener
        .local_addr()
        .context("failed to read the ephemeral listener's local address")?
        .port();
    drop(listener);
    Ok(port)
}

/// Run `op` until it succeeds or `total` elapses, sleeping `interval` between
/// attempts. The last error is returned with a "did not become ready" context.
///
/// The deadline is checked *between* attempts: each attempt carries its own
/// internal timeout (10 s for the DevTools HTTP calls), so the wall-clock
/// worst case is `total` plus one in-flight attempt.
async fn retry_until_ok<T, F, Fut>(
    what: &str,
    total: Duration,
    interval: Duration,
    mut op: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let deadline = tokio::time::Instant::now() + total;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(
                        error.context(format!("{what} did not become ready within {total:?}"))
                    );
                }
                tokio::time::sleep(interval).await;
            }
        }
    }
}

/// Default on-disk inject directory: the [`INJECT_DIR_ENV`] override when set,
/// otherwise `<workspace>/inject` resolved from this crate's manifest.
///
/// This is the *development* location. Production launches use the layer
/// embedded at compile time (see [`build_inject_source`]); this directory is
/// only read when the override is set (or by tests).
pub fn inject_dir() -> PathBuf {
    if let Some(dir) = inject_dir_override() {
        return dir;
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("inject")
}

/// The [`INJECT_DIR_ENV`] override, when set to a non-empty path.
fn inject_dir_override() -> Option<PathBuf> {
    std::env::var_os(INJECT_DIR_ENV)
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
}

/// `window.__MIRAGE_FP__ = <fingerprint JSON>;\n` — the header every page gets
/// before the spoof modules run.
pub fn fingerprint_prefix(fp: &Fingerprint) -> Result<String> {
    let json = serde_json::to_string(fp).context("failed to serialize fingerprint")?;
    Ok(format!("window.__MIRAGE_FP__ = {json};\n"))
}

/// Concatenate the fingerprint prefix with the module sources (the caller
/// fixes the order). A newline is ensured after each module so the IIFEs can
/// never glue together into one expression.
pub fn assemble_source(prefix: &str, modules: &[(&str, String)]) -> String {
    let mut source = String::from(prefix);
    for (_name, content) in modules {
        source.push_str(content);
        if !content.ends_with('\n') {
            source.push('\n');
        }
    }
    source
}

/// The inject modules embedded at compile time, in the contract order.
pub fn embedded_inject_modules() -> Vec<(&'static str, String)> {
    INJECT_MODULES
        .iter()
        .zip(EMBEDDED_INJECT.iter())
        .map(|(name, source)| (*name, (*source).to_owned()))
        .collect()
}

/// Read the inject modules from `dir` in the contract order.
///
/// A missing module aborts with `missing inject module: <name>` — the spoof
/// layer is all-or-nothing by design: a partial layer leaks real values.
pub async fn read_inject_modules(dir: &Path) -> Result<Vec<(&'static str, String)>> {
    let mut modules = Vec::with_capacity(INJECT_MODULES.len());
    for name in INJECT_MODULES {
        let path = dir.join(format!("{name}.js"));
        match fs::read_to_string(&path).await {
            Ok(content) => modules.push((name, content)),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(anyhow!(
                    "missing inject module: {name} (expected at {})",
                    path.display()
                ));
            }
            Err(error) => {
                return Err(anyhow!("failed to read inject module {name}: {error}"));
            }
        }
    }
    Ok(modules)
}

/// The full injected source: fingerprint header + every inject module,
/// concatenated in the contract order.
///
/// The module bodies come from the compile-time embedded copy unless
/// [`INJECT_DIR_ENV`] points at a directory, in which case they are read from
/// disk (development mode).
pub async fn build_inject_source(fp: &Fingerprint) -> Result<String> {
    let override_dir = inject_dir_override();
    build_inject_source_from(fp, override_dir.as_deref()).await
}

/// [`build_inject_source`] with an explicit override directory — `Some(dir)`
/// reads modules from disk, `None` uses the embedded layer.
///
/// Split out so tests can exercise both paths without mutating the process
/// environment (which is global and racy across parallel tests).
pub async fn build_inject_source_from(
    fp: &Fingerprint,
    override_dir: Option<&Path>,
) -> Result<String> {
    let prefix = fingerprint_prefix(fp)?;
    let modules = match override_dir {
        Some(dir) => read_inject_modules(dir).await?,
        None => embedded_inject_modules(),
    };
    Ok(assemble_source(&prefix, &modules))
}

/// `self.__MIRAGE_FP__ = <fingerprint JSON>;\n` — the header the worker snippet
/// reads its config from. Worker scope has no `window`; the global is `self`.
pub fn worker_fingerprint_prefix(fp: &Fingerprint) -> Result<String> {
    let json = serde_json::to_string(fp).context("failed to serialize fingerprint")?;
    Ok(format!("self.__MIRAGE_FP__ = {json};\n"))
}

/// The worker snippet embedded at compile time ([`EMBEDDED_WORKER_SNIPPET`]).
pub fn embedded_worker_snippet() -> &'static str {
    EMBEDDED_WORKER_SNIPPET
}

/// Read the worker snippet from `dir/worker.js`.
///
/// A missing file aborts with `missing worker snippet: worker.js` for the same
/// all-or-nothing reason as the page modules: a worker spoofed with a partial
/// layer leaks real values.
async fn read_worker_snippet(dir: &Path) -> Result<String> {
    let path = dir.join("worker.js");
    match fs::read_to_string(&path).await {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(anyhow!(
            "missing worker snippet: worker.js (expected at {})",
            path.display()
        )),
        Err(error) => Err(anyhow!("failed to read worker snippet: {error}")),
    }
}

/// The full worker-injected source: `self.__MIRAGE_FP__ = ...;` + worker.js.
///
/// The snippet comes from the compile-time embedded copy unless
/// [`INJECT_DIR_ENV`] points at a directory (development mode). This is NOT the
/// page bundle: the launcher sends it as a `Runtime.evaluate` source to each
/// worker-scope CDP target.
pub async fn build_worker_inject_source(fp: &Fingerprint) -> Result<String> {
    let override_dir = inject_dir_override();
    build_worker_inject_source_from(fp, override_dir.as_deref()).await
}

/// [`build_worker_inject_source`] with an explicit override directory —
/// `Some(dir)` reads `dir/worker.js` from disk, `None` uses the embedded copy.
pub async fn build_worker_inject_source_from(
    fp: &Fingerprint,
    override_dir: Option<&Path>,
) -> Result<String> {
    let prefix = worker_fingerprint_prefix(fp)?;
    let snippet = match override_dir {
        Some(dir) => read_worker_snippet(dir).await?,
        None => EMBEDDED_WORKER_SNIPPET.to_owned(),
    };
    Ok(assemble_source(&prefix, &[("worker", snippet)]))
}

/// One entry of `brands` / `fullVersionList`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CdpBrand<'a> {
    brand: &'a str,
    version: String,
}

/// `userAgentMetadata` of Network.setUserAgentOverride (contract field order).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CdpUserAgentMetadata<'a> {
    brands: Vec<CdpBrand<'a>>,
    full_version_list: Vec<CdpBrand<'a>>,
    full_version: String,
    platform: &'a str,
    platform_version: String,
    architecture: String,
    model: &'a str,
    mobile: bool,
    bitness: String,
    wow64: bool,
}

/// Network.setUserAgentOverride params (contract field order).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CdpUserAgentOverride<'a> {
    user_agent: &'a str,
    accept_language: String,
    platform: &'a str,
    user_agent_metadata: CdpUserAgentMetadata<'a>,
}

/// Params for `Network.setUserAgentOverride`, exactly per CONTRACT.md:
/// `userAgent`, `acceptLanguage` (`languages.join(",")`), `platform`, and
/// `userAgentMetadata` built from the fingerprint's client hints.
///
/// `fullVersionList` is the same brands (same order — the fingerprint is
/// the single source of truth for it) with full versions substituted per
/// the live-Chrome calibration: the GREASE brand gets `GREASE_FULL_VERSION`
/// (`"8.0.0.0"` for the Chrome-156 grease cycle), every other brand gets
/// `uaFullVersion`.
pub fn build_user_agent_override(fp: &Fingerprint) -> Result<Value> {
    let hints = &fp.client_hints;
    let full_version = hints.ua_full_version.clone();
    // fullVersionList = the same brands with the full version substituted;
    // the GREASE brand's full version follows the grease cycle, not the
    // Chrome version.
    let full_version_list = hints
        .brands
        .iter()
        .map(|brand| {
            let version = if brand.brand == GREASE_BRAND {
                GREASE_FULL_VERSION.to_string()
            } else {
                full_version.clone()
            };
            CdpBrand {
                brand: &brand.brand,
                version,
            }
        })
        .collect();
    let brands = hints
        .brands
        .iter()
        .map(|brand| CdpBrand {
            brand: &brand.brand,
            version: brand.version.clone(),
        })
        .collect();
    let params = CdpUserAgentOverride {
        user_agent: &fp.ua,
        accept_language: fp.languages.join(","),
        platform: &fp.platform,
        user_agent_metadata: CdpUserAgentMetadata {
            brands,
            full_version_list,
            full_version,
            platform: &hints.platform,
            platform_version: hints.platform_version.clone(),
            architecture: hints.architecture.clone(),
            model: "",
            mobile: hints.mobile,
            bitness: hints.bitness.clone(),
            wow64: false,
        },
    };
    serde_json::to_value(params).context("failed to serialize Network.setUserAgentOverride params")
}

/// Params for `Emulation.setTimezoneOverride`: `{ "timezoneId": ... }`.
pub fn build_timezone_override(fp: &Fingerprint) -> Value {
    json!({ "timezoneId": fp.timezone.clone() })
}

/// Params for `Emulation.setLocaleOverride`: `{ "locale": languages[0] }`.
pub fn build_locale_override(fp: &Fingerprint) -> Value {
    json!({ "locale": locale_for(fp) })
}

/// Params for `Emulation.setDeviceMetricsOverride`: native screen emulation.
///
/// `width`/`height` are the LEGEND screen size, not `0`: leaving them at `0`
/// kept the host monitor geometry, so CSS media queries (`(min-width: ...)`,
/// `resolution`, `(min-device-pixel-ratio)`) answered with the real
/// 1920x1080 while the JS layer's `screen.width`/`screen.height` reported the
/// legend's 2560x1440 — a main-vs-CSS mismatch.
///
/// `screenWidth`/`screenHeight` mirror the legend too: the viewport-only
/// override left `(device-width)` / `(device-height)` reporting the host
/// monitor, which creepjs surfaces as `screen query`. `deviceScaleFactor`
/// mirrors the legend's `screen.dpr` so `(min-device-pixel-ratio)` agrees with
/// the JS-layer `window.devicePixelRatio` getter — media queries cannot be
/// patched from JS.
pub fn build_device_metrics_override(fp: &Fingerprint) -> Value {
    json!({
        "width": fp.screen.width,
        "height": fp.screen.height,
        "screenWidth": fp.screen.width,
        "screenHeight": fp.screen.height,
        "deviceScaleFactor": fp.screen.dpr,
        "mobile": false,
    })
}

/// Params for `Page.addScriptToEvaluateOnNewDocument`: `{ "source": ... }`.
pub fn build_add_script_command(source: &str) -> Value {
    json!({ "source": source })
}

/// `languages[0]` with a defensive fallback: the core validator guarantees
/// non-empty languages, hand-built profiles may not.
fn locale_for(fp: &Fingerprint) -> String {
    fp.languages
        .first()
        .cloned()
        .unwrap_or_else(|| String::from("en-US"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_debug_port_returns_a_concrete_port() {
        // The whole point of port-0 semantics: the resolver must never hand
        // back 0 (which would silently fall back to Chrome's own random-port
        // mode that the launcher cannot talk to).
        let port = free_debug_port().expect("pick a free debug port");
        assert_ne!(
            port, 0,
            "free_debug_port must resolve port 0 to a real port"
        );
    }

    #[test]
    fn free_debug_port_is_usable_repeatedly() {
        // Two consecutive picks must both succeed and both be concrete (the
        // ports may legitimately be equal: the first listener is dropped
        // before the second bind, so the OS can reuse it).
        for _ in 0..2 {
            let port = free_debug_port().expect("pick a free debug port");
            assert_ne!(port, 0);
        }
    }

    #[test]
    fn device_metrics_override_uses_the_legend_screen_size() {
        let fp = test_fingerprint();
        let params = build_device_metrics_override(&fp);
        // Screen emulation: the legend's own width/height for the viewport AND
        // the emulated screen, the legend's dpr, desktop mode.
        assert_eq!(
            serde_json::to_string(&params).expect("serialize params"),
            r#"{"deviceScaleFactor":1.25,"height":1080,"mobile":false,"screenHeight":1080,"screenWidth":1920,"width":1920}"#
        );
    }

    #[test]
    fn worker_prefix_reads_the_worker_global() {
        let fp = test_fingerprint();
        let prefix = worker_fingerprint_prefix(&fp).expect("serialize prefix");
        assert!(
            prefix.starts_with("self.__MIRAGE_FP__ = {"),
            "worker prefix must define the worker-scope global: {prefix}"
        );
        assert!(prefix.ends_with("};\n"));
        // It must never touch `window`: worker scope has none.
        assert!(!prefix.contains("window."));
    }

    #[test]
    fn embedded_worker_snippet_is_not_a_page_module() {
        // worker.js is deliberately excluded from INJECT_MODULES (the page
        // bundle must stay window-only), yet it is embedded for worker targets.
        assert!(
            !INJECT_MODULES.contains(&"worker"),
            "worker.js must NOT be in the page module list"
        );
        let snippet = embedded_worker_snippet();
        assert!(!snippet.trim().is_empty(), "embedded worker.js is empty");
        assert!(
            !snippet.contains("window."),
            "worker snippet must be window-free"
        );
    }

    #[test]
    fn mark_and_unmark_session_is_idempotent() {
        let done: SpoofedSessions = Arc::new(StdMutex::new(HashSet::new()));
        assert!(mark_session(&done, "SID"), "first mark inserts");
        assert!(!mark_session(&done, "SID"), "second mark is rejected");
        unmark_session(&done, "SID");
        assert!(
            mark_session(&done, "SID"),
            "after unmark it can insert again"
        );
    }

    #[test]
    fn benign_and_gone_errors_are_classified() {
        let locale = anyhow!(
            "CDP error on Emulation.setLocaleOverride: \
             Another locale override is already in effect"
        );
        assert!(is_benign(&locale));
        assert!(!is_gone(&locale));

        let gone = anyhow!(
            "CDP error on Runtime.runIfWaitingForDebugger: \
             Session with given id not found."
        );
        assert!(is_gone(&gone));
        // A vanished session is also "benign" for spoofing purposes.
        assert!(is_benign(&gone));

        let real = anyhow!("CDP error on Network.enable: Target closed");
        assert!(!is_benign(&real));
        assert!(!is_gone(&real));
    }

    #[test]
    fn spoofable_worker_url_excludes_browser_components() {
        assert!(is_spoofable_worker_url("blob:https://site.test/uuid"));
        assert!(is_spoofable_worker_url("https://site.test/sw.js"));
        assert!(is_spoofable_worker_url("http://127.0.0.1:1/w.js"));
        assert!(!is_spoofable_worker_url(
            "chrome-extension://abc/service_worker.js"
        ));
        assert!(!is_spoofable_worker_url("chrome://omnibox/"));
        assert!(!is_spoofable_worker_url("devtools://devtools/x"));
    }

    #[test]
    fn full_version_list_uses_grease_full_version_for_the_grease_brand_only() {
        let fp = test_fingerprint();
        let params = build_user_agent_override(&fp).expect("build UA override");
        let list = params["userAgentMetadata"]["fullVersionList"]
            .as_array()
            .expect("fullVersionList is an array");
        // Fixture brands: GREASE, Chromium, Google Chrome (calibration order).
        assert_eq!(list[0]["brand"], serde_json::json!(GREASE_BRAND));
        assert_eq!(list[0]["version"], serde_json::json!(GREASE_FULL_VERSION));
        assert_eq!(list[1]["version"], serde_json::json!("156.0.8078.80"));
        assert_eq!(list[2]["version"], serde_json::json!("156.0.8078.80"));
        // The short brands keep the fingerprint's own short versions.
        let brands = params["userAgentMetadata"]["brands"]
            .as_array()
            .expect("brands is an array");
        assert_eq!(brands[0]["brand"], serde_json::json!(GREASE_BRAND));
        assert_eq!(brands[0]["version"], serde_json::json!("8"));
    }

    #[test]
    fn attached_target_parses_page_and_waiting_targets() {
        let event = json!({
            "method": "Target.attachedToTarget",
            "params": {
                "sessionId": "SID",
                "targetInfo": { "targetId": "TID", "type": "page", "url": "https://a.test/" },
                "waitingForDebugger": true
            }
        });
        let target = attached_target(&event).expect("must parse");
        assert_eq!(target.target_id, "TID");
        assert_eq!(target.session_id, "SID");
        assert_eq!(target.target_type, "page");
        assert_eq!(target.url, "https://a.test/");
        assert!(target.waiting_for_debugger);
        assert!(!target.is_worker());

        // A non-attach event and a malformed attach both yield None.
        assert!(attached_target(&json!({ "method": "Target.targetCreated" })).is_none());
        assert!(
            attached_target(&json!({ "method": "Target.attachedToTarget", "params": {} }))
                .is_none()
        );
    }

    #[test]
    fn attached_target_classifies_worker_scope_targets() {
        // A page's dedicated blob worker: a spoofable web worker.
        let worker = attached_target(&json!({
            "method": "Target.attachedToTarget",
            "params": {
                "sessionId": "W",
                "targetInfo": { "targetId": "WID", "type": "worker", "url": "blob:https://a.test/u" }
            }
        }))
        .expect("parse worker");
        assert!(worker.is_worker());
        assert!(worker.is_web_worker());
        assert!(!worker.waiting_for_debugger);

        // A service worker from a web origin is spoofable too.
        let sw = attached_target(&json!({
            "method": "Target.attachedToTarget",
            "params": {
                "sessionId": "S",
                "targetInfo": { "targetId": "SID", "type": "service_worker", "url": "https://a.test/sw.js" }
            }
        }))
        .expect("parse sw");
        assert!(sw.is_worker());
        assert!(sw.is_web_worker());

        // A browser component (extension) is a worker target but NOT ours.
        let ext = attached_target(&json!({
            "method": "Target.attachedToTarget",
            "params": {
                "sessionId": "E",
                "targetInfo": { "targetId": "EID", "type": "service_worker",
                                "url": "chrome-extension://abc/sw.js" }
            }
        }))
        .expect("parse ext sw");
        assert!(ext.is_worker());
        assert!(!ext.is_web_worker());
    }

    /// Minimal hand-built fingerprint exercising the brand logic: the
    /// calibration brand order [GREASE, Chromium, Google Chrome] with the
    /// GREASE short version, plus a fractional dpr.
    fn test_fingerprint() -> Fingerprint {
        Fingerprint {
            os: mirage_core::Os::Windows,
            os_version: String::from("11"),
            platform: String::from("Win32"),
            ua: String::from("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36"),
            chrome_version: String::from("156.0.8078.80"),
            languages: vec![String::from("en-US"), String::from("en")],
            timezone: String::from("Europe/London"),
            geo: mirage_core::GeoFp { lat: 51.5074, lon: -0.1278 },
            screen: mirage_core::ScreenFp {
                width: 1920,
                height: 1080,
                avail_width: 1920,
                avail_height: 1040,
                dpr: 1.25,
                color_depth: 24,
            },
            hardware_concurrency: 8,
            device_memory: 8,
            webgl: mirage_core::WebglFp {
                vendor: String::from("Google Inc. (NVIDIA)"),
                renderer: String::from(
                    "ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER Direct3D11 vs_5_0 ps_5_0, D3D11)",
                ),
                max_texture_size: 16384,
                max_viewport_dims: [32767, 32767],
                max_renderbuffer_size: 16384,
            },
            canvas_seed: 1234567890,
            audio_seed: 987654321,
            fonts: vec![String::from("Arial"), String::from("Calibri")],
            web_rtc: String::from("real"),
            client_hints: mirage_core::ClientHintsFp {
                brands: vec![
                    mirage_core::Brand {
                        brand: GREASE_BRAND.to_string(),
                        version: String::from("8"),
                    },
                    mirage_core::Brand {
                        brand: String::from("Chromium"),
                        version: String::from("156"),
                    },
                    mirage_core::Brand {
                        brand: String::from("Google Chrome"),
                        version: String::from("156"),
                    },
                ],
                mobile: false,
                platform: String::from("Windows"),
                platform_version: String::from("15.0.0"),
                architecture: String::from("x86"),
                bitness: String::from("64"),
                ua_full_version: String::from("156.0.8078.80"),
            },
        }
    }
}
