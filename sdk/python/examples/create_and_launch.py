"""Create a profile from a fixed seed, launch it, print the Puppeteer ws URL.

Run against a live ``mirage-api``::

    python examples/create_and_launch.py

If ``MIRAGE_API_TOKEN`` is set in the environment it is picked up automatically.
The script is idempotent-ish: it leaves the profile running so a downstream
tool (Playwright/Puppeteer) can attach to ``ws_url``. Stop it with
``client.stop(profile_id)`` when done.
"""

from __future__ import annotations

import os

from mirage import AdsPowerError, MirageApiError, MirageClient, MirageConnectionError


def main() -> int:
    token = os.environ.get("MIRAGE_API_TOKEN") or None
    base_url = os.environ.get("MIRAGE_API_BASE", "http://127.0.0.1:35500")
    client = MirageClient(base_url=base_url, token=token)

    try:
        health = client.health()
        print(f"server {health.get('version')} vault={health.get('vault')} running={health.get('running')}")
        if health.get("vault") != "unlocked":
            password = os.environ.get("MIRAGE_VAULT_PASSWORD")
            if not password:
                print("vault is locked; set MIRAGE_VAULT_PASSWORD to unlock it")
                return 1
            client.unlock_vault(password)
            print("vault unlocked")

        profile = client.create_profile(
            "Shop-01",
            tags=["example"],
            seed=20261103,
            os="windows",
        )
        profile_id = profile["id"]
        print(f"created profile {profile_id} ({profile.get('name')})")

        session = client.launch(profile_id)
        print(f"launched: target_id={session.get('target_id')} debug_port={session.get('debug_port')}")
        print(f"puppeteer ws_url: {session.get('ws_url')}")

        running = [entry["profile_id"] for entry in client.status()]
        print(f"running profiles: {running}")
        return 0
    except MirageApiError as exc:
        print(f"API error: {exc} (details={dict(exc.details)})")
        return 2
    except AdsPowerError as exc:
        print(f"AdsPower error: {exc}")
        return 3
    except MirageConnectionError as exc:
        print(f"cannot reach the API: {exc}")
        return 4


if __name__ == "__main__":
    raise SystemExit(main())
