# Mirage UI (Tauri 2 shell + static frontend)

Desktop control panel for [Mirage](../README.md): profile manager, fingerprint
editor, proxy checker, cookie tooling, importer and settings, in the style of
Dolphin Anty but cleaner.

The whole UI is plain static files (no npm, no node, no build step, no dev
server). Rust only hosts the WebView2 window.

```
ui/
  dist/                  static frontend served by the webview (frontendDist)
    index.html
    styles.css
    app.js               all UI logic (classic script, ES2020)
    mock.js              fetch interception + fixtures for offline demo
  src-tauri/             minimal Tauri 2 Rust shell
    Cargo.toml
    build.rs
    tauri.conf.json      points frontendDist at ../dist
    src/main.rs          + trivial config test
    icons/               app icons (generate_icons.py regenerates them)
```

## Run

```
cd ui/src-tauri
cargo run
```

First build compiles the whole Tauri/WebView2 dependency tree and takes a
while. Requires a WebView2 runtime (preinstalled on current Windows).

`cargo test` in `ui/src-tauri` runs the config-parsing test only.

This crate is deliberately **not** a member of the root workspace (it declares
its own `[workspace]`), so `cargo test --workspace` at the repo root stays fast
and never pulls in the heavy tauri dependency tree.

## Point it at the real API

By default the UI talks to the frozen local API from `docs/API.md` at
`http://127.0.0.1:35500`. Change it in **Settings → API URL**, and paste the
`MIRAGE_API_TOKEN` if the server requires one. Both are stored in
`localStorage`. The API already answers with `Access-Control-Allow-Origin: *`,
so the webview can call it directly.

## Mock mode (no backend needed)

The UI can run entirely offline against fixtures:

- toggle **Settings → Mock mode**, or
- append `?mock=1` to the document URL, or
- set `localStorage.mirage.mock = "1"`.

In mock mode every `fetch` to the API is intercepted by `mock.js`: read/create/
update/delete profiles, status polling, proxy check, cookie import/export and
the AdsPower/Dolphin/GoLogin importer all work against in-memory fixtures.

### Import warnings

`POST /api/v1/import` reports `warnings` as objects
`{ "field": "<logical field>", "kind": "mapped" | "defaulted" | "ignored" }`
(one per mapper decision), not as plain strings. The import screen renders each
as `field (kind)` and still tolerates a bare string. The mock importer emits the
same object shape, so the offline demo shows exactly what the live server sends.

### Deep links

- `?route=profiles|cookies|import|settings` opens a screen directly.
- `?select=<profile-id>` (or `?select=first`) preselects a profile, e.g. for the
  Cookies screen: `?mock=1&route=cookies&select=first`.

**Browser launch is the one thing mock mode does not fake.** Start/Stop carry a
"real server only" marker and explain as much instead of pretending to spawn
Chromium. Everything else is fully demonstrable without the backend.

## Fingerprint seed

The profile editor exposes the generation `seed` as an editable field, both when
creating a profile and when editing one (it used to be read-only on edit). Saving
with a `seed` sends it to the API, which rebuilds the whole fingerprint from
`generate(seed, os)` and then layers the form's `overrides` on top, so the same
seed always reproduces the same legend on any machine. Leave the field empty to
keep the current fingerprint (and to leave legacy `seed = 0` profiles alone).

The target **OS is fixed at creation**. The frozen `PUT /profiles/:id` contract
carries no `os` field and rebuilds from the profile's stored OS, so the editor
shows the OS read-only while editing (an editable control there would be a silent
no-op that also desyncs the version/screen pickers). To put a profile on another
OS, create a new one.

## Design

Deep graphite (`#0e1116` background, `#161b24` panels), a single cyan accent
(`#22d3ee`), 10-12px radii, inline SVG icons only (no external requests, no
CDN, no emoji), hover micro-animations, green/grey status dots, adaptive card
grid and skeletons while loading.
