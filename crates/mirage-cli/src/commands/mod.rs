//! Command implementations.
//!
//! Each `run_*` function takes parsed arguments plus a [`crate::context::Ctx`],
//! performs the work against the local vault / filesystem / crates and returns
//! a value that implements [`crate::output::Render`]. Printing is done by the
//! dispatch layer in [`crate::run`], which makes the commands testable without
//! a process or a terminal.

pub mod cookies;
pub mod fingerprint;
pub mod import;
pub mod profile;
pub mod proxy;
pub mod session;
