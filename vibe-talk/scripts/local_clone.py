#!/usr/bin/env python3
"""Run a local vibe-talk server against a read-only copy of the real cache.

WHY
===
Hermetic browser/server testing (Playwright against a throwaway server)
cannot reproduce bugs that live in real message shape: a long thread, a
pasted internal link, real bullet structure. This script clones the shape
without cloning the risk:

  1. it picks a server binary (a deployed build when one is named),
  2. it copies the message cache with SQLite's backup API from a
     read-only open of the live file (the live server keeps running; the
     copy is consistent and the source is never written),
  3. it starts that binary on a free loopback port with fake credentials
     and a loopback stub for the chat/voice backends -- nothing it runs
     can reach the live providers, and no live token is ever read,
  4. it prints the URL, and
  5. it deletes the copy when it exits.

The copy lives in a fresh 0700 directory and is created 0600. Message
content is never printed, logged here, or committed: `--probe` reports
only HTTP status, response byte length and whether the server survived.

USAGE
=====
    scripts/local_clone.py [--sha SHA | --binary PATH]
                           [--channel ID] [--thread TS]
                           [--message-id ID] [--probe]
                           [--internal-host HOST]...

With no --sha, the newest deployed build under
~/agent_scratch/vt-build/<sha8>/ is used when present, else the repo's
target/release build. Ctrl-C stops the server (by PID) and removes the
copy. --keep leaves the copy for debugging and says so loudly.
"""

from __future__ import annotations

import argparse
import http.server
import json
import os
import secrets
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path
from typing import TypedDict

REPO = Path(__file__).resolve().parent.parent
DATA_DIR = Path.home() / ".local" / "share" / "vibe-talk"
BUILD_ROOT = Path.home() / "agent_scratch" / "vt-build"


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def find_cache(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit).expanduser()
    candidates = sorted(
        DATA_DIR.glob("*.message-cache-*.sqlite3"),
        key=lambda p: p.stat().st_mtime,
    )
    if not candidates:
        sys.exit(f"local_clone: no message cache under {DATA_DIR}; pass --cache")
    return candidates[-1]


def find_binary(sha: str | None, binary: str | None) -> Path:
    if binary:
        return Path(binary).expanduser()
    if sha:
        for name in ("vibe-talk.bin", "vibe-talk"):
            candidate = BUILD_ROOT / sha[:8] / name
            if candidate.exists():
                return candidate
        sys.exit(
            f"local_clone: no built binary for sha {sha[:8]} under {BUILD_ROOT}; "
            "build it in a throwaway worktree and pass --binary"
        )
    builds = sorted(BUILD_ROOT.glob("*/vibe-talk.bin")) if BUILD_ROOT.exists() else []
    if builds:
        return builds[-1]
    candidate = REPO / "target" / "release" / "vibe-talk"
    if candidate.exists():
        return candidate
    sys.exit("local_clone: no binary found; pass --binary or --sha")


def copy_cache(source: Path, dest: Path) -> None:
    """Consistent copy via the backup API; the source is opened read-only."""
    src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    try:
        dst = sqlite3.connect(dest)
        try:
            src.backup(dst)
        finally:
            dst.close()
    finally:
        src.close()
    os.chmod(dest, 0o600)


ACCOUNT_ID = ""
TEAM_ID = ""


