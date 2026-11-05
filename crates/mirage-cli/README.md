# mirage-cli

The `mirage` binary: a command-line interface for the Mirage antidetect
browser. It drives the workspace crates directly — `mirage-vault` (encrypted
profiles), `mirage-core` (fingerprints), `mirage-launcher` (masked browser
sessions), `mirage-proxy` (exit-IP checks), `mirage-cookies` (cookie
import/export) and `mirage-import` (competitor migration). There is **no
background server** or daemon.

There is no daemon, but the CLI is not memoryless: a foreground `mirage launch`
writes a running-session descriptor under `<data-dir>/run/`, so later, separate
`mirage status` and `mirage stop` processes see the same session.

The binary is named `mirage`; the Rust library crate is `mirage-cli`.

## Install

```bash
# from the workspace root
cargo build -p mirage-cli --release
./target/release/mirage --help
```

The binary talks to a Chrome install found automatically by `mirage-launcher`
(none is needed for profile/proxy/import/fingerprint commands).

## Data directory

All state lives under one root (default `%LOCALAPPDATA%\Mirage` on Windows):

```
<data-dir>/
  vault/<id>.mprof       encrypted profiles (Argon2id + AES-256-GCM)
  vault/.vault.salt      per-vault salt (created on first open)
  profiles/<id>/         Chromium user-data-dir
  run/<id>.json          running-session descriptors (mirage launch/status/stop)
  run/<id>.lock          per-profile launch locks (transient)
```

Override it with `--data-dir <DIR>` or the `MIRAGE_DATA_DIR` environment
variable.

## Global options

| Option | Meaning |
|---|---|
| `--data-dir <DIR>` | Data directory (default `%LOCALAPPDATA%\Mirage`). |
| `--password <PASS>` | Vault master password. Falls back to `MIRAGE_VAULT_PASSWORD`. |
| `--json` | Print a single JSON document on stdout instead of human text. Errors always go to stderr. |

Global options may appear before or after the subcommand.

Vault-backed commands need a password. If none is given the CLI fails with
`vault password required: pass --password or set MIRAGE_VAULT_PASSWORD`.

## Commands

### Profiles

```bash
mirage profile list
mirage profile show <id>
mirage profile create --name Shop-01 [--os windows|macos|linux] [--seed N] [--tag t]... [--proxy scheme://user:pass@host:port]
mirage profile delete <id>
mirage profile duplicate <id> --name <new name>
```

- `create` prints the new id (human) or the full profile JSON (`--json`).
- `--seed` with no value (or omitted entirely) draws a random seed; an explicit
  seed makes the fingerprint reproducible on any machine.
- `list` columns: id, name, tags, proxy, created, updated.

### Launch / stop / status

```bash
mirage launch <id>          # foreground until Ctrl+C; prints target_id, ws_url, debug_port
mirage stop <id>
mirage stop --all
mirage status
```

`launch` starts the browser through `mirage-launcher`'s `SessionManager` and
keeps the process in the foreground, printing `Ctrl+C — остановить браузер`.
On Ctrl+C (or when the browser is closed externally) the session is stopped and
its registry entry removed.

`launch` also holds a per-profile lock and checks the run registry before
spawning, so a profile with a live browser — even one launched by a different
`mirage` process — is rejected with exit `5` instead of starting a second
Chrome. A stale descriptor left by a browser that is gone is cleared first.

`status` and `stop` run in other processes and use the `<data-dir>/run/`
descriptors. Liveness is probed with an HTTP `GET /json/version` that must
return a valid DevTools payload (a bare TCP connect would only prove *someone*
holds the port); `status` shows the browser pid and `running`/`stale`.

`stop` asks each browser to close over CDP and removes its descriptor **only**
once the shutdown is confirmed (`Browser.close` acknowledged, or the DevTools
endpoint actually stopped answering). A browser that is still reachable is left
in the registry and reported as `failed to stop <id>`, and `stop` exits non-zero
(`1`). `stop <id>` for an id with no descriptor exits `4`; `stop` with neither
an id nor `--all` is a usage error (exit `2`).

### Proxy

```bash
mirage proxy check 'socks5://user:pass@1.2.3.4:1080'
mirage proxy check 'http://1.2.3.4:8080' --timeout-ms 5000
```

Prints the exit IP and latency. A failed check prints the report and exits `6`.

### Cookies

```bash
mirage cookies import <id> --format netscape|json --file cookies.txt
mirage cookies export <id> --format netscape|json [--out cookies.txt]
```

Without `--out`, export writes the raw cookie payload to stdout. Cookie
import/export touches the Chromium store (`<profile>/Default/Network/Cookies`) and is
Windows-only (DPAPI); elsewhere it reports an unsupported error. Cookies can
only be read/written while the browser is **not** running against the profile.

### Import

```bash
mirage import --source adspower|dolphin|gologin --file export.json [--no-save]
```

Imports every profile found in the file and writes them to the vault (saving is
the default; `--no-save` skips it, `--save` re-enables it). Prints the imported
ids/names plus the field-level mapping warnings.

### Fingerprints

```bash
mirage fingerprint show --seed 20261103 [--os windows|macos|linux]
```

Generates and prints a fingerprint deterministically from the seed — the legend
debugging tool.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | Unclassified runtime error (I/O, import, fingerprint, a `stop` that could not close a browser, ...). |
| 2 | Command-line usage error (clap), e.g. `stop` with neither an id nor `--all`. |
| 3 | Vault error: password missing, wrong, or the vault is corrupted. |
| 4 | Requested profile or session not found (also `profile show <non-uuid>` and `stop <unknown id>`). |
| 5 | Conflicting state (profile already running, or another launch is in progress). |
| 6 | Proxy check failed (bad URL, unreachable proxy, no exit IP). |

## Tests

```bash
cargo test -p mirage-cli
```

Covers argument parsing for every command, profile CRUD against a temporary
vault, competitor import against the shared `mirage-import` fixture, fingerprint
determinism and proxy-URL conversion. `launch`/`stop` are not driven end to end
(they need Chrome); their browser-free paths are tested instead: the run-registry
launch guard against a mock DevTools endpoint, exit-code mapping, the CDP attach
timeout against a silent port, and `stop`'s confirmed-close logic.
