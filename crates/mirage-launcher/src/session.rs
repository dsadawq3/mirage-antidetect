//! Profile session registry: launch profiles under one manager, track what is
//! running, and stop them cleanly.
//!
//! A [`SessionManager`] owns at most one [`LaunchedBrowser`] per `profile_id`.
//! Launching an id that is already running is rejected with [`AlreadyRunning`],
//! which keeps a profile's fingerprint from being served by two live browsers
//! at once (a cross-contamination foot-gun).
//!
//! The manager is cheap to share: it is `Send + Sync` and every method takes
//! `&self`. The bookkeeping lives in [`Registry`], a plain data structure that
//! is unit-tested without spawning a browser.

use crate::args::LaunchConfig;
use crate::launch::{launch, LaunchedBrowser};
use anyhow::{Context, Result};
use mirage_core::Profile;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::sync::Mutex;

/// Error returned by [`SessionManager::launch_profile`] when `profile_id` is
/// already running. Carries the offending id and implements [`std::error::Error`],
/// so callers can recover it from an `anyhow::Error` via `downcast_ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlreadyRunning {
    /// The profile id that was already present in the registry.
    pub profile_id: String,
}

impl fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "profile {} is already running", self.profile_id)
    }
}

impl std::error::Error for AlreadyRunning {}

/// A snapshot of one running session's addressing information.
#[derive(Debug, Clone)]
pub struct RunningSession {
    /// Profile id this session belongs to.
    pub profile_id: String,
    /// DevTools target id of the first spoofed page.
    pub target_id: String,
    /// Browser-level websocket debugger URL of the session.
    pub ws_url: String,
    /// DevTools TCP port of the session.
    pub debug_port: u16,
    /// When the session was launched.
    pub started_at: SystemTime,
}

/// The value returned by a successful [`SessionManager::launch_profile`]:
/// enough to address the new session, without exposing the process handle.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    /// Profile id this handle refers to (its registry key).
    pub profile_id: String,
    /// DevTools target id of the first spoofed page.
    pub target_id: String,
    /// Browser-level websocket debugger URL.
    pub ws_url: String,
    /// DevTools TCP port.
    pub debug_port: u16,
    /// OS pid of the spawned browser process (`0` if it could not be read).
    ///
    /// Exposed so a caller that keeps its own cross-process registry (the
    /// `mirage-cli` run descriptors) can record the *browser's* pid rather than
    /// its own; the handle is the only place the launcher otherwise hides it.
    pub pid: u32,
}

/// Pure bookkeeping for the set of running sessions.
///
/// Kept separate from the process handles so the registry rules (no duplicate
/// ids, insertion-order listing, clean removal) are testable without a browser.
#[derive(Debug, Default)]
struct Registry {
    order: Vec<String>,
    by_id: HashMap<String, RunningSession>,
}

impl Registry {
    /// Add `session`, or fail with [`AlreadyRunning`] if its id is taken.
    fn insert(&mut self, session: RunningSession) -> std::result::Result<(), AlreadyRunning> {
        if self.by_id.contains_key(&session.profile_id) {
            return Err(AlreadyRunning {
                profile_id: session.profile_id,
            });
        }
        self.order.push(session.profile_id.clone());
        self.by_id.insert(session.profile_id.clone(), session);
        Ok(())
    }

    /// Remove and return the session for `profile_id`, if present.
    fn remove(&mut self, profile_id: &str) -> Option<RunningSession> {
        let removed = self.by_id.remove(profile_id)?;
        self.order.retain(|id| id != profile_id);
        Some(removed)
    }

    fn contains(&self, profile_id: &str) -> bool {
        self.by_id.contains_key(profile_id)
    }

    /// All sessions in insertion order.
    fn list(&self) -> Vec<RunningSession> {
        self.order
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect()
    }

    /// Remove every session, returning them in insertion order.
    fn drain(&mut self) -> Vec<RunningSession> {
        let sessions = self.list();
        self.order.clear();
        self.by_id.clear();
        sessions
    }
}

/// Registry + live browser handles, guarded by one mutex.
#[derive(Default)]
struct Inner {
    registry: Registry,
    browsers: HashMap<String, LaunchedBrowser>,
}

/// Owns running profile sessions. See the module docs.
pub struct SessionManager {
    inner: Mutex<Inner>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Launch `profile` (its user-data-dir is `profile_dir`) and register it.
    ///
    /// The debug port is chosen automatically (`0` = pick a free port). The
    /// internal lock is held across the launch so two concurrent calls with the
    /// same `profile_id` cannot both spawn: the second sees the first and fails
    /// with [`AlreadyRunning`]. Launches of different profiles are therefore
    /// serialized — acceptable for a launcher, and the price of a race-free
    /// duplicate check.
    pub async fn launch_profile(
        &self,
        profile: Profile,
        profile_dir: PathBuf,
    ) -> Result<SessionHandle> {
        let profile_id = profile.id.clone();
        let mut inner = self.inner.lock().await;
        if inner.registry.contains(&profile_id) {
            return Err(anyhow::Error::new(AlreadyRunning { profile_id }));
        }

        let cfg = LaunchConfig {
            profile,
            chrome_path: None,
            profile_dir,
            debug_port: 0,
        };
        let browser = launch(&cfg)
            .await
            .with_context(|| format!("failed to launch profile {profile_id}"))?;

        let session = RunningSession {
            profile_id: profile_id.clone(),
            target_id: browser.target_id.clone(),
            ws_url: browser.ws_url.clone(),
            debug_port: browser.debug_port,
            started_at: SystemTime::now(),
        };
        // Cannot conflict: the id was verified absent under this lock.
        inner.registry.insert(session).map_err(anyhow::Error::new)?;

        // tokio reports `None` only once the child has been reaped; the process
        // is live here (the setup above used its pid), so this is `Some` in
        // practice. Fall back to `0` for a defensive total.
        let pid = browser.child.id().unwrap_or(0);
        let handle = SessionHandle {
            profile_id: profile_id.clone(),
            target_id: browser.target_id.clone(),
            ws_url: browser.ws_url.clone(),
            debug_port: browser.debug_port,
            pid,
        };
        inner.browsers.insert(profile_id, browser);
        Ok(handle)
    }