class StubHandler(http.server.BaseHTTPRequestHandler):
    """A chat/voice backend that answers everything with a refusal.

    Cache-served reads never need it; anything that does reach it fails
    closed and locally instead of touching a live provider. The one
    exception is Slack's `auth.test`: the cache only serves rows read as
    the account they were read as, so the stub names that account (a
    public user id, supplied with --account-id) and nothing else.
    """

    def _refuse(self) -> None:
        if ACCOUNT_ID and self.path.rstrip("/").endswith("auth.test"):
            body = json.dumps({
                "ok": True, "user_id": ACCOUNT_ID, "user": "local-clone",
                **({"team_id": TEAM_ID} if TEAM_ID else {}),
            }).encode()
        else:
            body = json.dumps({"ok": False, "error": "local_clone_stub"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    do_GET = _refuse
    do_POST = _refuse

    def log_message(self, *_args: object) -> None:
        pass


def write_config(path: Path, *, port: int, stub_port: int, work: Path,
                 channel: str, internal_hosts: list[str],
                 read_token: str, write_token: str,
                 test_token: str | None = None) -> None:
    hosts = ", ".join(f'"{h}"' for h in internal_hosts)
    test_lines = ""
    if test_token:
        test_lines = (
            f'\ntest_token = "{test_token}"'
            f'\ntest_token_kill_file = "{work}/test-kill"'
        )
    text = f"""\
[server]
bind = "127.0.0.1:{port}"

[discord]
bot_token = "local-clone-bot-token-never-sent-anywhere"

[[providers]]
key = "slack"
kind = "slack"
api_base = "http://127.0.0.1:{stub_port}"
token = "local-clone-slack-token-never-sent-anywhere"
live_poll_seconds = 0

[auth]
read_token = "{read_token}"
write_token = "{write_token}"{test_lines}

[[channels]]
id = "{channel}"
label = "local clone"
writable = false
provider = "slack"

[elevenlabs]
agent_id = "agent_mock00000000000000000000000"
api_key = "xi-mock-api-key-not-a-real-one"
api_base = "http://127.0.0.1:{stub_port}/v1"

[read_aloud]
backend = "elevenlabs"

[speakable]
internal_link_hosts = [{hosts}]

[storage]
path = "{work}/store.sqlite3"

[message_cache]
enabled = true
serve = true
backfill = "off"
path = "{work}/message-cache.sqlite3"
"""
    path.write_text(text)
    os.chmod(path, 0o600)


def wait_up(port: int, proc: subprocess.Popen[bytes], timeout: float = 60.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if proc.poll() is not None:
            return False
        try:
            with urllib.request.urlopen(
                f"http://127.0.0.1:{port}/healthz", timeout=2
            ) as reply:
                if reply.status == 200:
                    return True
        except Exception:
            time.sleep(0.25)
    return False


class ProbeResult(TypedDict, total=False):
    status: int
    bytes: int
    tickets: int
    error: str


def get_status(port: int, token: str, path: str) -> int:
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}",
        headers={"Authorization": f"Bearer {token}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as reply:
            reply.read()
            return reply.status
    except urllib.error.HTTPError as error:
        return error.code
    except Exception:
        return 0


def serve_matrix(port: int, write_token: str, channel: str,
                 thread: str | None, message_id: str | None) -> dict[str, int]:
    """What this clone actually serves, by surface. Statuses only."""
    matrix = {
        "timeline (main, write token)": get_status(
            port, write_token,
            f"/api/v1/channels/{channel}/timeline?limit=5"),
    }
    if thread:
        matrix["timeline (thread view, write token)"] = get_status(
            port, write_token,
            f"/api/v1/channels/{channel}/timeline?view=thread"
            f"&thread_id={thread}&limit=5")
    if message_id:
        result = probe(port, write_token, channel, thread, message_id)
        matrix["speech prepare"] = result["status"]
    return matrix


def probe(port: int, write_token: str, channel: str,
          thread: str | None, message_id: str) -> ProbeResult:
    """Ask the clone to prepare speech for one message. Shape only."""
    body = json.dumps({
        "ids": [message_id],
        **({"thread_id": thread} if thread else {}),
    }).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/v1/channels/{channel}/speech/prepare",
        data=body,
        headers={
            "Authorization": f"Bearer {write_token}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as reply:
            payload = reply.read()
            return {"status": reply.status, "bytes": len(payload),
                    "tickets": len(json.loads(payload).get("prepared", []))}
    except urllib.error.HTTPError as error:
        return {"status": error.code, "bytes": 0, "tickets": 0}
    except Exception as error:  # connection refused/reset: server died
        return {"status": 0, "bytes": 0, "tickets": 0,
                "error": type(error).__name__}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--sha", help="deployed build sha (vt-build/<sha8>)")
    parser.add_argument("--binary", help="server binary to run")
    parser.add_argument("--cache", help="message cache file (default: newest in data dir)")
    parser.add_argument("--channel", required=True, help="allowlisted channel id")
    parser.add_argument("--thread", help="thread ts/id for --probe")
    parser.add_argument("--message-id", help="message id for --probe")
    parser.add_argument("--probe", action="store_true",
                        help="prepare speech for --message-id and report shape only")
    parser.add_argument("--internal-host", action="append", default=[],
                        help="internal link host for speech (repeatable)")
    parser.add_argument("--account-id", default="",
                        help="provider account id the cache was read as "
                             "(a public user id; the stub's auth.test names it)")
    parser.add_argument("--team-id", default="",
                        help="workspace/team id recorded with --account-id, if any")
    parser.add_argument("--port", type=int, default=0, help="loopback port (0 = free)")
    parser.add_argument("--keep", action="store_true",
                        help="keep the copied cache on exit (debugging only)")
    parser.add_argument("--test-token", action="store_true",
                        help="also issue the fake read-only test credential "
                             "(#2060 Part B) and print it")
    parser.add_argument("--strict", action="store_true",
                        help="exit 3 when any probed surface is not "
                             "cache-served (default: print the matrix "
                             "loudly and keep running)")
    args = parser.parse_args()

    global ACCOUNT_ID, TEAM_ID
    ACCOUNT_ID = args.account_id
    TEAM_ID = args.team_id
    binary = find_binary(args.sha, args.binary).resolve()
    source = find_cache(args.cache)
    work = Path(tempfile.mkdtemp(prefix="vibe-talk-local-clone-"))
    os.chmod(work, 0o700)
    port = args.port or free_port()
    stub = http.server.ThreadingHTTPServer(("127.0.0.1", 0), StubHandler)
    stub_port = stub.server_address[1]
    read_token = "local-clone-read-" + secrets.token_hex(12)
    write_token = "local-clone-write-" + secrets.token_hex(12)
    proc: subprocess.Popen[bytes] | None = None
    try:
        copy_cache(source, work / "message-cache.sqlite3")
        test_token = (
            "local-clone-test-" + secrets.token_hex(12) if args.test_token else None
        )
        write_config(work / "vibe-talk.toml", port=port, stub_port=stub_port,
                     work=work, channel=args.channel,
                     internal_hosts=args.internal_host,
                     read_token=read_token, write_token=write_token,
                     test_token=test_token)
        threading.Thread(target=stub.serve_forever, daemon=True).start()
        log = open(work / "server.log", "wb")
        proc = subprocess.Popen(
            [str(binary), "--config", str(work / "vibe-talk.toml"),
             "--skip-startup-probe"],
            stdout=log, stderr=subprocess.STDOUT, cwd=work,
        )
        if not wait_up(port, proc):
            tail = (work / "server.log").read_text(errors="replace")[-2000:]
            print(f"local_clone: server did not come up\n{tail}", file=sys.stderr)
            return 1
        print(f"http://127.0.0.1:{port}/")
        print(f"read token (fake, local only): {read_token}", flush=True)
        if not args.account_id:
            print(
                "local_clone: WARNING: no --account-id/--team-id, so the "
                "cache cannot verify its account and timeline reads will "
                "fall back to the stub backend (0 rows / 502). Pass the "
                "recorded provider account to serve from the cache.",
                file=sys.stderr, flush=True)
        matrix = serve_matrix(port, write_token, args.channel,
                              args.thread, args.message_id)
        for surface, status in matrix.items():
            print(f"served[{status}]: {surface}", flush=True)
        if args.strict and any(status != 200 for status in matrix.values()):
            print(
                "local_clone: --strict: a probed surface is not "
                "cache-served (see served[...] lines above); stopping. "
                "Thread-view timelines are not cache-served on any "
                "current build; main timeline and speech prepare are.",
                file=sys.stderr)
            return 3
        if test_token:
            print(f"test token (fake, local only): {test_token}", flush=True)
        if args.probe:
            if not args.message_id:
                print("local_clone: --probe needs --message-id", file=sys.stderr)
                return 2
            result = probe(port, write_token, args.channel,
                           args.thread, args.message_id)
            alive = proc.poll() is None
            print(json.dumps({"probe": result, "server_alive": alive}))
            return 0 if alive else 1
        print("local_clone: Ctrl-C to stop; the copy is deleted on exit",
              flush=True)
        try:
            proc.wait()
        except KeyboardInterrupt:
            pass
        return 0
    finally:
        if proc is not None and proc.poll() is None:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        stub.shutdown()
        if args.keep:
            print(f"local_clone: kept copy at {work}", file=sys.stderr)
        else:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
