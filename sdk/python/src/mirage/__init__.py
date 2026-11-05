"""Mirage Python SDK -- client for the Mirage local HTTP API (v1).

Public surface:

* :class:`~mirage.client.MirageClient` -- the client.
* :class:`~mirage.errors.MirageError`, :class:`~mirage.errors.MirageApiError`,
  :class:`~mirage.errors.MirageConnectionError`,
  :class:`~mirage.errors.AdsPowerError` -- the exception hierarchy.

Quick start::

    from mirage import MirageClient

    client = MirageClient()            # defaults to http://127.0.0.1:35500
    profile = client.create_profile("Shop-01", os="windows", seed=20261103)
    session = client.launch(profile["id"])
    print(session["ws_url"])
"""

from __future__ import annotations

from .client import DEFAULT_BASE_URL, DEFAULT_TIMEOUT, MirageClient
from .errors import (
    AdsPowerError,
    MirageApiError,
    MirageConnectionError,
    MirageError,
)

__version__ = "0.1.0"

__all__ = [
    "MirageClient",
    "DEFAULT_BASE_URL",
    "DEFAULT_TIMEOUT",
    "MirageError",
    "MirageApiError",
    "MirageConnectionError",
    "AdsPowerError",
    "__version__",
]
