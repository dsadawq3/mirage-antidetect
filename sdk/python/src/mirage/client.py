"""Python client for the Mirage local HTTP API (v1).

The SDK is a thin, typed wrapper over the frozen contract documented in
``docs/API.md``. It uses only the standard library (``urllib.request``) so it
imports anywhere Python 3.10+ runs, and it speaks to a *separate* process --
the ``mirage-api`` server -- over loopback.

Two endpoint families exist and they are handled differently on purpose:

* **Native** endpoints answer with real HTTP status codes and, on error, a body
  of ``{"error": {"code", "message"}}``. See :class:`~mirage.errors.MirageApiError`.
* **AdsPower-compatible** aliases always answer ``HTTP 200`` and carry success
  or failure in ``{"code": <0|non-zero>, "msg": "..."}``. See
  :class:`~mirage.errors.AdsPowerError`. The single exception is an invalid
  token, which is a genuine ``401``.

There are no retries: this is a local API, and a failure is a bug worth seeing,
not a transient to paper over.
"""

from __future__ import annotations

import json
import socket
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict, List, Mapping, Optional, Sequence, Union

from .errors import AdsPowerError, MirageApiError, MirageConnectionError

__all__ = ["MirageClient", "DEFAULT_BASE_URL", "DEFAULT_TIMEOUT"]

DEFAULT_BASE_URL = "http://127.0.0.1:35500"
DEFAULT_TIMEOUT = 30.0

_API_PREFIX = "/api/v1"

_COOKIE_FORMATS = frozenset({"netscape", "json"})
_IMPORT_SOURCES = frozenset({"adspower", "dolphin", "gologin"})

# A JSON object as returned by the API.
JsonDict = Dict[str, Any]


