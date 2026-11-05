"""Boundary tests for :class:`mirage.client.MirageClient`.

Every test drives a real HTTP round-trip against :class:`fake_server.FakeMirageAPI`
(a real ``http.server`` on loopback), so urllib, header plumbing, JSON encoding
and the error decoders are all exercised honestly.

Runnable either way::

    python -m pytest tests -q
    python -m unittest discover -s tests -v
"""

from __future__ import annotations

import os
import socket
import sys
import unittest

# Allow running this file directly (``python tests/test_client.py``) as well as
# through pytest/unittest, without an install.
_HERE = os.path.dirname(os.path.abspath(__file__))
_SRC = os.path.join(os.path.dirname(_HERE), "src")
for _path in (_HERE, _SRC):
    if _path not in sys.path:
        sys.path.insert(0, _path)

from fake_server import FakeMirageAPI  # noqa: E402

from mirage import (  # noqa: E402
    AdsPowerError,
    MirageApiError,
    MirageClient,
    MirageConnectionError,
)


def _closed_port() -> int:
    """Return a TCP port nothing is listening on (bind, read, close)."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


class ClientTestCase(unittest.TestCase):
    """Base class spinning up a fresh fake API per test."""

    def setUp(self) -> None:
        self.server = FakeMirageAPI().start()
        self.addCleanup(self.server.stop)

    def client(self, token: str | None = None) -> MirageClient:
        return MirageClient(base_url=self.server.base_url, token=token, timeout=5)


# --------------------------------------------------------------------------- #
# Native endpoints
# --------------------------------------------------------------------------- #

class TestNativeEndpoints(ClientTestCase):
    def test_health(self) -> None:
        self.server.on(
            "GET",
            "/api/v1/health",
            200,
            {"ok": True, "version": "0.1.0", "vault": "locked", "running": 0},
        )
        result = self.client().health()
        self.assertEqual(result["ok"], True)
        self.assertEqual(result["version"], "0.1.0")
        self.assertEqual(self.server.last_request().method, "GET")

    def test_unlock_vault_sends_password(self) -> None:
        self.server.on("POST", "/api/v1/vault/unlock", 200, {})
        result = self.client().unlock_vault("s3cret")
        self.assertEqual(result, {})
        self.assertEqual(self.server.last_request().json, {"password": "s3cret"})

    def test_create_profile_sends_only_supplied_fields(self) -> None:
        profile = {
            "id": "6f9619ff-8b86-d011-b42d-00cf4fc964ff",
            "name": "Shop-01",
            "tags": ["work"],
            "fingerprint": {"os": "windows"},
            "proxy": None,
            "notes": "",
            "createdAt": "2026-11-03T12:00:00Z",
            "updatedAt": "2026-11-03T12:00:00Z",
        }
        self.server.on("POST", "/api/v1/profiles", 201, profile)

        created = self.client().create_profile("Shop-01", tags=["work"], os="windows", seed=20261103)

        self.assertEqual(created["id"], profile["id"])
        sent = self.server.last_request().json
        self.assertEqual(
            sent,
            {"name": "Shop-01", "tags": ["work"], "os": "windows", "seed": 20261103},
        )
        # Fields left as None must not appear at all.
        self.assertNotIn("proxy", sent)
        self.assertNotIn("overrides", sent)

    def test_create_profile_with_proxy_and_overrides(self) -> None:
        self.server.on("POST", "/api/v1/profiles", 201, {"id": "x", "name": "p"})
        self.client().create_profile(
            "p",
            proxy={"scheme": "socks5", "host": "1.2.3.4", "port": 1080},
            overrides={"timezone": "Europe/Paris", "languages": ["fr-FR", "fr"]},
        )
        sent = self.server.last_request().json
        self.assertEqual(sent["proxy"]["host"], "1.2.3.4")
        self.assertEqual(sent["overrides"]["timezone"], "Europe/Paris")

    def test_get_update_delete_profile(self) -> None:
        pid = "6f9619ff-8b86-d011-b42d-00cf4fc964ff"
        self.server.on("GET", f"/api/v1/profiles/{pid}", 200, {"id": pid, "name": "A"})
        self.server.on("PUT", f"/api/v1/profiles/{pid}", 200, {"id": pid, "name": "B"})
        self.server.on("DELETE", f"/api/v1/profiles/{pid}", 204, None)

        client = self.client()
        self.assertEqual(client.get_profile(pid)["name"], "A")
        self.assertEqual(client.update_profile(pid, name="B")["name"], "B")
        self.assertIsNone(client.delete_profile(pid))
        self.assertEqual(self.server.last_request().method, "DELETE")
        self.assertEqual(self.server.requests[1].json, {"name": "B"})

    def test_update_profile_requires_a_field(self) -> None:
        with self.assertRaises(ValueError):
            self.client().update_profile("id")

    def test_list_profiles_returns_list(self) -> None:
        summaries = [
            {"id": "a", "name": "A", "tags": [], "running": False, "proxy": None,
             "createdAt": "t", "updatedAt": "t"},
            {"id": "b", "name": "B", "tags": ["x"], "running": True, "proxy": None,
             "createdAt": "t", "updatedAt": "t"},
        ]
        self.server.on("GET", "/api/v1/profiles", 200, {"profiles": summaries})
        result = self.client().list_profiles()
        self.assertIsInstance(result, list)
        self.assertEqual([p["id"] for p in result], ["a", "b"])

    def test_launch_and_status_and_stop(self) -> None:
        pid = "abc"
        session = {"target_id": "t1", "ws_url": "ws://127.0.0.1:9222/devtools/browser/x",
                   "debug_port": 9222, "profile_dir": "C:/x"}
        self.server.on("POST", f"/api/v1/profiles/{pid}/launch", 200, session)
        self.server.on("POST", f"/api/v1/profiles/{pid}/stop", 200, {})
        self.server.on("GET", "/api/v1/status", 200,
                       {"running": [{"profile_id": pid, "target_id": "t1",
                                     "debug_port": 9222, "started_at": "t"}]})

        client = self.client()
        self.assertEqual(client.launch(pid)["ws_url"], session["ws_url"])
        running = client.status()
        self.assertEqual(running[0]["profile_id"], pid)
        self.assertEqual(client.stop(pid), {})

    def test_proxy_check_ok_false_is_not_an_error(self) -> None:
        self.server.on("POST", "/api/v1/proxy/check", 200,
                       {"ok": False, "error": "connection refused"})
        result = self.client().proxy_check("socks5", "1.2.3.4", 1080, username="u", password="p")
        self.assertFalse(result["ok"])
        sent = self.server.last_request().json
        self.assertEqual(sent, {"scheme": "socks5", "host": "1.2.3.4", "port": 1080,
                                "username": "u", "password": "p"})

    def test_cookies_roundtrip(self) -> None:
        pid = "abc"
        self.server.on("GET", f"/api/v1/profiles/{pid}/cookies/export", 200,
                       {"format": "netscape", "data": "# Netscape HTTP Cookie File\n"})
        self.server.on("POST", f"/api/v1/profiles/{pid}/cookies/import", 200,
                       {"imported": 3, "skipped": 1})
        client = self.client()
        exported = client.cookies_export(pid, format="netscape")
        self.assertEqual(exported["format"], "netscape")
        self.assertEqual(self.server.last_request().query, {"format": "netscape"})

        imported = client.cookies_import(pid, exported["data"], format="netscape")
        self.assertEqual(imported, {"imported": 3, "skipped": 1})
        self.assertEqual(self.server.last_request().json,
                         {"format": "netscape", "data": exported["data"]})

    def test_cookies_rejects_bad_format(self) -> None:
        with self.assertRaises(ValueError):
            self.client().cookies_export("id", format="xml")

    def test_import_profiles_with_warnings(self) -> None:
        result = {
            "imported": [{"id": "p1", "name": "Imported"}],
            "warnings": ["profile 'old' uses unsupported webrtc mode"],
        }
        self.server.on("POST", "/api/v1/import", 200, result)
        got = self.client().import_profiles("adspower", '{"user_id":"1"}')
        self.assertEqual(got["imported"][0]["id"], "p1")
        self.assertEqual(got["warnings"], result["warnings"])
        self.assertEqual(self.server.last_request().json,
                         {"source": "adspower", "data": '{"user_id":"1"}'})

    def test_import_profiles_rejects_unknown_source(self) -> None:
        with self.assertRaises(ValueError):
            self.client().import_profiles("multilogin", "{}")

    def test_cookies_format_is_case_insensitive(self) -> None:
        # The server trims/lower-cases the value; the SDK mirrors that and sends
        # the canonical form instead of rejecting "JSON".
        pid = "abc"
        self.server.on("GET", f"/api/v1/profiles/{pid}/cookies/export", 200,
                       {"format": "json", "data": "[]"})
        self.client().cookies_export(pid, format="JSON")
        self.assertEqual(self.server.last_request().query, {"format": "json"})

    def test_import_source_is_case_insensitive(self) -> None:
        self.server.on("POST", "/api/v1/import", 200, {"imported": [], "warnings": []})
        self.client().import_profiles("Adspower", "{}")
        self.assertEqual(self.server.last_request().json,
                         {"source": "adspower", "data": "{}"})


# --------------------------------------------------------------------------- #
# Auth
# --------------------------------------------------------------------------- #

class TestAuth(ClientTestCase):
    def test_token_is_sent_as_bearer(self) -> None:
        self.server.on("GET", "/api/v1/health", 200,
                       {"ok": True, "version": "0.1.0", "vault": "locked", "running": 0})
        self.client(token="s3cr3t").health()
        self.assertEqual(self.server.last_request().header("authorization"), "Bearer s3cr3t")

    def test_no_token_omits_authorization_header(self) -> None:
        self.server.on("GET", "/api/v1/health", 200,
                       {"ok": True, "version": "0.1.0", "vault": "locked", "running": 0})
        self.client().health()
        self.assertIsNone(self.server.last_request().header("authorization"))

    def test_401_raises_mirage_api_error(self) -> None:
        self.server.on("POST", "/api/v1/vault/unlock", 401,
                       {"error": {"code": "invalid_request", "message": "wrong password"}})
        with self.assertRaises(MirageApiError) as ctx:
            self.client().unlock_vault("nope")
        err = ctx.exception
        self.assertEqual(err.status, 401)
        self.assertEqual(err.code, "invalid_request")
        self.assertEqual(err.message, "wrong password")


# --------------------------------------------------------------------------- #
# Native error mapping
# --------------------------------------------------------------------------- #

class TestNativeErrors(ClientTestCase):
    def test_404_not_found(self) -> None:
        self.server.on("GET", "/api/v1/profiles/missing", 404,
                       {"error": {"code": "not_found", "message": "no such profile"}})
        with self.assertRaises(MirageApiError) as ctx:
            self.client().get_profile("missing")
        self.assertEqual(ctx.exception.code, "not_found")
        self.assertEqual(ctx.exception.status, 404)

    def test_409_preserves_extra_error_keys(self) -> None:
        # The native 409 envelope is code/message only, but any extra keys the
        # server adds must survive verbatim in .details.
        self.server.on("POST", "/api/v1/profiles/abc/launch", 409,
                       {"error": {"code": "already_running", "message": "profile is running",
                                  "conflict_id": "abc"}})
        with self.assertRaises(MirageApiError) as ctx:
            self.client().launch("abc")
        err = ctx.exception
        self.assertEqual(err.code, "already_running")
        self.assertEqual(err.status, 409)
        self.assertEqual(err.details.get("conflict_id"), "abc")

    def test_non_envelope_error_still_raises(self) -> None:
        self.server.on("GET", "/api/v1/health", 500, {"unexpected": True})
        with self.assertRaises(MirageApiError) as ctx:
            self.client().health()
        self.assertEqual(ctx.exception.status, 500)
        self.assertEqual(ctx.exception.code, "http_500")


# --------------------------------------------------------------------------- #
# AdsPower compatibility layer
# --------------------------------------------------------------------------- #

class TestAdsPower(ClientTestCase):
    def test_browser_start_unwraps_data(self) -> None:
        self.server.on("GET", "/api/v1/browser/start", 200,
                       {"code": 0, "msg": "success",
                        "data": {"ws": {"puppeteer": "ws://127.0.0.1:9222/devtools/browser/z"},
                                 "debug_port": "9222", "target_id": "t1"}})
        data = self.client().browser_start("user-1")
        self.assertEqual(data["ws"]["puppeteer"], "ws://127.0.0.1:9222/devtools/browser/z")
        self.assertEqual(self.server.last_request().query, {"user_id": "user-1"})

    def test_browser_stop_and_list(self) -> None:
        self.server.on("GET", "/api/v1/browser/stop", 200, {"code": 0, "msg": "success", "data": {}})
        self.server.on("GET", "/api/v1/browser/list", 200,
                       {"code": 0, "msg": "success",
                        "data": {"list": [{"user_id": "u1", "name": "A", "group_id": "0",
                                           "status": "Active"}],
                                 "page": 1, "page_size": 50, "total": 1}})
        client = self.client()
        self.assertEqual(client.browser_stop("u1"), {})
        page = client.browser_list(page=1, page_size=50)
        self.assertEqual(page["total"], 1)
        self.assertEqual(self.server.last_request().query, {"page": "1", "page_size": "50"})

    def test_user_create_update_delete(self) -> None:
        self.server.on("POST", "/api/v1/user/create", 200,
                       {"code": 0, "msg": "success", "data": {"id": "new-id", "warnings": []}})
        self.server.on("POST", "/api/v1/user/update", 200, {"code": 0, "msg": "success", "data": {}})
        self.server.on("POST", "/api/v1/user/delete", 200, {"code": 0, "msg": "success", "data": {}})
        client = self.client()
        created = client.user_create({"name": "Ads", "user_proxy_config": {"proxy_soft": "no_proxy"}})
        self.assertEqual(created["id"], "new-id")
        self.assertEqual(client.user_update({"user_id": "new-id", "name": "Renamed"}), {})
        self.assertEqual(client.user_delete("new-id"), {})
        self.assertEqual(self.server.last_request().json, {"user_id": "new-id"})

    def test_user_delete_accepts_a_list(self) -> None:
        self.server.on("POST", "/api/v1/user/delete", 200, {"code": 0, "msg": "success", "data": {}})
        self.client().user_delete(["a", "b"])
        self.assertEqual(self.server.last_request().json, {"user_ids": ["a", "b"]})

    def test_user_update_requires_user_id(self) -> None:
        with self.assertRaises(ValueError):
            self.client().user_update({"name": "no id"})

    def test_envelope_nonzero_code_raises_adspower_error(self) -> None:
        self.server.on("GET", "/api/v1/browser/start", 200,
                       {"code": 1, "msg": "profile not found", "data": None})
        with self.assertRaises(AdsPowerError) as ctx:
            self.client().browser_start("ghost")
        self.assertEqual(ctx.exception.code, 1)
        self.assertEqual(ctx.exception.msg, "profile not found")

    def test_adspower_401_invalid_token_raises_mirage_api_error(self) -> None:
        # The documented exception: an invalid token is a genuine 401.
        self.server.on("GET", "/api/v1/browser/list", 401,
                       {"error": {"code": "invalid_request", "message": "bad token"}})
        with self.assertRaises(MirageApiError) as ctx:
            self.client(token="wrong").browser_list()
        self.assertEqual(ctx.exception.status, 401)

    def test_malformed_envelope_raises_mirage_api_error(self) -> None:
        self.server.on("GET", "/api/v1/browser/list", 200, {"oops": "no code"})
        with self.assertRaises(MirageApiError):
            self.client().browser_list()


# --------------------------------------------------------------------------- #
# Transport
# --------------------------------------------------------------------------- #

class TestTransport(unittest.TestCase):
    def test_connection_error_on_closed_port(self) -> None:
        # On this platform a connect to a closed loopback port times out rather
        # than being refused, so keep the timeout short.
        client = MirageClient(base_url=f"http://127.0.0.1:{_closed_port()}", timeout=1)
        with self.assertRaises(MirageConnectionError):
            client.health()

    def test_repr_hides_token(self) -> None:
        client = MirageClient(token="topsecret")
        self.assertNotIn("topsecret", repr(client))

    def test_empty_base_url_rejected(self) -> None:
        with self.assertRaises(ValueError):
            MirageClient(base_url="")


if __name__ == "__main__":
    unittest.main(verbosity=2)