    /// Stop the session for `profile_id`. Stopping an id that is not running is
    /// a no-op success (idempotent).
    pub async fn stop(&self, profile_id: &str) -> Result<()> {
        let browser = {
            let mut inner = self.inner.lock().await;
            inner.registry.remove(profile_id);
            inner.browsers.remove(profile_id)
        };
        match browser {
            Some(mut browser) => browser.stop().await,
            None => Ok(()),
        }
    }

    /// Stop every running session. All sessions are stopped even if one fails;
    /// the first error is returned.
    pub async fn stop_all(&self) -> Result<()> {
        let browsers = {
            let mut inner = self.inner.lock().await;
            inner.registry.drain();
            std::mem::take(&mut inner.browsers)
        };
        let mut first_error: Option<anyhow::Error> = None;
        for (_profile_id, mut browser) in browsers {
            if let Err(error) = browser.stop().await {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Snapshot of the running sessions, in launch order.
    pub async fn running(&self) -> Vec<RunningSession> {
        self.inner.lock().await.registry.list()
    }
}

impl Drop for SessionManager {
    /// Synchronous safety net: clearing the maps drops every [`LaunchedBrowser`],
    /// whose own `Drop` fires a `start_kill()` and aborts its auto-attach task —
    /// so a manager dropped without `stop_all()` cannot leave zombies behind.
    /// The graceful, reaping path is [`SessionManager::stop_all`].
    fn drop(&mut self) {
        let inner = self.inner.get_mut();
        inner.registry.drain();
        inner.browsers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn session(profile_id: &str, port: u16) -> RunningSession {
        RunningSession {
            profile_id: profile_id.to_string(),
            target_id: format!("target-{profile_id}"),
            ws_url: format!("ws://127.0.0.1:{port}/devtools/browser/{profile_id}"),
            debug_port: port,
            started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(port)),
        }
    }

    #[test]
    fn registry_rejects_duplicate_ids_with_the_offending_id() {
        let mut registry = Registry::default();
        registry.insert(session("a", 1)).expect("first insert");
        let error = registry
            .insert(session("a", 2))
            .expect_err("duplicate must fail");
        assert_eq!(error.profile_id, "a");
        assert!(registry.contains("a"));
        // The failed insert must not have replaced the original.
        assert_eq!(registry.list()[0].debug_port, 1);
    }

    #[test]
    fn registry_lists_in_insertion_order() {
        let mut registry = Registry::default();
        for (id, port) in [("a", 1), ("b", 2), ("c", 3)] {
            registry.insert(session(id, port)).expect("insert");
        }
        let ids: Vec<String> = registry.list().into_iter().map(|s| s.profile_id).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn registry_remove_is_idempotent_and_keeps_order() {
        let mut registry = Registry::default();
        for (id, port) in [("a", 1), ("b", 2), ("c", 3)] {
            registry.insert(session(id, port)).expect("insert");
        }
        assert_eq!(registry.remove("b").expect("present").debug_port, 2);
        assert!(registry.remove("b").is_none());
        assert!(!registry.contains("b"));
        let ids: Vec<String> = registry.list().into_iter().map(|s| s.profile_id).collect();
        assert_eq!(ids, ["a", "c"]);
        // The freed id can be inserted again.
        registry.insert(session("b", 4)).expect("re-insert");
        assert!(registry.contains("b"));
    }

    #[test]
    fn registry_drain_empties_everything() {
        let mut registry = Registry::default();
        registry.insert(session("a", 1)).expect("insert");
        registry.insert(session("b", 2)).expect("insert");
        let drained = registry.drain();
        assert_eq!(drained.len(), 2);
        assert!(registry.list().is_empty());
        assert!(!registry.contains("a"));
    }

    #[test]
    fn already_running_error_message_names_the_profile() {
        let error = AlreadyRunning {
            profile_id: String::from("6f9619ff"),
        };
        assert!(error.to_string().contains("6f9619ff"));
        // Must be recoverable through the Error trait.
        let boxed: Box<dyn std::error::Error> = Box::new(error.clone());
        assert!(boxed.downcast_ref::<AlreadyRunning>().is_some());
        // And through `anyhow`, which is how `launch_profile` hands it back.
        let wrapped = anyhow::Error::new(error);
        assert_eq!(
            wrapped
                .downcast_ref::<AlreadyRunning>()
                .map(|e| e.profile_id.as_str()),
            Some("6f9619ff")
        );
    }

    #[tokio::test]
    async fn manager_starts_empty_and_stop_is_idempotent() {
        let manager = SessionManager::new();
        assert!(manager.running().await.is_empty());
        // Stopping an unknown profile is a no-op, not an error.
        manager.stop("nobody").await.expect("idempotent stop");
        manager.stop_all().await.expect("stop_all on empty");
        assert!(manager.running().await.is_empty());
    }
}