class MirageClient:
    """Synchronous client for the Mirage local API.

    Args:
        base_url: Root URL of the ``mirage-api`` process. Defaults to
            ``http://127.0.0.1:35500`` (the documented default port).
        token: Value of ``MIRAGE_API_TOKEN``. When set, every request carries
            ``Authorization: Bearer <token>``. When ``None`` the header is
            omitted, which is what a localhost-only server without auth wants.
        timeout: Per-request socket timeout in seconds.

    Example:
        >>> client = MirageClient()
        >>> client.health()["ok"]
        True
    """

    def __init__(
        self,
        base_url: str = DEFAULT_BASE_URL,
        token: Optional[str] = None,
        timeout: float = DEFAULT_TIMEOUT,
    ) -> None:
        if not base_url:
            raise ValueError("base_url must be a non-empty string")
        self._base_url = base_url.rstrip("/")
        self._token = token or None
        self._timeout = float(timeout)

    # ------------------------------------------------------------------ #
    # Introspection
    # ------------------------------------------------------------------ #

    @property
    def base_url(self) -> str:
        """The configured root URL (without a trailing slash)."""
        return self._base_url

    @property
    def token(self) -> Optional[str]:
        """The configured bearer token, or ``None``."""
        return self._token

    def __repr__(self) -> str:  # pragma: no cover - cosmetic
        # Never leak the token in a repr.
        return (
            f"MirageClient(base_url={self._base_url!r}, "
            f"token={'***' if self._token else None!r}, timeout={self._timeout})"
        )

    # ------------------------------------------------------------------ #
    # Transport
    # ------------------------------------------------------------------ #

    def _request(
        self,
        method: str,
        path: str,
        *,
        query: Optional[Mapping[str, Any]] = None,
        json_body: Optional[Any] = None,
    ) -> "tuple[int, bytes]":
        """Perform one HTTP round-trip and return ``(status, raw_body)``.

        HTTP error statuses are *not* raised here: they are returned so the
        native/AdsPower layers can interpret the body their own way. Only
        network-level failures raise, as :class:`MirageConnectionError`.
        """
        url = self._base_url + path
        if query:
            clean = {k: v for k, v in query.items() if v is not None}
            if clean:
                url += "?" + urllib.parse.urlencode(clean)

        data: Optional[bytes] = None
        headers: Dict[str, str] = {"accept": "application/json"}
        if json_body is not None:
            data = json.dumps(json_body).encode("utf-8")
            headers["content-type"] = "application/json"
        if self._token:
            headers["authorization"] = "Bearer " + self._token

        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=self._timeout) as response:
                return int(response.status), response.read()
        except urllib.error.HTTPError as exc:
            # A real HTTP response with a 4xx/5xx status; body is still useful.
            return int(exc.code), exc.read()
        except (urllib.error.URLError, TimeoutError, socket.timeout, OSError) as exc:
            raise MirageConnectionError(
                f"cannot reach Mirage API at {self._base_url}: {exc}",
                base_url=self._base_url,
            ) from exc

    @staticmethod
    def _load_json(body: bytes) -> Any:
        """Decode a JSON body, returning ``None`` for empty or invalid data."""
        if not body:
            return None
        try:
            return json.loads(body.decode("utf-8"))
        except (UnicodeDecodeError, ValueError):
            return None

    @staticmethod
    def _native_error(status: int, payload: Any, body: bytes) -> MirageApiError:
        """Build a :class:`MirageApiError` from an error response."""
        if isinstance(payload, dict) and isinstance(payload.get("error"), dict):
            error = payload["error"]
            code = str(error.get("code", "internal"))
            message = str(error.get("message", ""))
            return MirageApiError(code, message, status, details=error)
        # No usable envelope: surface the raw text so nothing is swallowed.
        text = body.decode("utf-8", "replace") if body else ""
        return MirageApiError(f"http_{status}", text, status)

    def _native(
        self,
        method: str,
        path: str,
        *,
        query: Optional[Mapping[str, Any]] = None,
        json_body: Optional[Any] = None,
    ) -> Any:
        """Call a native endpoint; raise :class:`MirageApiError` on HTTP >= 400."""
        status, body = self._request(method, path, query=query, json_body=json_body)
        payload = self._load_json(body)
        if status >= 400:
            raise self._native_error(status, payload, body)
        if payload is None:
            return {}
        return payload

    @staticmethod
    def _ads_ok(code: Any) -> bool:
        """AdsPower success is ``code == 0``; tolerate a stringified ``"0"``."""
        return code == 0 or code == "0"

    def _adspower(
        self,
        method: str,
        path: str,
        *,
        query: Optional[Mapping[str, Any]] = None,
        json_body: Optional[Any] = None,
    ) -> Any:
        """Call an AdsPower alias and unwrap ``data``.

        Raises:
            MirageApiError: on a ``401`` (invalid token) or any HTTP >= 400.
            AdsPowerError: on HTTP 200 with a non-zero ``code``.
        """
        status, body = self._request(method, path, query=query, json_body=json_body)
        payload = self._load_json(body)
        if status >= 400:
            # The documented exception: an invalid token is a genuine 401 and
            # is reported with the native error envelope.
            raise self._native_error(status, payload, body)
        if not isinstance(payload, dict) or "code" not in payload:
            raise MirageApiError(
                "internal",
                "malformed AdsPower envelope (no 'code' field)",
                status,
                details={"raw": payload},
            )
        if not self._ads_ok(payload.get("code")):
            raise AdsPowerError(payload.get("code"), str(payload.get("msg", "")), details=payload)
        return payload.get("data")

    # ------------------------------------------------------------------ #
    # Native: health / vault
    # ------------------------------------------------------------------ #

    def health(self) -> JsonDict:
        """``GET /api/v1/health``.

        Returns the server health object:
        ``{"ok": true, "version": "...", "vault": "locked"|"unlocked", "running": N}``.
        """
        return self._native("GET", f"{_API_PREFIX}/health")

    def unlock_vault(self, password: str) -> JsonDict:
        """``POST /api/v1/vault/unlock`` with ``{"password": ...}``.

        Unlocks the profile vault for the lifetime of the session (kept in
        memory by the server). Returns ``{}``.

        Raises:
            MirageApiError: with status ``401`` when the password is wrong.
        """
        return self._native("POST", f"{_API_PREFIX}/vault/unlock", json_body={"password": password})

    # ------------------------------------------------------------------ #
    # Native: profiles
    # ------------------------------------------------------------------ #

    def list_profiles(self) -> List[JsonDict]:
        """``GET /api/v1/profiles``.

        Returns the list of :class:`ProfileSummary` objects from
        ``{"profiles": [...]}``.
        """
        payload = self._native("GET", f"{_API_PREFIX}/profiles")
        profiles = payload.get("profiles", []) if isinstance(payload, dict) else []
        return list(profiles)

    def create_profile(
        self,
        name: str,
        *,
        tags: Optional[Sequence[str]] = None,
        seed: Optional[int] = None,
        os: Optional[str] = None,
        proxy: Optional[Mapping[str, Any]] = None,
        overrides: Optional[Mapping[str, Any]] = None,
    ) -> JsonDict:
        """``POST /api/v1/profiles`` -> ``201`` ``Profile``.

        Only the fields you pass are sent; omitted ones are left to the server
        (``seed`` unset means a random fingerprint is generated).

        Args:
            name: Human label for the profile.
            tags: Free-form labels.
            seed: Deterministic fingerprint seed. Same seed + ``os`` yields the
                same fingerprint.
            os: ``"windows"`` | ``"macos"`` | ``"linux"``.
            proxy: ``{"scheme", "host", "port", "username"?, "password"?}`` or
                ``None`` for a direct connection.
            overrides: Partial ``Fingerprint`` merged over the generated one and
                re-validated by the server.

        Returns:
            The created ``Profile`` object.
        """
        body: JsonDict = {"name": name}
        if tags is not None:
            body["tags"] = list(tags)
        if seed is not None:
            body["seed"] = seed
        if os is not None:
            body["os"] = os
        if proxy is not None:
            body["proxy"] = dict(proxy)
        if overrides is not None:
            body["overrides"] = dict(overrides)
        return self._native("POST", f"{_API_PREFIX}/profiles", json_body=body)

    def get_profile(self, profile_id: str) -> JsonDict:
        """``GET /api/v1/profiles/:id`` -> ``Profile`` (404 -> ``not_found``)."""
        return self._native("GET", f"{_API_PREFIX}/profiles/{self._quote(profile_id)}")

    def update_profile(self, profile_id: str, **fields: Any) -> JsonDict:
        """``PUT /api/v1/profiles/:id`` -> ``Profile``.

        Any subset of ``name``, ``tags``, ``proxy``, ``overrides``. Changing
        ``seed``/``overrides`` makes the server recompute the fingerprint.

        Raises:
            ValueError: when no fields are supplied (an empty PUT is a mistake).
        """
        if not fields:
            raise ValueError("update_profile requires at least one field to update")
        return self._native(
            "PUT",
            f"{_API_PREFIX}/profiles/{self._quote(profile_id)}",
            json_body=dict(fields),
        )

    def delete_profile(self, profile_id: str) -> None:
        """``DELETE /api/v1/profiles/:id`` -> ``204`` with no body."""
        self._native("DELETE", f"{_API_PREFIX}/profiles/{self._quote(profile_id)}")

    # ------------------------------------------------------------------ #
    # Native: launch / status
    # ------------------------------------------------------------------ #

    def launch(self, profile_id: str) -> JsonDict:
        """``POST /api/v1/profiles/:id/launch``.

        Returns ``{"target_id", "ws_url", "debug_port", "profile_dir"}``.

        Raises:
            MirageApiError: status ``409`` (``already_running``) if it is up.
        """
        return self._native("POST", f"{_API_PREFIX}/profiles/{self._quote(profile_id)}/launch")

    def stop(self, profile_id: str) -> JsonDict:
        """``POST /api/v1/profiles/:id/stop`` -> ``{}``.

        Raises:
            MirageApiError: status ``409`` (``not_running``) if it is down.
        """
        return self._native("POST", f"{_API_PREFIX}/profiles/{self._quote(profile_id)}/stop")

    def status(self) -> List[JsonDict]:
        """``GET /api/v1/status``.

        Returns the list of running entries from ``{"running": [...]}``; each is
        ``{"profile_id", "target_id", "debug_port", "started_at"}``.
        """
        payload = self._native("GET", f"{_API_PREFIX}/status")
        running = payload.get("running", []) if isinstance(payload, dict) else []
        return list(running)

    # ------------------------------------------------------------------ #
    # Native: proxy
    # ------------------------------------------------------------------ #

    def proxy_check(
        self,
        scheme: str,
        host: str,
        port: int,
        username: Optional[str] = None,
        password: Optional[str] = None,
    ) -> JsonDict:
        """``POST /api/v1/proxy/check``.

        Returns ``{"ok": bool, "ip"?, "latency_ms"?, "error"?}``. Note this is a
        successful HTTP call even when ``ok`` is ``false`` -- a failing proxy is
        data, not an exception.
        """
        body: JsonDict = {"scheme": scheme, "host": host, "port": port}
        if username is not None:
            body["username"] = username
        if password is not None:
            body["password"] = password
        return self._native("POST", f"{_API_PREFIX}/proxy/check", json_body=body)

    # ------------------------------------------------------------------ #
    # Native: cookies
    # ------------------------------------------------------------------ #

    def cookies_import(self, profile_id: str, data: str, format: str = "netscape") -> JsonDict:
        """``POST /api/v1/profiles/:id/cookies/import``.

        Args:
            profile_id: Target profile.
            data: Cookie payload as a string (Netscape text or JSON text).
            format: ``"netscape"`` (default) or ``"json"``.

        Returns:
            ``{"imported": N, "skipped": N}``.
        """
        format = self._normalize_format(format)
        return self._native(
            "POST",
            f"{_API_PREFIX}/profiles/{self._quote(profile_id)}/cookies/import",
            json_body={"format": format, "data": data},
        )

    def cookies_export(self, profile_id: str, format: str = "netscape") -> JsonDict:
        """``GET /api/v1/profiles/:id/cookies/export?format=...``.

        Returns ``{"format", "data"}``; the payload string is under ``["data"]``.
        """
        format = self._normalize_format(format)
        return self._native(
            "GET",
            f"{_API_PREFIX}/profiles/{self._quote(profile_id)}/cookies/export",
            query={"format": format},
        )

    # ------------------------------------------------------------------ #
    # Native: bulk import
    # ------------------------------------------------------------------ #

    def import_profiles(self, source: str, data: str) -> JsonDict:
        """``POST /api/v1/import``.

        Args:
            source: ``"adspower"`` | ``"dolphin"`` | ``"gologin"``.
            data: Raw export (JSON string) from that tool.

        Returns:
            ``{"imported": [Profile, ...], "warnings": [...]}``. Anything the
            mapper cannot translate shows up in ``warnings``, never silently
            dropped.
        """
        source = self._normalize_source(source)
        return self._native("POST", f"{_API_PREFIX}/import", json_body={"source": source, "data": data})

    # ------------------------------------------------------------------ #
    # AdsPower-compatible aliases
    # ------------------------------------------------------------------ #

    def browser_start(self, user_id: str) -> JsonDict:
        """AdsPower ``GET /api/v1/browser/start?user_id=...``.

        Returns the ``data`` object:
        ``{"ws": {"puppeteer": "<ws_url>"}, "debug_port": ..., "target_id": ...}``.
        The Puppeteer websocket URL is ``data["ws"]["puppeteer"]``.
        """
        return self._adspower("GET", f"{_API_PREFIX}/browser/start", query={"user_id": user_id})

    def browser_stop(self, user_id: str) -> JsonDict:
        """AdsPower ``GET /api/v1/browser/stop?user_id=...`` -> ``{}``."""
        return self._adspower("GET", f"{_API_PREFIX}/browser/stop", query={"user_id": user_id})

    def browser_list(self, page: int = 1, page_size: int = 50) -> JsonDict:
        """AdsPower ``GET /api/v1/browser/list?page=&page_size=``.

        Returns ``{"list": [{"user_id", "name", "group_id", "status"}], "page",
        "page_size", "total"}``.
        """
        return self._adspower(
            "GET",
            f"{_API_PREFIX}/browser/list",
            query={"page": page, "page_size": page_size},
        )

    def user_create(self, payload: Mapping[str, Any]) -> JsonDict:
        """AdsPower ``POST /api/v1/user/create``.

        ``payload`` is an AdsPower-style profile body. Returns
        ``{"id": "<profile_id>", "warnings": [...]}``.
        """
        return self._adspower("POST", f"{_API_PREFIX}/user/create", json_body=dict(payload))

    def user_update(self, payload: Mapping[str, Any]) -> JsonDict:
        """AdsPower ``POST /api/v1/user/update``.

        ``payload`` must carry ``user_id`` plus the fields to change. Returns
        ``{}``.
        """
        if "user_id" not in payload:
            raise ValueError("user_update payload must contain 'user_id'")
        return self._adspower("POST", f"{_API_PREFIX}/user/update", json_body=dict(payload))

    def user_delete(self, user_id: Union[str, Sequence[str]]) -> JsonDict:
        """AdsPower ``POST /api/v1/user/delete``.

        Accepts a single id (sent as ``{"user_id": ...}``) or a sequence of ids
        (sent as ``{"user_ids": [...]}``). Returns ``{}``.
        """
        if isinstance(user_id, str):
            body: JsonDict = {"user_id": user_id}
        else:
            body = {"user_ids": list(user_id)}
        return self._adspower("POST", f"{_API_PREFIX}/user/delete", json_body=body)

    # ------------------------------------------------------------------ #
    # Helpers
    # ------------------------------------------------------------------ #

    @staticmethod
    def _quote(value: str) -> str:
        """Percent-encode a path segment (UUIDs are safe, but stay strict)."""
        return urllib.parse.quote(str(value), safe="")

    @staticmethod
    def _normalize_format(format: str) -> str:
        """Canonicalize a cookie ``format`` the way the server does.

        The API trims and lower-cases the value before matching, so ``"JSON"``
        and ``" JSON "`` are valid there. The SDK mirrors that (never stricter
        than the server) and returns the canonical lower-case form.
        """
        normalized = str(format).strip().lower()
        if normalized not in _COOKIE_FORMATS:
            raise ValueError(
                f"unknown cookie format {format!r}; expected one of {sorted(_COOKIE_FORMATS)}"
            )
        return normalized

    @staticmethod
    def _normalize_source(source: str) -> str:
        """Canonicalize an import ``source`` the way the server does.

        Case/whitespace-insensitive, matching ``mirage-api``: ``"Adspower"`` and
        ``" adspower "`` are accepted and sent as ``"adspower"``.
        """
        normalized = str(source).strip().lower()
        if normalized not in _IMPORT_SOURCES:
            raise ValueError(
                f"unknown import source {source!r}; expected one of {sorted(_IMPORT_SOURCES)}"
            )
        return normalized
