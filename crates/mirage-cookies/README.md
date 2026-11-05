# mirage-cookies

Cookie import and export for Mirage profiles.

A Mirage profile is a Chromium `user-data-dir`, the value passed as
`--user-data-dir=X`. This crate moves cookies between that store and two
portable formats:

* Netscape `cookies.txt` (`parse_netscape` / `serialize_netscape`);
* JSON (`parse_json` / `serialize_json`), the `Cookie` array shape in
  `camelCase`.

The high-level helpers `import_netscape` / `import_json` write into a profile,
and `export_netscape` / `export_json` read out of one. The profile-facing
functions are Windows only, because the cookie key is protected with DPAPI. The
parsers and serializers work on every platform.

## Profile directory layout

Chromium does not keep the cookie database at the root of the user-data-dir.
Under `--user-data-dir=X` (and no `--profile-directory`, which is what Mirage
does) it creates:

```
X/
  Local State                          # the AES key, os_crypt.encrypted_key
  Default/
    Network/
      Cookies                          # the SQLite cookie store
```

`Local State` sits at the root of `X`. The cookie database sits under a profile
subdirectory, normally `Default`, not next to `Local State`. A store created by
an older build of this crate used the sibling layout `X/Network/Cookies`, which
real Chrome never produces; that is fixed.

`chrome_db::cookies_db_path` resolves the database in this order:

1. `X/Default/Network/Cookies`;
2. the profile named by `Local State`'s `profile.last_used`;
3. any profile in `Local State`'s `profile.info_cache`;
4. otherwise `CookieError::NoProfileDb` naming the expected default path.

`oscrypt::load_key` always reads `X/Local State` (the root), which is correct.

## Encryption

Values are `v10` blobs: `"v10" || nonce[12] || AES-256-GCM(plaintext)`, with a
`SHA-256(domain)` prefix added by Chromium schema v24. This crate writes that
prefix and strips it on read, tolerating older values without it.

Chromium 127 and newer can store cookies with app-bound encryption, the `v20`
prefix (`os_crypt.app_bound_encrypted_key`). This crate does not implement
app-bound encryption.

* On read, `v20` and unknown-prefix rows are skipped, never fatal.
* `read_cookies_with_stats` reports how many rows were skipped
  (`ReadStats.skipped_unsupported`).
* If an export would be empty only because every cookie is app-bound, the
  export fails with `CookieError::UnsupportedEncryption` and the count, instead
  of returning an empty document. Use `export_netscape_with_stats` /
  `export_json_with_stats` to see `{ exported, skipped_unsupported }` when some
  cookies are readable.

## Schema version

The `cookies` table schema is Chromium v24. Before writing, the database's
`meta.version` must be 24 or newer. Writing v24 values (with the
`SHA-256(domain)` prefix) into a pre-v24 database would be corrupted again when
Chromium runs its own V23 to V24 migration, so `write_cookies` refuses with
`CookieError::SchemaTooOld { found }` rather than damaging the store. Open the
profile once with a current Chromium to migrate it first.

## Known approximations

The seven columns of Chromium's unique index are `(host_key,
top_frame_site_key, has_cross_site_ancestor, name, path, source_scheme,
source_port)`. Aside from host, name and path, none of them is carried by a
portable cookie format, so an imported cookie has no true provenance. This
crate resolves them in two steps:

* If a row already exists for the same logical cookie (same host, partition,
  name and path), that row's own `has_cross_site_ancestor`, `source_scheme` and
  `source_port` are reused. That is what lets an import replace a cookie
  Chromium wrote, instead of keying a second row beside it (Chromium stores
  concrete values such as `source_port` 443).
* Otherwise the defaults are used: `has_cross_site_ancestor` 1, the
  `Secure`-derived `source_scheme` and `source_port` -1.

`has_cross_site_ancestor` 1 is Chromium's value for an **unpartitioned** cookie
(empty `top_frame_site_key`), verified against a live Chrome 156 store.

The `Secure`-based `source_scheme` is only a starting default. Chromium records
the scheme of the origin that set the cookie (`0` unset, `1` non-secure, `2`
secure), which a portable format does not carry; `Secure` maps to 2, otherwise
1. `source_type`, `samesite` and `priority` are likewise best-effort constants
(1, -1, 1). Cookies are always written unpartitioned (empty
`top_frame_site_key`).

Every SQLite connection uses a 500 ms busy timeout. This is best effort: it
turns a brief overlap with a running browser into a short wait and a persistent
hold into `CookieError::DbLocked`, but the caller should still do cookie import
and export while the browser is closed.

## Tests

```bash
cargo test -p mirage-cookies
```

The live end-to-end test spawns a real Chrome window, so it is ignored and
gated on an environment variable:

```bash
MIRAGE_E2E=1 cargo test -p mirage-cookies --test live_roundtrip -- --ignored --nocapture
```

It launches a temporary profile through `mirage-launcher`, stops it, asserts
Chromium created `X/Default/Network/Cookies`, imports three cookies (a session
one and a secure one included), exports them back byte for byte, and checks
that an all-app-bound store fails the export with `UnsupportedEncryption`.
