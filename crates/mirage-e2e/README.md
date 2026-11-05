# mirage-e2e

End-to-end fingerprint checker for Mirage: it launches a masked Chrome from a
seed-generated legend, loads a **local, self-contained** checker page, and
verifies that the live document matches the legend and survives a set of
anti-detection invariants.

## Why

The spoof layer (`inject/`) is only worth anything if a page actually sees the
legend. The launcher smoke (`crates/mirage-launcher/examples/smoke.rs`) proved
that once, by hand, inside a `main`. This crate turns that protocol into a
repeatable artifact:

- **No third-party sites.** The checker page is embedded into the crate with
  `include_str!` and served by an in-process `axum` server on
  `127.0.0.1:0`. A run never touches the network, so the detection surface can
  be tested fully offline and the result never depends on someone else's
  uptime, rate limits or consent.
- **Deterministic.** The legend comes from `mirage_core::generate(seed, os)`,
  so a given `(seed, os)` reproduces the same fingerprint on any machine.
- **Reported, not eyeballed.** Probes produce structured pass/fail entries
  parsed into a Rust struct (`E2eReport`), usable from a test or a harness.

## How to run

Unit test only (no browser, no Chrome window):

```text
cargo test -p mirage-e2e
```

Live run (opens a real Chrome window; requires Chrome installed):

```text
MIRAGE_E2E=1 cargo test -p mirage-e2e -- --ignored --nocapture
```

`e2e_windows_legend` is `#[ignore]`d so the default `cargo test` never opens a
browser. Even under `--ignored` it only runs when `MIRAGE_E2E=1` is set;
otherwise it prints an explanation and returns (not a silent skip).

The crate API is also directly callable:

```rust
use mirage_core::Os::Windows;
let report = mirage_e2e::run_e2e(20_261_103, Windows).await?;
assert!(report.is_clean(), "{:?}", report.failed);
```

## What is covered

The page (`assets/checker.html`) runs three probe groups and writes one JSON
string to `window.__MIRAGE_CHECK__`:

```json
{ "fields": { }, "verdict": { "checks": [ { "name": "", "pass": true, "detail": {} } ] } }
```

### Values (legend comparison, in the verdict)

`userAgent`, `platform`, `languages`, `hardwareConcurrency`, `deviceMemory`,
`webdriver`, screen `size`/`avail`/`colorDepth`/`devicePixelRatio`, timezone
name and IANA offset cross-check, WebGL `vendor`/`renderer`
(`WEBGL_debug_renderer_info`), `userAgentData` `brands`/`mobile`/`platform`
(secure context only), plugin/mime counts (`plugins.length`,
`plugins.mimeCounts`, `mimeTypes.length`), plugin own-property hygiene, and
absence of `navigator.oscpu`.

### Adversarial (in the verdict)

`toString` of spoofed accessors looks native and leaks no internals, blank
canvas reads back zeros, drawn-canvas noise is stable across two reads,
`matchMedia('(min-resolution: …)')` agrees with `devicePixelRatio`, maximized
window `outerWidth/Height` matches the legend's available area, a blob Worker
sees the legend's `hardwareConcurrency`/`deviceMemory`, `JSON.stringify` of
`userAgentData` round-trips its brands, `getHighEntropyValues` keeps the base
fields, and all ten inject modules report loaded.

### Report-only (in `fields`, never in the verdict)

Font enumeration by canvas measurement (~33 names), `mediaDevices.enumerateDevices`,
an audio hash from an `OfflineAudioContext` + `DynamicsCompressor` (stability of
two runs), and WebGL `MAX_TEXTURE_SIZE`/`MAX_VIEWPORT_DIMS`. These are recorded
for diagnostics; they do not gate a run because they depend on the host or are
not (yet) spoofed.

## Design notes

- `src/lib.rs` holds `run_e2e`, the parser (`parse_report`) and the report
  types. `harvest` polls `Runtime.evaluate` for `document.readyState ==
  "complete"` **and** the presence of `window.__MIRAGE_CHECK__`, bounded to
  10 s.
- Every probe on the page runs in its own `try/catch`; every async wait has a
  bounded timeout (Worker/media/audio: 3 s each). A probe can fail; it can
  never hang or throw out of the driver.
- The browser is stopped on every path of `run_e2e`, including errors.
