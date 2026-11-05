//! `mirage launch` / `mirage stop` / `mirage status`.
//!
//! A launch runs in the foreground until Ctrl+C: the [`SessionManager`] owns
//! the browser and its `Drop` (or an explicit `stop_all`) tears it down. The
//! running descriptor written to [`crate::session_store`] lets later `status`
//! and `stop` runs, which are separate processes, act on the same sessions.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use chrono::Utc;
use mirage_launcher::{CdpClient, SessionManager};

use crate::context::Ctx;
use crate::error::CliError;
use crate::output::{render_table, to_json_value, Render};
use crate::session_store::{self, SessionRecord};

/// How often the foreground launcher checks whether the browser is still up.
const LIVENESS_POLL: Duration = Duration::from_secs(2);

/// How long a `stop` waits for a CDP attach to a recorded session.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a `stop` waits for the browser to acknowledge the close request,
/// and — independently — for the DevTools endpoint to stop answering.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the `stop` confirmation polls the DevTools endpoint.
const CLOSE_POLL: Duration = Duration::from_millis(100);

/// Addressing information of a freshly launched browser.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchInfo {
    /// Profile id.
    pub profile_id: String,
    /// Profile name.
    pub name: String,
    /// DevTools target id of the first spoofed page.
    pub target_id: String,
    /// Browser-level DevTools websocket URL.
    pub ws_url: String,
    /// DevTools TCP port.
    pub debug_port: u16,
}

impl LaunchInfo {
    /// Build from a stored descriptor.
    pub fn from_record(record: &SessionRecord) -> Self {
        LaunchInfo {
            profile_id: record.profile_id.clone(),
            name: record.name.clone(),
            target_id: record.target_id.clone(),
            ws_url: record.ws_url.clone(),
            debug_port: record.debug_port,
        }
    }
}

/// Result of a stop operation.
#[derive(Debug, Clone, Serialize)]
pub struct StopReport {
    /// Ids whose browser was confirmed closed (or which were already gone).
    pub stopped: Vec<String>,
    /// Ids whose browser could not be confirmed closed; their registry entry
    /// is kept so the session is not lost.
    pub failed: Vec<String>,
}

/// One row of `status`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    /// Profile id.
    pub profile_id: String,
    /// Profile name.
    pub name: String,
    /// OS pid of the browser process recorded at launch (`0` if unknown).
    pub pid: u32,
    /// DevTools TCP port.
    pub debug_port: u16,
    /// RFC 3339 start timestamp.
    pub started_at: String,
    /// Whether the DevTools endpoint currently answers with a valid payload.
    pub running: bool,
}

/// Result of `status`.
#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    /// Known sessions.
    pub sessions: Vec<SessionStatus>,
}

/// Which sessions a `stop` targets.
#[derive(Debug, Clone)]
pub enum StopTarget {
    /// One profile id.
    One(String),
    /// Every recorded session.
    All,
}

/// A browser held in the foreground by the current process.
pub struct LiveSession {
    /// Owns the browser; dropping it kills Chrome.
    pub manager: SessionManager,
    /// The descriptor written to the registry.
    pub record: SessionRecord,
}

