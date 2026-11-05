//! Cross-process running-session registry.
//!
//! `mirage launch` holds the browser in the foreground; `mirage status` and
//! `mirage stop` run in *other* processes, so the in-memory
//! [`mirage_launcher::SessionManager`] cannot be reached from them. A launcher
//! therefore records a plaintext descriptor at
//! `<data-dir>/run/<profile-id>.json`, and `status`/`stop` read that file:
//!
//! * liveness is probed with an HTTP `GET /json/version` and requires a valid
//!   DevTools JSON body carrying `webSocketDebuggerUrl` — a naked TCP connect
//!   would only prove *some* process holds the port, not that it is our browser
//!   (review finding M9);
//! * `launch` takes a per-profile file lock and consults this registry before
//!   spawning, so two launchers cannot each start a Chrome for one profile
//!   (review finding M4);
//! * `stop` closes the browser over CDP and removes the descriptor *only* once
//!   the shutdown is confirmed (review finding M3).
//!
//! The descriptor holds no secrets (id, name, pid, DevTools port/ws url), so it
//! is stored unencrypted next to the vault.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::context::Ctx;
use crate::error::CliError;

/// How long a liveness probe (`GET /json/version`) may take.
const LIVENESS_TIMEOUT: Duration = Duration::from_millis(1500);

/// How long a launcher may hold the per-profile lock before a later launcher
/// treats a leftover lock as abandoned (its holder died mid-spawn). A real
/// launch finishes well under this: `mirage-launcher`'s own DevTools and page
/// budgets cap the spawn at roughly 25 seconds.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(60);

/// One running (or stale) launcher descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    /// Profile id this session belongs to.
    pub profile_id: String,
    /// Profile display name (copied for `status` convenience).
    pub name: String,
    /// OS pid of the *browser* process (from the launcher's session handle).
    pub pid: u32,
    /// DevTools target id of the first spoofed page.
    pub target_id: String,
    /// Browser-level DevTools websocket URL.
    pub ws_url: String,
    /// DevTools TCP port.
    pub debug_port: u16,
    /// RFC 3339 UTC start timestamp.
    pub started_at: String,
}

/// Persist a session descriptor, overwriting any previous one for the id.
pub fn save(ctx: &Ctx, record: &SessionRecord) -> Result<(), CliError> {
    let dir = ctx.run_dir();
    std::fs::create_dir_all(&dir)?;
    let path = record_path(ctx, &record.profile_id);
    let bytes = serde_json::to_vec_pretty(record)?;
    std::fs::write(&path, bytes)?;
    Ok(())
}

/// Read the descriptor for `id`.
///
/// A missing file is [`Ok(None)`](Option); a file that exists but cannot be
/// parsed is a hard error ([`CliError::SessionDescriptor`]) rather than a
/// silent `None`, so a truncated descriptor is never mistaken for "not
/// launched" (review finding, minor).
pub fn load(ctx: &Ctx, id: &str) -> Result<Option<SessionRecord>, CliError> {
    let path = record_path(ctx, id);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::from_str(&text)
                .map(Some)
                .map_err(|error| CliError::SessionDescriptor {
                    path,
                    reason: error.to_string(),
                })
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CliError::Io(error)),
    }
}

/// Remove the descriptor for `id`. Missing files are not an error (idempotent).
pub fn remove(ctx: &Ctx, id: &str) -> Result<(), CliError> {
    let path = record_path(ctx, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::Io(error)),
    }
}

/// Path of the descriptor for `id` (`<run-dir>/<id>.json`).
fn record_path(ctx: &Ctx, id: &str) -> PathBuf {
    ctx.run_dir().join(format!("{id}.json"))
}

/// Path of the per-profile launch lock (`<run-dir>/<id>.lock`).
fn lock_path(ctx: &Ctx, id: &str) -> PathBuf {
    ctx.run_dir().join(format!("{id}.lock"))
}

/// Every well-formed descriptor in the registry, sorted by profile id.
///
/// Unreadable or corrupt entries are skipped rather than failing the listing.
pub fn list(ctx: &Ctx) -> Result<Vec<SessionRecord>, CliError> {
    let dir = ctx.run_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(CliError::Io(error)),
    };

    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(record) = serde_json::from_str::<SessionRecord>(&text) {
                records.push(record);
            }
        }
    }
    records.sort_by(|a, b| a.profile_id.cmp(&b.profile_id));
    Ok(records)
}

/// Whether the recorded DevTools endpoint is answering on `port`.
///
/// Liveness is an HTTP `GET /json/version` that must return a valid JSON body
/// carrying `webSocketDebuggerUrl`. A bare TCP connect would report *any*
/// process holding the port as "our session" (review finding M9); requiring the
/// DevTools payload proves the port belongs to a browser speaking CDP.
pub async fn devtools_alive_async(port: u16) -> bool {
    let probe = mirage_launcher::cdp::get_ws_url(port);
    matches!(
        tokio::time::timeout(LIVENESS_TIMEOUT, probe).await,
        Ok(Ok(_))
    )
}

