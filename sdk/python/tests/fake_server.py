"""A tiny fake of the Mirage HTTP API for boundary tests.

It runs a real ``http.server.ThreadingHTTPServer`` on an ephemeral loopback
port in a background thread, dispatches by ``(method, path)`` to responder
callables returning ``(status, body)``, and records every request so tests can
assert on headers, query strings and JSON bodies.

This is deliberately *not* a mock of the client's internals: it is a real HTTP
server on the other side of a real socket, so the SDK's urllib transport,
header handling, JSON encoding and error decoding are all exercised for real.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Callable, Dict, List, Optional, Tuple
from urllib.parse import parse_qs, urlparse

Responder = Callable[["RequestRecord"], Tuple[int, Any]]


class RequestRecord:
    """One captured HTTP request."""

    def __init__(
        self,
        method: str,
        path: str,
        query: Dict[str, str],
        headers: Dict[str, str],
        body: bytes,
    ) -> None:
        self.method = method
        self.path = path
        self.query = query
        self.headers = headers
        self.body = body

    @property
    def json(self) -> Any:
        """Decoded JSON body, or ``None`` when empty/invalid."""
        if not self.body:
            return None
        try:
            return json.loads(self.body.decode("utf-8"))
        except (UnicodeDecodeError, ValueError):
            return None

    def header(self, name: str) -> Optional[str]:
        return self.headers.get(name.lower())

    def __repr__(self) -> str:  # pragma: no cover - cosmetics
        return f"<Request {self.method} {self.path} body={self.body!r}>"


class FakeMirageAPI:
    """Context manager that serves canned responses over loopback HTTP."""

    def __init__(self) -> None:
        self.routes: Dict[Tuple[str, str], Responder] = {}
        self.requests: List[RequestRecord] = []
        self._server: Optional[ThreadingHTTPServer] = None
        self._thread: Optional[threading.Thread] = None
        self.port: Optional[int] = None

    # -- route registration ------------------------------------------------ #

    def on(
        self,
        method: str,
        path: str,
        status: int = 200,
        body: Any = None,
    ) -> None:
        """Register a fixed ``(status, body)`` response for ``method path``."""

        def responder(_record: RequestRecord) -> Tuple[int, Any]:
            return status, body

        self.routes[(method.upper(), path)] = responder

    def on_handler(self, method: str, path: str, responder: Responder) -> None:
        """Register a callback that computes ``(status, body)`` from the request."""
        self.routes[(method.upper(), path)] = responder

    # -- lifecycle --------------------------------------------------------- #

    def start(self) -> "FakeMirageAPI":
        api = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            # Disable Nagle: the canned responses are tiny and written in several
            # pieces, which otherwise costs ~0.5s per request on loopback.
            disable_nagle_algorithm = True

            def log_message(self, *args: Any) -> None:  # silence stderr spam
                pass

            def _dispatch(self) -> None:
                parsed = urlparse(self.path)
                length = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(length) if length else b""
                query = {k: v[0] for k, v in parse_qs(parsed.query).items()}
                headers = {k.lower(): v for k, v in self.headers.items()}
                record = RequestRecord(self.command, parsed.path, query, headers, raw)
                api.requests.append(record)

                responder = api.routes.get((self.command, parsed.path))
                if responder is None:
                    status, payload = 404, {
                        "error": {"code": "not_found", "message": f"no fake route {self.command} {parsed.path}"}
                    }
                else:
                    status, payload = responder(record)

                if status == 204 or payload is None:
                    data = b""
                else:
                    data = json.dumps(payload).encode("utf-8")

                self.send_response(status)
                self.send_header("content-length", str(len(data)))
                if data:
                    self.send_header("content-type", "application/json")
                self.end_headers()
                if self.command != "HEAD" and data:
                    self.wfile.write(data)

            def do_GET(self) -> None:
                self._dispatch()

            def do_POST(self) -> None:
                self._dispatch()

            def do_PUT(self) -> None:
                self._dispatch()

            def do_DELETE(self) -> None:
                self._dispatch()

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = int(self._server.server_address[1])
        # Small poll interval so teardown in tests is fast, not a 0.5s stall.
        self._thread = threading.Thread(
            target=self._server.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True
        )
        self._thread.start()
        return self

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
        if self._thread is not None:
            self._thread.join(timeout=5)

    @property
    def base_url(self) -> str:
        if self.port is None:
            raise RuntimeError("server not started")
        return f"http://127.0.0.1:{self.port}"

    def last_request(self) -> RequestRecord:
        return self.requests[-1]

    def __enter__(self) -> "FakeMirageAPI":
        return self.start()

    def __exit__(self, *_exc: Any) -> None:
        self.stop()
