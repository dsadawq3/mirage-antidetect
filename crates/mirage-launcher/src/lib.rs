//! mirage-launcher: locate a Chrome/Chromium binary, build masked launch
//! arguments (CONTRACT.md, "Аргументы Chromium"), spawn the browser and inject
//! the Mirage JS spoof layer through the Chrome DevTools Protocol (CONTRACT.md,
//! "CDP-команды" + "Спуф-слой inject/").
//!
//! Module map:
//! - [`find`]   — browser discovery over well-known paths (no registry access)
//! - [`args`]   — [`LaunchConfig`] and [`args::build_args`]
//! - [`cdp`]     — minimal CDP client (raw TCP HTTP discovery + websocket,
//!   flat-mode `sessionId` commands and event subscription)
//! - [`launch`]  — spawn, browser-level attach, flat auto-attach so *every*
//!   page target (new tabs included) is spoofed, inject glue
//! - [`session`] — [`session::SessionManager`]: launch/stop/track profiles

pub mod args;
pub mod cdp;
pub mod find;
pub mod launch;
pub mod session;

pub use args::{build_args, LaunchConfig};
pub use cdp::{CdpClient, PageTarget};
pub use find::{find_chrome, find_chrome_in};
pub use launch::{
    build_inject_source, build_inject_source_from, build_worker_inject_source,
    build_worker_inject_source_from, embedded_inject_modules, embedded_worker_snippet, inject_dir,
    launch, worker_fingerprint_prefix, LaunchedBrowser, INJECT_DIR_ENV, INJECT_MODULES,
    WORKER_TARGET_TYPES,
};
pub use session::{AlreadyRunning, RunningSession, SessionHandle, SessionManager};
