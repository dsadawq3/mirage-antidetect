# mirage-api

Local REST API over the whole Mirage core: profile CRUD, fingerprint
generation, the encrypted vault, browser launch/stop, proxy checks, cookie and
competitor import — plus an AdsPower-compatible alias layer.

The wire contract is frozen in [`docs/API.md`](../../docs/API.md) and
[`docs/CONTRACT.md`](../../docs/CONTRACT.md). This crate implements it verbatim.

## Run

```bash
cargo run -p mirage-api
```

The server binds `127.0.0.1:35500` and prints the address:

```
mirage-api: data dir C:\Users\<you>\AppData\Local\Mirage
mirage-api: vault unlocked
mirage-api: listening on http://127.0.0.1:35500
```

## Environment

| Variable | Default | Meaning |
|---|---|---|
| `MIRAGE_API_PORT` | `35500` | Listening port on `127.0.0.1`. |
| `MIRAGE_API_TOKEN` | _(unset)_ | When set, every `/api/v1/*` request except `/api/v1/health` needs `Authorization: Bearer <token>`. |
| `MIRAGE_VAULT_PASSWORD` | _(unset)_ | Unlocks the vault at startup. Unset → the vault starts locked; use `POST /api/v1/vault/unlock`. |
| `MIRAGE_DATA_DIR` | `%LOCALAPPDATA%\Mirage` | Root data directory (`vault\`, `profiles\<id>\`). |
| `MIRAGE_ALLOWED_ORIGINS` | Tauri origins | Comma-separated CORS origin allowlist; see below. |

## Layout

```
<data_dir>\
  vault\<id>.mprof      # Argon2id + AES-256-GCM encrypted profiles
  vault\.vault.salt     # 16-byte salt, created on first open
  profiles\<id>\        # Chromium user-data-dir per profile
```

## Errors

Native endpoints return the HTTP status plus
`{ "error": { "code": "<code>", "message": "<text>" } }`.
Codes: `invalid_request`, `not_found`, `already_running`, `not_running`,
`vault_locked`, `io`, `internal`.

A missing or wrong token is `401 invalid_request` (the contract defines no
dedicated auth code). A request body over the 2 MiB limit is `413` with the
same envelope (`code: "invalid_request"`).

AdsPower aliases always return HTTP `200` with
`{ "code": <0|non-0>, "msg": "...", "data": ... }` (the only 4xx is a bad token).
The contract defines no dedicated auth code, so a rejected token is reported as
`401` with `code: "invalid_request"`.

## CORS

The UI runs in the Tauri webview, so responses carry an
`Access-Control-Allow-Origin` only for an allowlisted `Origin`. The allowlist is
`MIRAGE_ALLOWED_ORIGINS` (comma-separated); when unset it defaults to the Tauri
webview origins:

```
http://tauri.localhost,https://tauri.localhost,tauri://localhost
```

- `Origin` present and allowlisted → it is mirrored back in
  `Access-Control-Allow-Origin`;
- `Origin` present but not allowlisted → no `Access-Control-Allow-Origin` is
  sent (the browser blocks the response) and a preflight `OPTIONS` is refused
  with `403`;
- no `Origin` (curl, SDK, server-to-server) → no `Access-Control-Allow-Origin`
  is needed.

`Access-Control-Allow-Headers: authorization, content-type` and
`Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS` are always sent;
an allowlisted preflight `OPTIONS` answers `204`.

Set `MIRAGE_ALLOWED_ORIGINS` to add a dev origin, e.g.
`MIRAGE_ALLOWED_ORIGINS=http://tauri.localhost,http://localhost:1420`.

## Endpoints

### Native

| Method | Path |
|---|---|
| GET | `/api/v1/health` |
| POST | `/api/v1/vault/unlock` |
| GET / POST | `/api/v1/profiles` |
| GET / PUT / DELETE | `/api/v1/profiles/:id` |
| POST | `/api/v1/profiles/:id/launch` |
| POST | `/api/v1/profiles/:id/stop` |
| GET | `/api/v1/status` |
| POST | `/api/v1/proxy/check` |
| POST | `/api/v1/profiles/:id/cookies/import` |
| GET | `/api/v1/profiles/:id/cookies/export?format=netscape\|json` |
| POST | `/api/v1/import` |

### AdsPower aliases

| Method | Path |
|---|---|
| GET | `/api/v1/browser/start?user_id=` |
| GET | `/api/v1/browser/stop?user_id=` |
| GET | `/api/v1/browser/list?page=&page_size=` |
| POST | `/api/v1/user/create` |
| POST | `/api/v1/user/update` |
| POST | `/api/v1/user/delete` |

## curl examples

```bash
# Health (never requires the token)
curl -s http://127.0.0.1:35500/api/v1/health

# Unlock the vault for this session
curl -s -X POST http://127.0.0.1:35500/api/v1/vault/unlock \
  -H 'content-type: application/json' \
  -d '{"password":"master-password"}'

# Create a profile (random seed when omitted)
curl -s -X POST http://127.0.0.1:35500/api/v1/profiles \
  -H 'content-type: application/json' \
  -d '{"name":"Shop-01","tags":["work"],"os":"windows","seed":20261103}'

# List / fetch / update / delete
curl -s http://127.0.0.1:35500/api/v1/profiles
curl -s http://127.0.0.1:35500/api/v1/profiles/<id>
curl -s -X PUT http://127.0.0.1:35500/api/v1/profiles/<id> \
  -H 'content-type: application/json' \
  -d '{"name":"Shop-01-renamed","overrides":{"timezone":"Europe/Paris"}}'
curl -s -X DELETE -o /dev/null -w '%{http_code}\n' \
  http://127.0.0.1:35500/api/v1/profiles/<id>

# Launch / stop / status
curl -s -X POST http://127.0.0.1:35500/api/v1/profiles/<id>/launch
curl -s http://127.0.0.1:35500/api/v1/status
curl -s -X POST http://127.0.0.1:35500/api/v1/profiles/<id>/stop

# Proxy exit-IP check (always 200; read "ok")
curl -s -X POST http://127.0.0.1:35500/api/v1/proxy/check \
  -H 'content-type: application/json' \
  -d '{"scheme":"socks5","host":"1.2.3.4","port":1080,"username":null,"password":null}'

# Cookies (profile must have launched at least once on Windows)
curl -s -X POST http://127.0.0.1:35500/api/v1/profiles/<id>/cookies/import \
  -H 'content-type: application/json' \
  -d '{"format":"json","data":"[{\"name\":\"a\",\"value\":\"1\",\"domain\":\".x.com\",\"path\":\"/\"}]"}'
curl -s 'http://127.0.0.1:35500/api/v1/profiles/<id>/cookies/export?format=netscape'

# Import from a competitor
curl -s -X POST http://127.0.0.1:35500/api/v1/import \
  -H 'content-type: application/json' \
  -d '{"source":"adspower","data":"{\"user_id\":\"u1\",\"name\":\"Shop\"}"}'

# AdsPower-style calls
curl -s 'http://127.0.0.1:35500/api/v1/browser/start?user_id=<id>'
curl -s 'http://127.0.0.1:35500/api/v1/browser/list?page=1&page_size=50'
```

With `MIRAGE_API_TOKEN` set, add `-H 'Authorization: Bearer <token>'` to every
call except `/api/v1/health`.

## Tests

```bash
cargo test -p mirage-api
```

The suite drives the router in-process (`tower::ServiceExt::oneshot`) against a
temp vault and data directory. Cookie import/export on a real Chromium store is
Windows-only; the format-parse and error branches are covered everywhere.
Launching a real browser is opt-in:

```bash
MIRAGE_E2E=1 cargo test -p mirage-api -- --ignored --nocapture
```
