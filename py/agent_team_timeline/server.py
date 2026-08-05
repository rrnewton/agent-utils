"""Loopback-only static server for a generated timeline archive."""

from __future__ import annotations

import functools
import http.server
import threading
import webbrowser
from pathlib import Path


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    """A static handler that adds safe defaults and keeps routine requests quiet."""

    def end_headers(self) -> None:
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Cache-Control", "no-cache")
        super().end_headers()

    def log_message(self, format: str, *args: object) -> None:
        if self.command != "GET" or self.path not in ("/", "/index.html"):
            super().log_message(format, *args)


def make_server(
    archive: Path, host: str = "127.0.0.1", port: int = 8765
) -> http.server.ThreadingHTTPServer:
    """Create, but do not start, a server rooted at *archive*."""

    root = archive.resolve()
    if not root.is_dir():
        raise ValueError(f"archive directory does not exist: {archive}")
    if not (root / "index.html").is_file():
        raise ValueError(f"archive has no index.html; run `agent-team-timeline build`: {root}")
    handler = functools.partial(QuietHandler, directory=str(root))
    return http.server.ThreadingHTTPServer((host, port), handler)


def serve(archive: Path, host: str, port: int, open_browser: bool) -> None:
    """Serve until interrupted, optionally opening the system browser."""

    server = make_server(archive, host, port)
    actual_port = int(server.server_address[1])
    url = f"http://{host}:{actual_port}/"
    print(f"agent-team-timeline: serving {archive.resolve()} at {url}")
    if open_browser:
        threading.Timer(0.2, lambda: webbrowser.open(url)).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nagent-team-timeline: stopped")
    finally:
        server.server_close()
