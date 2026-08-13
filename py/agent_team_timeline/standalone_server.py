#!/usr/bin/env python3
"""Dependency-free static timeline server, copied verbatim into generated archives."""

from __future__ import annotations

import argparse
import functools
import hashlib
import http.server
import io
import os
import re
import threading
import urllib.parse
import webbrowser
from email.utils import formatdate
from pathlib import Path
from typing import BinaryIO


_CONTENT_HASHED_NAME = re.compile(r"^[0-9a-f]{64}(?:\.[A-Za-z0-9][A-Za-z0-9._-]*)?$")
_REVALIDATE = "public, no-cache"
_IMMUTABLE = "public, max-age=31536000, immutable"


def accepts_gzip(header: str) -> bool:
    """Return whether an HTTP ``Accept-Encoding`` value permits gzip."""

    qualities: dict[str, float] = {}
    for raw_item in header.split(","):
        parts = [part.strip() for part in raw_item.split(";")]
        coding = parts[0].lower()
        if not coding:
            continue
        quality = 1.0
        for parameter in parts[1:]:
            name, separator, raw_value = parameter.partition("=")
            if separator and name.strip().lower() == "q":
                try:
                    quality = float(raw_value.strip())
                except ValueError:
                    quality = 0.0
        qualities[coding] = quality if 0.0 <= quality <= 1.0 else 0.0
    if "gzip" in qualities:
        return qualities["gzip"] > 0.0
    return qualities.get("*", 0.0) > 0.0


def cache_control_for_path(path: Path) -> str:
    """Choose conservative caching unless the filename is a complete content digest."""

    return _IMMUTABLE if _CONTENT_HASHED_NAME.fullmatch(path.name) else _REVALIDATE


def _strong_etag(handle: BinaryIO) -> str:
    digest = hashlib.sha256()
    while True:
        chunk = handle.read(1024 * 1024)
        if not chunk:
            break
        digest.update(chunk)
    handle.seek(0)
    return '"sha256-' + digest.hexdigest() + '"'


def _etag_matches(raw_header: str, etag: str) -> bool:
    for raw_value in raw_header.split(","):
        value = raw_value.strip()
        if value == "*" or value == etag or value.removeprefix("W/") == etag:
            return True
    return False


class TimelineRequestHandler(http.server.SimpleHTTPRequestHandler):
    """Serve identity files or deterministic ``.gz`` companions with validators."""

    _cache_control_sent = False

    def list_directory(self, path: str | os.PathLike[str]) -> io.BytesIO | None:
        """Never expose generated-object or transcript filenames by directory listing."""

        del path
        self.send_error(404, "File not found")
        return None

    def end_headers(self) -> None:
        """Add browser-safety headers and a fallback revalidation policy."""

        if not self._cache_control_sent:
            self.send_header("Cache-Control", _REVALIDATE)
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        super().end_headers()
        self._cache_control_sent = False

    def _send_cache_control(self, source_path: Path) -> None:
        self.send_header("Cache-Control", cache_control_for_path(source_path))
        self._cache_control_sent = True

    def _selected_source_path(self) -> Path | None:
        source_path = Path(self.translate_path(self.path))
        if not source_path.is_dir():
            return source_path
        request_path = urllib.parse.urlsplit(self.path).path
        if not request_path.endswith("/"):
            return None
        for index_name in ("index.html", "index.htm"):
            candidate = source_path / index_name
            if candidate.is_file():
                return candidate
        return None

    def send_head(self) -> BinaryIO | None:
        """Open the selected representation and emit content-negotiated headers."""

        source_path = self._selected_source_path()
        if source_path is None or not source_path.is_file() or source_path.is_symlink():
            return super().send_head()
        sidecar_path = source_path.with_name(source_path.name + ".gz")
        vary = sidecar_path.is_file() and not sidecar_path.is_symlink()
        use_gzip = vary and accepts_gzip(self.headers.get("Accept-Encoding", ""))
        selected_path = sidecar_path if use_gzip else source_path
        handle: BinaryIO | None = None
        try:
            handle = selected_path.open("rb")
            selected_stat = os.fstat(handle.fileno())
            source_stat = source_path.stat()
        except OSError:
            if handle is not None:
                handle.close()
            self.send_error(404, "File not found")
            return None

        etag = _strong_etag(handle)
        if _etag_matches(self.headers.get("If-None-Match", ""), etag):
            handle.close()
            self.send_response(304)
            self.send_header("ETag", etag)
            self.send_header("Last-Modified", formatdate(source_stat.st_mtime, usegmt=True))
            self._send_cache_control(source_path)
            if vary:
                self.send_header("Vary", "Accept-Encoding")
            if use_gzip:
                self.send_header("Content-Encoding", "gzip")
            self.end_headers()
            return None

        self.send_response(200)
        self.send_header("Content-Type", self.guess_type(str(source_path)))
        self.send_header("Content-Length", str(selected_stat.st_size))
        self.send_header("Last-Modified", formatdate(source_stat.st_mtime, usegmt=True))
        self.send_header("ETag", etag)
        self._send_cache_control(source_path)
        if vary:
            self.send_header("Vary", "Accept-Encoding")
        if use_gzip:
            self.send_header("Content-Encoding", "gzip")
        self.end_headers()
        return handle

    def log_message(self, format: str, *args: object) -> None:
        """Suppress routine index requests while retaining other request logs."""

        if self.command != "GET" or self.path not in ("/", "/index.html"):
            super().log_message(format, *args)


def make_static_server(
    root: Path, host: str = "127.0.0.1", port: int = 8765
) -> http.server.ThreadingHTTPServer:
    """Create a threaded static server rooted at *root*."""

    handler = functools.partial(TimelineRequestHandler, directory=str(root.resolve()))
    return http.server.ThreadingHTTPServer((host, port), handler)


def _main() -> None:
    """Serve the generated archive containing this script."""

    parser = argparse.ArgumentParser(
        description="Serve this timeline on loopback and optionally open it in a browser."
    )
    parser.add_argument(
        "--port", type=int, default=8765, help="loopback port (default: %(default)s)"
    )
    parser.add_argument("--open", action="store_true", help="open the timeline in a browser")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    server = make_static_server(root, port=args.port)
    url = f"http://127.0.0.1:{server.server_address[1]}/"
    print(f"Serving {root} at {url}")
    if args.open:
        threading.Timer(0.2, lambda: webbrowser.open(url)).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    _main()
