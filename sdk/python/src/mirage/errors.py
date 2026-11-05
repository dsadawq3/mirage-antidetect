"""Exception hierarchy for the Mirage Python SDK.

Three failure families are modelled, matching the frozen contract in
``docs/API.md``:

* :class:`MirageConnectionError` -- the loopback API could not be reached at
  all (connection refused, DNS, socket timeout). Nothing was decoded.
* :class:`MirageApiError` -- a *native* Mirage endpoint answered with an HTTP
  error status and the body ``{"error": {"code", "message"}}``. Any additional
  keys the server adds to that error object are preserved in
  :attr:`MirageApiError.details`.
* :class:`AdsPowerError` -- an *AdsPower-compatibility* endpoint answered with
  HTTP 200 and the envelope ``{"code": <non-zero>, "msg": "..."}``. AdsPower
  signals failure inside the body, not via the status line.

All three derive from :class:`MirageError`, so ``except MirageError`` catches
every SDK-level failure.
"""

from __future__ import annotations

from typing import Any, Mapping, Optional

__all__ = [
    "MirageError",
    "MirageConnectionError",
    "MirageApiError",
    "AdsPowerError",
]


class MirageError(Exception):
    """Base class for every error raised by the Mirage SDK."""


class MirageConnectionError(MirageError):
    """The Mirage API was unreachable (network-level failure).

    Raised for connection refused, unknown host, route errors and socket
    timeouts. It means the request never produced an HTTP response, which
    normally means the ``mirage-api`` process is not running or the base URL is
    wrong.
    """

    def __init__(self, message: str, base_url: Optional[str] = None) -> None:
        self.base_url = base_url
        super().__init__(message)


class MirageApiError(MirageError):
    """A native Mirage endpoint returned an HTTP error.

    Attributes:
        code: The machine-readable error code from the body
            (``invalid_request``, ``not_found``, ``already_running``,
            ``not_running``, ``vault_locked``, ``proxy_failed``, ``io``,
            ``internal``, or any code the server adds).
        message: The human-readable message from the body.
        status: The HTTP status code of the response.
        details: The full error object from the body; any extra keys beyond
            ``code``/``message`` survive here.
    """

    def __init__(
        self,
        code: str,
        message: str,
        status: int,
        details: Optional[Mapping[str, Any]] = None,
    ) -> None:
        self.code = code
        self.message = message
        self.status = status
        self.details: Mapping[str, Any] = dict(details) if details else {}
        super().__init__(f"HTTP {status} [{code}] {message}")

    def __str__(self) -> str:  # pragma: no cover - cosmetic
        return f"HTTP {self.status} [{self.code}] {self.message}"


class AdsPowerError(MirageError):
    """An AdsPower-compatibility endpoint returned ``{"code": <non-zero>}``.

    The transport itself succeeded (HTTP 200); the failure is carried inside the
    AdsPower envelope.

    Attributes:
        code: The non-zero ``code`` from the envelope.
        msg: The ``msg`` string from the envelope.
        details: The full envelope, retained for debugging.
    """

    def __init__(
        self,
        code: Any,
        msg: str,
        details: Optional[Mapping[str, Any]] = None,
    ) -> None:
        self.code = code
        self.msg = msg
        self.details: Mapping[str, Any] = dict(details) if details else {}
        super().__init__(f"AdsPower error {code}: {msg}")

    def __str__(self) -> str:  # pragma: no cover - cosmetic
        return f"AdsPower error {self.code}: {self.msg}"
