"""Round-trip cookies: export a profile's cookies, print them, then re-import.

Usage::

    python examples/cookie_roundtrip.py <profile_id> [netscape|json]

Exercises ``cookies_export`` then ``cookies_import`` on the same profile,
showing the ``imported``/``skipped`` counters the API returns.
"""

from __future__ import annotations

import os
import sys

from mirage import MirageApiError, MirageClient, MirageConnectionError


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3):
        print("usage: python examples/cookie_roundtrip.py <profile_id> [netscape|json]")
        return 2

    profile_id = argv[1]
    fmt = argv[2] if len(argv) == 3 else "netscape"

    client = MirageClient(
        base_url=os.environ.get("MIRAGE_API_BASE", "http://127.0.0.1:35500"),
        token=os.environ.get("MIRAGE_API_TOKEN") or None,
    )

    try:
        exported = client.cookies_export(profile_id, format=fmt)
    except MirageApiError as exc:
        print(f"export failed: {exc}")
        return 2
    except MirageConnectionError as exc:
        print(f"cannot reach the API: {exc}")
        return 4

    data = exported.get("data", "")
    print(f"exported format={exported.get('format')} bytes={len(data)}")
    print("---- cookies ----")
    print(data)
    print("-----------------")

    try:
        result = client.cookies_import(profile_id, data, format=fmt)
    except MirageApiError as exc:
        print(f"import failed: {exc}")
        return 2

    print(f"re-imported: imported={result.get('imported')} skipped={result.get('skipped')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