/// Launch `id` and register it, without yet waiting for Ctrl+C.
///
/// Before spawning, this takes the per-profile launch lock and consults the
/// shared run registry, so a profile with a *live* browser — even one launched
/// by a different `mirage` process — is rejected with [`CliError::AlreadyRunning`]
/// instead of starting a second Chrome (review finding M4).
pub async fn launch_session(ctx: &Ctx, id: &str) -> Result<LiveSession, CliError> {
    let vault = ctx.open_vault()?;
    let profile = vault
        .load(id)
        .map_err(|error| CliError::vault_lookup(error, id))?;
    let name = profile.name.clone();

    // Hold this for the whole check-then-spawn window; it is released (file
    // removed) when this function returns, after the descriptor is written.
    let _guard = session_store::acquire_lock(ctx, id).await?;
    match session_store::load(ctx, id) {
        Ok(Some(existing)) if session_store::devtools_alive_async(existing.debug_port).await => {
            // A browser for this profile is already up somewhere.
            return Err(CliError::AlreadyRunning(id.to_string()));
        }
        Ok(Some(_stale)) => {
            // Descriptor from a browser that is gone: clear it before relaunch.
            session_store::remove(ctx, id)?;
        }
        Ok(None) => {}
        Err(CliError::SessionDescriptor { reason, .. }) => {
            // A truncated descriptor must not block a launch forever; report
            // it (rather than ignore it) and drop it now that we hold the lock.
            eprintln!("mirage: discarding corrupt session descriptor for {id}: {reason}");
            session_store::remove(ctx, id)?;
        }
        Err(error) => return Err(error),
    }

    let manager = SessionManager::new();
    let handle = manager
        .launch_profile(profile, ctx.profile_dir(id))
        .await
        .map_err(
            |error| match error.downcast_ref::<mirage_launcher::AlreadyRunning>() {
                Some(_) => CliError::AlreadyRunning(id.to_string()),
                None => CliError::Launch(format!("{error:#}")),
            },
        )?;

    let record = SessionRecord {
        profile_id: id.to_string(),
        name,
        // The browser's own pid, not this launcher's: it lets `status` show
        // which OS process backs the session (review finding M9).
        pid: handle.pid,
        target_id: handle.target_id.clone(),
        ws_url: handle.ws_url.clone(),
        debug_port: handle.debug_port,
        started_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    session_store::save(ctx, &record)?;

    Ok(LiveSession { manager, record })
}

/// Block until Ctrl+C (or until the browser disappears), then stop everything
/// and clear the registry entry.
pub async fn wait_and_stop(ctx: &Ctx, session: LiveSession) -> Result<StopReport, CliError> {
    let port = session.record.debug_port;
    let profile_id = session.record.profile_id.clone();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(LIVENESS_POLL) => {
                if !session_store::devtools_alive_async(port).await {
                    break;
                }
            }
        }
    }

    session
        .manager
        .stop_all()
        .await
        .map_err(|error| CliError::Launch(format!("{error:#}")))?;
    session_store::remove(ctx, &profile_id)?;

    Ok(StopReport {
        stopped: vec![profile_id],
        failed: Vec::new(),
    })
}

/// Stop the targeted sessions by asking each browser to close over CDP.
///
/// A registry entry is removed — and its id reported in `stopped` — *only*
/// once the shutdown is confirmed: `Browser.close` was acknowledged, or the
/// recorded DevTools endpoint actually stopped answering. A session that is
/// still reachable is left in the registry and reported in `failed`, so `stop`
/// never claims to have stopped a browser that is still running (M3).
pub async fn run_stop(ctx: &Ctx, target: &StopTarget) -> Result<StopReport, CliError> {
    let selected: Vec<SessionRecord> = match target {
        StopTarget::All => session_store::list(ctx)?,
        StopTarget::One(id) => match session_store::load(ctx, id)? {
            Some(record) => vec![record],
            None => return Err(CliError::SessionNotFound(id.clone())),
        },
    };

    let mut stopped = Vec::with_capacity(selected.len());
    let mut failed = Vec::new();
    for record in selected {
        if close_session(&record, ATTACH_TIMEOUT, CLOSE_TIMEOUT).await {
            session_store::remove(ctx, &record.profile_id)?;
            stopped.push(record.profile_id);
        } else {
            failed.push(record.profile_id);
        }
    }

    Ok(StopReport { stopped, failed })
}

/// Ask the browser behind `record` to close and confirm it is really gone.
///
/// Returns `true` when `Browser.close` was acknowledged, when the recorded
/// DevTools endpoint stopped answering within `close_timeout`, or when the
/// endpoint answers but is provably *not* our process (a recycled port). The
/// last case matters: it keeps `stop` from closing a stranger's browser, and it
/// is why the recorded pid is read, not just written (review finding M9).
async fn close_session(
    record: &SessionRecord,
    attach_timeout: Duration,
    close_timeout: Duration,
) -> bool {
    let Ok(mut client) = attach_with_timeout(&record.ws_url, attach_timeout).await else {
        // Could not attach: the endpoint disappearing is the only proof.
        return wait_until_dead(record.debug_port, close_timeout).await;
    };

    // The port speaks DevTools, but ownership decides whether it is ours.
    if owns_pid(&mut client, record.pid).await == Some(false) {
        // Our browser is gone and the port was recycled: the record is stale.
        return true;
    }

    let acknowledged = tokio::time::timeout(
        close_timeout,
        client.send("Browser.close", serde_json::json!({})),
    )
    .await
    .map(|result| result.is_ok())
    .unwrap_or(false);
    if acknowledged {
        return true;
    }
    wait_until_dead(record.debug_port, close_timeout).await
}

