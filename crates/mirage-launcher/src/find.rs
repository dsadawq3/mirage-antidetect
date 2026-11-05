//! Chrome/Chromium binary discovery over well-known install paths.
//!
//! Deliberately registry-free (CONTRACT.md, module map for mirage-launcher):
//! probing a fixed candidate list keeps behavior identical across machines
//! without pulling in winreg or shelling out to `where`.

use std::path::{Path, PathBuf};

/// Find the default Chrome/Chromium executable.
///
/// Probes well-known install locations per OS, in priority order, and returns
/// the first one that exists. `None` means "not found — pass an explicit path".
pub fn find_chrome() -> Option<PathBuf> {
    default_candidates()
        .into_iter()
        .find(|p| is_executable_file(p))
}

/// Find a Chrome/Chromium executable inside `dir` (manual override support).
///
/// Accepts, in priority order:
/// - `dir` itself being the executable file,
/// - `dir` directly containing the executable,
/// - `dir` being an install root (e.g. `%ProgramFiles%` or `%ProgramFiles%\Google`),
///   including the `Application` layout and the Chrome SxS (Canary) layout.
pub fn find_chrome_in(dir: &Path) -> Option<PathBuf> {
    if is_executable_file(dir) {
        return Some(dir.to_path_buf());
    }
    if !dir.is_dir() {
        return None;
    }
    relative_layouts()
        .iter()
        .map(|rel| dir.join(rel))
        .find(|p| is_executable_file(p))
}

/// Well-known install locations, in priority order (Windows).
///
/// Order: Google Chrome (system 64-bit, system 32-bit, per-user), Chromium
/// (the same three layouts), then Google Chrome SxS (Canary, per-user only).
#[cfg(windows)]
fn default_candidates() -> Vec<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles");
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)");
    let local_app_data = std::env::var_os("LOCALAPPDATA");

    let mut candidates = Vec::new();
    // Google Chrome: system-wide (64-bit), system-wide (32-bit), per-user.
    if let Some(dir) = &program_files {
        candidates.push(PathBuf::from(dir).join("Google/Chrome/Application/chrome.exe"));
    }
    if let Some(dir) = &program_files_x86 {
        candidates.push(PathBuf::from(dir).join("Google/Chrome/Application/chrome.exe"));
    }
    if let Some(dir) = &local_app_data {
        candidates.push(PathBuf::from(dir).join("Google/Chrome/Application/chrome.exe"));
    }
    // Chromium: the same three layouts; the executable is still chrome.exe.
    if let Some(dir) = &program_files {
        candidates.push(PathBuf::from(dir).join("Chromium/Application/chrome.exe"));
    }
    if let Some(dir) = &program_files_x86 {
        candidates.push(PathBuf::from(dir).join("Chromium/Application/chrome.exe"));
    }
    if let Some(dir) = &local_app_data {
        candidates.push(PathBuf::from(dir).join("Chromium/Application/chrome.exe"));
    }
    // Google Chrome SxS (Canary) is a per-user install.
    if let Some(dir) = &local_app_data {
        candidates.push(PathBuf::from(dir).join("Google/Chrome SxS/Application/chrome.exe"));
    }
    candidates
}

/// Well-known install locations, in priority order (Linux).
#[cfg(target_os = "linux")]
fn default_candidates() -> Vec<PathBuf> {
    [
        "/usr/bin/google-chrome-stable",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// Well-known install locations, in priority order (macOS).
#[cfg(target_os = "macos")]
fn default_candidates() -> Vec<PathBuf> {
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// Unsupported platform: nothing to probe automatically.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn default_candidates() -> Vec<PathBuf> {
    Vec::new()
}

/// Relative layouts probed by [`find_chrome_in`], in priority order (Windows).
#[cfg(windows)]
fn relative_layouts() -> &'static [&'static str] {
    &[
        "chrome.exe",
        "Application/chrome.exe",
        "Google/Chrome/Application/chrome.exe",
        "Chromium/Application/chrome.exe",
        "Google/Chrome SxS/Application/chrome.exe",
    ]
}

/// Relative layouts probed by [`find_chrome_in`], in priority order (Linux).
#[cfg(target_os = "linux")]
fn relative_layouts() -> &'static [&'static str] {
    &[
        "chrome",
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ]
}

/// Relative layouts probed by [`find_chrome_in`], in priority order (macOS).
#[cfg(target_os = "macos")]
fn relative_layouts() -> &'static [&'static str] {
    &[
        "Google Chrome",
        "Chromium",
        "Google Chrome.app/Contents/MacOS/Google Chrome",
        "Chromium.app/Contents/MacOS/Chromium",
    ]
}

/// Relative layouts for unsupported platforms.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn relative_layouts() -> &'static [&'static str] {
    &[]
}

/// `true` when `path` points at a regular executable file.
///
/// Windows: existence of the file is enough (candidates carry an explicit
/// `.exe` name). Unix: at least one execute bit must be set.
fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (nested relative layout, bare file name) used by the tests below.
    #[cfg(windows)]
    fn test_layout() -> (&'static str, &'static str) {
        ("Google/Chrome/Application/chrome.exe", "chrome.exe")
    }

    #[cfg(target_os = "linux")]
    fn test_layout() -> (&'static str, &'static str) {
        ("google-chrome-stable", "chromium")
    }

    #[cfg(target_os = "macos")]
    fn test_layout() -> (&'static str, &'static str) {
        ("Google Chrome.app/Contents/MacOS/Google Chrome", "Chromium")
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    fn test_layout() -> (&'static str, &'static str) {
        ("chrome", "chrome")
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod 755");
        }
        #[cfg(not(unix))]
        let _ = path;
    }

    #[test]
    fn find_chrome_in_locates_nested_layout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (nested, _) = test_layout();
        let chrome = tmp.path().join(nested);
        std::fs::create_dir_all(chrome.parent().expect("parent dir")).expect("create layout dirs");
        std::fs::write(&chrome, b"").expect("create fake chrome binary");
        make_executable(&chrome);

        let found = find_chrome_in(tmp.path()).expect("chrome must be found");
        assert_eq!(found, chrome);
    }

    #[test]
    fn find_chrome_in_locates_bare_executable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (_, bare) = test_layout();
        let chrome = tmp.path().join(bare);
        std::fs::write(&chrome, b"").expect("create fake chrome binary");
        make_executable(&chrome);

        let found = find_chrome_in(tmp.path()).expect("chrome must be found");
        assert_eq!(found, chrome);
    }

    #[test]
    fn find_chrome_in_accepts_executable_path_itself() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (_, bare) = test_layout();
        let chrome = tmp.path().join(bare);
        std::fs::write(&chrome, b"").expect("create fake chrome binary");
        make_executable(&chrome);

        let found = find_chrome_in(&chrome).expect("chrome must be found");
        assert_eq!(found, chrome);
    }

    #[test]
    fn find_chrome_in_rejects_empty_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(find_chrome_in(tmp.path()).is_none());
    }

    #[test]
    fn find_chrome_in_rejects_missing_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        assert!(find_chrome_in(&missing).is_none());
    }
}
