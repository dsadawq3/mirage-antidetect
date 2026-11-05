"""Import an AdsPower export via ``POST /api/v1/import`` and show warnings.

Pass the path to an AdsPower JSON export::

    python examples/import_ads_power.py adspower_export.json

The server maps what it can and returns ``imported`` profiles plus a
``warnings`` list for everything it could not translate. Nothing is dropped
silently, so warnings are always worth printing.
"""

from __future__ import annotations

import json
import os
import sys

from mirage import MirageApiError, MirageClient, MirageConnectionError


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: python examples/import_ads_power.py <adspower_export.json>")
        return 2

    path = argv[1]
    with open(path, "r", encoding="utf-8") as handle:
        data = handle.read()

    # Validate that it is JSON before sending; the API expects a JSON string.
    try:
        json.loads(data)
    except ValueError as exc:
        print(f"{path} is not valid JSON: {exc}")
        return 2

    client = MirageClient(
        base_url=os.environ.get("MIRAGE_API_BASE", "http://127.0.0.1:35500"),
        token=os.environ.get("MIRAGE_API_TOKEN") or None,
    )
    try:
        result = client.import_profiles("adspower", data)
    except MirageApiError as exc:
        print(f"API error: {exc}")
        return 2
    except MirageConnectionError as exc:
        print(f"cannot reach the API: {exc}")
        return 4

    imported = result.get("imported", [])
    warnings = result.get("warnings", [])
    print(f"imported {len(imported)} profile(s)")
    for profile in imported:
        print(f"  - {profile.get('id')}  {profile.get('name')}")
    if warnings:
        print(f"{len(warnings)} warning(s):")
        for warning in warnings:
            print(f"  ! {warning}")
    else:
        print("no warnings")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