/// Whether the attached browser reports `pid` among its processes.
///
/// `Some(true)` — it is our browser; `Some(false)` — it answered but the pid is
/// absent (a stranger on a recycled port); `None` — the pid is unknown or the
/// probe is unavailable (older Chrome), so ownership cannot be decided.
async fn owns_pid(client: &mut CdpClient, pid: u32) -> Option<bool> {
    if pid == 0 {
        return None;
    }
    let info = client
        .send("SystemInfo.getProcessInfo", serde_json::json!({}))
        .await
        .ok()?;
    let present = info
        .get("processInfo")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.get("id").and_then(Value::as_u64) == Some(u64::from(pid)))
        });
    Some(present)
}

/// Attach a CDP client to `ws_url`, bounding the whole handshake.
///
/// Without this bound a wedged or merely-silent endpoint would hang `stop`
/// forever on a TCP connect / websocket handshake (review finding M9).
async fn attach_with_timeout(ws_url: &str, timeout: Duration) -> Result<CdpClient, CliError> {
    match tokio::time::timeout(timeout, CdpClient::attach(ws_url)).await {
        Ok(Ok(client)) => Ok(client),
        Ok(Err(error)) => Err(CliError::Launch(format!("{error:#}"))),
        Err(_) => Err(CliError::Message(format!(
            "CDP attach to {ws_url} timed out after {timeout:?}"
        ))),
    }
}

/// Wait until the DevTools endpoint on `port` stops answering, or `timeout`.
async fn wait_until_dead(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if !session_store::devtools_alive_async(port).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(CLOSE_POLL).await;
    }
}

/// List recorded sessions, each flagged with live/dead state.
pub async fn run_status(ctx: &Ctx) -> Result<StatusReport, CliError> {
    let records = session_store::list(ctx)?;
    let mut sessions = Vec::with_capacity(records.len());
    for record in records {
        sessions.push(SessionStatus {
            running: session_store::devtools_alive_async(record.debug_port).await,
            profile_id: record.profile_id,
            name: record.name,
            pid: record.pid,
            debug_port: record.debug_port,
            started_at: record.started_at,
        });
    }
    Ok(StatusReport { sessions })
}