/// Cross-process launch lock for one profile.
///
/// [`acquire_lock`] creates `<run-dir>/<id>.lock` atomically; the guard removes
/// it on drop (only while it still holds our pid, so a lock broken as stale by
/// another launcher is not deleted by its original owner).
#[derive(Debug)]
pub struct SessionLock {
    path: PathBuf,
    owner: String,
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let still_ours = std::fs::read_to_string(&self.path)
            .map(|content| content.trim() == self.owner)
            .unwrap_or(false);
        if still_ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Acquire the per-profile launch lock, or fail if another launch holds it.
///
/// The lock closes the check-then-spawn race in `mirage launch`: two processes
/// racing on the same profile cannot both pass the registry check and spawn a
/// Chrome (review finding M4). A lock older than [`LOCK_STALE_AFTER`] whose
/// process died mid-spawn is reclaimed.
pub async fn acquire_lock(ctx: &Ctx, id: &str) -> Result<SessionLock, CliError> {
    let dir = ctx.run_dir();
    std::fs::create_dir_all(&dir)?;
    let path = lock_path(ctx, id);

    if lock_is_stale(&path) {
        // Leftover from a launcher that died before releasing it: reclaim.
        let _ = std::fs::remove_file(&path);
    }

    let owner = std::process::id().to_string();
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            // Best effort: a stray lock names the launcher pid that made it.
            use std::io::Write as _;
            let _ = writeln!(file, "{owner}");
            Ok(SessionLock { path, owner })
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Err(CliError::AlreadyRunning(id.to_string()))
        }
        Err(error) => Err(CliError::Io(error)),
    }
}

/// Whether a lock file is old enough to be considered abandoned.
///
/// A missing file, an unreadable mtime or a future mtime (clock skew) is *not*
/// stale: only a genuinely old, unreleased lock is broken.
fn lock_is_stale(path: &Path) -> bool {
    match std::fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .map(|age| age >= LOCK_STALE_AFTER)
            .unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ctx(dir: &TempDir) -> Ctx {
        Ctx {
            data_dir: dir.path().to_path_buf(),
            password: Some("pw".into()),
            json: false,
        }
    }

    fn record(id: &str) -> SessionRecord {
        SessionRecord {
            profile_id: id.into(),
            name: format!("profile {id}"),
            pid: 4242,
            target_id: "T".into(),
            ws_url: "ws://127.0.0.1:1/devtools/browser/x".into(),
            debug_port: 1,
            started_at: "2026-11-05T00:00:00Z".into(),
        }
    }

    #[test]
    fn save_load_list_remove_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);

        assert!(list(&ctx).expect("empty list").is_empty());
        assert_eq!(load(&ctx, "a").expect("missing load"), None);

        save(&ctx, &record("b")).expect("save b");
        save(&ctx, &record("a")).expect("save a");

        let ids: Vec<String> = list(&ctx)
            .expect("list")
            .into_iter()
            .map(|r| r.profile_id)
            .collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(
            load(&ctx, "a").expect("load a").map(|r| r.name),
            Some("profile a".into())
        );

        remove(&ctx, "a").expect("remove a");
        remove(&ctx, "a").expect("remove a is idempotent");
        assert_eq!(list(&ctx).expect("list").len(), 1);
    }

    #[test]
    fn corrupt_entries_are_skipped() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        std::fs::create_dir_all(ctx.run_dir()).expect("run dir");
        std::fs::write(ctx.run_dir().join("bad.json"), "{ not json").expect("write bad");
        std::fs::write(ctx.run_dir().join("ignore.txt"), "x").expect("write txt");
        assert!(list(&ctx).expect("list").is_empty());
    }

    #[test]
    fn load_reports_corrupt_descriptors() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        std::fs::create_dir_all(ctx.run_dir()).expect("run dir");
        std::fs::write(ctx.run_dir().join("bad.json"), "{ not json").expect("write bad");

        // A corrupt descriptor is a hard error, never a silent "not launched".
        let error = load(&ctx, "bad").expect_err("corrupt must error");
        assert!(matches!(error, CliError::SessionDescriptor { .. }));
        assert_eq!(error.exit_code(), crate::error::exit::GENERAL);
    }

    /// Tiny HTTP server handing `body` to every request; returns its port.
    fn spawn_http_server(body: String) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
        let port = listener.local_addr().expect("mock addr").port();
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
                let _ = stream.flush();
            }
        });
        port
    }

    #[tokio::test]
    async fn devtools_liveness_requires_a_version_payload() {
        // The mock answers with an empty body: it is not a DevTools payload.
        let port = spawn_http_server(String::new());
        assert!(!devtools_alive_async(port).await);

        // A port nobody listens on is dead.
        assert!(!devtools_alive_async(1).await);
    }

    #[tokio::test]
    async fn devtools_liveness_accepts_a_valid_version_body() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let body =
            format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/x"}}"#);
        std::thread::spawn(move || {
            use std::io::{Read, Write};
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
        assert!(devtools_alive_async(port).await);
    }

    #[tokio::test]
    async fn launch_lock_is_exclusive_and_released_on_drop() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);

        let guard = acquire_lock(&ctx, "a").await.expect("first lock");
        let conflict = acquire_lock(&ctx, "a").await.expect_err("second lock");
        assert!(matches!(conflict, CliError::AlreadyRunning(_)));
        assert_eq!(conflict.exit_code(), crate::error::exit::CONFLICT);

        // A different profile has its own lock.
        let other = acquire_lock(&ctx, "b").await.expect("other lock");
        drop(other);

        drop(guard);
        // Released on drop: the id can be locked again.
        let again = acquire_lock(&ctx, "a").await.expect("re-lock");
        drop(again);
    }

    #[tokio::test]
    async fn stale_lock_is_reclaimed() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);

        let guard = acquire_lock(&ctx, "a").await.expect("lock");
        let path = lock_path(&ctx, "a");
        // Age the lock past the threshold without releasing it.
        let old = SystemTime::now() - LOCK_STALE_AFTER - Duration::from_secs(5);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open lock");
        file.set_modified(old).expect("age lock");
        drop(file);

        let reclaimed = acquire_lock(&ctx, "a").await.expect("reclaim stale lock");
        drop(reclaimed);
        // The original guard no longer owns the file: its drop is a no-op.
        drop(guard);
        assert!(std::fs::metadata(&path).is_err(), "lock file must be gone");
    }
}
