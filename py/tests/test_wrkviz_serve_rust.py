"""Differential: Rust `wrkviz serve` against the Python oracle.

Both editions serve the same fixture archive over loopback;
status line, headers (except `Date` and `Server`, which embed
the wall clock and the interpreter version), body bytes and the
request log on stderr must agree. Pre-flight failures are the
one deliberate divergence: the Python edition raises (so its
stderr is a traceback ending in the `ValueError` line) while
the Rust edition prints that same message cleanly. The same
holds for an out-of-range or occupied port (interpreter
traceback versus a clean one-line refusal).
"""

from __future__ import annotations

import gzip
import hashlib
import http.client
import re
import socket
import subprocess
import sys
import time
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"

Request = tuple[str, str, dict[str, str]]
Observed = tuple[int, int, str, tuple[tuple[str, str], ...], bytes]

REQUESTS: list[Request] = [
    ("GET", "/", {}),
    ("GET", "/index.html", {}),
    ("GET", "/app.js", {}),
    ("HEAD", "/app.js", {}),
    ("GET", "/app.js", {"Accept-Encoding": "gzip"}),
    ("GET", "/pair.json", {"Accept-Encoding": "gzip"}),
    ("GET", "/pair.json", {}),
    ("GET", "/only.json", {"Accept-Encoding": "gzip"}),
    ("GET", "/only.json", {}),
    ("GET", "/only.json", {"Range": "bytes=2-5"}),
    ("GET", "/app.js", {"Range": "bytes=0-3"}),
    ("GET", "/app.js", {"Range": "bytes=-4"}),
    ("GET", "/app.js", {"Range": "bytes=10000-"}),
    ("GET", "/app.js", {"Range": "bytes=2-1"}),
    ("GET", "/app.js", {"Range": "bytes=0-1,3-4"}),
    ("GET", "/app.js", {"If-Range": '"nope"', "Range": "bytes=0-3"}),
    ("GET", "/empty.txt", {}),
    ("GET", "/empty.txt", {"Range": "bytes=0-"}),
    ("GET", "/empty.txt", {"Range": "bytes=-5"}),
    ("GET", "/link.js", {}),
    ("GET", "/link.js", {"Range": "bytes=0-3"}),
    ("GET", "/sub", {}),
    ("GET", "/sub?x=1", {}),
    ("GET", "/sub/", {}),
    ("GET", "/missing.txt", {}),
    ("GET", "/source_snapshots/secret.txt", {}),
    ("GET", "/" + hashlib.sha256(b"hashed").hexdigest() + ".", {}),
    ("GET", "/" + hashlib.sha256(b"hashed").hexdigest() + ".!bad", {}),
    ("GET", "/f.tar", {}),
    ("GET", "/f.log", {}),
    ("GET", "/json", {}),
    ("GET", "/link.js",
     {"If-Modified-Since": "Wed, 01 Jan 2042 00:00:00 GMT"}),
    ("POST", "/app.js", {}),
    ("GET", "/data.json.gz", {}),
    ("GET", "/" + hashlib.sha256(b"hashed").hexdigest() + ".json", {}),
]


def _free_port() -> int:
    for port in range(29920, 29950):
        with socket.socket() as probe:
            try:
                probe.bind(("127.0.0.1", port))
            except OSError:
                continue
            return port
    raise AssertionError("no free port in the test range")


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


@pytest.fixture(scope="module")
def archive(tmp_path_factory: pytest.TempPathFactory) -> Path:
    root = tmp_path_factory.mktemp("wrkviz-serve-archive")
    (root / "index.html").write_bytes(b"<h1>timeline</h1>\n")
    (root / "app.js").write_bytes(b"console.log('timeline');\n")
    (root / "empty.txt").write_bytes(b"")
    pair = b'{"pair": true}\n'
    (root / "pair.json").write_bytes(pair)
    (root / "pair.json.gz").write_bytes(gzip.compress(pair))
    only = b'{"only": [1, 2, 3]}\n'
    (root / "only.json.gz").write_bytes(gzip.compress(only))
    (root / "data.json.gz").write_bytes(gzip.compress(b'{"data": 1}\n'))
    hashed = hashlib.sha256(b"hashed").hexdigest()
    (root / f"{hashed}.json").write_bytes(b'{"hashed": true}\n')
    (root / f"{hashed}.").write_bytes(b"dot\n")
    (root / f"{hashed}.!bad").write_bytes(b"bang\n")
    (root / "f.tar").write_bytes(b"tar\n")
    (root / "f.log").write_bytes(b"log\n")
    (root / "json").write_bytes(b"json\n")
    (root / "sub").mkdir()
    (root / "sub" / "index.htm").write_bytes(b"<h1>sub</h1>\n")
    (root / "source_snapshots").mkdir()
    (root / "source_snapshots" / "secret.txt").write_bytes(b"secret\n")
    (root / "link.js").symlink_to("app.js")
    return root


