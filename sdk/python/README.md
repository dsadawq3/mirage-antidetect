# Mirage Python SDK

Typed Python client for the **Mirage local HTTP API (v1)** — the antidetect
browser core. It is a thin wrapper over the frozen contract in
[`docs/API.md`](../../docs/API.md) and the data schemas in
[`docs/CONTRACT.md`](../../docs/CONTRACT.md).

- **Standard library only** (`urllib.request`). No third-party runtime deps.
- **Python 3.10+**, full type hints, ships `py.typed`.
- Covers every native endpoint **and** the AdsPower-compatibility aliases.

> The server is a **separate process** — the `mirage-api` binary (the Rust core
> exposing the local REST interface). This package is only the client; start
> the server first, then point the SDK at it (default
> `http://127.0.0.1:35500`).

## Layout choice: src-layout

The package uses a **src-layout** (`src/mirage/`) rather than a flat module.
Reason: tests and examples then import the *installed* package (or the tree via
an explicit `src` path), never a stray `mirage.py` picked up from the current
directory. It is the modern setuptools default and keeps the import graph
honest.

```
sdk/python/
  pyproject.toml          # metadata, no runtime deps
  README.md
  src/mirage/
    __init__.py           # public surface
    client.py             # MirageClient (native + AdsPower)
    errors.py             # exception hierarchy
    py.typed
  examples/
    create_and_launch.py
    import_ads_power.py
    cookie_roundtrip.py
  tests/
    fake_server.py        # real http.server on loopback, canned responses
    test_client.py        # 32 boundary tests
    conftest.py
```

## Install

```bash
pip install -e .
# optional, for the test suite:
pip install -e ".[test]"
```

## Quick start

```python
from mirage import MirageClient, MirageApiError

client = MirageClient()                      # http://127.0.0.1:35500
# client = MirageClient("http://127.0.0.1:35500", token="...")   # if MIRAGE_API_TOKEN is set

health = client.health()
print(health["vault"])                       # "locked" | "unlocked"

if health["vault"] != "unlocked":
    client.unlock_vault("my-password")

profile = client.create_profile("Shop-01", os="windows", seed=20261103, tags=["work"])
session = client.launch(profile["id"])
print(session["ws_url"])                     # attach Playwright/Puppeteer here

print([p["profile_id"] for p in client.status()])
client.stop(profile["id"])
```

## Methods → endpoints

### Native

| Method | HTTP | Endpoint | Returns |
|---|---|---|---|
| `health()` | GET | `/api/v1/health` | `{ok, version, vault, running}` |
| `unlock_vault(password)` | POST | `/api/v1/vault/unlock` | `{}` (401 on bad password) |
| `list_profiles()` | GET | `/api/v1/profiles` | `[ProfileSummary]` |
| `create_profile(name, *, tags, seed, os, proxy, overrides)` | POST | `/api/v1/profiles` | `Profile` (201) |
| `get_profile(id)` | GET | `/api/v1/profiles/:id` | `Profile` |
| `update_profile(id, **fields)` | PUT | `/api/v1/profiles/:id` | `Profile` |
| `delete_profile(id)` | DELETE | `/api/v1/profiles/:id` | `None` (204) |
| `launch(id)` | POST | `/api/v1/profiles/:id/launch` | `{target_id, ws_url, debug_port, profile_dir}` (409 if running) |
| `stop(id)` | POST | `/api/v1/profiles/:id/stop` | `{}` |
| `status()` | GET | `/api/v1/status` | `[running entry]` |
| `proxy_check(scheme, host, port, username, password)` | POST | `/api/v1/proxy/check` | `{ok, ip?, latency_ms?, error?}` |
| `cookies_import(id, data, format="netscape")` | POST | `/api/v1/profiles/:id/cookies/import` | `{imported, skipped}` |
| `cookies_export(id, format="netscape")` | GET | `/api/v1/profiles/:id/cookies/export?format=` | `{format, data}` |
| `import_profiles(source, data)` | POST | `/api/v1/import` | `{imported: [Profile], warnings: []}` |

`source` is one of `adspower`, `dolphin`, `gologin`. `format` is `netscape` or
`json`. Both are validated client-side, case-insensitively (matching the
server), and raise `ValueError` on a bad value.

### AdsPower-compatible aliases

These return the unwrapped `data` object (the envelope's `code`/`msg` are
consumed by the client). Success is `code == 0`.

| Method | HTTP | Endpoint | `data` |
|---|---|---|---|
| `browser_start(user_id)` | GET | `/api/v1/browser/start?user_id=` | `{ws: {puppeteer}, debug_port, target_id}` |
| `browser_stop(user_id)` | GET | `/api/v1/browser/stop?user_id=` | `{}` |
| `browser_list(page=1, page_size=50)` | GET | `/api/v1/browser/list?page=&page_size=` | `{list, page, page_size, total}` |
| `user_create(payload)` | POST | `/api/v1/user/create` | `{id, warnings}` |
| `user_update(payload)` | POST | `/api/v1/user/update` | `{}` |
| `user_delete(user_id)` | POST | `/api/v1/user/delete` | `{}` |

The Puppeteer websocket URL from `browser_start` is
`client.browser_start(uid)["ws"]["puppeteer"]`.

## Error handling

```python
from mirage import (
    MirageError,            # base class of all of the below
    MirageConnectionError,  # API unreachable (refused / timeout / bad host)
    MirageApiError,         # native endpoint returned 4xx/5xx
    AdsPowerError,          # AdsPower alias returned {"code": <non-zero>}
)

try:
    client.launch(profile_id)
except MirageApiError as exc:
    print(exc.status, exc.code, exc.message, exc.details)
except AdsPowerError as exc:
    print(exc.code, exc.msg)
except MirageConnectionError as exc:
    print("is mirage-api running?", exc)
```

- `MirageApiError(code, message, status, details)` — `details` keeps the full
  error object, so any keys the server adds beyond `code`/`message` land in
  `.details`.
- `AdsPowerError(code, msg, details)` — the envelope's `code` is non-zero.
- `MirageConnectionError(message, base_url)` — no HTTP response was produced.

The one crossover: an **invalid token on an AdsPower alias is a genuine
`401`**, reported as `MirageApiError`.

There are **no retries**: this is a loopback API, and a failure should surface.

## Tests

The suite runs a real `http.server` on an ephemeral loopback port and drives
real HTTP round-trips through it — no mocking of client internals.

```bash
python -m pytest tests -q
# or, without pytest:
python -m unittest discover -s tests -v
```

Covered: successful create/launch, bearer-token forwarding, `401` →
`MirageApiError`, `409` preserving extra error keys, AdsPower `{code: 1}` →
`AdsPowerError`, AdsPower `401` → `MirageApiError`, bad base URL →
`MirageConnectionError`, plus list/status/stop/proxy/cookies/import/user_* and
client-side validation.

## Out of scope

- The server itself (`mirage-api`) — this is client-only.
- Async/HTTP2, retries, connection pooling.
- LAUNCH websocket/CDP driving (use the `ws_url` with Playwright/Puppeteer).
