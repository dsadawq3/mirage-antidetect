# mirage-sync

Self-hosted team synchronization server for Mirage profiles: an axum + SQLite
REST API keyed by Bearer API keys. A profile body is opaque JSON — this crate
does not depend on `mirage-core` and never validates the fingerprint shape;
the client does that before it pushes a profile.

## Running

```
cargo run -p mirage-sync
```

Configuration is **environment-only**:

| Variable | Default | Meaning |
|---|---|---|
| `MIRAGE_SYNC_DB` | `mirage-sync.db` | SQLite file path |
| `MIRAGE_SYNC_ADDR` | `127.0.0.1:8787` | listen address |
| `MIRAGE_SYNC_BOOTSTRAP_KEY` | *(generated)* | admin key to install on a brand-new database |
| `MIRAGE_SYNC_KEY` | *(unset)* | key material for at-rest body encryption |
| `MIRAGE_SYNC_ALLOW_REMOTE` | *(unset)* | `1` to allow a non-loopback bind |

## Bootstrap admin key

On a brand-new database the server creates the first **admin** key exactly
once; only its `sha256` is stored.

* `MIRAGE_SYNC_BOOTSTRAP_KEY` set → that value is installed and is **never
  logged** (must be ≥ 16 characters). Use it to seed a key you already manage.
* unset → a fresh key is generated and printed **once, to stderr only** (never
  through the tracing/stdout pipeline). Copy it immediately: it cannot be
  recovered.

## Encryption at rest (trust model)

Profile bodies (`profiles.body_json`) include anything the client puts in them
— notably `proxy.username` / `proxy.password`. Ciphertext depends on
`MIRAGE_SYNC_KEY`:

* **unset (default):** bodies are stored as **plaintext**. A startup warning
  documents this. This is only safe when the database file itself is protected
  (full-disk encryption, restrictive filesystem permissions). Anyone who can
  read the file can read every proxy credential.
* **64 hex characters:** used verbatim as the AES-256 key.
* **any other value:** treated as a passphrase and stretched with **Argon2id**
  (OWASP minimum: m = 19456 KiB, t = 2, p = 1) over a random 16-byte salt kept
  in the `meta` table, so the same passphrase reproduces the key on restart.

When a key is set, every body is written as

```
"enc:v1:" || hex( nonce[12] || ciphertext||tag )      # AES-256-GCM
```

Reads are transparent: a row written before the key existed (plaintext) is
still returned as-is, and the next write re-encrypts it. Once a key is set, a
row that *is* an envelope but cannot be opened (wrong key, tampering) is a
hard error — the server fails closed and never returns corrupted data.

Rotating or losing `MIRAGE_SYNC_KEY` makes existing encrypted rows
unreadable; decrypt with the old key and rewrite, or restore the old key.

## Network exposure

The API speaks cleartext HTTP and authenticates with a Bearer key. A
non-loopback bind is therefore **refused** unless `MIRAGE_SYNC_ALLOW_REMOTE=1`
is set, and even then the server warns: terminate TLS in front of it.

## API

All routes require `Authorization: Bearer <key>` except `GET /healthz`.

| Method & path | Role | Notes |
|---|---|---|
| `GET /healthz` | — | liveness |
| `POST /v1/keys` | admin | mint a key (returned once) |
| `GET /v1/keys?limit=&offset=` | admin | all keys, revoked included |
| `DELETE /v1/keys/{label}` | admin | revoke the active key with this label |
| `GET /v1/profiles?tag=&limit=&offset=` | any | summaries, no bodies |
| `GET /v1/profiles/{id}` | any | full profile |
| `PUT /v1/profiles/{id}` | any | create/update under optimistic concurrency |
| `DELETE /v1/profiles/{id}` | admin | remove a profile |
| `GET /v1/audit?limit=&offset=` | admin | newest audit entries first |

List endpoints are paginated: `limit` defaults to 100 and is capped at 1000,
`offset` defaults to 0.

`PUT /v1/profiles/{id}` body: `body` and `base_revision` are required; `name`
and `tags` are **optional** — an absent field is left unchanged on update (and
defaulted to empty on first insert), so a client can bump only the body without
losing metadata. An explicit `""`/`[]` clears them.

## Errors

Every error reaches the wire as `{"error": "..."}`. The revision conflict
additionally carries `current_revision` so clients can rebase and retry.
Authentication runs before query parsing, so an unauthenticated request with a
malformed query string gets a clean `401`, never a parser error.

## Audit

Mutations and their audit line commit in the **same transaction**, so a
rejected write never leaves an audit trail claiming it happened. The subject is
the immutable `key_id` (a 16-hex prefix of `sha256(key)`), not the reusable key
label — a label freed by revocation can be reassigned, so it is never used to
identify the actor. Audit rows are pruned to the newest 10 000.