def _serve(command: list[str], archive_dir: Path) -> tuple[str, list[Observed], str]:
    port = _free_port()
    process = subprocess.Popen(
        [*command, "serve", "--output", str(archive_dir), "--port", str(port)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert process.stdout is not None
        banner = process.stdout.readline()
        observed: list[Observed] = []
        etag = ""
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        for method, path, headers in REQUESTS:
            connection.request(method, path, headers=headers)
            response = connection.getresponse()
            body = response.read()
            selected = tuple(
                (name.lower(), value)
                for name, value in response.getheaders()
                if name.lower() not in ("date", "server")
            )
            if path == "/app.js" and response.status == 200:
                etag = dict(selected).get("etag", "")
            observed.append((response.version, response.status, response.reason, selected, body))
        connection.request("GET", "/app.js", headers={"If-None-Match": etag})
        response = connection.getresponse()
        selected = tuple(
            (name.lower(), value)
            for name, value in response.getheaders()
            if name.lower() not in ("date", "server")
        )
        observed.append((response.version, response.status, response.reason, selected, response.read()))
        connection.close()
        time.sleep(0.3)
    finally:
        process.terminate()
        _, stderr = process.communicate(timeout=10)
    log = re.sub(r"\[[^\]]*\]", "[T]", stderr)
    return banner, observed, log


def test_serve_matches_python(archive: Path, rust_wrkviz: Path) -> None:
    py_banner, py_observed, py_log = _serve(
        [sys.executable, str(PYTHON_WRKVIZ)], archive)
    rs_banner, rs_observed, rs_log = _serve([str(rust_wrkviz)], archive)
    def normalize(banner: str) -> str:
        return re.sub(r"http://127\.0\.0\.1:\d+/", "http://127.0.0.1:P/", banner)

    assert normalize(rs_banner) == normalize(py_banner)
    assert rs_observed == py_observed
    assert rs_log == py_log


def _cli(args: list[str], *, rust: bool,
         rust_wrkviz: Path) -> subprocess.CompletedProcess[str]:
    if rust:
        command = [str(rust_wrkviz), *args]
    else:
        command = [sys.executable, str(PYTHON_WRKVIZ), *args]
    return subprocess.run(
        command, cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=60,
    )


def test_serve_cli_matches_python(rust_wrkviz: Path, archive: Path) -> None:
    for args in (
        ["serve", "--help"],
        ["serve", "-h"],
        ["serve"],
        ["serve", "--output"],
        ["serve", "--output", str(archive), "--port", "abc"],
        ["serve", "--output", str(archive), "--port"],
        ["serve", "--bogus"],
        ["serve", "extra"],
        ["serve", "--output", str(archive), "extra"],
        ["serve", "--open=1", "--output", str(archive)],
        ["serve", "--port", "8765"],
        ["serve", "--he"],
        ["serve", "--help=foo"],
        ["serve", "--output", "--help"],
        ["serve", "--port", "abc", "--help"],
        ["serve", "extra", "--help"],
    ):
        oracle = _cli(args, rust=False, rust_wrkviz=rust_wrkviz)
        result = _cli(args, rust=True, rust_wrkviz=rust_wrkviz)
        assert result.returncode == oracle.returncode, (args, result.stderr)
        assert result.stdout == oracle.stdout, args
        assert result.stderr == oracle.stderr, (args, result.stderr, oracle.stderr)


def test_serve_preflight_messages(rust_wrkviz: Path, tmp_path: Path) -> None:
    cases = (
        (tmp_path / "does-not-exist", "archive directory does not exist"),
        (tmp_path, "archive has no index.html"),
    )
    for output, fragment in cases:
        args = ["serve", "--output", str(output)]
        oracle = _cli(args, rust=False, rust_wrkviz=rust_wrkviz)
        result = _cli(args, rust=True, rust_wrkviz=rust_wrkviz)
        assert oracle.returncode == 1
        assert fragment in oracle.stderr
        assert result.returncode == 2
        assert result.stderr.startswith("wrkviz: ")
        assert fragment in result.stderr


RAW_REQUESTS: list[bytes] = [
    b"GET /app.js FOO\r\n\r\n",
    b"GET\r\n\r\n",
    b"GET /app.js\r\n\r\n",
    b"POST /app.js\r\n\r\n",
    b"GET /app.js HTTP/0.9\r\n\r\n",
    b"GET /app.js HTTP/2.0\r\n\r\n",
    b"GET /app.js HTTP/1.1\nHost: x\n\n",
    b"GET /x HTTP/1.1 EXTRA\r\n\r\n",
    b"GET /app.js HTTP/1.1\r\nHost: x\r\n"
    b"Range: bytes=99999999999999999999999999999999999999999-\r\n\r\n",
    b"GET /app.js HTTP/1.1\r\nHost: x\r\n"
    b"Range: bytes=-99999999999999999999999999999999999999999\r\n\r\n",
    b"<script> /app.js HTTP/1.1\r\n\r\n",
]


def _raw_serve(command: list[str], archive_dir: Path) -> list[bytes]:
    port = _free_port()
    process = subprocess.Popen(
        [*command, "serve", "--output", str(archive_dir), "--port", str(port)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert process.stdout is not None
        process.stdout.readline()
        observed: list[bytes] = []
        for request in RAW_REQUESTS:
            with socket.create_connection(("127.0.0.1", port), timeout=10) as sock:
                sock.sendall(request)
                chunks: list[bytes] = []
                while True:
                    chunk = sock.recv(65536)
                    if not chunk:
                        break
                    chunks.append(chunk)
                observed.append(b"".join(chunks))
        time.sleep(0.3)
    finally:
        process.terminate()
        process.communicate(timeout=10)
    return [
        re.sub(rb"(Date|Server): [^\r]+", rb"\1: X", response)
        for response in observed
    ]


def test_serve_raw_requests_match_python(archive: Path, rust_wrkviz: Path) -> None:
    oracle = _raw_serve([sys.executable, str(PYTHON_WRKVIZ)], archive)
    result = _raw_serve([str(rust_wrkviz)], archive)
    assert result == oracle