impl Render for LaunchInfo {
    fn human(&self) -> String {
        format!(
            "launched {name} ({id})\n  debug port : {port}\n  ws url     : {ws}\n  target id  : {target}\n\nCtrl+C — остановить браузер",
            name = self.name,
            id = self.profile_id,
            port = self.debug_port,
            ws = self.ws_url,
            target = self.target_id,
        )
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

impl Render for StopReport {
    fn human(&self) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(self.stopped.len() + self.failed.len());
        for id in &self.stopped {
            lines.push(format!("stopped {id}"));
        }
        for id in &self.failed {
            lines.push(format!("failed to stop {id}"));
        }
        if lines.is_empty() {
            return String::from("nothing to stop");
        }
        lines.join("\n")
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

impl Render for StatusReport {
    fn human(&self) -> String {
        if self.sessions.is_empty() {
            return String::from("no running sessions");
        }
        let headers = ["PROFILE", "NAME", "PID", "DEBUG PORT", "STARTED", "STATE"];
        let rows = self
            .sessions
            .iter()
            .map(|session| {
                vec![
                    session.profile_id.clone(),
                    session.name.clone(),
                    session.pid.to_string(),
                    session.debug_port.to_string(),
                    session.started_at.clone(),
                    if session.running { "running" } else { "stale" }.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        render_table(&headers, &rows)
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use tempfile::TempDir;

    fn ctx(dir: &TempDir) -> Ctx {
        Ctx {
            data_dir: dir.path().to_path_buf(),
            password: Some("pw".into()),
            json: false,
        }
    }

    fn record(id: &str, ws_url: &str, port: u16) -> SessionRecord {
        SessionRecord {
            profile_id: id.into(),
            name: format!("profile {id}"),
            pid: 4242,
            target_id: "T".into(),
            ws_url: ws_url.into(),
            debug_port: port,
            started_at: "2026-11-05T00:00:00Z".into(),
        }
    }

    /// HTTP server answering every request with a valid `/json/version` body;
    /// returns its port. The thread is detached and dies with the test process.
    fn spawn_version_server() -> u16 {
        use std::io::{Read, Write};
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind version server");
        let port = listener.local_addr().expect("version server addr").port();
        let body =
            format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/x"}}"#);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    #[tokio::test]
    async fn status_without_registry_is_empty() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let report = run_status(&ctx).await.expect("status");
        assert!(report.sessions.is_empty());
    }

    #[tokio::test]
    async fn status_marks_dead_ports_as_stale() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        // Port 1 is privileged/closed: liveness must be false.
        session_store::save(
            &ctx,
            &record(
                "00000000-0000-4000-8000-000000000000",
                "ws://127.0.0.1:1/x",
                1,
            ),
        )
        .expect("save");
        let report = run_status(&ctx).await.expect("status");
        assert_eq!(report.sessions.len(), 1);
        assert!(!report.sessions[0].running);
        assert_eq!(report.sessions[0].pid, 4242);
        assert!(report.human().contains("stale"));
    }

    #[tokio::test]
    async fn stop_clears_a_session_whose_browser_is_gone() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        // Unreachable endpoint: the browser is already gone, so `stop` may
        // confirm death and remove the record.
        session_store::save(
            &ctx,
            &record(
                "11111111-1111-4111-8111-111111111111",
                "ws://127.0.0.1:1/x",
                1,
            ),
        )
        .expect("save");

        let report = run_stop(&ctx, &StopTarget::All).await.expect("stop");
        assert_eq!(report.stopped.len(), 1);
        assert!(report.failed.is_empty());
        assert!(session_store::list(&ctx).expect("list").is_empty());
    }

    #[tokio::test]
    async fn stop_unknown_id_is_not_found() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let error = run_stop(&ctx, &StopTarget::One("nope".into()))
            .await
            .expect_err("unknown id");
        assert!(matches!(error, CliError::SessionNotFound(_)));
        assert_eq!(error.exit_code(), crate::error::exit::NOT_FOUND);
    }

    #[tokio::test]
    async fn stop_reports_a_corrupt_descriptor() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        std::fs::create_dir_all(ctx.run_dir()).expect("run dir");
        std::fs::write(ctx.run_dir().join("bad.json"), "{ not json").expect("write bad");
        let error = run_stop(&ctx, &StopTarget::One("bad".into()))
            .await
            .expect_err("corrupt descriptor");
        assert!(matches!(error, CliError::SessionDescriptor { .. }));
    }

    #[tokio::test]
    async fn close_session_is_unconfirmed_while_the_browser_still_answers() {
        // Live DevTools endpoint, but the close cannot reach it: the session
        // must NOT be reported stopped.
        let port = spawn_version_server();
        let rec = record("live", "ws://127.0.0.1:1/not-devtools", port);
        let closed =
            close_session(&rec, Duration::from_millis(200), Duration::from_millis(400)).await;
        assert!(
            !closed,
            "a browser still serving DevTools must not count as closed"
        );
    }

    #[tokio::test]
    async fn close_session_confirms_when_the_endpoint_is_gone() {
        let rec = record("dead", "ws://127.0.0.1:1/x", 1);
        let closed =
            close_session(&rec, Duration::from_millis(200), Duration::from_millis(400)).await;
        assert!(closed);
    }

    #[tokio::test]
    async fn attach_times_out_on_a_silent_endpoint() {
        // A listener that accepts but never completes the websocket handshake.
        let silent = TcpListener::bind(("127.0.0.1", 0)).expect("bind silent");
        let port = silent.local_addr().expect("silent addr").port();
        let started = tokio::time::Instant::now();
        let result = attach_with_timeout(
            &format!("ws://127.0.0.1:{port}/x"),
            Duration::from_millis(300),
        )
        .await;
        assert!(result.is_err(), "attach to a silent endpoint must fail");
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "attach must honor its timeout"
        );
        drop(silent);
    }

    #[tokio::test]
    async fn launch_rejects_a_profile_with_a_live_registered_session() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);

        // A real profile in the vault.
        let fingerprint = mirage_core::generate(7, Some(mirage_core::Os::Windows)).expect("fp");
        let profile = mirage_core::Profile::new("Live", fingerprint, None);
        let id = profile.id.clone();
        ctx.open_vault()
            .expect("vault")
            .save(&profile)
            .expect("save profile");

        // A descriptor pointing at a live DevTools endpoint.
        let port = spawn_version_server();
        session_store::save(
            &ctx,
            &record(
                &id,
                &format!("ws://127.0.0.1:{port}/devtools/browser/x"),
                port,
            ),
        )
        .expect("save record");

        // The store guard must fire before any Chrome is spawned.
        match launch_session(&ctx, &id).await {
            Err(CliError::AlreadyRunning(got)) => assert_eq!(got, id),
            Ok(_) => panic!("expected AlreadyRunning"),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn launch_missing_profile_is_not_found() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        // Debug isn't available for the Ok type (it owns a SessionManager), so
        // match instead of `expect_err`.
        match launch_session(&ctx, "00000000-0000-4000-8000-000000000000").await {
            Err(CliError::ProfileNotFound(_)) => {}
            Ok(_) => panic!("expected a not-found error"),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
