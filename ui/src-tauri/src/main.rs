// Mirage desktop UI — minimal Tauri 2 shell.
//
// Everything the user sees lives in ../dist (plain static files served by the
// webview). Rust only opens the WebView2 window; it exposes no commands, so the
// frontend talks to the local Mirage REST API directly over fetch().

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running Mirage UI");
}

#[cfg(test)]
mod tests {
    // Embedded at compile time so the tests validate the real config, not a copy.
    const TAURI_CONF: &str = include_str!("../tauri.conf.json");

    fn conf() -> serde_json::Value {
        serde_json::from_str(TAURI_CONF).expect("tauri.conf.json must be valid JSON")
    }

    #[test]
    fn config_is_a_json_object() {
        assert!(conf().is_object());
    }

    #[test]
    fn config_points_at_the_static_frontend() {
        assert_eq!(conf()["build"]["frontendDist"].as_str(), Some("../dist"));
    }

    #[test]
    fn config_has_identifier_and_main_window() {
        let c = conf();
        assert_eq!(c["identifier"].as_str(), Some("com.mirage.desktop"));
        let windows = c["app"]["windows"].as_array().expect("app.windows must be an array");
        assert!(!windows.is_empty(), "at least one window is required");
        assert_eq!(windows[0]["title"].as_str(), Some("Mirage"));
    }
}
